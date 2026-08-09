//! 版本检测与一键更新（文档 §3.9）
//!
//! 更新对象是 **vnt-cli 二进制**：对比本地 vnt-cli 版本与 GitHub 最新 release。
//! GUI 自身版本（app_version / app_has_update）独立返回，作为预留接口。

use std::io::Write;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_shell::ShellExt;

/// GitHub Releases API（文档 §3.9.1）
/// vnt-cli 上游仓库
const GITHUB_API: &str = "https://api.github.com/repos/vnt-dev/vnt/releases/latest";
/// GUI 自身仓库（本仓库，更新接口）
const GITHUB_API_APP: &str = "https://api.github.com/repos/w2018/vnt-gui-dev/releases/latest";

/// 更新信息
#[derive(Debug, Serialize)]
pub struct UpdateInfo {
    /// vnt-cli 是否有新版本
    pub has_update: bool,
    /// vnt-cli 最新版本号
    pub latest_version: String,
    /// 本地 vnt-cli 版本号
    pub current_version: String,
    pub download_url: Option<String>,
    /// GUI 应用版本（预留，独立于 vnt-cli）
    pub app_version: String,
    /// GUI 是否有更新（查询本仓库 w2018/vnt-gui-dev 的 GitHub Releases）
    pub app_has_update: bool,
    /// GUI 最新版本号（无 release 时为 None）
    pub app_latest_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// 获取本地 vnt-cli 版本（解析 `--help` 输出中的 version: 行）
pub async fn local_vnt_version(app: &AppHandle) -> Result<String, String> {
    let output = app
        .shell()
        .sidecar("vnt-cli")
        .map_err(|e| format!("sidecar 不可用: {}", e))?
        .args(["--help"])
        .output()
        .await
        .map_err(|e| format!("执行 vnt-cli --help 失败: {}", e))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text
        .lines()
        .find(|l| l.to_lowercase().contains("version"))
        .map(|l| l.trim().to_string())
        .unwrap_or_else(|| "未知版本".to_string());
    Ok(version)
}

/// 检查 vnt-cli 是否有新版本（对比本地 vnt-cli 版本与 GitHub 最新 release）
pub async fn check_update(app: &AppHandle) -> Result<UpdateInfo, String> {
    // 1. 本地 vnt-cli 版本
    let local_raw = local_vnt_version(app).await?;
    let local_ver = extract_version(&local_raw)
        .ok_or_else(|| format!("无法解析本地 vnt-cli 版本: {}", local_raw))?;

    // 2. GitHub 最新 release
    let client = Client::new();
    let resp = client
        .get(GITHUB_API)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "vnt-gui")
        .send()
        .await
        .map_err(|e| format!("请求 GitHub API 失败: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API 返回 {}", resp.status()));
    }

    let release: GithubRelease = resp
        .json()
        .await
        .map_err(|e| format!("解析 JSON 失败: {}", e))?;

    let latest_tag = release.tag_name.trim_start_matches('v').to_string();
    let latest_ver = extract_version(&latest_tag).unwrap_or(latest_tag.clone());

    // 3. semver 比较（任一解析失败则保守提示有更新）
    let has_update = match (
        semver::Version::parse(&latest_ver),
        semver::Version::parse(&local_ver),
    ) {
        (Ok(latest), Ok(local)) => latest > local,
        _ => latest_ver != local_ver,
    };

    // 4. 找到 Windows x86_64 的 asset
    let download_url = release
        .assets
        .iter()
        .find(|a| a.name.contains("x86_64-pc-windows-msvc"))
        .map(|a| a.browser_download_url.clone());

    // 5. GUI 自身更新检测（本仓库 GitHub Releases；失败/无 release 不影响主流程）
    let app_version = env!("CARGO_PKG_VERSION").to_string();
    let (app_has_update, app_latest_version) = check_app_update(&client, &app_version).await;

    Ok(UpdateInfo {
        has_update,
        latest_version: latest_ver,
        current_version: local_ver,
        download_url,
        app_version,
        app_has_update,
        app_latest_version,
    })
}

/// 查询本仓库最新 release，比较 GUI 版本
/// 返回 (是否有更新, 最新版本号)；查询失败或仓库无 release 时返回 (false, None)
async fn check_app_update(
    client: &Client,
    app_version: &str,
) -> (bool, Option<String>) {
    let resp = match client
        .get(GITHUB_API_APP)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "vnt-gui")
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return (false, None),
    };
    if !resp.status().is_success() {
        return (false, None); // 404 = 尚无 release
    }
    let release: Option<GithubRelease> = resp.json().await.ok();
    let Some(release) = release else {
        return (false, None);
    };
    let tag = release.tag_name.trim_start_matches('v').to_string();
    let latest = extract_version(&tag).unwrap_or(tag.clone());
    let has = match (
        semver::Version::parse(&latest),
        semver::Version::parse(app_version),
    ) {
        (Ok(l), Ok(c)) => l > c,
        _ => latest != app_version,
    };
    (has, Some(latest))
}

/// 从 "version:1.2.16" 或 "v1.2.17" 中提取纯版本号 "1.2.16"
fn extract_version(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let start = lower.find("version")?;
    let after = &lower[start + "version".len()..];
    // 跳过冒号/空格/v 前缀
    let after = after.trim_start_matches(|c: char| c == ':' || c == ' ' || c == 'v');
    let end = after
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(after.len());
    let ver = &after[..end];
    let parts: Vec<&str> = ver.split('.').collect();
    if parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
        Some(ver.to_string())
    } else {
        None
    }
}

/// 下载并原子替换 vnt-cli 二进制（文档 §3.9.2）
pub async fn download_and_replace(app: AppHandle, url: &str) -> Result<(), String> {
    let client = Client::new();
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("下载失败: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("下载失败: HTTP {}", resp.status()));
    }

    // 创建临时目录
    let tmp_dir = std::env::temp_dir().join("vnt-gui-update");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("创建临时目录失败: {}", e))?;
    let tmp_zip = tmp_dir.join("vnt-cli-new.zip");
    let mut file =
        std::fs::File::create(&tmp_zip).map_err(|e| format!("创建临时文件失败: {}", e))?;

    // 流式下载（带进度回调）
    let mut downloaded: u64 = 0;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("下载中断: {}", e))?
    {
        file.write_all(&chunk)
            .map_err(|e| format!("写入失败: {}", e))?;
        downloaded += chunk.len() as u64;
        let _ = app.emit(
            "update-progress",
            &serde_json::json!({ "downloaded": downloaded }),
        );
    }
    drop(file);

    // 解压 zip
    let extract_dir = tmp_dir.join("extracted");
    if extract_dir.exists() {
        std::fs::remove_dir_all(&extract_dir).ok();
    }
    std::fs::create_dir_all(&extract_dir).map_err(|e| format!("创建解压目录失败: {}", e))?;
    extract_zip(&tmp_zip, &extract_dir)?;

    // 找到 vnt-cli.exe
    let new_exe = find_vnt_cli(&extract_dir)?;

    // 原子替换：先重命名旧文件为 .old，再移动新文件
    // 运行时 sidecar 位于资源根目录（与 exe 同级），文件名无平台三元组后缀
    let resource_dir = app
        .path()
        .resolve("", tauri::path::BaseDirectory::Resource)
        .map_err(|e| e.to_string())?;

    let target_exe = resource_dir.join("vnt-cli.exe");
    let old_exe = resource_dir.join("vnt-cli.exe.old");

    if target_exe.exists() {
        std::fs::rename(&target_exe, &old_exe).map_err(|e| format!("备份旧版本失败: {}", e))?;
    }

    std::fs::copy(&new_exe, &target_exe).map_err(|e| format!("替换二进制失败: {}", e))?;

    // 清理
    std::fs::remove_dir_all(&tmp_dir).ok();
    std::fs::remove_file(&old_exe).ok();

    let _ = app.emit("update-complete", &serde_json::json!({ "version": "new" }));
    Ok(())
}

fn extract_zip(zip_path: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let out_path = dest.join(entry.name());
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path).ok();
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            let mut out_file = std::fs::File::create(&out_path).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out_file).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn find_vnt_cli(dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            if let Ok(found) = find_vnt_cli(&path) {
                return Ok(found);
            }
        } else if let Some(name) = path.file_name() {
            let name = name.to_string_lossy().to_lowercase();
            if name.contains("vnt-cli") && name.ends_with(".exe") {
                return Ok(path);
            }
        }
    }
    Err("解压后未找到 vnt-cli.exe".to_string())
}
