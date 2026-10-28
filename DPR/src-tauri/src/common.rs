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

/// 日志目录：默认 **`<程序目录>/logs`**，跟 `output/`、`config/` 做邻居（便携）；
/// 程序目录不可写时（装进 Program Files、被受控文件夹拦住……）退回数据目录下的 `logs/`。
pub fn logs_dir() -> Result<PathBuf> {
    let dir = match exe_dir().map(|dir| dir.join("logs")) {
        Some(dir) if is_dir_writable(&dir) => dir,
        _ => DATA_DIR.get().unwrap().join("logs"),
    };
    if dir.exists() && !dir.is_dir() {
        bail!("logs directory is not a directory");
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// 这次运行写的那份日志文件（`init_log_file` 设的）。
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// 打开这次运行的日志文件：**按天一份**（`logs/dpr-YYYY-MM-DD.log`），追加写。
///
/// 同一个进程只做一次，返回文件路径。失败（目录不可写、文件被占住）不致命 ——
/// 那就只打终端，程序照常跑。
pub fn init_log_file() -> Result<PathBuf> {
    let path = logs_dir()?.join(format!("dpr-{}.log", chrono::Local::now().format("%Y-%m-%d")));
    // 立刻真开一次：目录不可写、文件被别的进程独占时在这里就报错，别等到写第一行才发现
    append_log(&path, "")?;
    dps::log::set_log_file_path(path.clone());
    let _ = LOG_PATH.set(path.clone());
    Ok(path)
}

/// 往日志文件里追加一行（没初始化成功就什么也不做）。
///
/// 行尾换行去掉，ANSI 颜色码剥掉 —— 这份文件是要给人看、给 issue 贴的纯文本。
pub fn log_line(line: &str) {
    let Some(path) = LOG_PATH.get() else {
        return;
    };
    let _ = append_log(path, line);
}

/// 追加一行（末尾补换行）。
///
/// 写完就关句柄：长期占着写句柄会把文件锁住，用户就没法在程序运行时打开、拷走日志。
/// 日志量不大，多开的这几下开销可以忽略。
fn append_log(path: &Path, line: &str) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0x0000_0007); // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
    }
    let mut file = options.open(path)?;
    let line = strip_ansi(line);
    let line = line.trim_end_matches(['\r', '\n']);
    if !line.is_empty() {
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
    }
    Ok(())
}

/// 去掉 `colored` 留下的 ANSI 转义序列（CSI：`ESC [` 之后直到某个终止字节）。
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    out
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

/// 附带资源包目录：`<程序目录>/appendant`，放用户自己下载的 zip（皮肤包、Minecraft 材质等）。
///
/// 这些包**不随程序分发**，缺了不算错误：界面在用户启用对应功能时提示去下载。
/// 程序目录不可写时（例如装在 Program Files）退回配置目录里的 `appendant`。
pub fn appendant_dir() -> Result<PathBuf> {
    // 先在程序目录里找（用户看得见、和 exe 放一起最直观）；写不进去才退配置目录。
    // 预览子进程没有 `CONFIG_DIR`，所以这一步不能依赖它，否则子进程读不到任何附带包。
    if let Some(dir) = exe_dir().map(|dir| dir.join("appendant")) {
        if dir.is_dir() {
            return Ok(dir);
        }
        if std::fs::create_dir(&dir).is_ok() {
            return Ok(dir);
        }
    }
    let Some(config_dir) = CONFIG_DIR.get() else {
        bail!("appendant directory is unavailable");
    };
    let dir = config_dir.join("appendant");
    if dir.exists() {
        if !dir.is_dir() {
            bail!("appendant directory is not a directory");
        }
    } else {
        std::fs::create_dir(&dir)?;
    }
    Ok(dir)
}

/// 「默认」皮肤包：`appendant/appendant.official.zip`（或同名目录）。
///
/// 这个包**可选**：不在（或里面缺文件）就退回编进程序的内置皮肤，界面不该为此提示下载；
/// 齐全时当默认皮肤用。
pub fn default_respack_path() -> Option<String> {
    let dir = appendant_dir().ok()?;
    let pack = dps::core::appendant::pack_in(&dir, dps::core::appendant::OFFICIAL)?;
    dps::core::appendant::has_files(&pack, dps::core::appendant::SKIN_FILES).then(|| pack.to_string_lossy().into_owned())
}

/// 解析配置里选的资源包路径。
///
/// 没选（`None`）、「选中的包已经被用户删掉」、以及「包在但缺文件」都退回默认皮肤 —— 后者前端
/// 会拦，但配置可能来自别处（命令行参数、旧配置文件），后端不该拿着一个加载不了的路径硬读。
pub fn resolve_respack_path(path: Option<String>) -> Option<String> {
    match path {
        Some(path) if dps::core::appendant::has_files(Path::new(&path), dps::core::appendant::SKIN_FILES) => Some(path),
        _ => default_respack_path(),
    }
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

/// 把一份配置序列化成 TOML 表，并去掉与默认值相同的项 —— 配置文件里只留真正改过的字段。
///
/// 读回来不用担心缺键：写出的结构体与读它的结构体都带 `#[serde(default)]`，缺的字段按 `Default`
/// 补上，所以「只写改动」与「写全字段」等价，文件还小得多。比较放在序列化之后做而不是逐字段比
/// `PartialEq`：`Option` 的 None、以后加的其他 `skip_serializing_if` 都由同一套序列化负责，
/// 不会出现「两份数据看起来相等、写出来却不一样」。
fn config_table<T: Serialize>(value: &T, default: &T) -> Result<toml::Value> {
    let mut table: toml::Value = toml::from_str(&toml::to_string(value)?)?;
    let reference: toml::Value = toml::from_str(&toml::to_string(default)?)?;
    strip_default(&mut table, &reference);
    Ok(table)
}

/// 递归删掉与默认值相同的项：子表继续往下比，数组和标量整块比。
fn strip_default(actual: &mut toml::Value, reference: &toml::Value) {
    let (toml::Value::Table(actual), toml::Value::Table(reference)) = (actual, reference) else {
        return;
    };
    actual.retain(|key, value| {
        let Some(reference) = reference.get(key) else {
            return true;
        };
        strip_default(value, reference);
        // 全是默认值的子表会被清空：留着只会写出一行 `键 = {}`，读回来也没有意义
        let emptied = matches!(value, toml::Value::Table(table) if table.is_empty());
        !emptied && !same_value(value, reference)
    });
}

/// JSON 版的 `strip_default`：任务快照（`tasks.json`）里与默认配置相同的项也删掉。
pub fn strip_default_json(actual: &mut serde_json::Value, reference: &serde_json::Value) {
    let (serde_json::Value::Object(actual), serde_json::Value::Object(reference)) = (actual, reference)
    else {
        return;
    };
    actual.retain(|key, value| {
        let Some(reference) = reference.get(key) else {
            return true;
        };
        strip_default_json(value, reference);
        let emptied = matches!(value, serde_json::Value::Object(object) if object.is_empty());
        !emptied && !same_json(value, reference)
    });
}

fn same_value(a: &toml::Value, b: &toml::Value) -> bool {
    match (a, b) {
        (toml::Value::Float(a), toml::Value::Float(b)) => floats_close(*a, *b),
        (toml::Value::Array(a), toml::Value::Array(b)) => {
            if let Some(same) = same_color(array_floats(a).as_deref(), array_floats(b).as_deref()) {
                return same;
            }
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_value(a, b))
        }
        (toml::Value::Table(a), toml::Value::Table(b)) => {
            a.len() == b.len() && a.iter().all(|(key, value)| b.get(key).is_some_and(|it| same_value(value, it)))
        }
        _ => a == b,
    }
}

fn array_floats(values: &[toml::Value]) -> Option<Vec<f64>> {
    values.iter().map(|it| it.as_float()).collect()
}

fn same_json(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    match (a, b) {
        (serde_json::Value::Number(a), serde_json::Value::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => floats_close(a, b),
            _ => a == b,
        },
        (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
            let floats = |values: &[serde_json::Value]| -> Option<Vec<f64>> {
                values.iter().map(|it| it.as_f64()).collect()
            };
            if let Some(same) = same_color(floats(a).as_deref(), floats(b).as_deref()) {
                return same;
            }
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_json(a, b))
        }
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            a.len() == b.len() && a.iter().all(|(key, value)| b.get(key).is_some_and(|it| same_json(value, it)))
        }
        _ => a == b,
    }
}

/// 三个 0~1 的浮点数当作颜色来比：颜色在文件里最终是 8 位色，
/// `0.9` 与 `230/255`、`0.4118` 与 `105/255` 都是同一个颜色。
/// 返回 `None` 表示这两个数组不是颜色，交给普通比较。
fn same_color(a: Option<&[f64]>, b: Option<&[f64]>) -> Option<bool> {
    let (a, b) = (a?, b?);
    if a.len() != 3 || b.len() != 3 || !a.iter().chain(b).all(|it| (0. ..=1.).contains(it)) {
        return None;
    }
    Some(a.iter().zip(b).all(|(a, b)| color8(*a) == color8(*b)))
}

fn color8(value: f64) -> i32 {
    // 加一点地板再取整：默认值是以十进制写出来的（`0.9` 存成 f32 是 0.89999998，
    // 乘 255 得 229.49999），正好卡在两档中间；不加这点就会和 `230/255` 判成不同的颜色
    (value * 255. + 1e-3).round() as i32
}

/// 两个浮点数是否算「同一个值」。
///
/// 配置在「内存 → 文件 → 内存」之间往返会过一遍十进制文本，f32 还会被抬成 f64
/// （`0.3` 读回来是 `0.30000000000000004`），默认值本身也可能是写出来的近似值
/// （`105/255` 在界面上是 `0.4118`）。逐位比较会把这些噪声当成改动、永远留在文件里，
/// 所以按 1e-4 的相对误差判定 —— 换算到画面上是看不出来的量级。
fn floats_close(a: f64, b: f64) -> bool {
    if a == b {
        return true;
    }
    let scale = a.abs().max(b.abs()).max(1.);
    (a - b).abs() <= 1e-4 * scale
}

pub async fn save_presets(presets: &HashMap<String, RenderConfig>) -> Result<()> {
    let file = get_presets_toml_file()?;
    let default = RenderConfig::default();
    let mut table = toml::map::Map::new();
    for (name, config) in presets {
        table.insert(name.clone(), config_table(config, &default)?);
    }
    let toml_string = toml::to_string(&toml::Value::Table(table))?;
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
            enable_play_button: true,
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
    let table = config_table(&config, &AppConfig::default())?;
    std::fs::write(file, toml::to_string(&table)?)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 与默认值相同的字段不落盘，读回来按 `Default` 补齐 —— 即「写改动」与「写全字段」等价。
    #[test]
    fn config_table_drops_defaults() {
        let changed = AppConfig {
            high_performance: true,
            ..AppConfig::default()
        };
        let text = toml::to_string(&config_table(&changed, &AppConfig::default()).unwrap()).unwrap();
        assert!(text.contains("highPerformance = true"), "{text}");
        assert!(!text.contains("previewFullscreen"), "{text}");
        let back: AppConfig = toml::from_str(&text).unwrap();
        assert!(back.high_performance);
        assert_eq!(back.preview_fullscreen, AppConfig::default().preview_fullscreen);
    }

    /// 预设里只留改过的渲染项；全默认的一份写成空表，读回来仍是默认值。
    #[test]
    fn preset_table_drops_defaults() {
        let default = RenderConfig::default();
        let mut changed = default.clone();
        changed.chart_mod_copy = true;
        changed.chart_mod_copy_times = 3;
        let text = toml::to_string(&config_table(&changed, &default).unwrap()).unwrap();
        assert!(text.contains("chartModCopy = true"), "{text}");
        assert!(!text.contains("renderNote"), "{text}");
        let back: RenderConfig = toml::from_str(&text).unwrap();
        assert!(back.chart_mod_copy);
        assert_eq!(back.chart_mod_copy_times, 3);
        assert_eq!(back.render_note, default.render_note);

        let empty = toml::to_string(&config_table(&default, &default).unwrap()).unwrap();
        assert!(empty.trim().is_empty(), "{empty}");
        let back: RenderConfig = toml::from_str(&empty).unwrap();
        assert_eq!(back.chart_mod_copy_times, default.chart_mod_copy_times);
    }

    /// 任务快照（tasks.json）：默认配置整块删掉，改过的项留下，浮点噪声不算改动。
    #[test]
    fn task_config_drops_defaults() {
        let mut task = serde_json::json!({
            "params": { "config": {
                "fps": 60,
                "hires": true,
                "judgeRangeFadeTime": 0.30000000000000004,
                "mopemopeSpColorA": [0.4117647, 0.29803923, 0.14117648],
                "blockAreaColor": [0.9019608, 0.101960786, 0.101960786],
                "chartModCopyTimes": 3
            } },
            "status": "done"
        });
        let default = serde_json::to_value(RenderConfig::default()).unwrap();
        let config = task.pointer_mut("/params/config").unwrap();
        strip_default_json(config, &default);
        let text = config.to_string();
        assert!(text.contains("\"chartModCopyTimes\":3"), "{text}");
        assert!(text.contains("\"hires\":true"), "{text}");
        // 与默认值只差浮点噪声的也删掉（0.3 与 0.30000000000000004、105/255 与 0.4118）；
        // 颜色按 8 位色深比，`230/255` 与默认的 `0.9` 是同一个颜色
        assert!(!text.contains("judgeRangeFadeTime"), "{text}");
        assert!(!text.contains("mopemopeSpColorA"), "{text}");
        assert!(!text.contains("blockAreaColor"), "{text}");
        assert!(!text.contains("fps"), "{text}");
        // 删掉的部分读回来按 `Default` 补齐
        let back: RenderConfig = serde_json::from_value(config.clone()).unwrap();
        assert_eq!(back.chart_mod_copy_times, 3);
        assert_eq!(
            back.judge_range_fade_time,
            RenderConfig::default().judge_range_fade_time
        );
        assert_eq!(back.mopemope_sp_color_a, RenderConfig::default().mopemope_sp_color_a);
    }

    /// 真正落盘的 presets.toml：只留改过的项，全默认的那份留一个空表，且都能原样读回来。
    #[test]
    fn save_presets_round_trip() {
        let dir = std::env::temp_dir().join("dpr-config-trim-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // `CONFIG_DIR` 是全局一次性设置的；已被别的测试占住就跳过（这条测的是落盘格式）
        if CONFIG_DIR.set(dir.clone()).is_err() {
            return;
        }
        let changed = RenderConfig {
            chart_mod_copy: true,
            chart_mod_copy_times: 5,
            ..RenderConfig::default()
        };
        let presets = HashMap::from([
            ("改过的".to_string(), changed),
            ("全默认".to_string(), RenderConfig::default()),
        ]);
        tauri::async_runtime::block_on(save_presets(&presets)).unwrap();
        let text = std::fs::read_to_string(dir.join("presets.toml")).unwrap();
        assert!(text.contains("chartModCopyTimes = 5"), "{text}");
        assert!(!text.contains("renderNote"), "{text}");
        let back: HashMap<String, RenderConfig> = toml::from_str(&text).unwrap();
        assert!(back["改过的"].chart_mod_copy);
        assert_eq!(back["改过的"].chart_mod_copy_times, 5);
        assert_eq!(
            back["全默认"].chart_mod_copy_times,
            RenderConfig::default().chart_mod_copy_times
        );
    }
}
