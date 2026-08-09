# VNT GUI

为开源项目 [vnt-dev/vnt](https://github.com/vnt-dev/vnt) 提供图形化界面的 **Windows 桌面应用**（Rust + Tauri 2）。

本应用不重新实现 VPN 逻辑：将官方编译的 `vnt-cli.exe` 作为 **Tauri Sidecar** 嵌入，由 Rust 后端管理其进程生命周期、采集输出、注入配置，React 前端提供可视化交互。

## 功能特性

| 类别 | 功能 |
|------|------|
| 连接管理 | vnt-cli Sidecar 启动/停止、5 态状态机（未连接/连接中/已连接/重连中/错误）、崩溃后指数退避自动重连（最多 10 次） |
| 系统集成 | 系统托盘（连接切换/显示隐藏/设置/退出）、关闭窗口最小化到托盘、开机自启（静默启动自动连接） |
| 配置管理 | 多配置 CRUD 与历史切换（`%APPDATA%\vnt-gui\config.json`）、配置 JSON 导入/导出、Token 显示脱敏 |
| 实时日志 | 环形缓冲（2000 行）、级别过滤、关键字搜索、导出文件、清空 |
| 流量统计 | Windows 原生 `GetIfTable2` 采集虚拟网卡收发增量（无需安装 Npcap）、Recharts 折线图、速率告警（系统通知） |
| 设备列表 | 每 5 秒轮询 `vnt-cli --list` 解析在线设备（P2P/中继、延迟、复制 IP） |
| 软件更新 | **vnt-cli 版本检测**（对比本地与 GitHub 上游 vnt-dev/vnt 最新 release，一键下载替换）；**GUI 版本检测**（对比本仓库 w2018/vnt-gui-dev 的 GitHub Releases） |
| 体验 | 首次启动向导、深色/浅色主题、Ctrl+Shift+V 全局快捷键（显示/隐藏窗口） |

## 技术栈

- **前端**：React 19 + TypeScript + Vite 7 + Ant Design 5 + Zustand + Recharts
- **桌面壳**：Tauri 2（tray-icon / shell / autostart / dialog / fs / notification / global-shortcut 插件）
- **后端**：Rust（tokio / reqwest / semver / zip / windows-sys）

## 目录结构

```
vnt-gui/
├── src/                        # React 前端
│   ├── components/             # StatusPanel / ConfigForm / ConfigHistory / LogViewer / TrafficChart / DeviceList / UpdateDialog / SettingsPanel / FirstRunWizard
│   ├── stores/                 # Zustand：connection / config / log / traffic / device
│   └── lib/                    # tauri.ts（invoke 封装 + 事件监听）、types.ts、theme.ts
├── src-tauri/
│   ├── src/                    # Rust 后端
│   │   ├── lib.rs              # Tauri 入口、插件注册、18 个 tauri::command
│   │   ├── sidecar.rs          # vnt-cli 进程管理（spawn/停止/输出解析/重连）★核心
│   │   ├── config.rs           # 配置持久化（%APPDATA%/vnt-gui/config.json）
│   │   ├── tray.rs             # 系统托盘
│   │   ├── logger.rs           # 日志环形缓冲
│   │   ├── traffic.rs          # GetIfTable2 流量统计
│   │   ├── updater.rs          # vnt-cli / GUI 版本检测与一键更新
│   │   ├── state.rs            # 全局状态（AppState）
│   │   └── autostart.rs
│   ├── binaries/               # sidecar：vnt-cli-x86_64-pc-windows-msvc.exe + wintun.dll
│   └── capabilities/default.json
├── .github/workflows/build.yml # CI：Windows 构建（自动下载 vnt-cli sidecar）
└── package.json
```

## 环境要求与构建

- Node.js 18+（本机已验证 v24）、Rust 1.77.2+（tauri 2 要求）、tauri-cli 2.x
- Windows 10/11 x86_64，WebView2（Win11 自带）

```bash
npm install

# 开发模式（前端热更新 + Tauri 窗口）
npm run tauri dev

# 生产构建（NSIS 安装包）
npm run tauri build
# 产物：src-tauri/target/release/bundle/nsis/VNT GUI_*.x64-setup.exe
```

> **注意**：项目路径含非 ASCII 字符（如中文目录）时，`vite` 会解析 junction 导致构建路径错乱，请直接在真实路径下运行 npm/vite 命令；cargo 构建可走 junction 路径。

### Sidecar 二进制

`src-tauri/binaries/` 需要放置（构建期 tauri-build 会检查存在性）：

| 文件 | 来源 |
|------|------|
| `vnt-cli-x86_64-pc-windows-msvc.exe` | [vnt-dev/vnt Releases](https://github.com/vnt-dev/vnt/releases) 下载 `vnt-cli-x86_64-pc-windows-msvc.zip` 解压 |
| `wintun.dll` | [Wintun](https://www.wintun.net/) 官方发布（amd64） |

CI 工作流会自动下载最新 vnt-cli。

## 使用说明

1. 首次启动进入向导 → 环境检测通过后开始使用
2. 「配置」页新建组网配置（**组网编号 Token 必填**，可选虚拟 IP/服务器/协议等）
3. 首页或「连接」页点击「连接」，连接成功后显示虚拟 IP 与设备列表
4. 关闭窗口即最小化到托盘；托盘右键可快速连接/退出

## 更新机制

- **vnt-cli 更新**：「更新」页点击检查更新 → 对比本地 vnt-cli 版本与 [vnt-dev/vnt](https://github.com/vnt-dev/vnt/releases) 最新 release → 一键下载替换（原子替换，失败自动回滚）
- **GUI 更新**：对比本仓库 [w2018/vnt-gui-dev](https://github.com/w2018/vnt-gui-dev/releases) 的 Releases（发布新版本时打 `v*` tag 并附带安装包即可自动检测）

## 已知限制

- 本机中文路径下 WiX（MSI 打包）不可用，生产交付使用 NSIS
- 设备列表依赖 `vnt-cli --list` 输出格式，为尽力解析（未连接时返回空）
- Token/密码明文存储于 `%APPDATA%\vnt-gui\config.json`（keyring/DPAPI 加密列为后续增强）
- 托盘图标暂用 Tauri 默认图标（5 态共用，tooltip 区分状态）

## 致谢

- [vnt-dev/vnt](https://github.com/vnt-dev/vnt) —— 底层组网工具（GPL-3.0）
- [Tauri](https://tauri.app/) / [Ant Design](https://ant.design/)
