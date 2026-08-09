# VNT GUI —— Sidecar 二进制放置说明

Tauri 2 sidecar 机制要求二进制按平台三元组命名，放置于本目录：

| 平台 | 文件名 |
|------|--------|
| Windows x86_64 | `vnt-cli-x86_64-pc-windows-msvc.exe` |
| Windows ARM64 | `vnt-cli-aarch64-pc-windows-msvc.exe` |

## 需要放置的文件

1. **`vnt-cli-x86_64-pc-windows-msvc.exe`** —— 从 [vnt-dev/vnt Releases](https://github.com/vnt-dev/vnt/releases) 下载官方编译的 `vnt-cli-x86_64-pc-windows-msvc.zip` 解压得到
2. **`wintun.dll`** —— 与 exe 同目录（TUN 虚拟网卡驱动，缺失时程序会告警）

> 程序启动时会检查该文件是否存在，缺失则提示。构建前必须就位。
