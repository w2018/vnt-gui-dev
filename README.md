# VNT GUI

[vnt-dev/vnt](https://github.com/vnt-dev/vnt) 的 Windows 图形化管理工具（Rust + Tauri 2 + React）。

将官方 `vnt-cli.exe` 作为 Sidecar 嵌入。**GUI 与后台服务（vnt-daemon）解耦**：daemon 常驻管理 vnt-cli 与 FTP，关闭/重启 GUI 不影响已建立的组网与 FTP 服务；GUI 通过本地 RPC 控制 daemon，重新打开自动恢复服务。

## 功能

- **连接管理**：一键连接/断开、5 态状态机、断线自动重连、防多实例
- **连接信息**：设备名、虚拟 IP、真实服务器（IP:端口）、NAT 类型、延时检测（分级着色）
- **系统集成**：系统托盘（状态图标/快速连接/双击显示）、最小化到托盘、开机自启
- **配置管理**：多配置 CRUD、JSON 导入/导出、Token 脱敏
- **实时日志**：环形缓冲、过滤/搜索/导出
- **流量统计**：实时速率折线图、今日/昨日/本月/累计流量（按天持久化）、速率告警
- **设备列表**：本组网设备（自动过滤本机）、P2P/中继标识、延迟着色、离线识别
- **FTP 服务**：内嵌 FTP 服务器（libunftp）、用户权限管理（上传/下载/删除/只读）、ROOT 目录选择、端口与 PASV 配置、监听地址展示、连接日志、随应用/系统自启（密码经 Windows DPAPI 加密存储）
- **软件更新**：vnt-cli 与 GUI 双通道版本检测 + 一键更新
- **体验**：首次启动向导、深浅主题、全局快捷键、开机自启隐藏窗口

## 构建

环境：Node.js 18+、Rust、tauri-cli 2.x、Windows 10/11 x64

```bash
npm install
npm run tauri dev       # 开发模式
npm run tauri build     # 生产构建（NSIS）
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
