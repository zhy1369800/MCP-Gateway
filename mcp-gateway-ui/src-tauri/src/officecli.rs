//! OfficeCLI local install / detect commands (Tauri-native, no shell script).
//!
//! 设计要点：
//! - 只认我们自己下载到的固定路径和用户配置的路径，不依赖 PATH 刷新
//! - 下载走 reqwest + bytes_stream，真实进度通过 `officecli://progress` emit
//! - 失败返回 `release_url` 让前端兜底展示 GitHub 链接
//! - 开发模式 / 打包后行为一致

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// 全局互斥标志，防止并发安装写同一个临时文件
static INSTALLING: AtomicBool = AtomicBool::new(false);

const REPO: &str = "iOfficeAI/OfficeCLI";
const RELEASES_PAGE: &str = "https://github.com/iOfficeAI/OfficeCLI/releases/latest";
const PROGRESS_EVENT: &str = "officecli://progress";
/// 连接超时 & 读取超时（单个 chunk 无数据的最大等待时间）
const NETWORK_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub installed: bool,
    pub version: Option<String>,
    /// 真正用来执行的完整二进制路径（供前端写回 config）
    pub path: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InstallResult {
    pub ok: bool,
    pub installed_path: Option<String>,
    pub version: Option<String>,
    pub error: Option<String>,
    /// 失败时始终附带 releases 页面链接，让前端一键兜底
    pub release_url: String,
    pub asset_name: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload {
    downloaded: u64,
    total: u64,
    percent: u8,
}

fn binary_filename() -> &'static str {
    if cfg!(target_os = "windows") {
        "officecli.exe"
    } else {
        "officecli"
    }
}

/// 组装当前平台的 release asset 名（与 install.ps1 / install.sh 保持一致）
fn asset_name() -> Result<&'static str, String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match (os, arch) {
        ("windows", "x86_64") => Ok("officecli-win-x64.exe"),
        ("macos", "aarch64") => Ok("officecli-mac-arm64"),
        ("macos", "x86_64") => Ok("officecli-mac-x64"),
        ("linux", "x86_64") => Ok("officecli-linux-x64"),
        ("linux", "aarch64") => Ok("officecli-linux-arm64"),
        _ => Err(format!("unsupported platform: {os}/{arch}")),
    }
}

/// 我们自己管理的固定安装目录
fn managed_install_dir() -> Result<PathBuf, String> {
    let base = dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| "cannot resolve local data dir".to_string())?;
    Ok(base.join("mcp-gateway").join("officecli"))
}

/// 老脚本装到的目录（存量兼容）
fn legacy_install_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(target_os = "windows") {
        if let Some(local) = dirs::data_local_dir() {
            out.push(local.join("OfficeCLI").join(binary_filename()));
        }
    } else if let Some(home) = dirs::home_dir() {
        out.push(home.join(".local").join("bin").join(binary_filename()));
    }
    out
}

/// 把用户填的"文件或目录"解析成实际二进制路径
fn resolve_user_path(input: &str) -> PathBuf {
    let p = PathBuf::from(input.trim());
    if p.is_dir() {
        p.join(binary_filename())
    } else {
        p
    }
}

/// 同步跑 `<binary> --version`，限时 5 秒，返回 trim 后的字符串（失败返回 None）
fn run_version_check(binary: &Path) -> Option<String> {
    #[cfg(target_os = "windows")]
    use std::os::windows::process::CommandExt;

    let mut cmd = std::process::Command::new(binary);
    cmd.arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null());
    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

    let mut child = cmd.spawn().ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let out = child.wait_with_output().ok()?;
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                return if s.is_empty() { Some(String::new()) } else { Some(s) };
            }
            Ok(Some(_)) => return None,
            Ok(None) if start.elapsed() < Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(80));
            }
            _ => {
                let _ = child.kill();
                return None;
            }
        }
    }
}



/// 按稳定顺序找一个能跑起来的 officecli：
/// 1) 用户手动路径（前端传进来的 hint，可能是文件/目录/空）
/// 2) 我们自己的固定安装目录
/// 3) 老脚本目录（`%LOCALAPPDATA%\OfficeCLI`、`~/.local/bin`）
///
/// 命中即返回真实绝对路径，配合 `--version` 验证可执行。
fn detect_officecli(hint: Option<&str>) -> Option<(PathBuf, String)> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(h) = hint.map(str::trim).filter(|s| !s.is_empty()) {
        candidates.push(resolve_user_path(h));
    }
    if let Ok(dir) = managed_install_dir() {
        candidates.push(dir.join(binary_filename()));
    }
    candidates.extend(legacy_install_paths());

    for c in candidates {
        if !c.is_file() {
            continue;
        }
        if let Some(ver) = run_version_check(&c) {
            return Some((c, ver));
        }
    }
    None
}

#[tauri::command]
pub async fn officecli_check(path: Option<String>) -> CheckResult {
    let hint_owned = path.clone();
    let res = tokio::task::spawn_blocking(move || detect_officecli(hint_owned.as_deref()))
        .await
        .ok()
        .flatten();

    match res {
        Some((p, ver)) => CheckResult {
            installed: true,
            version: if ver.is_empty() { None } else { Some(ver) },
            path: Some(p.to_string_lossy().into_owned()),
            error: None,
        },
        None => CheckResult {
            installed: false,
            version: None,
            path: None,
            error: None,
        },
    }
}

#[tauri::command]
pub async fn officecli_open_releases() -> Result<(), String> {
    open_url(RELEASES_PAGE)
}

#[cfg(target_os = "windows")]
fn open_url(url: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("open url failed: {e}"))
}

#[cfg(target_os = "macos")]
fn open_url(url: &str) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("open url failed: {e}"))
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn open_url(url: &str) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("open url failed: {e}"))
}

#[tauri::command]
pub async fn officecli_uninstall() -> Result<(), String> {
    let dir = managed_install_dir()?;
    if dir.exists() {
        fs::remove_dir_all(&dir)
            .await
            .map_err(|e| format!("remove install dir failed: {e}"))?;
    }
    Ok(())
}


/// 节流推进度：每 ~150ms 或最终完成时 emit 一次
struct ProgressEmitter {
    app: AppHandle,
    total: u64,
    last_emit: Instant,
    last_percent: u8,
}

impl ProgressEmitter {
    fn new(app: AppHandle, total: u64) -> Self {
        Self {
            app,
            total,
            last_emit: Instant::now() - Duration::from_secs(1),
            last_percent: 255,
        }
    }

    fn emit(&mut self, downloaded: u64, force: bool) {
        let percent = if self.total > 0 {
            ((downloaded as f64 / self.total as f64) * 100.0).clamp(0.0, 100.0) as u8
        } else {
            0
        };
        let elapsed = self.last_emit.elapsed();
        if !force && elapsed < Duration::from_millis(150) && percent == self.last_percent {
            return;
        }
        self.last_emit = Instant::now();
        self.last_percent = percent;
        let _ = self.app.emit(
            PROGRESS_EVENT,
            ProgressPayload {
                downloaded,
                total: self.total,
                percent,
            },
        );
    }
}

/// 流式下载到 `dest.tmp`，返回文件实际大小（字节）
async fn download_to_temp(
    client: &reqwest::Client,
    url: &str,
    tmp: &Path,
    emitter: &mut ProgressEmitter,
) -> Result<u64, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    if emitter.total == 0 {
        emitter.total = resp.content_length().unwrap_or(0);
    }

    if let Some(parent) = tmp.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let mut file = fs::File::create(tmp)
        .await
        .map_err(|e| format!("create {}: {e}", tmp.display()))?;

    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;
    emitter.emit(0, true);
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("chunk error: {e}"))?;
        file.write_all(&bytes)
            .await
            .map_err(|e| format!("write: {e}"))?;
        downloaded += bytes.len() as u64;
        emitter.emit(downloaded, false);
    }
    file.flush().await.map_err(|e| format!("flush: {e}"))?;
    drop(file);
    emitter.emit(downloaded, true);
    Ok(downloaded)
}

/// 拉 SHA256SUMS 对比，拿不到或对不上返回 Err（调用方决定是否致命）
async fn verify_sha256(
    client: &reqwest::Client,
    checksum_url: &str,
    asset: &str,
    file: &Path,
) -> Result<(), String> {
    let text = client
        .get(checksum_url)
        .send()
        .await
        .map_err(|e| format!("checksum fetch: {e}"))?
        .error_for_status()
        .map_err(|e| format!("checksum http: {e}"))?
        .text()
        .await
        .map_err(|e| format!("checksum read: {e}"))?;

    let expected = text
        .lines()
        .find(|line| line.contains(asset))
        .and_then(|line| line.split_whitespace().next())
        .ok_or_else(|| "checksum line not found".to_string())?
        .to_ascii_lowercase();

    let bytes = fs::read(file).await.map_err(|e| format!("read: {e}"))?;
    let actual = hex::encode(Sha256::digest(&bytes));
    if actual == expected {
        Ok(())
    } else {
        Err(format!("sha256 mismatch: expected {expected}, got {actual}"))
    }
}


fn failure(release_url: &str, asset: &str, err: impl Into<String>) -> InstallResult {
    InstallResult {
        ok: false,
        installed_path: None,
        version: None,
        error: Some(err.into()),
        release_url: release_url.to_string(),
        asset_name: asset.to_string(),
    }
}

#[tauri::command]
pub async fn officecli_install(app: AppHandle) -> InstallResult {
    let asset = match asset_name() {
        Ok(a) => a,
        Err(e) => return failure(RELEASES_PAGE, "", e),
    };
    let install_dir = match managed_install_dir() {
        Ok(d) => d,
        Err(e) => return failure(RELEASES_PAGE, asset, e),
    };
    let final_path = install_dir.join(binary_filename());
    let tmp_path = install_dir.join(format!("{}.download", binary_filename()));

    let download_url = format!(
        "https://github.com/{REPO}/releases/latest/download/{asset}"
    );
    let checksum_url = format!(
        "https://github.com/{REPO}/releases/latest/download/SHA256SUMS"
    );

    // 防止并发安装
    if INSTALLING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return failure(RELEASES_PAGE, asset, "installation already in progress".to_string());
    }

    let result = do_install(app, asset, install_dir, final_path, tmp_path, &download_url, &checksum_url).await;
    INSTALLING.store(false, Ordering::SeqCst);
    result
}

async fn do_install(
    app: AppHandle,
    asset: &str,
    install_dir: PathBuf,
    final_path: PathBuf,
    tmp_path: PathBuf,
    download_url: &str,
    checksum_url: &str,
) -> InstallResult {
    let _ = &install_dir; // suppress unused warning

    let client = match reqwest::Client::builder()
        .user_agent("mcp-gateway-ui-officecli-installer/1.0")
        .connect_timeout(NETWORK_TIMEOUT)
        .read_timeout(NETWORK_TIMEOUT)
        .build()
    {
        Ok(c) => c,
        Err(e) => return failure(RELEASES_PAGE, asset, format!("http client: {e}")),
    };

    // 发初始 0% 事件，UI 能立刻显示进度条
    let mut emitter = ProgressEmitter::new(app.clone(), 0);
    emitter.emit(0, true);

    // 1) 下载
    if let Err(e) = download_to_temp(&client, &download_url, &tmp_path, &mut emitter).await {
        let _ = fs::remove_file(&tmp_path).await;
        return failure(RELEASES_PAGE, asset, format!("download failed: {e}"));
    }

    // 2) SHA256 校验（失败 warn，不致命——老 release 可能没挂 SHA256SUMS）
    match verify_sha256(&client, &checksum_url, asset, &tmp_path).await {
        Ok(()) => eprintln!("[officecli] sha256 verified"),
        Err(e) => eprintln!("[officecli] sha256 verification skipped: {e}"),
    }

    // 3) Unix 下给可执行位
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(&tmp_path).await {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = fs::set_permissions(&tmp_path, perms).await;
        }
    }

    // 4) 原子落地：rename tmp -> final（已存在先删，Windows 上 rename 不覆盖）
    if final_path.exists() {
        if let Err(e) = fs::remove_file(&final_path).await {
            let _ = fs::remove_file(&tmp_path).await;
            return failure(
                RELEASES_PAGE,
                asset,
                format!("remove old binary: {e}"),
            );
        }
    }
    if let Err(e) = fs::rename(&tmp_path, &final_path).await {
        let _ = fs::remove_file(&tmp_path).await;
        return failure(RELEASES_PAGE, asset, format!("rename: {e}"));
    }

    // 5) 跑一次 --version 做真实可执行校验
    let final_for_check = final_path.clone();
    let version = tokio::task::spawn_blocking(move || run_version_check(&final_for_check))
        .await
        .ok()
        .flatten();

    if version.is_none() {
        return failure(
            RELEASES_PAGE,
            asset,
            "downloaded binary is not executable (--version failed)".to_string(),
        );
    }

    // 确保最终 100%
    let mut done = ProgressEmitter::new(app.clone(), 1);
    done.emit(1, true);

    InstallResult {
        ok: true,
        installed_path: Some(final_path.to_string_lossy().into_owned()),
        version: version.and_then(|v| if v.is_empty() { None } else { Some(v) }),
        error: None,
        release_url: RELEASES_PAGE.to_string(),
        asset_name: asset.to_string(),
    }
}
