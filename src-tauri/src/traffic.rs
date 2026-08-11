//! 流量统计（文档 §3.7）
//!
//! 方案（实测后调整）：`netraffic`(pcap) 需要 Npcap/WinPcap 运行时，本机不可用；
//! 改用 Windows 原生 `GetIfTable2`（iphlpapi.dll，系统自带），统计虚拟网卡的
//! InOctets/OutOctets 增量（可区分上传/下载）。找不到虚拟网卡时降级为空快照，
//! 后续可补充 `vnt-cli --chart_a` 解析。

use std::time::Duration;

use chrono::Local;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::{AppState, PeerTraffic, TrafficSnapshot};

/// 单日流量（字节）
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct DayTraffic {
    pub sent: u64,
    pub recv: u64,
}

impl DayTraffic {
    #[allow(dead_code)]
    pub fn total(&self) -> u64 {
        self.sent.saturating_add(self.recv)
    }
}

/// 按天累计的流量统计（持久化到 %APPDATA%\vnt-gui\traffic_stats.json）
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TrafficStats {
    /// key: "YYYY-MM-DD"（本地日期）
    pub days: std::collections::BTreeMap<String, DayTraffic>,
}

/// 分时间段流量汇总（今日/昨日/本月/累计）
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct PeriodTraffic {
    pub today: DayTraffic,
    pub yesterday: DayTraffic,
    pub month: DayTraffic,
    pub total: DayTraffic,
}

const STATS_FILE: &str = "traffic_stats.json";
/// 保留最近天数（超出的历史并入累计后裁剪，控制文件大小）
const KEEP_DAYS: i64 = 62;

impl TrafficStats {
    /// 从磁盘加载（失败返回空统计）
    pub fn load(config_dir: &std::path::Path) -> Self {
        let path = config_dir.join(STATS_FILE);
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// 原子写盘（先写临时文件再替换）
    pub fn save(&self, config_dir: &std::path::Path) {
        let path = config_dir.join(STATS_FILE);
        let tmp = config_dir.join(format!("{}.tmp", STATS_FILE));
        if let Ok(json) = serde_json::to_string_pretty(self) {
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    /// 累计今日流量增量
    pub fn accumulate(&mut self, sent_inc: u64, recv_inc: u64) {
        let key = Local::now().format("%Y-%m-%d").to_string();
        let entry = self.days.entry(key).or_default();
        entry.sent = entry.sent.saturating_add(sent_inc);
        entry.recv = entry.recv.saturating_add(recv_inc);
    }

    /// 裁剪：只保留最近 KEEP_DAYS 天（更早的历史在 month/total 中已含，但文件精简）
    pub fn prune(&mut self) {
        let today = Local::now().date_naive();
        self.days
            .retain(|k, _| chrono::NaiveDate::parse_from_str(k, "%Y-%m-%d").is_ok_and(|d| (today - d).num_days() <= KEEP_DAYS));
    }

    /// 今日流量（截至当前）
    pub fn today(&self) -> DayTraffic {
        let key = Local::now().format("%Y-%m-%d").to_string();
        self.days.get(&key).copied().unwrap_or_default()
    }

    /// 昨日全天流量
    pub fn yesterday(&self) -> DayTraffic {
        let key = (Local::now().date_naive() - chrono::Days::new(1))
            .format("%Y-%m-%d")
            .to_string();
        self.days.get(&key).copied().unwrap_or_default()
    }

    /// 本月累计流量（本月所有天求和）
    pub fn month(&self) -> DayTraffic {
        let now = Local::now();
        let prefix = now.format("%Y-%m").to_string();
        let mut acc = DayTraffic::default();
        for (k, v) in &self.days {
            if k.starts_with(&prefix) {
                acc.sent = acc.sent.saturating_add(v.sent);
                acc.recv = acc.recv.saturating_add(v.recv);
            }
        }
        acc
    }

    /// 累计流量（全部历史）
    pub fn total(&self) -> DayTraffic {
        let mut acc = DayTraffic::default();
        for v in self.days.values() {
            acc.sent = acc.sent.saturating_add(v.sent);
            acc.recv = acc.recv.saturating_add(v.recv);
        }
        acc
    }

    /// 四时段汇总
    pub fn period(&self) -> PeriodTraffic {
        PeriodTraffic {
            today: self.today(),
            yesterday: self.yesterday(),
            month: self.month(),
            total: self.total(),
        }
    }
}

/// 启动流量监控任务（每秒采集并 emit `traffic-update`；累计按天统计并定期落盘）
pub fn start_traffic_monitor(app: AppHandle) {
    // 加载历史按天统计
    {
        let state: State<'_, AppState> = app.state();
        let loaded = TrafficStats::load(&state.config_dir);
        *state.traffic_daily.lock() = loaded;
    }

    tauri::async_runtime::spawn(async move {
        let mut last = sample_interface();

        // 上一个周期用于计算速率
        let mut prev_bytes_up = 0u64;
        let mut prev_bytes_down = 0u64;
        let mut first = true;
        // 落盘节流计数（每 60 秒写一次）
        let mut save_counter = 0u32;

        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;

            let current = sample_interface();
            let (up, down) = match (last, current) {
                (Some(a), Some(b)) => {
                    // 网卡计数器可能重置，做防回退保护
                    let up = if b.up_bytes >= a.up_bytes {
                        b.up_bytes - a.up_bytes
                    } else {
                        0
                    };
                    let down = if b.down_bytes >= a.down_bytes {
                        b.down_bytes - a.down_bytes
                    } else {
                        0
                    };
                    last = current;
                    (up, down)
                }
                _ => {
                    last = current;
                    (0, 0)
                }
            };

            let upload_speed = if first { 0.0 } else { up as f64 };
            let download_speed = if first { 0.0 } else { down as f64 };
            first = false;

            // 总量累计（会话内）
            prev_bytes_up += up;
            prev_bytes_down += down;

            // 按天统计累计 + 定期落盘（每 60 秒）
            save_counter += 1;
            let snapshot = TrafficSnapshot {
                upload_bytes: prev_bytes_up,
                download_bytes: prev_bytes_down,
                upload_speed,
                download_speed,
                peers: Vec::<PeerTraffic>::new(),
            };
            {
                let state: State<'_, AppState> = app.state();
                {
                    let mut daily = state.traffic_daily.lock();
                    daily.accumulate(up, down);
                    if save_counter % 60 == 0 {
                        daily.prune();
                        let dir = state.config_dir.clone();
                        daily.save(&dir);
                    }
                }
                *state.traffic_snapshot.write() = snapshot.clone();
            }
            let _ = app.emit("traffic-update", &snapshot);
        }
    });
}

/// 网卡采样结果
#[derive(Debug, Clone, Copy, Default)]
struct Sample {
    up_bytes: u64,
    down_bytes: u64,
}

/// 通过 GetIfTable2 采样 VNT 虚拟网卡的收发字节数
fn sample_interface() -> Option<Sample> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
        use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;

        unsafe {
            let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
            if GetIfTable2(&mut table) != 0 {
                return None;
            }
            if table.is_null() {
                return None;
            }

            let mut best: Option<(usize, u64, u64)> = None; // (index, up, down)
            let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);

            for row in rows {
                if row.OperStatus != IfOperStatusUp {
                    continue;
                }
                // 匹配 VNT 虚拟网卡：描述含 wintun/vnt，或别名含 vnt/tun
                let desc = wide_to_string(&row.Description);
                let name = wide_to_string(&row.Alias);
                let is_vnt = desc.to_lowercase().contains("wintun")
                    || desc.to_lowercase().contains("vnt")
                    || name.to_lowercase().contains("vnt")
                    || name.to_lowercase().contains("tun")
                    || name.to_lowercase().contains("tap");
                if !is_vnt {
                    continue;
                }
                let (up, down) = (row.OutOctets, row.InOctets);
                match best {
                    Some((_, bup, bdown)) if bup + bdown >= up + down => {}
                    _ => best = Some((row.InterfaceIndex as usize, up, down)),
                }
            }

            FreeMibTable(table as _);

            best.map(|(_, up, down)| Sample {
                up_bytes: up,
                down_bytes: down,
            })
        }
    }

    #[cfg(not(windows))]
    {
        None
    }
}

/// UTF-16 宽字符数组转 String
#[cfg(windows)]
fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).trim().to_string()
}
