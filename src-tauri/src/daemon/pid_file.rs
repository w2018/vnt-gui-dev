//! PID 文件读写 + daemon 存活检测

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

/// 数据目录（与 GUI 配置同源：%APPDATA%\vnt-gui）；测试可注入（每次覆盖，
/// 配合 DATA_DIR_LOCK 串行；OnceLock 会让后续测试静默失败导致 flaky）
static DATA_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// 测试互斥锁：set_data_dir 是全局状态，daemon 相关测试必须串行
#[cfg(test)]
pub(crate) static DATA_DIR_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 设置数据目录（默认 = 可执行文件所在目录，即应用安装目录；测试注入临时目录）
pub fn set_data_dir(dir: PathBuf) {
    *DATA_DIR.lock().unwrap() = Some(dir);
}

/// 默认数据目录：<安装目录>/data（与 GUI 配置目录统一，卸载时随"删除应用数据"一并清除）
fn default_data_dir() -> PathBuf {
    crate::config::app_data_dir()
}

/// 数据目录
pub fn daemon_data_dir() -> PathBuf {
    DATA_DIR
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(default_data_dir)
}

fn pid_path() -> PathBuf {
    daemon_data_dir().join("daemon.pid")
}

/// 写入当前进程 PID
pub fn write_current_pid() -> std::io::Result<()> {
    let path = pid_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, std::process::id().to_string())
}

/// 读取 PID（无文件或解析失败 → None）
pub fn read_pid() -> Option<u32> {
    fs::read_to_string(pid_path())
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
}

/// 删除 PID 文件
pub fn remove() -> std::io::Result<()> {
    fs::remove_file(pid_path()).ok();
    Ok(())
}

/// 检查 daemon 是否仍在运行（PID 文件存在 + 进程存活）
pub fn is_daemon_running() -> bool {
    let Some(pid) = read_pid() else {
        return false;
    };
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut exit_code: u32 = 0;
            let alive = GetExitCodeProcess(handle, &mut exit_code) != 0 && exit_code == STILL_ACTIVE as u32;
            CloseHandle(handle);
            alive
        }
    }
    #[cfg(not(windows))]
    {
        // 非 Windows：仅凭 PID 文件存在判定（保守）
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        set_data_dir(dir.path().to_path_buf());
        dir
    }

    #[test]
    fn test_pid_file_full_cycle() {
        // V5 要求：write → read 断言等于当前 PID → is_daemon_running true → remove → None
        // set_data_dir 全局竞争 → 串行锁
        let _g = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(DATA_DIR_LOCK.lock());
        let _dir = test_dir();

        write_current_pid().expect("写 PID 文件应成功");
        assert_eq!(read_pid(), Some(std::process::id()), "读回 PID 应等于当前进程");
        assert!(is_daemon_running(), "当前进程必然存活，daemon 应判定运行中");

        remove().expect("删除应成功");
        assert_eq!(read_pid(), None, "删除后不应再读到 PID");
        assert!(!is_daemon_running(), "无 PID 文件 → 未运行");
    }

    #[test]
    fn test_pid_read_missing_file() {
        // set_data_dir 全局竞争 → 串行锁
        let _g = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(DATA_DIR_LOCK.lock());
        let _dir = test_dir();
        assert_eq!(read_pid(), None);
    }

    #[test]
    fn test_pid_parse_rejects_garbage() {
        assert!("not-a-pid".parse::<u32>().is_err());
        assert_eq!("12345".parse::<u32>().ok(), Some(12345));
    }
}
