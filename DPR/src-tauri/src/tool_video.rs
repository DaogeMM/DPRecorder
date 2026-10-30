//! 工具页「噪域播放器」：把一段视频逐格取色，写成一张**全程只有噪域**的官方格式谱面。
//!
//! 为什么产出谱面而不是视频：噪域逐像素的样子（底色、噪点、描边、辉光、岩浆）全在引擎的
//! 着色器里，`blockAreaList` 是那套着色器唯一的输入。写一张谱面既不必在这里重做一遍着色器，
//! 产出的谱面也能在渲染页里照常调参、预览、续渲。
//!
//! 时间轴：谱面里的 `0` 就是视频截取段的起点，`song.mp3` 也从这一点截到终点 —— 两边用同一把
//! 尺子，噪域与声音自然对得上（`info.yml` 里 `offset` 固定 0）。
//!
//! 坐标：谱面的百分比坐标原点在**左下角**、`(1, 1)` 在右上角；视频的像素行从上往下，所以
//! 第 `row` 行（0 起）落在 `y ∈ [1 - (row + 1) / rows, 1 - row / rows]`。

use std::{
    cmp::Ordering,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        Mutex,
    },
};

use anyhow::{anyhow, bail, Context, Result};
use dps::core::simple_block_area;
use serde::{Deserialize, Serialize};
use tauri::ipc::InvokeError;
use tokio::io::AsyncReadExt;

/// 抽帧时最多保留多少格。再大只是把谱面写成一坨、渲染也跑不动。
const MAX_CELLS: u32 = 65_536;
/// 颜色合并容差的上限（与 0~255 的颜色分量同量纲）。
const MAX_TOLERANCE: u32 = 64;

// ---------------------------------------------------------------------------
// 进度：抽帧在上一秒几十帧地跑，前端按固定间隔轮询这个静态量
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConversionProgress {
    pub running: bool,
    /// `probe` / `audio` / `frames` / `chart` / `done` / `failed`
    pub stage: String,
    pub frame: u64,
    pub frames: u64,
}

static PROGRESS: Mutex<ConversionProgress> = Mutex::new(ConversionProgress {
    running: false,
    stage: String::new(),
    frame: 0,
    frames: 0,
});

/// 同一时间只跑一趟：两趟同时抽帧既抢 CPU，又会往同一个输出目录里写。
static BUSY: AtomicBool = AtomicBool::new(false);

fn report(stage: &str, frame: u64, frames: u64) {
    if let Ok(mut progress) = PROGRESS.lock() {
        progress.running = true;
        progress.stage = stage.to_owned();
        progress.frame = frame;
        progress.frames = frames;
    }
}

fn report_done(stage: &str) {
    if let Ok(mut progress) = PROGRESS.lock() {
        progress.running = false;
        progress.stage = stage.to_owned();
    }
}

#[tauri::command]
pub fn tool_video_progress() -> ConversionProgress {
    PROGRESS.lock().map(|it| it.clone()).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// 参数与返回值
// ---------------------------------------------------------------------------

fn default_rate() -> f64 {
    8.0
}

fn default_tolerance() -> u32 {
    6
}

fn default_keep_audio() -> bool {
    true
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VideoNoiseParams {
    /// 视频文件路径。
    pub video: String,
    /// 横向格数（纵向按视频宽高比算，或由 `rows` 指定）。
    pub cols: u32,
    /// 纵向格数，0 = 按宽高比自动算。
    #[serde(default)]
    pub rows: u32,
    /// 每秒取多少次色，也就是谱面里颜色更新的频率。
    #[serde(default = "default_rate")]
    pub rate: f64,
    /// 截取起点（秒）。
    #[serde(default)]
    pub start: f64,
    /// 截取终点（秒），`<= start` 表示到视频结尾。
    #[serde(default)]
    pub end: f64,
    /// 同一格颜色变化不超过它（0~255）就延用上一条噪域 —— 谱面条目数几乎由它决定。
    #[serde(default = "default_tolerance")]
    pub tolerance: u32,
    /// 保留视频原声；没有音轨或被关掉时生成等长静音。
    #[serde(default = "default_keep_audio")]
    pub keep_audio: bool,
    /// 输出画面的宽高比（宽 / 高）；0 = 跟视频自己的比例。
    ///
    /// 写进谱面的 `info.yml` 与自带的 `config.toml`（带 `forceAspectRatio`）：格子仍是
    /// 视频里那一块的颜色，只是整张谱面按选定的比例铺满画面。
    #[serde(default)]
    pub aspect: f64,
    /// 谱面名（同时是输出的 `.pez` 文件名）；空 = 视频文件名。
    #[serde(default)]
    pub name: String,
    /// 输出目录；空 = `<程序目录>/tools/blockAreaPlayer`。
    #[serde(default)]
    pub output_dir: String,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct VideoMeta {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub has_audio: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChartReport {
    /// 生成的谱面包（`.pez`）。谱面文件夹的四个文件平铺在包根上，渲染页「导入谱面」直接认它。
    pub file: String,
    /// 谱面包所在的文件夹。
    pub dir: String,
    /// `blockAreaList` 的条目数。
    pub entries: u64,
    pub cols: u32,
    pub rows: u32,
    /// 实际读到的帧数。
    pub frames: u64,
    /// 截取段的秒数。
    pub duration: f64,
}

// ---------------------------------------------------------------------------
// 命令
// ---------------------------------------------------------------------------

async fn wrap<T>(future: impl std::future::Future<Output = Result<T>>) -> Result<T, InvokeError> {
    future.await.map_err(|e| {
        tracing::error!("{e:?}");
        InvokeError::from_anyhow(e)
    })
}

#[tauri::command]
pub async fn tool_video_probe(path: String) -> Result<VideoMeta, InvokeError> {
    wrap(async move {
        let video = PathBuf::from(path.trim());
        if !video.is_file() {
            bail!("tool-video.video-missing");
        }
        probe(&ffmpeg()?, &video).await
    })
    .await
}

#[tauri::command]
pub async fn tool_video_convert(params: VideoNoiseParams) -> Result<ChartReport, InvokeError> {
    wrap(async move {
        if BUSY.swap(true, AtomicOrdering::SeqCst) {
            bail!("tool-video.busy");
        }
        let result = convert(params).await;
        BUSY.store(false, AtomicOrdering::SeqCst);
        report_done(if result.is_ok() { "done" } else { "failed" });
        result
    })
    .await
}

/// 打开谱面文件夹：直接复用程序里那个「打开目录」，免得再写一套 explorer 兜底。
#[tauri::command]
pub fn tool_video_reveal(dir: String) -> Result<(), InvokeError> {
    crate::open_dir(dir)
}

/// 「输出列表」里的一行。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutput {
    /// 文件名（带 `.pez` 后缀）。
    pub name: String,
    /// 绝对路径，前端拿它去渲染页载入。
    pub path: String,
    /// 字节数。
    pub size: u64,
    /// 修改时间（毫秒时间戳）；取不到时是 0。
    pub modified: i64,
}

/// 列出输出目录里的 `.pez`，按修改时间从新到旧（新的在上面）。
///
/// 目录还不存在（一次都没生成过）时返回空列表，不报错 —— 前端那栏就显示「还没有生成过谱面」。
#[tauri::command]
pub async fn tool_video_outputs(dir: String) -> Result<Vec<ToolOutput>, InvokeError> {
    wrap(async move {
        let dir = PathBuf::from(dir.trim());
        if dir.as_os_str().is_empty() || !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut outputs = Vec::new();
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let is_pez = path
                .extension()
                .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("pez"));
            if !is_pez || !path.is_file() {
                continue;
            }
            let Ok(metadata) = entry.metadata().await else {
                continue;
            };
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|delta| delta.as_millis() as i64)
                .unwrap_or(0);
            outputs.push(ToolOutput {
                name: path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default(),
                path: path.to_string_lossy().to_string(),
                size: metadata.len(),
                modified,
            });
        }
        outputs.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.name.cmp(&b.name)));
        Ok(outputs)
    })
    .await
}

// ---------------------------------------------------------------------------
// 主流程
// ---------------------------------------------------------------------------

fn ffmpeg() -> Result<String> {
    crate::render::find_ffmpeg()?.ok_or_else(|| anyhow!("tool-video.no-ffmpeg"))
}

async fn convert(params: VideoNoiseParams) -> Result<ChartReport> {
    let video = PathBuf::from(params.video.trim());
    if !video.is_file() {
        bail!("tool-video.video-missing");
    }
    let ffmpeg = ffmpeg()?;
    report("probe", 0, 0);
    let meta = probe(&ffmpeg, &video).await?;

    // 截取段：终点越界就收到视频结尾，起点夹在片子中间。
    let start = params.start.max(0.).min((meta.duration - 0.05).max(0.));
    let end = if params.end > start + 0.05 {
        params.end.min(meta.duration)
    } else {
        meta.duration
    };
    let duration = end - start;
    if duration <= 0.05 {
        bail!("tool-video.range-empty");
    }

    let cols = params.cols.clamp(2, 256);
    let rows = if params.rows == 0 {
        ((cols as f64 * meta.height as f64 / meta.width as f64).round() as u32)
            .max(1)
            .min(256)
    } else {
        params.rows.clamp(1, 256)
    };
    if cols * rows > MAX_CELLS {
        bail!("tool-video.grid-too-large");
    }
    let rate = params.rate.clamp(1., 60.);
    let tolerance = params.tolerance.min(MAX_TOLERANCE) as u8;
    // 输出比例：0 = 跟视频；选了比例就夹一下（免得写出 0 或无穷大那种宽高比）
    let video_aspect = meta.width as f64 / meta.height as f64;
    let aspect = if params.aspect > 0. {
        params.aspect.clamp(0.1, 10.)
    } else {
        video_aspect
    };
    let force_aspect = params.aspect > 0.;

    let name = {
        let requested = params.name.trim();
        if requested.is_empty() {
            video
                .file_stem()
                .map(|it| it.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            requested.to_owned()
        }
    };
    let name = sanitize(&name);

    // 输出目录先算出来：谱面包直接在这里成型，不再经临时目录 —— 谱面 JSON 有几十 MB，
    // 先写进临时目录、打包时再读回来压缩，等于把同样的字节在磁盘上搬两遍。
    let dir = if params.output_dir.trim().is_empty() {
        let preferred = crate::common::video_tool_dir()?;
        // 程序目录不可写（装进 Program Files、便携版落在受控文件夹里）就退回设置里的输出目录
        if tokio::fs::create_dir_all(&preferred).await.is_ok() {
            preferred
        } else {
            crate::common::get_output_dir()?
        }
    } else {
        PathBuf::from(params.output_dir.trim())
    };
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("tool-video.output-dir: {}", dir.display()))?;
    let pez = dir.join(format!("{name}.pez"));

    // 音频与曲绘先取到内存（几百 KB 到几 MB）：它们要成为包里的条目，而条目一旦开始写
    // 就没法撤销，所以等 ffmpeg 真交出内容之后再动手。背景取不到也不打断整趟转换。
    report("audio", 0, 0);
    let (music, music_data) = make_audio(
        &ffmpeg,
        &video,
        start,
        duration,
        meta.has_audio && params.keep_audio,
    )
    .await?;
    let cover = make_cover(&ffmpeg, aspect).await.ok();

    // 打成**一个 `.pez`**（谱面文件夹的四个文件平铺在包根上，渲染页「导入谱面」直接认
    // 这种包）。先写 `<名字>.pez.part`，全部成型才改名，半路失败不会留下半套文件。
    // 压缩级别取 3：谱面 JSON 是这一趟里最大的一块（一份 60 s / 847k 条的谱面约 144 MB
    // 文本），实测 9 级 4.5 s、6 级 2.9 s、3 级 2.1 s、1 级 1.6 s，而 3 级只比 6 级大 4%
    // （1 级要大一倍多）—— 再往上调省不下几个字节，纯粹烧时间。
    let mut pack = crate::common::ZipPack::create(&pez, 3)?;
    pack.bytes(
        "info.yml",
        info_yaml(&name, duration, aspect, &music, cover.is_some())?.as_bytes(),
    )?;
    pack.bytes(
        "config.toml",
        chart_config_toml(aspect, force_aspect, meta.width, meta.height, meta.fps).as_bytes(),
    )?;
    pack.bytes(&music, &music_data)?;
    if let Some(cover) = &cover {
        pack.bytes("background.png", cover)?;
    }

    let total = (duration * rate).round().max(1.) as u64;
    report("frames", 0, total);
    let mut grid = Grid::new(cols, rows, tolerance);
    decode_frames(
        &ffmpeg, &video, start, duration, cols, rows, rate, total, &mut grid,
    )
    .await?;
    let frames = grid.frames;
    grid.close_all(duration);
    let runs = grid.finish();

    let entries = runs.len() as u64;
    report("chart", 0, entries);
    // 谱面 JSON 直接从内存流进 zip 里那个条目：不落盘，也就没有「写完再读回来压缩」那一趟。
    pack.entry("chart.json", |out| write_chart(out, &runs, cols, rows))?;
    // 收尾：写完中央目录再改名成 `.pez`，这一步只剩几毫秒
    report("save", 0, 0);
    pack.finish()?;

    Ok(ChartReport {
        file: pez.display().to_string(),
        dir: dir.display().to_string(),
        entries,
        cols,
        rows,
        frames,
        duration,
    })
}

// ---------------------------------------------------------------------------
// ffmpeg：探测 / 抽帧 / 音频 / 封面
// ---------------------------------------------------------------------------

async fn probe(ffmpeg: &str, video: &Path) -> Result<VideoMeta> {
    // 不指定输出文件：ffmpeg 把流信息打到 stderr 后以非 0 退出，这里要的就是那段文字，
    // 所以退出码不看。
    let output = crate::cmd_hidden(ffmpeg)
        .args(["-hide_banner", "-nostdin"])
        .arg("-i")
        .arg(video)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| "tool-video.ffmpeg-spawn")?;
    let text = String::from_utf8_lossy(&output.stderr).into_owned();

    let mut meta = VideoMeta::default();
    let duration = regex::Regex::new(r"Duration:\s*(\d+):(\d+):(\d+(?:\.\d+)?)").unwrap();
    if let Some(caps) = duration.captures(&text) {
        let hours: f64 = caps[1].parse().unwrap_or(0.);
        let minutes: f64 = caps[2].parse().unwrap_or(0.);
        let seconds: f64 = caps[3].parse().unwrap_or(0.);
        meta.duration = hours * 3600. + minutes * 60. + seconds;
    }
    let size = regex::Regex::new(r"(\d{2,5})x(\d{2,5})").unwrap();
    let fps = regex::Regex::new(r"(\d+(?:\.\d+)?)\s+fps").unwrap();
    // 封面图（attached picture）同样是一条 `Video:` 流，取面积最大的那条才是正片。
    for line in text
        .lines()
        .filter(|line| line.contains("Video:") && !line.contains("attached pic"))
    {
        let Some(caps) = size.captures(line) else {
            continue;
        };
        let width: u32 = caps[1].parse().unwrap_or(0);
        let height: u32 = caps[2].parse().unwrap_or(0);
        if width as u64 * height as u64 > meta.width as u64 * meta.height as u64 {
            meta.width = width;
            meta.height = height;
        }
        if let Some(caps) = fps.captures(line) {
            meta.fps = caps[1].parse().unwrap_or(0.);
        }
    }
    meta.has_audio = text.lines().any(|line| line.contains("Audio:"));
    if meta.duration <= 0. || meta.width == 0 || meta.height == 0 {
        bail!("tool-video.probe-failed");
    }
    Ok(meta)
}

/// 逐帧取色。`scale` 把整帧缩到 `cols × rows`：`area` 是平均采样，
/// 缩小时比默认的双线性更接近「这一格的平均颜色」。
#[allow(clippy::too_many_arguments)]
async fn decode_frames(
    ffmpeg: &str,
    video: &Path,
    start: f64,
    duration: f64,
    cols: u32,
    rows: u32,
    rate: f64,
    total: u64,
    grid: &mut Grid,
) -> Result<()> {
    let filter = format!("fps={rate:.6},scale={cols}:{rows}:flags=area+accurate_rnd+full_chroma_int");
    let mut child = crate::cmd_hidden(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-v", "error"])
        .arg("-ss")
        .arg(format!("{start:.3}"))
        .arg("-i")
        .arg(video)
        .arg("-t")
        .arg(format!("{duration:.3}"))
        .arg("-an")
        .args(["-vf", &filter])
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| "tool-video.ffmpeg-spawn")?;
    let mut stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    // stderr 必须有人一直读：管道写满时 ffmpeg 会卡住，最后表现成「读帧超时」。
    let stderr_reader = tokio::spawn(async move {
        let mut bytes = Vec::new();
        let _ = stderr.take(16 * 1024).read_to_end(&mut bytes).await;
        String::from_utf8_lossy(&bytes).into_owned()
    });

    let mut frame = vec![0u8; (cols * rows * 3) as usize];
    let mut index = 0u64;
    loop {
        match stdout.read_exact(&mut frame).await {
            Ok(_) => {
                grid.push_frame(&frame, index as f64 / rate);
                index += 1;
                if index % 4 == 0 {
                    report("frames", index, total);
                }
            }
            // 末尾不足一帧的残片直接丢掉：`-t` 截出来的最后一帧本来就可能只写了一半。
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(err) => {
                let _ = child.kill().await;
                bail!("tool-video.decode-failed: {err}");
            }
        }
    }
    let status = child.wait().await?;
    let stderr = stderr_reader.await.unwrap_or_default();
    if !status.success() {
        bail!("tool-video.decode-failed: {}", tail(&stderr));
    }
    if index == 0 {
        bail!("tool-video.decode-failed: {}", tail(&stderr));
    }
    grid.frames = index;
    Ok(())
}

/// 取音频：先试原声（mp3，编不出来退 wav），都不行就铺等长静音。返回 (包里的文件名, 内容)。
///
/// 谱面里的 `music` 必须存在 —— 渲染管线开跑第一件事就是把它读进来（读不到直接报错），
/// 所以这里无论如何都要交出一份音频。内容走管道读进内存：只有几 MB，换来的是失败时
/// 可以整个换一套参数重试，而 zip 条目一旦开始写就没法撤销。
async fn make_audio(
    ffmpeg: &str,
    video: &Path,
    start: f64,
    duration: f64,
    keep: bool,
) -> Result<(String, Vec<u8>)> {
    /// 编码器、包里的文件名、容器格式（`-f`）。
    type Attempt = (Vec<String>, &'static str, &'static str);
    let mut attempts: Vec<Attempt> = Vec::new();
    let input = video.display().to_string();
    let seconds = format!("{duration:.3}");
    for (codec, file, format) in [
        ("libmp3lame", "song.mp3", "mp3"),
        ("pcm_s16le", "song.wav", "wav"),
    ] {
        if !keep {
            break;
        }
        let mut args = source_args(start, &input, &seconds);
        args.extend(["-c:a".into(), codec.into()]);
        args.extend(mp3_bitrate(codec));
        args.extend(["-f".into(), format.into(), "pipe:1".into()]);
        attempts.push((args, file, format));
    }
    for (codec, file, format) in [
        ("libmp3lame", "song.mp3", "mp3"),
        ("pcm_s16le", "song.wav", "wav"),
    ] {
        let mut args: Vec<String> = [
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=44100:cl=stereo",
            "-t",
            &seconds,
            "-c:a",
            codec,
        ]
        .iter()
        .map(|it| it.to_string())
        .collect();
        args.extend(mp3_bitrate(codec));
        args.extend(["-f".into(), format.into(), "pipe:1".into()]);
        attempts.push((args, file, format));
    }

    for (args, file, format) in attempts {
        if let Ok(mut data) = capture_ffmpeg(ffmpeg, &args).await {
            // wav 写到管道时 ffmpeg 没法回填两处长度（头里会留 0xFFFFFFFF），在这里补上
            if format == "wav" {
                fix_wav_sizes(&mut data);
            }
            return Ok((file.to_owned(), data));
        }
    }
    bail!("tool-video.audio-failed");
}

/// 修一下流式写出的 wav：`RIFF` 与 `data` 两处大小 ffmpeg 写管道时填不回去，
/// 留着 0xFFFFFFFF 会让读回来的一方算错时长。
fn fix_wav_sizes(data: &mut [u8]) {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return;
    }
    let length = (data.len() - 8) as u32;
    data[4..8].copy_from_slice(&length.to_le_bytes());
    let mut at = 12;
    while at + 8 <= data.len() {
        let size = u32::from_le_bytes([data[at + 4], data[at + 5], data[at + 6], data[at + 7]]) as usize;
        // 块之间按偶数字节对齐
        let next = at + 8 + size + (size & 1);
        if &data[at..at + 4] == b"data" {
            let length = (data.len() - at - 8) as u32;
            data[at + 4..at + 8].copy_from_slice(&length.to_le_bytes());
            return;
        }
        at = next;
    }
}

/// `-ss` 放输入之前是「先定位再解码」，比解码到起点再丢弃快得多。
fn source_args(start: f64, input: &str, seconds: &str) -> Vec<String> {
    [
        "-hide_banner".to_owned(),
        "-nostdin".to_owned(),
        "-v".to_owned(),
        "error".to_owned(),
        "-ss".to_owned(),
        format!("{start:.3}"),
        "-i".to_owned(),
        input.to_owned(),
        "-t".to_owned(),
        seconds.to_owned(),
        "-vn".to_owned(),
        "-ac".to_owned(),
        "2".to_owned(),
        "-ar".to_owned(),
        "44100".to_owned(),
    ]
    .to_vec()
}

fn mp3_bitrate(codec: &str) -> Vec<String> {
    if codec == "libmp3lame" {
        vec!["-b:a".to_owned(), "192k".to_owned()]
    } else {
        Vec::new()
    }
}

/// 曲绘：**一张纯白图**（谱面列表缩略图与画面背景都用它）。
///
/// 不用视频首帧：噪域之间总有缝，背景色会从缝里透出来，首帧那种花底会让整幅画面发闷。
/// 尺寸按谱面宽高比算（`forceAspectRatio` 打开时列表缩略图也才是那个比例）。
/// 图片走管道读进内存（几 KB），调用方按「取不到就跳过」处理。
async fn make_cover(ffmpeg: &str, aspect: f64) -> Result<Vec<u8>> {
    let height = ((1280. / aspect.clamp(0.1, 10.)).round() as u32).clamp(2, 4096) & !1;
    let source = format!("color=c=0xFFFFFF:s=1280x{height}:d=1");
    let args = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        &source,
        "-frames:v",
        "1",
        "-f",
        "image2pipe",
        "-c:v",
        "png",
        "pipe:1",
    ]
    .iter()
    .map(|it| it.to_string())
    .collect::<Vec<_>>();
    capture_ffmpeg(ffmpeg, &args).await
}

/// 跑一趟 ffmpeg，把它写到 stdout 的内容整段收下来（`pipe:1` 的产物）。
async fn capture_ffmpeg(ffmpeg: &str, args: &[String]) -> Result<Vec<u8>> {
    let result = crate::cmd_hidden(ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| "tool-video.ffmpeg-spawn")?;
    if !result.status.success() {
        bail!("{}", tail(&String::from_utf8_lossy(&result.stderr)));
    }
    if result.stdout.is_empty() {
        bail!("ffmpeg 没有输出内容");
    }
    Ok(result.stdout)
}

/// 报错时只留最后几行：ffmpeg 的 stderr 前面全是无害的横幅与警告。
fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().filter(|it| !it.trim().is_empty()).collect();
    let from = lines.len().saturating_sub(3);
    lines[from..].join(" | ")
}

// ---------------------------------------------------------------------------
// 取色与行程编码
// ---------------------------------------------------------------------------

struct Run {
    cell: u32,
    start: f64,
    end: f64,
    color: [u8; 3],
}

/// 逐格攒「颜色连续的一段」（行程编码）。
///
/// 每一段最后写成一条噪域。不做这层合并的话，条目数就是 `格数 × 帧数`，
/// 一分钟的视频能写出上百万条 —— 谱面文件上百 MB，渲染时每帧还要把所有条目扫一遍。
struct Grid {
    tolerance: u8,
    /// 每格当前那段颜色；`None` = 还没开始（或已收口）。
    open: Vec<Option<[u8; 3]>>,
    start: Vec<f64>,
    runs: Vec<Run>,
    frames: u64,
}

impl Grid {
    fn new(cols: u32, rows: u32, tolerance: u8) -> Self {
        let cells = (cols * rows) as usize;
        Self {
            tolerance,
            open: vec![None; cells],
            start: vec![0.; cells],
            runs: Vec::new(),
            frames: 0,
        }
    }

    fn push_frame(&mut self, frame: &[u8], time: f64) {
        for cell in 0..self.open.len() {
            let at = cell * 3;
            let color = [frame[at], frame[at + 1], frame[at + 2]];
            match self.open[cell] {
                Some(previous) if within(previous, color, self.tolerance) => {}
                Some(_) => {
                    self.close(cell, time);
                    self.start_at(cell, time, color);
                }
                None => self.start_at(cell, time, color),
            }
        }
    }

    fn start_at(&mut self, cell: usize, time: f64, color: [u8; 3]) {
        self.open[cell] = Some(color);
        self.start[cell] = time;
    }

    fn close(&mut self, cell: usize, end: f64) {
        let Some(color) = self.open[cell].take() else {
            return;
        };
        let start = self.start[cell];
        // 零长的段在谱面里永远不可见，不如不写。
        if end > start + 1e-4 {
            self.runs.push(Run {
                cell: cell as u32,
                start,
                end,
                color,
            });
        }
    }

    fn close_all(&mut self, end: f64) {
        for cell in 0..self.open.len() {
            self.close(cell, end);
        }
    }

    /// 按时间排一遍：谱面条目通常也按时间顺序读，排好之后文件里的顺序就是播放顺序。
    fn finish(mut self) -> Vec<Run> {
        self.runs.sort_by(|a, b| {
            a.start
                .partial_cmp(&b.start)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.cell.cmp(&b.cell))
        });
        self.runs
    }
}

fn within(a: [u8; 3], b: [u8; 3], tolerance: u8) -> bool {
    a.iter()
        .zip(b.iter())
        .all(|(x, y)| x.abs_diff(*y) <= tolerance)
}

// ---------------------------------------------------------------------------
// 写谱面 / info.yml
// ---------------------------------------------------------------------------

/// 一条**不可见**的判定线：`alpha` 恒 0，没有音符。
///
/// 输出只要噪域，判定线与按键都不该出现；但留一条线比留空列表稳 ——
/// 引擎里几处「取第一条线」的地方不必为此多一个分支。
const JUDGE_LINE: &str = concat!(
    r#"{"bpm":120.0,"#,
    r#""judgeLineDisappearEvents":[{"startTime":0.0,"endTime":0.0,"start":0.0,"end":0.0,"start2":0.0,"end2":0.0}],"#,
    r#""judgeLineRotateEvents":[{"startTime":0.0,"endTime":0.0,"start":0.0,"end":0.0}],"#,
    r#""judgeLineMoveEvents":[{"startTime":0.0,"endTime":0.0,"start":0.0,"end":0.0,"start2":0.0,"end2":0.0}],"#,
    r#""speedEvents":[{"startTime":0.0,"endTime":0.0,"value":1.0}],"#,
    r#""notesAbove":[],"notesBelow":[]}"#,
);

/// 把谱面 JSON 写进 `out` —— 打包时 `out` 就是 zip 里那个条目，所以这几十 MB 文本不落盘，
/// 写完也不必再读回来。每 4096 条报一次进度（这一阶段的分母是噪域条目数）。
///
/// 噪域写的是紧凑表 `simpleBlockArea`（格式见 `dps::core::simple_block_area`）：一条记录
/// 6~9 字节，同一份画面比「一条一个 JSON 对象」小一个数量级。这不只是省磁盘 —— 上百万个
/// 对象能把 chart.json 撑到几 GB，而 zip 的单个条目超过 4 GiB 会直接写不进去。
fn write_chart(out: &mut dyn Write, runs: &[Run], cols: u32, rows: u32) -> Result<()> {
    let mut out = BufWriter::with_capacity(1 << 20, out);
    out.write_all(br#"{"formatVersion":3,"offset":0.0,"judgeLineList":["#)?;
    out.write_all(JUDGE_LINE.as_bytes())?;
    let data = simple_block_area::pack(
        runs.iter()
            .map(|run| (run.cell as u16, millis(run.start), millis(run.end), run.color)),
        |done, total| report("chart", done, total),
    );
    out.write_all(br#"],"blockAreaList":[],"simpleBlockArea":"#)?;
    out.write_all(simple_block_area::field_prefix(cols, rows).as_bytes())?;
    out.write_all(simple_block_area::encode(&data).as_bytes())?;
    // 关掉 `data` 的引号、`simpleBlockArea` 对象、以及谱面根对象
    out.write_all(br#""}}"#)?;
    out.flush()?;
    Ok(())
}

/// 秒 → 毫秒（紧凑表的时间单位）。视频长过 49 天才会越界，直接截住。
fn millis(seconds: f64) -> u32 {
    (seconds.max(0.) * 1000.).round().min(u32::MAX as f64) as u32
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InfoFile {
    name: String,
    composer: String,
    charter: String,
    illustrator: String,
    level: String,
    /// 定数（数字），谱面难度文本在 `level` 里，见 `DPS::fs::info_from_yaml`。
    difficulty: f64,
    format: String,
    chart: String,
    music: String,
    illustration: String,
    aspect_ratio: f64,
    preview_start: f64,
    preview_end: f64,
    offset: f64,
    /// 背景压暗程度；纯白曲绘要写 0，否则白底会压成灰。
    background_dim: f32,
    tags: Vec<String>,
}

fn info_yaml(name: &str, duration: f64, aspect: f64, music: &str, cover: bool) -> Result<String> {
    let info = InfoFile {
        name: name.to_owned(),
        composer: name.to_owned(),
        charter: "DPRecorder".to_owned(),
        illustrator: "DPRecorder".to_owned(),
        level: "SP Lv.0".to_owned(),
        difficulty: 0.,
        format: "pgr".to_owned(),
        chart: "chart.json".to_owned(),
        music: music.to_owned(),
        illustration: if cover {
            "background.png".to_owned()
        } else {
            String::new()
        },
        aspect_ratio: aspect,
        preview_start: 0.,
        preview_end: duration,
        offset: 0.,
        background_dim: 0.,
        tags: vec!["噪域".to_owned()],
    };
    Ok(serde_yaml::to_string(&info)?)
}

/// 谱面自带的渲染配置（`config.toml`）：这张谱面只该长出噪域，别的元素一概不画。
///
/// 只有 DPRecorder 读它（引擎不读），键名与程序的 `config.toml` 一致（见
/// `chart_config::ALLOWED`）。写死的这几项是「噪域播放器的成品长什么样」：
/// 偏移 0（音画已经对齐）、总分 1、只留噪域；宽高比按工具页选的输出比例，
/// 选过比例才带 `forceAspectRatio`（没选就让画面自己按视频比例换边）。
/// 出片分辨率与帧率跟原视频一致（`resolution` / `fps`），成片就与视频同尺寸、同节奏。
/// 背景：`renderBg` 打开、压在 0 —— 曲绘是一张纯白图，底就是白的；要别的底色
/// 在渲染页「界面 → 纯色背景」里填，那项优先于这里。
fn chart_config_toml(aspect: f64, force_aspect: bool, width: u32, height: u32, fps: f64) -> String {
    // 帧率取整、至少 1：出片帧率是整数，视频里的 29.97 这种按四舍五入走
    let fps = fps.round().clamp(1., 240.) as u32;
    format!(
        "# 这张谱面自带的渲染配置：只留噪域，其它谱面元素全部关掉。\n\
         # 只有 DPRecorder 读它（引擎不读），键名与程序的 config.toml 一致。\n\
         offset = 0.0\n\
         scoreTotal = 1\n\
         aspectRatio = {aspect:.6}\n\
         forceAspectRatio = {force_aspect}\n\
         backgroundDim = 0.00\n\
         resolution = [{width}, {height}]\n\
         fps = {fps}\n\
         \n\
         renderBlockArea = true\n\
         renderLine = false\n\
         renderNote = false\n\
         renderDoubleHint = false\n\
         renderPendingNote = false\n\
         renderBg = true\n\
         renderUiPause = false\n\
         renderUiName = false\n\
         renderUiLevel = false\n\
         renderUiDifficulty = false\n\
         renderUiScore = false\n\
         renderUiCombo = false\n\
         renderUiBar = false\n"
    )
}

/// 文件名里不能出现的字符换掉，首尾的点与空白也去掉（Windows 上会被静默丢弃）。
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || r#"\/:*?"<>|"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim().to_owned();
    if trimmed.is_empty() {
        "噪域谱面".to_owned()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 端到端：ffmpeg 抽帧 → 写谱面 → 用引擎自己的解析器读回来。
    ///
    /// 需要一段真视频：把 `DPR_TOOL_VIDEO_TEST` 指过去才跑，否则直接跳过 ——
    /// 这个用例依赖本机文件，不能让别处的 `cargo test` 一律挂在这上面。
    #[test]
    fn writes_a_chart_the_engine_can_parse() {
        let Ok(path) = std::env::var("DPR_TOOL_VIDEO_TEST") else {
            return;
        };
        let video = PathBuf::from(path);
        if !video.is_file() {
            return;
        }

        // 产物写在视频旁边：`.tmp` 里的东西随时可删，不会落在仓库里
        let out = video.with_extension("vn-pez");
        let _ = std::fs::remove_dir_all(&out);

        // `find_ffmpeg` 会读配置，而配置目录是进程启动时一次性登记进 `CONFIG_DIR` 的；
        // 测试里没有那一步，这里自己指一个（别的测试先占住了就沿用它的）。
        let _ = crate::common::CONFIG_DIR.set(std::env::temp_dir().join("dpr-tool-video-test"));

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .build()
            .expect("建 tokio runtime");
        let report = runtime
            .block_on(convert(VideoNoiseParams {
                video: video.display().to_string(),
                cols: 24,
                rows: 0,
                rate: 4.,
                start: 0.,
                end: 2.,
                tolerance: 6,
                keep_audio: true,
                aspect: 0.,
                name: "vn-test".to_owned(),
                output_dir: out.display().to_string(),
            }))
            .expect("转换应当成功");

        assert!(report.entries > 0, "没有写出任何噪域");
        assert!(report.frames >= 4, "两秒的视频至少该取到几帧");

        // 产出一个 .pez：谱面文件夹的四个文件平铺在包根上
        let pez = Path::new(&report.file);
        assert!(pez.is_file(), "没有写出 .pez 文件");
        let mut archive = zip::ZipArchive::new(std::fs::File::open(pez).expect("打开 pez")).expect("读 pez");
        assert!(
            ["song.mp3", "song.wav"].iter().any(|it| archive.by_name(it).is_ok()),
            "缺少音频文件：渲染管线读不到 music 就起不来"
        );
        let (mut chart_text, mut info_text) = (String::new(), String::new());
        {
            use std::io::Read as _;
            archive
                .by_name("chart.json")
                .expect(".pez 里没有 chart.json")
                .read_to_string(&mut chart_text)
                .unwrap();
            archive
                .by_name("info.yml")
                .expect(".pez 里没有 info.yml")
                .read_to_string(&mut info_text)
                .unwrap();
        }
        assert!(
            info_text.contains("charter: DPRecorder"),
            "info.yml 里的谱师应当是 DPRecorder"
        );

        // 关键断言：引擎能把它当 PGR 谱面读进来，噪域条目一条不少
        let chart = dps::parse::parse_phigros(&chart_text, Default::default()).expect("引擎应当能解析");
        assert_eq!(
            chart.block_areas.len() as u64, report.entries,
            "读回来的噪域条数应当与写出的一致"
        );

        // 谱面自带的渲染配置：只留噪域、偏移 0、总分 1
        let forced = runtime
            .block_on(crate::chart_config::read(pez))
            .expect("读取 config.toml")
            .expect("谱面里应当带 config.toml");
        assert_eq!(forced.get("offset"), Some(&serde_json::json!(0.0)));
        assert_eq!(forced.get("scoreTotal"), Some(&serde_json::json!(1)));
        assert_eq!(forced.get("renderLine"), Some(&serde_json::json!(false)));
        assert_eq!(forced.get("renderBlockArea"), Some(&serde_json::json!(true)));
        // 背景：纯白曲绘要真的画出来（renderBg），压暗归零，白底才不会被压成灰
        assert_eq!(forced.get("renderBg"), Some(&serde_json::json!(true)));
        assert_eq!(forced.get("backgroundDim"), Some(&serde_json::json!(0.0)));
    }
}
