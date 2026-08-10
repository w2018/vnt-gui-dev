//! Daemon 模块：独立进程 vnt-daemon（服务解耦核心）
//!
//! - `rpc_protocol`  GUI ↔ daemon 契约（JSON-RPC over TCP）
//! - `rpc_server`    daemon 侧 TCP 服务端
//! - `rpc_client`    GUI 侧 TCP 客户端
//! - `vnt_manager`   vnt-cli 生命周期（守护重启 + 状态解析 + peers/nat 轮询）
//! - `ftp_manager`   FTP 生命周期（复用 libunftp 实现）
//! - `state_store`   运行时状态持久化（重启恢复）
//! - `pid_file`      daemon PID 文件 + 存活检测

pub mod ftp_manager;
pub mod pid_file;
pub mod rpc_client;
pub mod rpc_protocol;
pub mod rpc_server;
pub mod state_store;
pub mod vnt_manager;
