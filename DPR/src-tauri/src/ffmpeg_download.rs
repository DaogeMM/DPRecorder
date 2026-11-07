//! 自动下载 FFmpeg（用户在「未找到 FFmpeg」的提示窗口里点「尝试下载」时走这里）。
//!
//! 把官方的 win64 gpl zip 拉下来，只解出里面的 `bin/ffmpeg.exe` 放到**程序目录下的 `ffmpeg\`**
//! —— `render::find_ffmpeg()` 认这个位置，设置里那格空着也能直接用。
//!
//! 那个目录建不出来或不可写时（装在 Program Files、被受控文件夹拦住……）改放到配置目录的 `ffmpeg\`，
//! 并把路径写进配置的 `ffmpeg_path`（`find_ffmpeg()` 的第一条就是它）。
//!
//! 下载与解压的进度通过 `ffmpeg-download` 事件推给前端，弹窗里显示百分比。

use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use anyhow::{bail, Context, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;

use crate::common::{self, CONFIG_DIR};

/// 下载源：两个都是 zip，里面都有 `bin/ffmpeg.exe`。
///
/// 第一个是 BtbN 的 `latest` 直链 —— 不查 GitHub API，也就不占它的匿名限流；
/// 第二个是 gyan.dev 的常规包，第一个不通时兜一下。
const URLS: [&str; 2] = [
    "https://github.com/BtbN/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-gpl.zip",
    "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip",
];

/// 进度事件的载荷。`stage` 取值 `download` / `extract`；`total` 未知（服务器没给长度）时是 `null`。
#[derive(Clone, Serialize)]
struct Progress {
    stage: &'static str,
    downloaded: u64,
    total: Option<u64>,
}

fn emit(app: &AppHandle, stage: &'static str, downloaded: u64, total: Option<u64>) {
    // 前端没开（CLI 渲染、子进程）时没人听，发送失败无所谓
    let _ = app.emit(
        "ffmpeg-download",
        Progress {
            stage,
            downloaded,
            total,
        },
    );
}

fn exe_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

/// 放 ffmpeg 的目录：程序目录下的 `ffmpeg\`（`find_ffmpeg()` 认这里，不用改任何配置），
/// 那个目录建不出来或不可写时，退到配置目录下的 `ffmpeg\`。
fn target_dir() -> Result<PathBuf> {
    if let Some(dir) = common::exe_dir() {
        // 自己下一份就单独占一个子目录：程序目录根上已经被 assets/、config/ 占着，
        // 再摊一个 160 MB 的 exe 进去，用户自己整理文件时分不清哪个是程序的、哪个是下载的
        let dir = dir.join("ffmpeg");
        if std::fs::create_dir_all(&dir).is_ok() && common::is_dir_writable(&dir) {
            return Ok(dir);
        }
    }
    let dir = CONFIG_DIR
        .get()
        .context("配置目录尚未初始化")?
        .join("ffmpeg");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建目录失败：{}", dir.display()))?;
    if !common::is_dir_writable(&dir) {
        bail!("目录不可写：{}", dir.display());
    }
    Ok(dir)
}

/// 用户在下载弹窗里点了「取消」。下载循环每读一块查一次，换源与解压前也各查一次。
///
/// 用一个全局开关而不是 `JoinHandle::abort`：reqwest 的响应流在 `await` 里，abort 会在
/// 半路丢掉 `.part` 文件，还得再收拾一遍；这里让循环自己顺着错误路径退出去，清理都在原地。
static CANCELLED: AtomicBool = AtomicBool::new(false);

/// 请求取消正在进行的下载。没在下也照样记下来，下一次 `download_ffmpeg` 开头会清掉。
pub fn cancel_download() {
    CANCELLED.store(true, Ordering::Relaxed);
}

fn cancelled() -> bool {
    CANCELLED.load(Ordering::Relaxed)
}

/// 下载 → 解出 `ffmpeg.exe` → 验一下能跑。返回 ffmpeg 的完整路径。
pub async fn download_ffmpeg(app: &AppHandle) -> Result<String> {
    if !cfg!(target_os = "windows") {
        // 非 Windows 的官方包是 tar.xz，解压要多一个依赖；这类平台仍旧让用户手动放到程序目录
        bail!("当前平台暂不支持自动下载，请手动下载 FFmpeg 后放到程序目录");
    }

    CANCELLED.store(false, Ordering::Relaxed);
    let dir = target_dir()?;
    let zip_path = dir.join("ffmpeg-download.zip");
    let target = dir.join(exe_name());

    crate::console!("[FFmpeg] 自动下载到 {}", dir.display());
    let mut last_error: Option<anyhow::Error> = None;
    for url in URLS {
        crate::console!("[FFmpeg] 下载源：{url}");
        match download_zip(app, url, &zip_path).await {
            Ok(()) => {
                last_error = None;
                break;
            }
            Err(err) => {
                remove_partial(&zip_path);
                // 取消不是「这个源坏了」：别再试第二个源，也别在界面上当失败报
                if cancelled() {
                    crate::console!("[FFmpeg] 已取消");
                    return Err(err.context("下载已取消"));
                }
                crate::console!(warn, "[FFmpeg] 这个源失败：{}", common::error_chain(&err));
                last_error = Some(err);
            }
        }
    }
    if let Some(err) = last_error {
        return Err(err.context("所有下载源都失败了"));
    }
    if cancelled() {
        remove_partial(&zip_path);
        bail!("下载已取消");
    }

    emit(app, "extract", 0, None);
    let zip_for_extract = zip_path.clone();
    let target_for_extract = target.clone();
    tokio::task::spawn_blocking(move || extract_ffmpeg(&zip_for_extract, &target_for_extract))
        .await
        .context("解压任务失败")??;
    let _ = std::fs::remove_file(&zip_path);

    if !crate::render::test_ffmpeg(&target) {
        let _ = std::fs::remove_file(&target);
        bail!("下载到的 ffmpeg 无法运行：{}", target.display());
    }

    // 落在配置目录时把它记进配置；放程序目录 `ffmpeg\` 的那份不用记，`find_ffmpeg()` 自己认得
    let in_exe_dir = common::exe_dir().is_some_and(|exe_dir| dir == exe_dir.join("ffmpeg"));
    if !in_exe_dir {
        let mut config = common::read_config()?;
        config.ffmpeg_path = Some(dir.to_string_lossy().to_string());
        common::save_config(config)?;
    }

    let path = target.display().to_string();
    crate::console!("[FFmpeg] 已就绪：{path}");
    emit(app, "done", 0, None);
    Ok(path)
}

/// 下到一半的那个 `.part` 与下完的 zip 一起删掉 —— 取消或换源时都不留垃圾。
fn remove_partial(zip_path: &Path) {
    let _ = std::fs::remove_file(zip_path);
    let _ = std::fs::remove_file(zip_path.with_file_name("ffmpeg-download.zip.part"));
}

/// 把 `url` 拉进 `dest`，边下边把进度推给前端。
async fn download_zip(app: &AppHandle, url: &str, dest: &Path) -> Result<()> {
    let client = tauri_plugin_http::reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        // 130 MB 的包在慢网下要十几分钟，给足；真卡死时由这个兜底收场
        .timeout(Duration::from_secs(1800))
        .build()?;
    let mut response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("请求失败：{url}"))?
        .error_for_status()
        .with_context(|| format!("服务器返回错误：{url}"))?;
    let total = response.content_length();

    // 先写 `.part`，下完再改名 —— 中途断掉不会留下一个能被当真的 zip
    let part = dest.with_file_name("ffmpeg-download.zip.part");
    let mut file = tokio::fs::File::create(&part)
        .await
        .with_context(|| format!("写入失败：{}", part.display()))?;
    let mut downloaded = 0u64;
    let mut reported = 0u64;
    emit(app, "download", 0, total);
    while let Some(chunk) = response.chunk().await? {
        if cancelled() {
            drop(file);
            bail!("下载已取消");
        }
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        // 每 1 MB 报一次：130 MB 的包发出上千条事件纯属浪费，界面也用不上这么细
        if downloaded - reported >= 1 << 20 {
            reported = downloaded;
            emit(app, "download", downloaded, total);
        }
    }
    file.flush().await?;
    drop(file);
    if let Some(total) = total {
        if downloaded < total {
            bail!("下载不完整：{downloaded} / {total} 字节");
        }
    }
    std::fs::rename(&part, dest)?;
    Ok(())
}

/// 从 zip 里只取 `bin/ffmpeg.exe`（各家的压缩包里目录名不一样，按路径后缀认）。
fn extract_ffmpeg(zip_path: &Path, target: &Path) -> Result<()> {
    let file =
        std::fs::File::open(zip_path).with_context(|| format!("打开失败：{}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("这个文件不是 zip")?;
    let part = target.with_file_name("ffmpeg.exe.part");
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().replace('\\', "/").to_ascii_lowercase();
        if !(name.ends_with("bin/ffmpeg.exe") || name == "ffmpeg.exe") {
            continue;
        }
        let mut out = std::fs::File::create(&part)
            .with_context(|| format!("写入失败：{}", part.display()))?;
        io::copy(&mut entry, &mut out).context("解压 ffmpeg.exe 失败")?;
        out.flush()?;
        drop(out);
        std::fs::rename(&part, target)?;
        return Ok(());
    }
    bail!("压缩包里没有 bin/ffmpeg.exe");
}
