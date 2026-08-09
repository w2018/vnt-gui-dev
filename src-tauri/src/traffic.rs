//! 流量统计（文档 §3.7）
//!
//! 方案（实测后调整）：`netraffic`(pcap) 需要 Npcap/WinPcap 运行时，本机不可用；
//! 改用 Windows 原生 `GetIfTable2`（iphlpapi.dll，系统自带），统计虚拟网卡的
//! InOctets/OutOctets 增量（可区分上传/下载）。找不到虚拟网卡时降级为空快照，
//! 后续可补充 `vnt-cli --chart_a` 解析。

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::{AppState, PeerTraffic, TrafficSnapshot};

/// 启动流量监控任务（每秒采集并 emit `traffic-update`）
pub fn start_traffic_monitor(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last = sample_interface();

        // 上一个周期用于计算速率
        let mut prev_bytes_up = 0u64;
        let mut prev_bytes_down = 0u64;
        let mut first = true;

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

            // 总量累计
            prev_bytes_up += up;
            prev_bytes_down += down;

            let snapshot = TrafficSnapshot {
                upload_bytes: prev_bytes_up,
                download_bytes: prev_bytes_down,
                upload_speed,
                download_speed,
                peers: Vec::<PeerTraffic>::new(),
            };

            {
                let state: State<'_, AppState> = app.state();
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
