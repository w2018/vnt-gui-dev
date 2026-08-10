# VNT GUI

[vnt-dev/vnt](https://github.com/vnt-dev/vnt) 的 Windows 图形化管理工具（Rust + Tauri 2 + React）。当前版本 **v2.3.1**。

将官方 `vnt-cli.exe` 作为 Sidecar 嵌入。**GUI 与后台服务（vnt-daemon）解耦**：daemon 常驻管理 vnt-cli 与 FTP，关闭/重启 GUI 不影响已建立的组网与 FTP 服务；GUI 通过本地 RPC 控制 daemon，重新打开自动恢复服务。

## 功能

- **连接管理**：一键连接/断开、5 态状态机、断线自动重连、防多实例
- **首页状态总览**：连接状态（虚拟 IP/服务器/延迟，延迟分级着色、统一字号；GUI 重启后自动恢复连接信息）+ FTP 服务状态卡（v2.1.0 新增）
- **连接信息**：设备名、虚拟 IP、真实服务器（IP:端口）、NAT 类型、延时检测（分级着色）
- **系统集成**：系统托盘（状态图标/实时 IP/快速连接/双击显示）、最小化到托盘、开机自启（静默后台 + daemon 自动恢复服务）
- **配置管理**：多配置 CRUD、JSON 导入/导出、Token 脱敏
- **实时日志**：GUI 与 daemon 日志合并实时展示（v2.1.0 起 daemon 日志经 RPC 合并）、过滤/搜索/导出/清空
- **流量统计**：实时速率折线图、今日/昨日/本月/累计流量（按天持久化）、速率告警
- **设备列表**：本组网设备（自动过滤本机）、P2P/中继标识、延迟着色、离线识别
- **桌面共享（v2.3.1 重构）**：控制端/被控端双模、标签页收纳式布局（画面最大化）——MF H.264 低延迟编码（GOP 1s / CBR / 无 B 帧）、高分屏分辨率缩放、fps 节流、WebCodecs 硬解 + 软解自动回退（虚拟机可用）、彩色光标、点击画面聚焦后拦截键盘、连接请求全局授权弹窗（前后台/任意页面）、断开重连 / 切页切回无黑屏
- **FTP 服务**：内嵌 FTP 服务器（libunftp）、用户权限管理（上传/下载/删除/只读）、ROOT 目录选择、端口与 PASV 配置、监听地址展示、**连接日志（登录与操作均显示真实客户端 IP，v2.1.0 修复）**、随应用/系统自启（密码经 Windows DPAPI 加密存储）
- **软件更新**：vnt-cli 与 GUI 双通道版本检测 + 一键更新
- **体验**：首次启动向导、深浅主题、全局快捷键、开机自启隐藏窗口

## 数据目录（v2.1.0 起）

所有配置与日志统一存放于**应用安装目录**，卸载时在卸载器中勾选"删除应用数据"可整体清除（v2.1.0 新增）：

```
<安装目录>/
├── VNT GUI.exe        # 主程序
├── vnt-daemon.exe     # 后台服务
├── vnt-cli.exe        # 组网命令行
├── data/              # 配置（config.json / ftp_config.json / runtime_state.json / daemon.pid）
└── logs/              # 日志（app.log / vnt-daemon.log）
```

> 旧版本数据（`%APPDATA%\vnt-gui` 或安装目录根残留）首次运行自动迁移。

## 构建

环境：Node.js 18+、Rust、tauri-cli 2.x、Windows 10/11 x64

```bash
npm install
npm run tauri dev       # 开发模式
npm run tauri build     # 生产构建（仅 NSIS 安装包）
```

> 中文路径下请直接在真实路径运行 npm/vite 命令（junction 会导致 vite 构建路径错乱）。

### Sidecar 二进制

`src-tauri/binaries/` 需放置（构建期检查存在性）：

| 文件 | 来源 |
|------|------|
| `vnt-cli-x86_64-pc-windows-msvc.exe` | [vnt-dev/vnt Releases](https://github.com/vnt-dev/vnt/releases) |
| `wintun.dll` | [Wintun](https://www.wintun.net/)（amd64） |

## 使用

1. 「配置」页新建组网配置（Token 必填）
2. 「连接」页点击连接，成功后查看虚拟 IP、延时与设备列表
3. 关闭窗口最小化到托盘；托盘右键连接/退出

## 更新机制

- **vnt-cli**：对比上游 vnt-dev/vnt 最新 release，一键下载替换
- **GUI**：对比本仓库 [Releases](https://github.com/w2018/vnt-gui-dev/releases)（发布时打 `v*` tag 自动构建并生成安装包）

## 致谢

[vnt-dev/vnt](https://github.com/vnt-dev/vnt)（GPL-3.0）、[Tauri](https://tauri.app/)、[Ant Design](https://ant.design/)
