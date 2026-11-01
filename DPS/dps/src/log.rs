use chrono::Utc;
use colored::Colorize;
use miniquad::{debug, error, info, trace, warn};
use std::{
    fs::OpenOptions,
    io::Write,
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
