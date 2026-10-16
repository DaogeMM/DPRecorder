use crate::{
    cmd_hidden,
    common::{get_output_dir, get_tasks_file, read_config, remove_file_retry},
    ipc::IPCEvent,
    render::{RenderConfig, RenderParams},
    ASSET_PATH,
};
use anyhow::Result;
use chrono::Local;
use humantime::format_duration;
use dps::{fs, info::ChartInfo};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    io::Write,
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, Mutex, Semaphore},
    task::JoinHandle,
    time::sleep,
};
use tracing::{error, info};

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum TaskStatus {
    Pending,
    Loading,
    Mixing,
    MixingSfx {
        progress: f64,
    },
    Rendering {
        progress: f64,
        fps: u64,
        estimate: f64,
    },
    Paused {
        progress: f64,
    },
    Done {
        duration: f64,
        output: String,
    },
    Canceled {
        output: String,
    },
    Failed {
        output: String,
    },
}

/// 落盘用的任务快照，见 `save_tasks` / `restore_queue`。
///
/// `Task` 本身没法序列化（`Mutex` / `AtomicBool` / `NamedTempFile` 都没实现），
/// 但它能**完全从 `RenderParams` 重建**（`Task::new` 里读谱面、解出曲绘、算输出路径），
/// 所以只要把参数存下来就能复活。另外把当时的输出路径与耗时一起存：重启后按当前配置
/// 重算文件名可能得到别的路径（用户可能改过「文件名格式」），历史记录不该因此指错文件。
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedTask {
    id: u32,
    params: RenderParams,
    output: PathBuf,
    status: SavedStatus,
    #[serde(default)]
    duration: f64,
}

/// 重启后怎么复活一条任务。只有 `Pending` 是「还没渲染完」：
/// `Loading` / `Mixing*` / `Rendering` / `Paused` 这些中途状态没有意义了
/// （渲染子进程已经随程序退出被杀），一律回到等待队列，重启后接着排队渲染。
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SavedStatus {
    Pending,
    Done,
    Canceled,
    Failed,
}

/// 读回上次的任务队列。文件不存在、是空的、或者内容坏了都只当成空队列并记一条日志 ——
/// 不能让一个坏掉的文件把启动卡住。
fn read_saved_tasks() -> Vec<SavedTask> {
    let file = match get_tasks_file() {
        Ok(file) => file,
        Err(err) => {
            error!("Failed to locate tasks.json: {err:?}");
            return Vec::new();
        }
    };
    if !file.exists() {
        return Vec::new();
    }
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(err) => {
            error!("Failed to read tasks.json: {err:?}");
            return Vec::new();
        }
    };
    if text.trim().is_empty() {
        return Vec::new();
    }
    match serde_json::from_str::<Vec<SavedTask>>(&text) {
        Ok(saved) => saved,
        Err(err) => {
            error!("Failed to parse tasks.json: {err:?}");
            Vec::new()
        }
    }
}

fn write_saved_tasks(saved: &[SavedTask]) -> Result<()> {
    let file = get_tasks_file()?;
    std::fs::write(file, serde_json::to_string_pretty(saved)?)?;
    Ok(())
}

/// 把共享的任务列表写进 `tasks.json`。存不下来不该影响渲染，所以失败只记日志。
async fn save_tasks(tasks: &Arc<Mutex<Vec<Arc<Task>>>>) {
    let saved = {
        let guard = tasks.lock().await;
        let mut saved = Vec::with_capacity(guard.len());
        for task in guard.iter() {
            saved.push(task.snapshot().await);
        }
        saved
    };
    if let Err(err) = write_saved_tasks(&saved) {
        error!("Failed to save task queue: {err:?}");
    }
}

/// 把落盘的队列装回共享列表：顺序、id、状态（包括已完成的历史）照原样，
/// 中途被打断的一律回到 `Pending`（见 `SavedStatus`）。
async fn restore_queue(tasks: &Arc<Mutex<Vec<Arc<Task>>>>, next_id: &AtomicU32) {
    let saved = read_saved_tasks();
    if saved.is_empty() {
        return;
    }
    let mut restored: Vec<Arc<Task>> = Vec::with_capacity(saved.len());
    let mut max_id = 0;
    for item in saved {
        let id = item.id;
        // `Task::new` 要读谱面、解出曲绘：谱面文件被删/被改坏时这一条就还原不了，
        // 跳过它、别让整个队列读不回来。
        match Task::restore(item).await {
            Ok(task) => {
                max_id = max_id.max(id);
                restored.push(Arc::new(task));
            }
            Err(err) => error!("Failed to restore task #{id}: {err:?}"),
        }
    }
    if restored.is_empty() {
        return;
    }
    info!("Restored {} task(s) from tasks.json", restored.len());
    // id 只增不减（见 `next_id` 的注释）：新任务不能和读回来的任务撞 id
    next_id.fetch_max(max_id + 1, Ordering::Relaxed);
    tasks.lock().await.extend(restored);
    // 立刻回写一遍：中途状态在文件里也一并规整成 `Pending`，还原不了的条目顺带清掉
    save_tasks(tasks).await;
}

pub fn generate_filename(info: &ChartInfo, config: &RenderConfig) -> String {
    let re = Regex::new(r"%([^%]+)%").unwrap();
    let input = config.file_name_format.as_str();
    let mut output = input.to_string();

    let match_config = |key: &str| -> String {
        match key {
            "resolution" => format!("{}x{}", config.resolution.0, config.resolution.1),
            "ending_length" => config.ending_length.to_string(),
            "render_loading" => config.render_loading.to_string(),
            "hires" => config.hires.to_string(),
            "fps" => config.fps.to_string(),
            "hardware_accel" => config.hardware_accel.to_string(),
            "hevc" => config.hevc.to_string(),
            "mpeg4" => config.mpeg4.to_string(),
            "custom_encoder" => config.custom_encoder.clone().unwrap_or_default(),
            "dynamic_bitrate_control" => config.dynamic_bitrate_control.to_string(),
            "bitrate" => config.bitrate.clone(),
            "aggressive_chart" => config.aggressive_chart.to_string(),
            "aggressive_note" => config.aggressive_note.to_string(),
            "aggressive_particle" => config.aggressive_particle.to_string(),
            "challenge_color" => config.challenge_color.to_string(),
            "challenge_rank" => config.challenge_rank.to_string(),
            "note_scale" => config.note_scale.to_string(),
            "particle" => config.particle.to_string(),
            "particle_square" => config.particle_square.to_string(),
            "player_avatar" => config.player_avatar.clone().unwrap_or_default(),
            "player_name" => config.player_name.clone(),
            "player_rks" => config.player_rks.to_string(),
            "sample_count" => config.sample_count.to_string(),
            "fxaa" => config.fxaa.to_string(),
            "res_pack_path" => config.res_pack_path.clone().unwrap_or_default(),
            "speed" => config.speed.to_string(),
            "volume_music" => config.volume_music.to_string(),
            "volume_sfx" => config.volume_sfx.to_string(),
            "force_limit" => config.force_limit.to_string(),
            "limit_threshold" => config.limit_threshold.to_string(),
            "loudness_equalization" => config.loudness_equalization.to_string(),
            "audio_mix_optimization" => config.audio_mix_optimization.to_string(),
            "chart_debug_line" => config.chart_debug_line.to_string(),
            "chart_debug_note" => config.chart_debug_note.to_string(),
            "chart_ratio" => config.chart_ratio.to_string(),
            "all_good" => config.all_good.to_string(),
            "all_bad" => config.all_bad.to_string(),
            "watermark" => config.watermark.clone(),
            "notecolor" => config.notecolor.clone(),
            "hitcolor" => config.hitcolor.clone(),
            "linecolor" => config.linecolor.clone(),
            "roman" => config.roman.to_string(),
            "chinese" => config.chinese.to_string(),
            "combo" => config.combo.clone(),
            "diff" => config.diff.clone(),
            "lvl" => config.lvl.to_string(),
            "difficulty" => config.difficulty.clone(),
            "judge_offset" => config.judge_offset.to_string(),
            "render_line" => config.render_line.to_string(),
            "render_line_extra" => config.render_line_extra.to_string(),
            "render_note" => config.render_note.to_string(),
            "render_double_hint" => config.render_double_hint.to_string(),
            "render_ui_pause" => config.render_ui_pause.to_string(),
            "render_ui_name" => config.render_ui_name.to_string(),
            "render_ui_level" => config.render_ui_level.to_string(),
            "render_ui_difficulty" => config.render_ui_difficulty.to_string(),
            "render_ui_score" => config.render_ui_score.to_string(),
            "render_ui_combo" => config.render_ui_combo.to_string(),
            "render_ui_bar" => config.render_ui_bar.to_string(),
            "render_bg" => config.render_bg.to_string(),
            "render_bg_dim" => config.render_bg_dim.to_string(),
            "render_extra" => config.render_extra.to_string(),
            "preserve_framebuffer" => config.preserve_framebuffer.to_string(),
            "bg_blurriness" => config.bg_blurriness.to_string(),
            "max_particles" => config.max_particles.to_string(),
            "play_start_time" => config.play_start_time.to_string(),
            "play_end_time" => config
                .play_end_time
                .map_or_else(String::new, |v| v.to_string()),
            "fade" => config.fade.to_string(),
            "alpha_tint" => config.alpha_tint.to_string(),
            _ => key.to_string(),
        }
    };

    let match_info = |key: &str| -> String {
        match key {
            "id" => info.id.map_or_else(String::new, |v| v.to_string()),
            "uploader" => info.uploader.map_or_else(String::new, |v| v.to_string()),
            "name" => info.name.clone(),
            "difficulty" => info.difficulty.clone(),
            "diff" => info.diff.clone(),
            "lvl" => info.lvl.to_string(),
            "charter" => info.charter.clone(),
            "composer" => info.composer.clone(),
            "illustrator" => info.illustrator.clone(),
            "chart" => info.chart.clone(),
            "music" => info.music.clone(),
            "illustration" => info.illustration.clone(),
            "preview_start" => info.preview_start.to_string(),
            "preview_end" => info.preview_end.map_or_else(String::new, |v| v.to_string()),
            "aspect_ratio" => info.aspect_ratio.to_string(),
            "force_aspect_ratio" => info.force_aspect_ratio.to_string(),
            "background_dim" => info.background_dim.to_string(),
            "line_length" => info.line_length.to_string(),
            "offset" => info.offset.to_string(),
            "tip" => info.tip.clone().unwrap_or_default(),
            "tags" => info.tags.join(","),
            "intro" => info.intro.clone(),
            "hold_partial_cover" => info.hold_partial_cover.to_string(),
            "note_uniform_scale" => info.note_uniform_scale.to_string(),
            "score_total" => info.score_total.to_string(),
            "hold_particle_interval_ratio" => info.hold_particle_interval_ratio.to_string(),
            "fold_animation" => info.fold_animation.to_string(),
            "created" => info.created.map(|dt| dt.to_rfc3339()).unwrap_or_default(),
            "updated" => info.updated.map(|dt| dt.to_rfc3339()).unwrap_or_default(),
            "chart_updated" => info
                .chart_updated
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
            _ => key.to_string(),
        }
    };

    for caps in re.captures_iter(input) {
        let whole = caps.get(0).unwrap().as_str();
        let key = caps.get(1).unwrap().as_str();

        let replacement = if let Some(key) = key.strip_prefix("config.") {
            match_config(key)
        } else if let Some(key) = key.strip_prefix("info.") {
            match_info(key)
        } else {
            match key {
                "date" => Local::now().format("%Y-%m-%d").to_string(),
                "time" => Local::now().format("%H-%M-%S").to_string(),
                "level_prefix" => info.diff.clone(),
                _ => whole.to_string(),
            }
        };
        output = output.replace(whole, &replacement);
    }

    fn safe_filename(name: String) -> String {
        name.trim()
            .chars()
            .filter(|&it| it.is_alphanumeric() || " !#$%&'()+,-.;=@[]^_`{}~".contains(it))
            .collect()
    }

    let format = if config.hires { "mov" } else { "mp4" };

    format!("{}.{}", safe_filename(output), format)
}

pub struct Task {
    id: u32,
    name: String,
    cover: NamedTempFile,
    output: PathBuf,

    params: RenderParams,
    status: Mutex<TaskStatus>,
    request_cancel: AtomicBool,
    request_pause: AtomicBool,
}

impl Task {
    async fn new(id: u32, params: RenderParams) -> Result<Self> {
        let mut fs = fs::fs_from_file(&params.path)?;
        let info = params.info.clone();
        let mut cover = NamedTempFile::new()?;
        cover.write_all(&fs.load_file(&info.illustration).await?)?;

        let file_name = generate_filename(&info, &params.config);

        let output = get_output_dir()?.join(file_name);

        Ok(Self {
            id,
            name: info.name,
            cover,
            output,

            params,
            status: Mutex::new(TaskStatus::Pending),
            request_cancel: AtomicBool::default(),
            request_pause: AtomicBool::default(),
        })
    }

    /// 从 `tasks.json` 里的一条快照复活任务。先按 `Task::new` 重建（读谱面、解曲绘），
    /// 再把**存下来的**输出路径与状态盖回去：历史任务必须指回它真正产出的那个文件，
    /// 中途被打断的则回到 `Pending`、由 worker 重新排队。
    async fn restore(saved: SavedTask) -> Result<Self> {
        let output = saved.output.clone();
        let mut task = Self::new(saved.id, saved.params).await?;
        task.output = output.clone();
        let output_text = output.display().to_string();
        *task.status.get_mut() = match saved.status {
            SavedStatus::Pending => TaskStatus::Pending,
            SavedStatus::Done => TaskStatus::Done {
                duration: saved.duration,
                output: output_text,
            },
            SavedStatus::Canceled => TaskStatus::Canceled {
                output: output_text,
            },
            SavedStatus::Failed => TaskStatus::Failed {
                output: output_text,
            },
        };
        Ok(task)
    }

    /// 落盘用的一份快照：中途状态一律记成「还没渲染完」。
    async fn snapshot(&self) -> SavedTask {
        let status = self.status.lock().await;
        let (status, duration) = match &*status {
            TaskStatus::Done { duration, .. } => (SavedStatus::Done, *duration),
            TaskStatus::Canceled { .. } => (SavedStatus::Canceled, 0.0),
            TaskStatus::Failed { .. } => (SavedStatus::Failed, 0.0),
            TaskStatus::Pending
            | TaskStatus::Loading
            | TaskStatus::Mixing
            | TaskStatus::MixingSfx { .. }
            | TaskStatus::Rendering { .. }
            | TaskStatus::Paused { .. } => (SavedStatus::Pending, 0.0),
        };
        SavedTask {
            id: self.id,
            params: self.params.clone(),
            output: self.output.clone(),
            status,
            duration,
        }
    }

    pub async fn run(&self) -> Result<()> {
        if self
            .request_cancel
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Ok(());
        }

        info!(
            "Task #{} '{}' started ({})",
            self.id,
            self.name,
            self.params.path.display()
        );

        *self.status.lock().await = TaskStatus::Loading;

        let mut child = cmd_hidden(std::env::current_exe()?)
            .arg("render")
            .arg(ASSET_PATH.get().unwrap())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        stdin
            .write_all(format!("{}\n", serde_json::to_string(&self.params)?).as_bytes())
            .await?;
        stdin
            .write_all(format!("{}\n", serde_json::to_string(&self.output)?).as_bytes())
            .await?;
        stdin.flush().await?;

        let mut stdout_lines = BufReader::new(stdout).lines();
        let mut stderr_lines = BufReader::new(stderr).lines();
        let mut output_stderr = String::new();
        let mut total_mixing: u64 = 0;
        let mut mixing_count: u64 = 0;
        let mut total_frame: u64 = 0;
        let mut frame_count: u64 = 0;
        let start = Instant::now();
        let mut frame_times = VecDeque::new();
        let mut last_update_fps_sec: u32 = 0;
        let mut last_fps: usize = 0;
        let mut pause_written = false;
        let mut total_pause_duration = Duration::ZERO;
        let mut pause_start: Option<Instant> = None;

        let config = read_config()?;

        loop {
            tokio::select! {
                stderr_result = stderr_lines.next_line() => {
                    let line = stderr_result?;
                    let Some(line) = line else { break };
                    if config.print_stderr {
                        eprintln!("{}", line);
                    }
                    output_stderr.push_str(&line);
                    output_stderr.push('\n');
                },

                stdout_result = stdout_lines.next_line() => {
                    let line = stdout_result?;
                    let Some(line) = line else { break };
                    let Ok(event): Result<IPCEvent, _> = serde_json::from_str(line.trim()) else {
                        continue;
                    };

                    match event {
                        IPCEvent::Loading => {
                            *self.status.lock().await = TaskStatus::Loading;
                        },
                        IPCEvent::Mixing => {
                            *self.status.lock().await = TaskStatus::Mixing;
                        },
                        IPCEvent::MixingSfx(total) => {
                            *self.status.lock().await = TaskStatus::MixingSfx {
                                progress: 0.0,
                            };
                            total_mixing = total;
                        },
                        IPCEvent::Sfx => {
                            mixing_count += 1;
                            *self.status.lock().await = TaskStatus::MixingSfx {
                                progress: mixing_count as f64 / total_mixing as f64,
                            };
                        },
                        IPCEvent::RenderFrame(total) => {
                            *self.status.lock().await = TaskStatus::Rendering {
                                progress: 0.0,
                                fps: 0,
                                estimate: 0.0,
                            };
                            total_frame = total;
                        },
                        IPCEvent::Frame => {
                            frame_count += 1;
                            let cur = start.elapsed().as_secs_f64() - total_pause_duration.as_secs_f64() - pause_start.map_or(0.0, |s| s.elapsed().as_secs_f64());
                            let sec = cur as u32;
                            frame_times.push_back(cur);
                            while frame_times.front().is_some_and(|it| cur - *it > 1.0) {
                                frame_times.pop_front();
                            }
                            if last_update_fps_sec != sec {
                                last_fps = frame_times.len();
                                last_update_fps_sec = sec;
                            }
                            let estimate = total_frame.saturating_sub(frame_count).max(1) as f64 / last_fps.max(1) as f64;
                            *self.status.lock().await = TaskStatus::Rendering {
                                progress: frame_count as f64 / total_frame as f64,
                                fps: last_fps as u64,
                                estimate,
                            };
                        },
                        IPCEvent::Paused => {
                            let mut status = self.status.lock().await;
                            if let TaskStatus::Rendering { progress, .. } = *status {
                                *status = TaskStatus::Paused { progress };
                            }
                            pause_start = Some(Instant::now());
                        },
                        IPCEvent::Resumed => {
                            if let Some(s) = pause_start.take() {
                                total_pause_duration += s.elapsed();
                            }
                            frame_times.clear();
                            last_fps = 0;
                            last_update_fps_sec = 0;
                            let mut status = self.status.lock().await;
                            if let TaskStatus::Paused { progress } = *status {
                                *status = TaskStatus::Rendering {
                                    progress,
                                    fps: 0,
                                    estimate: 0.0,
                                };
                            }
                        },
                        IPCEvent::Done(duration) => {
                            info!("Task #{} completed in {}", self.id, format_duration(Duration::from_secs_f64(duration)));
                            child.wait().await?;

                            *self.status.lock().await = TaskStatus::Done {
                                duration,
                                output: output_stderr,
                            };
                            return Ok(());
                        }
                    }
                },

                _ = async {
                    loop {
                        let cancel = self.request_cancel.load(Ordering::Relaxed);
                        let pause = self.request_pause.load(Ordering::Relaxed);
                        if cancel || (pause && !pause_written) || (!pause && pause_written) {
                            break;
                        }
                        sleep(Duration::from_millis(50)).await;
                    }
                } => {
                    if self.request_cancel.load(Ordering::Relaxed) {
                        info!("Task #{} cancelled", self.id);
                        child.kill().await?;
                        *self.status.lock().await = TaskStatus::Canceled {
                            output: output_stderr,
                        };
                        return Ok(());
                    }
                    if self.request_pause.load(Ordering::Relaxed) && !pause_written {
                        info!("Task #{} paused", self.id);
                        let _ = stdin.write_all(b"pause\n").await;
                        let _ = stdin.flush().await;
                        pause_written = true;
                    }
                    if !self.request_pause.load(Ordering::Relaxed) && pause_written {
                        info!("Task #{} resumed", self.id);
                        let _ = stdin.write_all(b"resume\n").await;
                        let _ = stdin.flush().await;
                        pause_written = false;
                    }
                },
            }
        }

        info!("Task #{} not completed", self.id);
        let status = child.wait().await?;
        if !status.success() {
            *self.status.lock().await = TaskStatus::Failed {
                output: format!(
                    "Child process exited abnormally ({:?})\n{}",
                    status.code().unwrap_or_default(),
                    output_stderr
                ),
            };
            return Ok(());
        }

        Ok(())
    }

    pub fn cancel(&self) {
        self.request_cancel.store(true, Ordering::Relaxed);
    }

    pub fn pause(&self) {
        self.request_pause.store(true, Ordering::Relaxed);
    }

    pub fn resume(&self) {
        self.request_pause.store(false, Ordering::Relaxed);
    }

    pub async fn to_view(&self) -> TaskView {
        TaskView {
            id: self.id,
            name: self.name.clone(),
            output: self.output.clone(),
            info: self.params.info.clone(),
            config: self.params.config.clone(),
            path: self.params.path.display().to_string(),
            cover: self.cover.path().display().to_string(),
            status: self.status.lock().await.clone(),
        }
    }
}

#[derive(Serialize)]
pub struct TaskView {
    pub id: u32,
    name: String,
    pub output: PathBuf,
    info: ChartInfo,
    config: RenderConfig,
    path: String,
    cover: String,
    status: TaskStatus,
}

pub struct TaskQueue {
    sender: mpsc::UnboundedSender<Arc<Task>>,
    worker: JoinHandle<()>,

    /// 这个向量**本身的顺序就是渲染顺序**（下标 0 = 下一个渲染），worker 与它共享同一份
    /// （`Arc`）。任务页可以拖动任务条调整渲染顺序，顺序就不能再藏在 worker
    /// 私有的 `VecDeque` 里，见 `move_task`。
    tasks: Arc<Mutex<Vec<Arc<Task>>>>,

    /// 任务 id 只增不减。以前直接拿 `tasks.len()` 当 id，删过任务后新任务会和现有任务
    /// **撞 id**（前端 `item-key="id"`、展开态、按 id 取消/暂停都会指错任务）。
    /// 放在 `Arc` 里是为了让「读回 `tasks.json`」那个后台任务也能抬高它（见 `restore_queue`）。
    next_id: Arc<AtomicU32>,
}
impl TaskQueue {
    pub fn new() -> Self {
        let (sender, mut receiver) = mpsc::unbounded_channel::<Arc<Task>>();
        let tasks: Arc<Mutex<Vec<Arc<Task>>>> = Arc::new(Mutex::new(Vec::new()));
        let worker_tasks = Arc::clone(&tasks);
        let next_id = Arc::new(AtomicU32::new(0));

        let worker = tokio::spawn(async move {
            let spawn_quene = Arc::new(Semaphore::new(1));

            loop {
                // 通道只用来唤醒 worker；真正的渲染顺序由共享的 `tasks` 决定，前端可以拖动改它。
                while receiver.try_recv().is_ok() {}

                // 取消请求要顺手落盘：`Canceled` 是会被读回来的状态之一
                let mut canceled_any = false;
                for task in worker_tasks.lock().await.iter() {
                    if task
                        .request_cancel
                        .load(std::sync::atomic::Ordering::Relaxed)
                    {
                        let mut status = task.status.lock().await;
                        if !matches!(*status, TaskStatus::Canceled { .. }) {
                            *status = TaskStatus::Canceled {
                                output: String::new(),
                            };
                            info!("Task #{} canceled", task.id);
                            canceled_any = true;
                        }
                    }
                }
                if canceled_any {
                    save_tasks(&worker_tasks).await;
                }

                // 先拿许可、再挑任务：`Semaphore::new(1)` 保证同一时刻只有一条在渲染，而许可被
                // 移进下面的 `tokio::spawn`、渲染结束才释放 ⇒ 这一轮挑中的任务不会被下一轮重复
                // 挑到（它在跑的时候 status 已经不是 `Pending` 了）。
                let Ok(permit) = spawn_quene.clone().try_acquire_owned() else {
                    sleep(Duration::from_millis(50)).await;
                    continue;
                };

                // 顺序取「列表里第一个还在等待（`Pending`）的任务」⇒ 列表顺序就是渲染顺序。
                let next = {
                    let guard = worker_tasks.lock().await;
                    let mut chosen = None;
                    for task in guard.iter() {
                        if matches!(*task.status.lock().await, TaskStatus::Pending) {
                            chosen = Some(Arc::clone(task));
                            break;
                        }
                    }
                    chosen
                };

                if let Some(task) = next {
                    // 任务跑完（成功/失败/取消）要把终态写回 `tasks.json`：关掉程序再打开，
                    // 列表里「已完成 / 失败」的历史也还在
                    let finished_tasks = Arc::clone(&worker_tasks);
                    tokio::spawn(async move {
                        if let Err(err) = task.run().await {
                            error!("Task #{} failed: {:?}", task.id, err);
                            let mut status = task.status.lock().await;
                            *status = TaskStatus::Failed {
                                output: format!("{err:?}"),
                            };
                        }
                        drop(permit);
                        save_tasks(&finished_tasks).await;
                    });
                }

                sleep(Duration::from_millis(50)).await;
            }
        });

        let queue = Self {
            sender,
            worker,

            tasks,
            next_id: Arc::clone(&next_id),
        };

        // 读回上次的队列同样放后台：`Task::restore` 要读谱面、解曲绘（async），
        // 不能在 `new()` 这个同步函数里干等。
        let restore_tasks = Arc::clone(&queue.tasks);
        tokio::spawn(async move {
            restore_queue(&restore_tasks, &next_id).await;
        });

        queue
    }

    pub async fn post(&self, params: RenderParams) -> Result<u32> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut guard = self.tasks.lock().await;
        let task = Arc::new(Task::new(id, params).await?);
        guard.push(Arc::clone(&task));
        self.sender.send(task)?;
        drop(guard);

        // 新任务立刻落盘：程序被关掉（或崩掉）时排好的队列不会丢
        self.save().await;

        Ok(id)
    }

    /// 把当前队列写回 `tasks.json`（顺序与 `tasks` 一致）。存不下来只记日志，不影响渲染。
    async fn save(&self) {
        save_tasks(&self.tasks).await;
    }

    pub async fn tasks(&self) -> Vec<TaskView> {
        let guard = self.tasks.lock().await;
        let mut result = Vec::with_capacity(guard.capacity());
        for task in guard.iter() {
            result.push(task.to_view().await);
        }
        result.reverse();
        result
    }

    /// 按 id 取任务。注意内部 `tasks` 的顺序是「最旧的在前」，`tasks()` 才倒成「最新在前」
    async fn find(&self, id: u32) -> Option<Arc<Task>> {
        self.tasks
            .lock()
            .await
            .iter()
            .find(|task| task.id == id)
            .cloned()
    }

    /// 按 id 删除一条任务；`remove_file` 为真时连输出文件一起删。
    /// 返回是否真的找到了（`Task` 的字段是私有的，所以文件删除放在这里做，`lib.rs` 只调这一句）。
    pub async fn remove_with_output(&self, id: u32, remove_file: bool) -> Result<bool> {
        let Some(task) = self.find(id).await else {
            return Ok(false);
        };
        info!("Task #{}(id: {}) deleted", task.id, id);
        // 还在跑的任务被移除时要先让它停下来：`remove` 只是把它从列表里摘出去，
        // worker 手里那个 `Arc<Task>` 照样继续渲染 —— 不置取消位的话它会变成一条
        // 「看不见的任务」，继续写文件、继续占着那一格并发额度（报告的「批量移除后还在渲染」就是它）。
        task.cancel();
        if remove_file && task.output.exists() && task.output.is_file() {
            // 刚取消的任务：ffmpeg 那头的文件锁可能还在，退避重试几秒（见 `remove_file_retry`）
            remove_file_retry(&task.output).await?;
        }
        self.remove(id).await;
        self.save().await;
        Ok(true)
    }

    pub async fn cancel(&self, id: u32) {
        if let Some(task) = self.find(id).await {
            task.cancel();
        }
    }

    pub async fn pause(&self, id: u32) {
        if let Some(task) = self.find(id).await {
            task.pause();
        }
    }

    pub async fn resume(&self, id: u32) {
        if let Some(task) = self.find(id).await {
            task.resume();
        }
    }

    /// 把任务挪到 `index`：**下标是移动之后的下标**，0 = 下一个渲染。
    /// 任务页可以拖动任务条调整渲染顺序（只有还在等待渲染的 `Pending` 任务
    /// 允许拖，由前端拦住；这里只负责改顺序，不校验状态）。
    pub async fn move_task(&self, id: u32, index: usize) -> bool {
        let mut guard = self.tasks.lock().await;
        let Some(from) = guard.iter().position(|task| task.id == id) else {
            return false;
        };
        let task = guard.remove(from);
        let index = index.min(guard.len());
        guard.insert(index, task);
        drop(guard);
        // 拖过的顺序也要落盘，否则重启后回不到用户排好的顺序
        self.save().await;
        true
    }

    async fn remove(&self, id: u32) {
        let mut guard = self.tasks.lock().await;
        if let Some(index) = guard.iter().position(|task| task.id == id) {
            guard.remove(index);
        }
    }

    pub async fn clear(&self) {
        for task in self.tasks.lock().await.drain(..) {
            task.cancel();
        }
        self.save().await;
    }
}

impl Drop for TaskQueue {
    fn drop(&mut self) {
        self.worker.abort();
    }
}
