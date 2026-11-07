//! 「噪域播放器」的几条 tauri 命令。
//!
//! 转换本身在 `src/dpe/noise_player.rs`（与编辑器那一侧的预览进程放在同一个目录），
//! 这里只补 tauri 这一侧必须存在的东西：找 ffmpeg、给默认输出目录、把错误翻成 `InvokeError`。
//! 模块按路径引用而不是把预览进程那几个模块也拉进来 —— 这里只需要 `noise_player` 这一段纯逻辑。

#[path = "dpe/noise_player.rs"]
mod noise_player;

pub use noise_player::{ChartReport, ConversionProgress, ToolOutput, VideoMeta, VideoNoiseParams};

use anyhow::{anyhow, Result};
use std::path::PathBuf;
use tauri::ipc::InvokeError;

/// ffmpeg 的位置：与渲染那边共用同一份设置。
fn ffmpeg() -> Result<String> {
    crate::render::find_ffmpeg()?.ok_or_else(|| anyhow!("tool-video.no-ffmpeg"))
}

/// 参数里没指定输出目录时写到这里：`<程序目录>/dpe/noisePlayer`。
/// 程序目录不可写（装进 Program Files、便携版落在受控文件夹里）就退回设置里的输出目录。
async fn default_dir() -> Result<PathBuf> {
    let preferred = crate::common::noise_player_dir()?;
    if tokio::fs::create_dir_all(&preferred).await.is_ok() {
        return Ok(preferred);
    }
    crate::common::get_output_dir()
}

#[tauri::command]
pub fn noise_player_progress() -> ConversionProgress {
    noise_player::progress()
}

#[tauri::command]
pub async fn noise_player_probe(path: String) -> Result<VideoMeta, InvokeError> {
    let ffmpeg = ffmpeg();
    crate::wrap_async(async move {
        let ffmpeg = ffmpeg?;
        noise_player::probe_video(&ffmpeg, &path).await
    })
    .await
}

#[tauri::command]
pub async fn noise_player_convert(params: VideoNoiseParams) -> Result<ChartReport, InvokeError> {
    let ffmpeg = ffmpeg();
    crate::wrap_async(async move {
        let ffmpeg = ffmpeg?;
        let dir = default_dir().await?;
        noise_player::convert(&ffmpeg, &dir, params).await
    })
    .await
}

/// 打开谱面文件夹：直接复用程序里那个「打开目录」，免得再写一套 explorer 兜底。
#[tauri::command]
pub fn noise_player_reveal(dir: String) -> Result<(), InvokeError> {
    crate::open_dir(dir)
}

#[tauri::command]
pub async fn noise_player_outputs(dir: String) -> Result<Vec<ToolOutput>, InvokeError> {
    crate::wrap_async(async move { noise_player::outputs(&dir).await }).await
}
