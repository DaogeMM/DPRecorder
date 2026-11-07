use chrono::{Local, Utc};
use colored::Colorize;
use miniquad::{debug, error, info, trace, warn};
use std::{
    fs::OpenOptions,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        OnceLock,
    },
};
use tracing::{field::Visit, Level, Subscriber};
use tracing_subscriber::{prelude::*, EnvFilter, Layer};

/// 日志落盘的文件路径。
///
/// 设了之后每个事件除了照旧打到终端（渲染子进程的 stderr 会被父进程收进「输出」面板），
/// 还以**纯文本**追加进这个文件。路径由宿主程序给（DPR 的 `common::init_log_file`）：
/// 目录创建、按日期换文件都是宿主的事，日志层只管写。
/// 一个进程一份 —— 日志层在一个进程里只会注册一次。
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// 让后续日志同时写进 `path`（追加写，文件不存在就建）。
///
/// 第一次设的那个生效，之后再调用不会换文件（同一个进程只有一份日志）。
pub fn set_log_file_path(path: PathBuf) {
    let _ = LOG_PATH.set(path);
}

static CONSOLE_ENABLED: AtomicBool = AtomicBool::new(true);

/// 设置「输出日志到控制台」：`false` 时只拦终端那一路，日志文件与「输出」面板照旧。
///
/// 关掉后**信息与调试**不打终端，**警告与错误仍然打** —— 这个开关是为了清掉进度那类刷屏，
/// 不是让人看不见出错。
pub fn set_console_enabled(enabled: bool) {
    CONSOLE_ENABLED.store(enabled, Ordering::Relaxed);
}

/// 终端那一路现在开着没有（见 `set_console_enabled`）。
pub fn console_enabled() -> bool {
    CONSOLE_ENABLED.load(Ordering::Relaxed)
}

static CONSOLE_DEBUG: AtomicBool = AtomicBool::new(false);

/// 设置终端是否显示 DEBUG / TRACE：默认不显示，只写进日志文件。
///
/// 解复用、字体缓存这类第三方库会按帧打调试行，全打到控制台会把真正的进度与错误冲掉。
/// 设了 `RUST_LOG` 的进程按「详细」处理 —— 愿意自己配过滤规则的人本来就想要细节。
pub fn set_console_debug(enabled: bool) {
    CONSOLE_DEBUG.store(enabled, Ordering::Relaxed);
}

/// 终端现在显示不显示 DEBUG / TRACE（见 `set_console_debug`）。
pub fn console_debug() -> bool {
    CONSOLE_DEBUG.load(Ordering::Relaxed)
}

/// 控制台认不认 ANSI 转义序列（`init_console` 里探一次，之后 `wants_color` 只看它）。
static CONSOLE_VT: AtomicBool = AtomicBool::new(false);

/// 启动时准备控制台：切到 UTF-8 代码页、打开 ANSI 转义处理。
///
/// 代码页不设成 65001 的话，中文在 936（GBK）的 cmd 窗口里是乱码；ANSI 转义在旧版
/// Windows 控制台里不认，所以先试着打开，打不开就整场不打颜色码。
pub fn init_console() {
    #[cfg(windows)]
    {
        use winapi::um::consoleapi::{GetConsoleMode, SetConsoleMode};
        use winapi::um::processenv::GetStdHandle;
        use winapi::um::wincon::{SetConsoleOutputCP, ENABLE_VIRTUAL_TERMINAL_PROCESSING};

        // 控制台的标准句柄号（winbase.h 里的常量）。
        const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
        const STD_ERROR_HANDLE: u32 = -12i32 as u32;

        // 只影响输出；输入代码页留给控制台自己管。
        unsafe { SetConsoleOutputCP(65001) };
        let mut vt = true;
        for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            unsafe {
                let handle = GetStdHandle(which);
                let mut mode = 0;
                if handle.is_null() || GetConsoleMode(handle, &mut mode) == 0 || SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) == 0 {
                    vt = false;
                }
            }
        }
        CONSOLE_VT.store(vt, Ordering::Relaxed);
    }
}

/// 这一行要不要带颜色码：接在终端上就带；渲染子进程的 stderr 被父进程收进「输出」面板，
/// 面板按 ANSI 上色，所以父进程起子进程时会塞一个 `DPR_CONSOLE_ANSI` 让它也带上。
pub fn wants_color() -> bool {
    std::env::var_os("DPR_CONSOLE_ANSI").is_some() || (CONSOLE_VT.load(Ordering::Relaxed) && std::io::stderr().is_terminal())
}

/// 控制台一行的语气：决定颜色，也决定开关关掉之后还打不打。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// 普通进度与提示 —— 受「输出日志到控制台」控制。
    Info,
    /// 警告（黄）：环境不对、配置退回默认之类，但程序还能跑 —— 开关关着也打。
    Warn,
    /// 错误（红）：这一步失败了 —— 开关关着也打。
    Error,
}

/// 打一行到控制台；`Tone::Info` 受开关控制，警告与错误不论开关都打。
pub fn console_line(tone: Tone, args: std::fmt::Arguments) {
    if tone == Tone::Info && !console_enabled() {
        return;
    }
    let text = args.to_string();
    let code = match tone {
        Tone::Info => "",
        Tone::Warn => "\x1b[33m",
        Tone::Error => "\x1b[31m",
    };
    if code.is_empty() || !wants_color() {
        eprintln!("{text}");
    } else {
        eprintln!("{code}{text}\x1b[0m");
    }
}

/// 原地刷新一行进度：回车回到行首、清掉这一行，不换行。
///
/// 只在真的接在终端上时这么做 —— 输出被重定向或管道接走时，一堆 `\r` 只会把文件搞花。
pub fn console_progress(text: &str) {
    if !console_enabled() || !std::io::stderr().is_terminal() {
        return;
    }
    // 认得转义序列就顺手清掉这一行（新内容比旧的短时不留尾巴），不认就只回行首。
    if CONSOLE_VT.load(Ordering::Relaxed) {
        eprint!("\r\x1b[2K{text}");
    } else {
        eprint!("\r{text}");
    }
}

/// 进度打完之后收尾换行，免得后面的日志接在同一行尾巴上。
pub fn console_progress_done() {
    if !console_enabled() || !std::io::stderr().is_terminal() {
        return;
    }
    eprintln!();
}

/// 改控制台窗口标题（最小化后任务栏上也能看到进度）。没有控制台时什么也不做。
pub fn set_console_title(title: &str) {
    #[cfg(windows)]
    unsafe {
        use winapi::um::wincon::{GetConsoleWindow, SetConsoleTitleW};

        if GetConsoleWindow().is_null() {
            return;
        }
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        SetConsoleTitleW(wide.as_ptr());
    }
    #[cfg(not(windows))]
    let _ = title;
}

/// 打印一行到控制台，受「输出日志到控制台」开关控制。
///
/// 日志宏（`info!` 等）已经按同一个开关拦掉了；这个宏给那些不走日志层的提示
/// （启动早期的诊断、子进程 stderr 的转发）用，免得控制台一半有一半没有。
/// `warn,` / `error,` 这两种语气带颜色，且开关关着也照打。
#[macro_export]
macro_rules! console {
    (warn, $($arg:tt)*) => {
        $crate::log::console_line($crate::log::Tone::Warn, format_args!($($arg)*))
    };
    (error, $($arg:tt)*) => {
        $crate::log::console_line($crate::log::Tone::Error, format_args!($($arg)*))
    };
    ($($arg:tt)*) => {
        $crate::log::console_line($crate::log::Tone::Info, format_args!($($arg)*))
    };
}

/// 打开这次运行的日志文件：**按天一份**（`dir/{prefix}-YYYY-MM-DD.log`），追加写。
///
/// 目录建不出来、文件写不进去就返回 `Err`，由调用方决定退到哪（DPR 退回数据目录）：
/// 日志写不了不是致命错误，程序该照常跑。
pub fn init_log_file(dir: &Path, prefix: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{prefix}-{}.log", Local::now().format("%Y-%m-%d")));
    // 立刻真开一次：目录不可写、文件被别的进程独占时在这里就报错，别等到写第一行才发现。
    open_log(&path)?;
    set_log_file_path(path.clone());
    Ok(path)
}

/// 往日志文件里追加一行人看的文字（没初始化成功就什么也不做）。
///
/// 行尾换行去掉、ANSI 颜色码剥掉 —— 这份文件是要给人看、给 issue 贴的纯文本。
pub fn log_line(line: &str) {
    let line = strip_ansi(line);
    let line = line.trim_end_matches(['\r', '\n']);
    if !line.is_empty() {
        write_log_file(line);
    }
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

/// 启动横幅：`名称 版本 · 键 值 · …`，直打 stderr。
///
/// 不受「输出日志到控制台」开关管 —— 那个开关默认关着，否则启动时控制台一行都没有，
/// 看着像没起来。版本号取本 crate 的：工作区里各包共用一个版本。
pub fn startup_banner(name: &str, fields: &[(&str, &str)]) {
    let mut line = format!("{name} {}", env!("CARGO_PKG_VERSION"));
    for (key, value) in fields {
        line.push_str(" · ");
        line.push_str(key);
        line.push(' ');
        line.push_str(value);
    }
    eprintln!("{line}");
}

/// 往日志文件里追加一行；没设路径就只打终端。
///
/// 每次写完就关掉句柄：长期占着写句柄会把文件锁住，用户就没法在程序运行时打开、
/// 拷走这份日志去提问。日志量不大，多开的这几下开销可以忽略。
fn write_log_file(line: &str) {
    let Some(path) = LOG_PATH.get() else {
        return;
    };
    let Ok(mut file) = open_log(path) else {
        return;
    };
    let _ = file.write_all(line.as_bytes());
    let _ = file.write_all(b"\n");
}

/// 打开日志文件（追加）。给别的程序留出读权限，编辑器才能同时打开它。
fn open_log(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0x0000_0007); // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
    }
    options.open(path)
}

struct CustomLayer;

impl<S> Layer<S> for CustomLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        struct Visitor(Option<String>, Vec<(&'static str, String)>);
        impl Visit for Visitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                let val = format!("{value:?}");
                if field.name() == "message" {
                    self.0 = Some(val);
                } else if !field.name().starts_with("log.") {
                    self.1.push((field.name(), val));
                }
            }
        }

        let mut v = Visitor(None, Vec::new());
        event.record(&mut v);

        let meta = event.metadata();

        let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
        #[cfg(not(target_os = "android"))]
        let level = match *meta.level() {
            Level::TRACE => "TRACE",
            Level::DEBUG => "DEBUG",
            Level::INFO => " INFO",
            Level::WARN => " WARN",
            Level::ERROR => "ERROR",
        };

        // 结构化字段拼成 `{name=value name=value}`：文件里要纯文本，终端那份的字段名还带斜体，
        // 所以两种形态一起拼，省得后面再拆一遍。
        let mut fields = String::new();
        let mut fields_colored = String::new();
        for (idx, (name, val)) in v.1.iter().enumerate() {
            use std::fmt::Write as _;
            if idx > 0 {
                fields.push(' ');
                fields_colored.push(' ');
            }
            let _ = write!(fields, "{name}={val}");
            let _ = write!(fields_colored, "{}={val}", name.italic());
        }

        // 落盘：和终端同一份内容，只是没有 ANSI 颜色码 —— 导出去给人看、贴进 issue 都干净。
        let mut plain = format!("{timestamp} {level} {}", meta.target());
        if !fields.is_empty() {
            plain.push('{');
            plain.push_str(&fields);
            plain.push('}');
        }
        if let Some(content) = &v.0 {
            plain.push_str(": ");
            plain.push_str(content);
        }
        write_log_file(&plain);

        // 终端那一路按开关拦掉信息与调试（警告与错误照旧）；文件已经写完了。
        if !console_enabled() && *meta.level() != Level::WARN && *meta.level() != Level::ERROR {
            return;
        }

        // 控制台默认只留本程序自己的 INFO 及以上：解复用器、字体缓存这些第三方库的调试行
        // 会把真正的进度与错误冲掉。它们照旧写进日志文件，配了 `RUST_LOG` 就全放出来。
        if !console_debug() {
            if matches!(*meta.level(), Level::TRACE | Level::DEBUG) {
                return;
            }
            let target = meta.target();
            let ours =
                target.starts_with("dpr") || target.starts_with("dpe") || target.starts_with("dps");
            if !ours && *meta.level() != Level::WARN && *meta.level() != Level::ERROR {
                return;
            }
        }

        #[cfg(not(target_os = "android"))]
        let mut msg = format!("{} ", timestamp.bright_black())
            + &match *meta.level() {
                Level::TRACE => "TRACE".bright_black(),
                Level::DEBUG => "DEBUG".magenta(),
                Level::INFO => " INFO".green(),
                Level::WARN => " WARN".yellow(),
                Level::ERROR => "ERROR".red(),
            }
            .to_string()
            + " ";

        #[cfg(target_os = "android")]
        let mut msg = String::new();

        msg += &meta.target().bright_black().to_string();
        if !fields_colored.is_empty() {
            msg += &"{".bold().to_string();
            msg += &fields_colored;
            msg += &"}".bold().to_string();
        }
        if let Some(content) = v.0 {
            msg += ": ";
            msg += &content;
        }

        match *meta.level() {
            Level::TRACE => trace!("{}", msg),
            Level::DEBUG => debug!("{}", msg),
            Level::INFO => info!("{}", msg),
            Level::WARN => warn!("{}", msg),
            Level::ERROR => error!("{}", msg),
        }
    }
}

pub fn register() {
    // 控制台不认 ANSI（或压根没接终端）时整场不打颜色码：`colored` 自己判断不出控制台支不支持。
    colored::control::set_override(wants_color());
    // 没配 `RUST_LOG` 的控制台只到 INFO：调试行照旧写进日志文件
    set_console_debug(std::env::var("RUST_LOG").is_ok());
    let filter = if std::env::var("RUST_LOG").is_ok() {
        EnvFilter::from_default_env()
    } else {
        EnvFilter::try_new("hyper=info,rustls=info,debug").unwrap()
    };
    tracing_subscriber::registry().with(CustomLayer).with(filter).init();
}

pub fn register_with_colorize(override_colorize: bool) {
    colored::control::set_override(override_colorize);
    set_console_debug(std::env::var("RUST_LOG").is_ok());
    let filter = if std::env::var("RUST_LOG").is_ok() {
        EnvFilter::from_default_env()
    } else {
        EnvFilter::try_new("hyper=info,rustls=info,debug").unwrap()
    };
    tracing_subscriber::registry().with(CustomLayer).with(filter).init();
}
