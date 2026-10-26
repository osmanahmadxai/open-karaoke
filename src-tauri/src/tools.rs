//! First-run installation of what Open Karaoke needs besides itself:
//! yt-dlp (fetches audio), Deno (the JavaScript runtime yt-dlp needs to read
//! YouTube) and the AI vocal-separation model. All come from their official
//! GitHub releases into the app data folder, so users need nothing but an
//! internet connection.

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;

pub fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

pub fn ytdlp_path(dir: &Path) -> PathBuf {
    dir.join(exe("yt-dlp"))
}

pub fn deno_path(dir: &Path) -> PathBuf {
    dir.join(exe("deno"))
}

pub fn model_path(dir: &Path) -> PathBuf {
    dir.join("models").join(crate::audio::separate::MODEL.file)
}

fn ytdlp_asset() -> Result<&'static str, String> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", _) => "yt-dlp_macos",
        ("windows", "aarch64") => "yt-dlp_arm64.exe",
        ("windows", _) => "yt-dlp.exe",
        ("linux", "aarch64") => "yt-dlp_linux_aarch64",
        ("linux", _) => "yt-dlp_linux",
        (os, arch) => return Err(format!("Unsupported platform {os}/{arch}")),
    })
}

fn deno_asset() -> Result<&'static str, String> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "deno-aarch64-apple-darwin.zip",
        ("macos", _) => "deno-x86_64-apple-darwin.zip",
        ("windows", "aarch64") => "deno-aarch64-pc-windows-msvc.zip",
        ("windows", _) => "deno-x86_64-pc-windows-msvc.zip",
        ("linux", "aarch64") => "deno-aarch64-unknown-linux-gnu.zip",
        ("linux", _) => "deno-x86_64-unknown-linux-gnu.zip",
        (os, arch) => return Err(format!("Unsupported platform {os}/{arch}")),
    })
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub ready: bool,
    pub ytdlp: bool,
    pub deno: bool,
    pub model: bool,
}

pub fn status(dir: &Path) -> ToolStatus {
    let ytdlp = ytdlp_path(dir).is_file();
    let deno = deno_path(dir).is_file();
    let model = model_path(dir).is_file();
    ToolStatus { ready: ytdlp && deno && model, ytdlp, deno, model }
}

#[derive(Serialize, Clone)]
struct SetupProgress<'a> {
    step: &'a str,
    label: &'a str,
    fraction: f64,
}

pub fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("Open Karaoke/", env!("CARGO_PKG_VERSION"), " (open-source karaoke)"))
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .expect("http client")
}

async fn download(app: &AppHandle, url: &str, dest: &Path, step: &str, label: &str) -> Result<(), String> {
    let mut resp = http()
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Download failed ({label}): {e}"))?;
    let total = resp.content_length().unwrap_or(0) as f64;
    let part = dest.with_extension("part");
    let mut file = tokio::fs::File::create(&part).await.map_err(|e| e.to_string())?;
    let mut done = 0f64;
    let mut last_emit = 0f64;
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("Download interrupted ({label}): {e}"))? {
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        done += chunk.len() as f64;
        if total > 0.0 && (done - last_emit) / total > 0.01 {
            last_emit = done;
            let _ = app.emit("setup://progress", SetupProgress { step, label, fraction: done / total });
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    tokio::fs::rename(&part, dest).await.map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(unix)]
fn make_executable(p: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> Result<(), String> {
    Ok(())
}

pub async fn install(app: &AppHandle, dir: &Path) -> Result<ToolStatus, String> {
    tokio::fs::create_dir_all(dir).await.map_err(|e| e.to_string())?;

    if !ytdlp_path(dir).is_file() {
        let url = format!("https://github.com/yt-dlp/yt-dlp/releases/latest/download/{}", ytdlp_asset()?);
        download(app, &url, &ytdlp_path(dir), "ytdlp", "Audio downloader (yt-dlp)").await?;
        make_executable(&ytdlp_path(dir))?;
    }

    if !deno_path(dir).is_file() {
        let url = format!("https://github.com/denoland/deno/releases/latest/download/{}", deno_asset()?);
        let zip_path = dir.join("deno.zip");
        download(app, &url, &zip_path, "deno", "JavaScript runtime (Deno)").await?;
        let dir2 = dir.to_path_buf();
        let zp = zip_path.clone();
        tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
            let f = std::fs::File::open(&zp).map_err(|e| e.to_string())?;
            let mut archive = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
            let mut entry = archive.by_name(&exe("deno")).map_err(|e| format!("Deno archive: {e}"))?;
            let tmp = deno_path(&dir2).with_extension("part");
            let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
            drop(out);
            std::fs::rename(&tmp, deno_path(&dir2)).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??;
        let _ = tokio::fs::remove_file(&zip_path).await;
        make_executable(&deno_path(dir))?;
    }

    ensure_model(app, dir).await?;

    let _ = app.emit("setup://progress", SetupProgress { step: "done", label: "Ready", fraction: 1.0 });
    Ok(status(dir))
}

/// Download the separation model if missing, verifying its SHA-256.
pub async fn ensure_model(app: &AppHandle, dir: &Path) -> Result<PathBuf, String> {
    use sha2::{Digest, Sha256};
    let path = model_path(dir);
    if path.is_file() {
        return Ok(path);
    }
    let model = &crate::audio::separate::MODEL;
    tokio::fs::create_dir_all(path.parent().unwrap()).await.map_err(|e| e.to_string())?;
    let tmp = path.with_extension("download");
    download(app, model.url, &tmp, "model", "AI vocal separation model").await?;
    let check = tmp.clone();
    let digest = tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let mut file = std::fs::File::open(&check).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher).map_err(|e| e.to_string())?;
        Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
    })
    .await
    .map_err(|e| e.to_string())??;
    if digest != model.sha256 {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err("The downloaded AI model was corrupted. Please try again.".into());
    }
    tokio::fs::rename(&tmp, &path).await.map_err(|e| e.to_string())?;
    Ok(path)
}

/// yt-dlp must keep pace with YouTube; refresh it at most once a day.
pub async fn maybe_update_ytdlp(dir: &Path) {
    let stamp = dir.join(".ytdlp-updated");
    let fresh = std::fs::metadata(&stamp)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|age| age.as_secs() < 86_400)
        .unwrap_or(false);
    if fresh || !ytdlp_path(dir).is_file() {
        return;
    }
    let mut cmd = crate::youtube::command(dir);
    cmd.arg("-U");
    if let Ok(out) = cmd.output().await {
        if out.status.success() {
            let _ = std::fs::write(&stamp, b"");
        }
    }
}
