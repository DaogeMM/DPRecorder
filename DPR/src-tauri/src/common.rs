use anyhow::{anyhow, bail, Context, Result};
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

/// 是否往控制台打日志（「输出日志到控制台」开关，落在 `AppConfig::print_stderr`）。
///
/// 开关本身在 `dps::log` 里（日志层的终端那一路也用同一个），这里只转调一下，
/// 免得「日志宏打不打」和「`console!` 打不打」两边不同步。
pub fn set_console_log(enabled: bool) {
    dps::log::set_console_enabled(enabled);
}

/// 把 anyhow 的错误链压成一行：`外层：内层：最底层原因`。
///
/// 控制台与日志里 `{e:?}` 会展开成多行 `Caused by:`，一条报错撑成四五行；
/// 这里改用「：」把各层连起来，报错行自己就把原因说完。
pub fn error_chain(err: &anyhow::Error) -> String {
    let mut out = String::new();
    for (i, cause) in err.chain().enumerate() {
        if i > 0 {
            out.push('：');
        }
        out.push_str(&cause.to_string());
    }
    out
}

/// 启动时准备控制台：切到 UTF-8 代码页、打开 ANSI 转义处理（实现在 `dps::log`）。
pub fn init_console() {
    dps::log::init_console();
}

/// 原地刷新一行进度：回车回到行首、清掉这一行，不换行（只在真的接在终端上时动）。
pub fn console_progress(text: &str) {
    dps::log::console_progress(text);
}

/// 进度打完之后收尾换行，免得后面的日志接在同一行尾巴上。
pub fn console_progress_done() {
    dps::log::console_progress_done();
}

/// 改控制台窗口标题（最小化后任务栏上也能看到进度）。没有控制台时什么也不做。
pub fn set_console_title(title: &str) {
    dps::log::set_console_title(title);
}

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
                    crate::console!("[命令行] 未知参数：{}", arg);
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

/// 打开这次运行的日志文件：**按天一份**（`logs/dpr-YYYY-MM-DD.log`），追加写。
///
/// 同一个进程只做一次，返回文件路径。失败（目录不可写、文件被占住）不致命 ——
/// 那就只打终端，程序照常跑。
pub fn init_log_file() -> Result<PathBuf> {
    Ok(dps::log::init_log_file(&logs_dir()?, "dpr")?)
}

/// 往日志文件里追加一行（没初始化成功就什么也不做）。
pub fn log_line(line: &str) {
    dps::log::log_line(line);
}

pub use dps::log::strip_ansi;

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

/// 临时目录的**根**：设置里填了就用它，否则退回系统临时目录
/// （`lib.rs` 启动时已把 `TEMP` 指到 `<程序目录>/tmp`）。
///
/// 两个部分各自在根下开一层子目录：DPR 用 `DPR`、DPE 用 `DPE`，
/// 两边的缓存互不覆盖，清一边不会带走另一边。
pub fn temp_root() -> Result<PathBuf> {
    // 没设置时用 `<程序目录>/tmp`，跟 `lib.rs` 启动时给 `TEMP` 指的地方一样
    // （只有那里也写不进去，`lib.rs` 才退回系统临时目录）。
    let dir = match read_config()?.temp_dir {
        Some(dir) => dir,
        None => exe_dir().map_or_else(std::env::temp_dir, |dir| dir.join("tmp")),
    };
    // Windows 的 `GetTempPath` 把结尾的分隔符一起返回（`...\tmp\`），
    // 带着它往下拼就成了 `...\tmp\\DPR`、界面上还会多显示一个反斜杠，这里统一去掉
    // （盘根 `C:\` 保留原样）。
    let dir = match dir.to_str() {
        Some(text) if text.len() > 3 => {
            let trimmed = text.trim_end_matches(['\\', '/']);
            if trimmed.len() >= 3 {
                PathBuf::from(trimmed)
            } else {
                dir
            }
        }
        _ => dir,
    };
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
    } else if !dir.is_dir() {
        bail!("temp directory is not a directory");
    }
    Ok(dir)
}

/// 确保临时目录下的某一层存在且确实是目录（没有就建）。
fn ensure_subdir(dir: PathBuf) -> Result<PathBuf> {
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
    } else if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    Ok(dir)
}

/// DPR（录制与渲染这条线）用的临时目录：临时目录根下的 `DPR`。
///
/// 调用方把中间产物先写这里，成型后再搬进输出目录 —— 转换半路失败时输出目录里
/// 不会留下半套文件，大文件的中间产物也能指到别的盘上。
pub fn get_temp_dir() -> Result<PathBuf> {
    ensure_subdir(temp_root()?.join("DPR"))
}

/// DPE（谱面编辑这条线）用的临时目录：临时目录根下的 `DPE`。
///
/// 主要用来缓存解包出来的谱面文件（谱面目录与 `.pez` 都要先摊成一堆文件才好改）。
pub fn get_dpe_temp_dir() -> Result<PathBuf> {
    ensure_subdir(temp_root()?.join("DPE"))
}

/// DPE（谱面编辑这条线）的输出根：**`<程序目录>/dpe`**。
///
/// 路径固定、不给用户改：每份产物按页面各占一层子目录（[`DPE_NOISE_DIR`] /
/// [`DPE_MODIFY_DIR`]），与 DPR 的 `output/`（渲染成片）分开，找东西时按部分就能定位。
pub const DPE_OUTPUT_DIR: &str = "dpe";

/// 噪域播放器的产物目录：`<dpe>/noisePlayer`。
pub const DPE_NOISE_DIR: &str = "noisePlayer";

/// 谱面修改的产物目录：`<dpe>/modify`。
pub const DPE_MODIFY_DIR: &str = "modify";

/// 编辑窗口的工作目录：`<dpe>/editor`。
///
/// 编辑窗口加载本地谱面时先复制一份到它下面的 [`EDITOR_CHARTS_DIR`]，之后的编辑只动副本，
/// 原文件保持不动。
pub const DPE_EDITOR_DIR: &str = "editor";

/// 编辑窗口的谱面目录：`<dpe>/editor/charts`，加载进来的谱面摊在这里。
pub const EDITOR_CHARTS_DIR: &str = "charts";

/// DPE 的输出根（没有就建）。
pub fn dpe_output_root() -> Result<PathBuf> {
    let exe_dir = exe_dir().ok_or_else(|| anyhow!("failed to locate exe dir"))?;
    ensure_subdir(exe_dir.join(DPE_OUTPUT_DIR))
}

/// DPE 某一页面的输出目录（没有就建）。
pub fn dpe_output_dir(sub: &str) -> Result<PathBuf> {
    ensure_subdir(dpe_output_root()?.join(sub))
}

/// 工具页「噪域播放器」的输出目录：**`<dpe>/noisePlayer`**。
///
/// 它是 DPE 的产物（谱面 `.pez`，不是渲染视频），所以放在 `<程序目录>/dpe` 之下。
pub fn noise_player_dir() -> Result<PathBuf> {
    dpe_output_dir(DPE_NOISE_DIR)
}

/// 编辑窗口的工作目录：**`<dpe>/editor`**（没有就建）。
pub fn editor_dir() -> Result<PathBuf> {
    dpe_output_dir(DPE_EDITOR_DIR)
}

/// 编辑窗口的谱面目录：**`<dpe>/editor/charts`**（没有就建）。
///
/// 加载进来的谱面摊在这里；`editor/` 底下留给编辑过程自己的东西（中间产物、
/// 导出草稿之类），不和用户看得到的那份谱面列表混在一层。
pub fn editor_charts_dir() -> Result<PathBuf> {
    ensure_subdir(editor_dir()?.join(EDITOR_CHARTS_DIR))
}

/// 导出目录的目录名：草稿导出的 `.pez` 就落在 `<dpe>/editor/output`。
pub const EDITOR_OUTPUT_DIR: &str = "output";

/// 编辑窗口的导出目录：**`<dpe>/editor/output`**（没有就建）。
pub fn editor_output_dir() -> Result<PathBuf> {
    ensure_subdir(editor_dir()?.join(EDITOR_OUTPUT_DIR))
}

/// 把一份谱面摊进编辑窗口的谱面目录，返回解包后目录的绝对路径。
///
/// 编辑窗口要动的是摊开的文件（谱面 + 曲绘 + 音频），所以这里**总是落在目录上**：
/// 源是目录就整棵复制，源是 `.pez`（zip 包）就解压成去掉扩展名的同名目录。
/// 重名时另起名字（`名字 (2)`），不覆盖已经存在的目录。
pub fn stage_chart_in_editor(src: &Path) -> Result<PathBuf> {
    let dir = editor_charts_dir()?;
    stage_chart_into(src, &dir)
}

/// 把一份谱面摊进 `dir`（[`stage_chart_in_editor`] 的落点由调用方给，测试用）。
pub fn stage_chart_into(src: &Path, dir: &Path) -> Result<PathBuf> {
    if !src.exists() {
        bail!("{} does not exist", src.display());
    }
    let name = src
        .file_name()
        .ok_or_else(|| anyhow!("cannot stage {}: no file name", src.display()))?;
    if src.is_dir() {
        let dest = unique_path(dir, Path::new(name), true);
        copy_dir_all(src, &dest)?;
        return Ok(dest);
    }
    // 文件：按 zip 解包。`.pez` 就是 zip，解出来的目录名与包名去掉 `.pez` 后一致，
    // 引擎那边认目录也认包，两种取到的是同一份谱面。
    let stem = src
        .file_stem()
        .ok_or_else(|| anyhow!("cannot stage {}: no file stem", src.display()))?;
    let dest = unique_path(dir, Path::new(stem), true);
    extract_zip(src, &dest)?;
    Ok(dest)
}

/// 把 zip 整个解到 `to`。包里的绝对路径与 `..` 由 `enclosed_name` 挡掉。
fn extract_zip(from: &Path, to: &Path) -> Result<()> {
    let file =
        std::fs::File::open(from).with_context(|| format!("打不开谱面 {}", from.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("{} 不是合法的谱面包", from.display()))?;
    std::fs::create_dir_all(to)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let dest = to.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&dest)?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&dest)?;
        std::io::copy(&mut entry, &mut out)?;
    }
    Ok(())
}

/// 编辑窗口解包出来的谱面包：`<dpe>/editor/unpacked`。
///
/// 素材库把 `.pez` / `.zip` 按包里的层级摊成文件夹，摊出来的文件就得有个落脚处：这里只是
/// 缓存，删掉不丢原包（重新导入会再解一次），素材库里记的也是这里的路径。
pub const EDITOR_UNPACKED_DIR: &str = "unpacked";

/// 解包缓存目录（没有就建）。
pub fn editor_unpacked_dir() -> Result<PathBuf> {
    ensure_subdir(editor_dir()?.join(EDITOR_UNPACKED_DIR))
}

/// 把 `.pez` / `.zip` 解到 [`editor_unpacked_dir`]，返回解出来的那个目录。
///
/// 目录名是「包名 + 路径哈希」：同名的包放在不同目录里不会互相盖掉，同一个包重复导入则总
/// 落到同一个目录 —— 素材库里记的就是这里的路径，重导一次等于刷新内容。
pub fn unpack_editor_archive(archive: &Path) -> Result<PathBuf> {
    let stem = archive
        .file_stem()
        .map(|it| it.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dest = editor_unpacked_dir()?
        .join(format!("{}-{:08x}", safe_file_name(&stem), path_hash(archive)));
    if dest.exists() {
        // 包可能换过内容：整棵重解，免得旧条目留在缓存里
        std::fs::remove_dir_all(&dest)
            .with_context(|| format!("清空解包缓存失败：{}", dest.display()))?;
    }
    extract_zip(archive, &dest)?;
    Ok(dest)
}

/// 文件名里不能出现的字符（Windows 的 `\/:*?"<>|` 与控制字符）换成 `_`，再去掉结尾的点和空格。
pub fn safe_file_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    let len = out.trim_end_matches(['.', ' ']).len();
    out.truncate(len);
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// 路径的 FNV-1a 哈希：只用来给解包目录起名，换个位置就是另一个目录，够用。
fn path_hash(path: &Path) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in path.to_string_lossy().to_lowercase().bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// `dir/name`；已经有同名文件（或目录）时换成 `dir/名字 (2).ext`、`(3)` ……
fn unique_path(dir: &Path, name: &Path, is_dir: bool) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = if is_dir {
        (name.to_string_lossy().to_string(), String::new())
    } else {
        (
            name.file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_default(),
            name.extension()
                .map(|ext| format!(".{}", ext.to_string_lossy()))
                .unwrap_or_default(),
        )
    };
    for n in 2..1000 {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(name)
}

fn copy_dir_all(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_dir_all(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
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
                    info!("[文件] {} 被占用（{}），稍后重试", path.display(), err);
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
    /// 临时目录的**根**（通用设置）：留空就用系统临时目录（`<程序目录>/tmp`）。
    ///
    /// 根下面按部分各开一层：DPR 的中间产物（抽出来的音轨、封面、谱面草稿）在 `DPR`，
    /// DPE 缓存解包出来的谱面文件在 `DPE`。中间产物先写临时目录、成型后再搬进输出目录，
    /// 半路失败不会在输出目录里留下半成品，大文件的中间产物也能指到另一块盘上。
    pub temp_dir: Option<PathBuf>,
    pub encoder_avc: Option<String>,
    pub encoder_hevc: Option<String>,
    pub ffmpeg_path: Option<String>,
    pub print_stderr: bool,
    /// 是否把预览的帧率（每秒一行）打到控制台。默认 `false`：那行是刷屏的主力，
    /// 只在量性能时才需要。
    pub print_preview_stats: bool,
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
            temp_dir: None,
            encoder_avc: None,
            encoder_hevc: None,
            ffmpeg_path: None,
            print_stderr: false,
            print_preview_stats: false,
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
    // 「输出日志到控制台」改完立刻生效，不用重启。
    set_console_log(config.print_stderr);
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

/// 已经是压缩格式的后缀：再 deflate 一遍体积几乎不变，只是白烧 CPU（几十 MB 的谱面包里
/// 音频与图片占大头，跳过它们能省下打包时间的大半）。
pub(crate) fn zip_method_of(path: &Path) -> CompressionMethod {
    let extension = path
        .extension()
        .map(|it| it.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "mp3" | "ogg" | "m4a" | "aac" | "flac" | "jpg" | "jpeg" | "png" | "webp" | "zip" | "pez" => CompressionMethod::Stored,
        _ => CompressionMethod::Deflated,
    }
}

/// 边生成边打包的 zip 写入器：条目的内容由调用方现写（内存块或者流），**不落中间文件**。
///
/// 工具页的噪域谱面靠它省掉一趟搬运：谱面 JSON 有几十 MB，先写进临时目录再读回来压缩，
/// 等于把同样的字节在磁盘上搬两遍。这里内容直接从生成处流进 zip，谱面包从一开始就写在
/// 输出目录里。
///
/// 写到一半失败要能收场：先落 `<目标名>.part`，[`Self::finish`] 时才改名成目标名 ——
/// 半路出错（抽帧失败、程序被杀）不会在输出目录里留下一个看起来能打开、其实不完整的包。
pub struct ZipPack {
    /// `finish` 之后变成 `None`：那时文件已经改名，`Drop` 不该再删。
    zip: Option<ZipWriter<BufWriter<File>>>,
    path: PathBuf,
    part: PathBuf,
    level: i64,
}

impl ZipPack {
    /// 建一个包，`level` 是 deflate 压缩级别（0~9）。
    pub fn create(path: impl Into<PathBuf>, level: i64) -> Result<Self> {
        let path = path.into();
        let part = path.with_extension(match path.extension() {
            Some(extension) => format!("{}.part", extension.to_string_lossy()),
            None => "part".to_owned(),
        });
        let file = File::create(&part)?;
        Ok(Self {
            zip: Some(ZipWriter::new(BufWriter::new(file))),
            path,
            part,
            level,
        })
    }

    /// 写一个已经在内存里的条目（`info.yml`、`config.toml`、音频、曲绘这类）。
    pub fn bytes(&mut self, name: &str, data: &[u8]) -> Result<()> {
        self.entry(name, |out| Ok(out.write_all(data)?))
    }

    /// 写一个内容现生成的条目：`write` 拿到的是 zip 里那条数据的写入端。
    pub fn entry<F>(&mut self, name: &str, write: F) -> Result<()>
    where
        F: FnOnce(&mut dyn Write) -> Result<()>,
    {
        let zip = self.zip.as_mut().ok_or_else(|| anyhow!("zip 已经收尾"))?;
        let method = zip_method_of(Path::new(name));
        let options = FileOptions::<()>::default()
            .compression_method(method)
            // `Stored` 不接受压缩级别：给了会得到 `Unsupported compression level`
            .compression_level(matches!(method, CompressionMethod::Deflated).then_some(self.level))
            .unix_permissions(0o755);
        zip.start_file_from_path(name, options)?;
        write(zip)
    }

    /// 收尾：写完中央目录，再把 `.part` 改名成目标文件。
    pub fn finish(mut self) -> Result<()> {
        let zip = self.zip.take().ok_or_else(|| anyhow!("zip 已经收尾"))?;
        let mut file = zip.finish()?;
        file.flush()?;
        drop(file);
        std::fs::rename(&self.part, &self.path)?;
        info!("[文件] 创建压缩包 {}", self.path.display());
        Ok(())
    }
}

impl Drop for ZipPack {
    fn drop(&mut self) {
        // 没收尾就丢掉：删掉半成品
        if self.zip.is_some() {
            let _ = std::fs::remove_file(&self.part);
        }
    }
}

/// 打包成 zip（`.pez` 谱面包、导出谱面共用），最高压缩级别。
pub async fn create_zip(output_path: PathBuf, files: HashMap<String, PathBuf>) -> Result<()> {
    let mut pack = ZipPack::create(output_path, 9)?;
    let mut buffer = vec![0u8; 1 << 20];
    for (file_name, file_path) in files {
        let mut source = File::open(&file_path)?;
        pack.entry(&file_name, |out| {
            loop {
                // 读到 0 就等于 EOF：一块一块往下送，不整份读进内存
                let read = source.read(&mut buffer)?;
                if read == 0 {
                    return Ok(());
                }
                out.write_all(&buffer[..read])?;
            }
        })?;
    }
    pack.finish()
}

/// 预览窗口的初始尺寸（逻辑像素）：把成片的画幅（`RenderConfig::resolution`）等比缩进
/// 1280×720 的框里，取最大的一档。
///
/// 预览窗口画的就是成片那块画布。窗口固定成 1280×720（16:9）时，自定义比例的谱面在窗口里
/// 四周是一圈黑边，看着跟成片对不上；按比例开窗口，引擎（`Resource::update_size`）摆出来的
/// 画面才正好铺满。分辨率填得再大也只缩到这个框，不跟着放大。
pub fn preview_window_size(resolution: (u32, u32)) -> (f32, f32) {
    const BOX_W: f32 = 1280.;
    const BOX_H: f32 = 720.;
    // 谱面自带的 config.toml 允许 `resolution = [0, 0]`（只校验非负），这种值按 16:9 兜底
    let (w, h) = match resolution {
        (w, h) if w > 0 && h > 0 => (w as f32, h as f32),
        _ => (BOX_W, BOX_H),
    };
    let scale = (BOX_W / w).min(BOX_H / h);
    ((w * scale).round().max(2.), (h * scale).round().max(2.))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 预览窗口取成片画幅的比例，并且永远塞在 1280×720 的框里。
    #[test]
    fn preview_window_follows_resolution_ratio() {
        // 16:9 就是原来那个默认尺寸
        assert_eq!(preview_window_size((1920, 1080)), (1280., 720.));
        // 自定义比例：框里最长的一边顶满
        assert_eq!(preview_window_size((1440, 1080)), (960., 720.));
        assert_eq!(preview_window_size((1080, 1080)), (720., 720.));
        assert_eq!(preview_window_size((1080, 1920)), (405., 720.));
        assert_eq!(preview_window_size((2560, 1080)), (1280., 540.));
        // 小分辨率也放大到框里，窗口不至于小得看不清
        assert_eq!(preview_window_size((640, 360)), (1280., 720.));
        // 填了 0 这种不合法的值也不能除出 NaN
        assert_eq!(preview_window_size((0, 0)), (1280., 720.));
    }

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

    /// 「输出预览性能到控制台」是一个独立开关：按 camelCase 落盘、读回来还是 true。
    #[test]
    fn preview_stats_switch_round_trips() {
        let changed = AppConfig {
            print_preview_stats: true,
            ..AppConfig::default()
        };
        let text = toml::to_string(&config_table(&changed, &AppConfig::default()).unwrap()).unwrap();
        assert!(text.contains("printPreviewStats = true"), "{text}");
        assert!(!text.contains("printStderr"), "{text}");
        let back: AppConfig = toml::from_str(&text).unwrap();
        assert!(back.print_preview_stats);
        assert!(!back.print_stderr);
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

    /// 摊进编辑窗口：源是目录就整棵复制，重名时另起名字且不动已有的那份。
    #[test]
    fn stage_chart_into_copies_dirs() {
        let base = std::env::temp_dir().join("dpr-stage-dir-test");
        let _ = std::fs::remove_dir_all(&base);
        let (from, to) = (base.join("from"), base.join("editor"));
        std::fs::create_dir_all(from.join("sub")).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(from.join("a.txt"), "a").unwrap();
        std::fs::write(from.join("sub/b.txt"), "b").unwrap();

        let first = stage_chart_into(&from, &to).unwrap();
        assert_eq!(first, to.join("from"));
        assert_eq!(std::fs::read_to_string(first.join("sub/b.txt")).unwrap(), "b");

        // 再摊一次：名字带序号，第一份的内容不被覆盖
        std::fs::write(from.join("a.txt"), "changed").unwrap();
        let second = stage_chart_into(&from, &to).unwrap();
        assert_eq!(second, to.join("from (2)"));
        assert_eq!(std::fs::read_to_string(to.join("from/a.txt")).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(second.join("a.txt")).unwrap(), "changed");
    }

    /// 摊进编辑窗口：`.pez` 解成去掉扩展名的同名目录，重名另起一个、不覆盖已有的目录。
    #[test]
    fn stage_chart_into_unpacks_pez() {
        let base = std::env::temp_dir().join("dpr-stage-chart-test");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let (dir, source) = (base.join("editor"), base.join("Song.artist.0.AT.pez"));
        std::fs::create_dir_all(&dir).unwrap();

        // 造一个 `.pez`（zip 包）
        {
            let file = std::fs::File::create(&source).unwrap();
            let mut writer = ZipWriter::new(file);
            let options = FileOptions::<()>::default();
            for (name, body) in [("chart.json", "{}"), ("illustration.png", "png")] {
                writer.start_file(name, options).unwrap();
                writer.write_all(body.as_bytes()).unwrap();
            }
            writer.finish().unwrap();
        }

        let first = stage_chart_into(&source, &dir).unwrap();
        assert_eq!(first, dir.join("Song.artist.0.AT"));
        assert!(first.is_dir());
        assert_eq!(
            std::fs::read_to_string(first.join("chart.json")).unwrap(),
            "{}"
        );
        assert_eq!(
            std::fs::read_to_string(first.join("illustration.png")).unwrap(),
            "png"
        );

        // 再摊一次：另起名字，第一份留着
        let second = stage_chart_into(&source, &dir).unwrap();
        assert_eq!(second, dir.join("Song.artist.0.AT (2)"));
        assert!(first.join("chart.json").is_file());
    }
}
