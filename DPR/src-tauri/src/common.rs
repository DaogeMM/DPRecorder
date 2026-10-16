use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
};
use tracing::info;
use zip::{write::FileOptions, CompressionMethod, ZipWriter};

use crate::render::RenderConfig;

pub static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();
pub static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn parse_args(
    args: Vec<String>,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let mut args_input = None;
    let mut args_output = None;
    let mut args_config = None;
    let mut args_info = None;
    let mut args_now = 1; // the first argument is the binary file name.
    while args_now < args.len() {
        match args[args_now].as_str() {
            "--input" | "-i" => {
                args_input = args.get(args_now + 1).cloned();
                args_now += 2;
            }
            "--output" | "-o" => {
                args_output = args.get(args_now + 1).cloned();
                args_now += 2;
            }
            "--config" | "-c" => {
                args_config = args.get(args_now + 1).cloned();
                args_now += 2;
            }
            "--info" | "ci" => {
                args_info = args.get(args_now + 1).cloned();
                args_now += 2;
            }
            arg => {
                if !arg.starts_with("--") && args_input.is_none() {
                    args_input = Some(arg.to_string());
                } else if args_now != 1 {
                    // the second argument is the mode.
                    eprintln!("Unknown argument: {}", arg);
                }
                args_now += 1;
            }
        }
    }
    return (args_input, args_output, args_config, args_info);
}

pub fn ensure_dir(path: PathBuf) -> PathBuf {
    if path.exists() {
        if !path.is_dir() {
            panic!("{} is not a directory", path.display());
        }
    } else {
        std::fs::create_dir_all(&path).unwrap();
    }
    path
}

/// 该目录（或它最近的已存在祖先）能否新建文件。
///
/// 用来发现「标准用户目录不可写」的环境（受控文件夹、沙箱、只读安装目录……）。
pub fn is_dir_writable(dir: &Path) -> bool {
    let mut probe = dir;
    loop {
        if probe.exists() {
            break;
        }
        match probe.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => probe = parent,
            _ => return false,
        }
    }
    let test = probe.join(".dpr-write-test");
    match File::create(&test) {
        Ok(_) => {
            let _ = std::fs::remove_file(&test);
            true
        }
        Err(_) => false,
    }
}

/// exe 所在目录（受限环境里的兜底写点都挨着它）。
pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

pub fn default_output_dir() -> Result<PathBuf> {
    let dir = if let Some(set_output_dir) = read_config()?.output_dir {
        set_output_dir
    } else {
        // 默认导出到**程序目录下的 output/**（便携，也是受限环境里唯一写得进去的地方）；
        // 只有程序目录不可写时（例如装在 Program Files）才退回标准数据目录。
        match exe_dir().map(|dir| dir.join("output")) {
            Some(dir) if is_dir_writable(&dir) => dir,
            _ => DATA_DIR.get().unwrap().join("output"),
        }
    };
    if dir.exists() {
        if !dir.is_dir() {
            bail!("output directory is not a directory");
        }
    } else {
        std::fs::create_dir(&dir)?;
    }
    Ok(dir)
}

pub fn get_output_dir() -> Result<PathBuf> {
    if let Some(output_dir) = read_config()?.output_dir {
        Ok(output_dir)
    } else {
        Ok(default_output_dir()?)
    }
}

pub fn test_output_dir(dir: PathBuf) -> Result<()> {
    //let dir = DATA_DIR.get().unwrap().clone();
    if dir.exists() {
        if !dir.is_dir() {
            bail!("output directory is not a directory");
        }
    } else {
        std::fs::create_dir(&dir)?;
    }
    Ok(())
}

/// 删一个文件，被别的进程占着时**等一会儿再试**。
///
/// 为什么不能直接删：取消一条正在渲染的任务时，后台杀掉的只是渲染子进程，
/// 真正写输出文件的是它拉起来的 **ffmpeg**（孙进程）。ffmpeg 要等到 stdin 那一头
/// 断开、把手头的缓冲冲完才退出，这几百毫秒里文件是锁着的，立刻删就是
/// Windows 的「另一个程序正在使用此文件」(os error 32)。
///
/// 退避重试的总时长约 5 秒：这个量级已经比「等 ffmpeg 退出」宽裕得多，
/// 又不会让「取消并移除文件」在真的删不掉（文件被别的软件占着）时卡太久。
/// 文件本来就不在（`NotFound`）算成功 —— 「取消渲染并移除文件」经常遇到
/// 还没落盘的输出。
pub async fn remove_file_retry(path: &Path) -> Result<()> {
    const DELAYS_MS: [u64; 8] = [200, 250, 350, 500, 700, 900, 1200, 1500];
    let mut last_error = None;
    for (attempt, delay) in DELAYS_MS.iter().enumerate() {
        match tokio::fs::remove_file(path).await {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => {
                if attempt == 0 {
                    info!("remove {} is locked ({}), will retry", path.display(), err);
                }
                last_error = Some(err);
                // 最后一次失败就别再等了，直接把错误抛出去
                if attempt + 1 < DELAYS_MS.len() {
                    tokio::time::sleep(std::time::Duration::from_millis(*delay)).await;
                }
            }
        }
    }
    let last_error = last_error.expect("loop always sets last_error before falling through");
    Err(anyhow::anyhow!(
        "failed to remove {}: {}",
        path.display(),
        last_error
    ))
}

/// 资源包目录：和 output 一样**优先放在程序目录**下（`<程序目录>/respack`），
/// 程序目录不可写时（例如装在 Program Files）才退回配置目录里的 `respack`。
///
/// 注意 `AppConfig::res_pack_path` 里存的是**绝对路径**，挪目录后旧路径会失效；
/// 前端 `ConfigView.vue:484` 找不到就退回列表第一项，不会崩。
pub fn respack_dir() -> Result<PathBuf> {
    let fallback = CONFIG_DIR.get().unwrap().join("respack");
    let dir = match exe_dir().map(|dir| dir.join("respack")) {
        Some(dir) if dir == fallback || is_dir_writable(&dir) => dir,
        _ => fallback,
    };
    if dir.exists() {
        if !dir.is_dir() {
            bail!("resource pack directory is not a directory");
        }
    } else {
        std::fs::create_dir(&dir)?;
    }
    Ok(dir)
}

pub fn get_presets_toml_file() -> Result<PathBuf> {
    let file = CONFIG_DIR.get().unwrap().join("presets.toml");
    if file.exists() && !file.is_file() {
        bail!("presets.toml is not a file");
    }
    Ok(file)
}

pub fn get_presets_json_file() -> Result<PathBuf> {
    let file = CONFIG_DIR.get().unwrap().join("presets.json");
    if file.exists() && !file.is_file() {
        bail!("presets.json is not a file");
    }
    Ok(file)
}

/// 任务队列的落盘文件（`config/tasks.json`）。
///
/// 这里用 JSON 而不是和预设/配置一样的 TOML：存的是 `RenderParams`，
/// 里面有 `Option` 字段，TOML 表示不了。
pub fn get_tasks_file() -> Result<PathBuf> {
    let file = CONFIG_DIR.get().unwrap().join("tasks.json");
    if file.exists() && !file.is_file() {
        bail!("tasks.json is not a file");
    }
    Ok(file)
}

pub async fn save_presets(presets: &HashMap<String, RenderConfig>) -> Result<()> {
    let file = get_presets_toml_file()?;
    let toml_string = toml::to_string(presets)?;
    std::fs::write(file, toml_string)?;
    Ok(())
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    pub rpe_dir: Option<PathBuf>,
    pub output_dir: Option<PathBuf>,
    pub encoder_avc: Option<String>,
    pub encoder_hevc: Option<String>,
    pub ffmpeg_path: Option<String>,
    pub print_stderr: bool,
    pub show_detailed_log: bool,
    /// 是否在「渲染」页显示「游玩」按钮（进入游戏内试玩）。默认 `false`。
    pub enable_play_button: bool,

    /// 预览 / 试玩 / 调整偏移的窗口是否全屏。默认 `true`。
    pub preview_fullscreen: bool,
    /// 高性能模式（默认关闭）：只影响渲染进程**怎么跑**，不改画面像素。
    /// 勾选后在提交渲染 / 游玩时套用到那个渲染进程上，做两件事：
    ///   1. Windows 下给渲染进程设高能显卡偏好（`NvOptimusEnablement` /
    ///      `AmdPowerXpressRequestHighPerformance`），双显卡机器优先用独显；
    ///   2. 若这次用上硬件编码器，就由 GPU 的帧读回直接给出「已上下翻转」的画面，
    ///      省掉 ffmpeg 那趟 CPU 的 `-vf vflip`。
    /// 它住在 app 配置里而不是每份渲染配置里 —— 这是「这台机器该怎么跑」的选择，
    /// 跟谱面/预设无关；渲染时由 `post_render` 从这份配置注入 `RenderConfig`。
    pub high_performance: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            rpe_dir: None,
            output_dir: None,
            encoder_avc: None,
            encoder_hevc: None,
            ffmpeg_path: None,
            print_stderr: false,
            show_detailed_log: true,
            enable_play_button: false,
            preview_fullscreen: true,
            // 默认关闭：默认渲染流程与从前逐字节一致
            // （「高性能模式」只让渲染进程更偏向 GPU，不该在用户没勾的时候改变行为）
            high_performance: false,
        }
    }
}

pub fn get_config_file() -> Result<PathBuf> {
    let file = CONFIG_DIR.get().unwrap().join("config.toml");
    if file.exists() && !file.is_file() {
        bail!("presets.toml is not a file");
    }
    Ok(file)
}

pub fn read_config() -> Result<AppConfig> {
    let file = get_config_file()?;
    if file.exists() {
        let config: AppConfig = toml::from_str(&std::fs::read_to_string(file)?)?;
        Ok(config)
    } else {
        Ok(AppConfig::default())
    }
}

pub fn save_config(config: AppConfig) -> Result<()> {
    let file = get_config_file()?;
    let string = toml::to_string(&config)?;
    std::fs::write(file, string)?;
    Ok(())
}

pub fn get_rpe_dir() -> Result<PathBuf> {
    if let Some(dir) = read_config()?.rpe_dir {
        if dir.exists() {
            Ok(dir)
        } else {
            bail!("rpe_dir does not exist");
        }
    } else {
        bail!("rpe_dir is not set");
    }
}

pub fn set_rpe_dir(set_dir: Option<PathBuf>) -> Result<()> {
    let mut config = read_config()?;
    config.rpe_dir = set_dir;
    save_config(config)?;
    Ok(())
}

pub fn check_rpe_dir(dir: PathBuf) -> bool {
    if !dir.is_dir()
        || ["PhiEdit.exe", "Resources"]
            .iter()
            .any(|it| !dir.join(*it).exists())
    {
        false
    } else {
        true
    }
}

#[derive(Deserialize, Serialize, Debug)]
pub struct Effect {
    pub shader: String,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct Extra {
    pub effects: Vec<Effect>,
}

pub fn collect_chart_files(
    directory: PathBuf,
    parent: PathBuf,
) -> Result<HashMap<String, PathBuf>> {
    let mut file_list: HashMap<String, PathBuf> = HashMap::new();

    if directory.is_dir() {
        for entry in std::fs::read_dir(directory)? {
            if let Ok(entry) = entry {
                let path = entry.path();

                if path.is_file() {
                    let filename = path.file_name().unwrap().to_string_lossy();
                    let file_path = path.strip_prefix(&parent)?.display().to_string();
                    if filename.starts_with("AutoSave_")
                        || filename.starts_with("blur_")
                        || filename.starts_with("blur1_")
                        || filename.starts_with("tempfile_")
                        || filename == "createTime.txt"
                    {
                        continue;
                    }
                    file_list.insert(file_path, path);
                } else if path.is_dir() {
                    file_list.extend(collect_chart_files(path, parent.clone())?);
                }
            }
        }
    }

    Ok(file_list)
}

pub async fn create_zip(output_path: PathBuf, files: HashMap<String, PathBuf>) -> Result<()> {
    let file = File::create(&output_path)?;
    let mut zip = ZipWriter::new(BufWriter::new(file));

    let options = FileOptions::<()>::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(9))
        .unix_permissions(0o755);

    for (file_name, file_path) in files {
        zip.start_file_from_path(file_name, options)?;
        let mut buffer = Vec::new();
        File::open(&file_path)?.read_to_end(&mut buffer)?;
        zip.write_all(&buffer)?;
    }

    zip.finish()?;
    info!("Create ZIP: {}", output_path.display());
    Ok(())
}
