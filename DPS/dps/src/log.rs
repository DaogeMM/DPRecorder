use chrono::Utc;
use colored::Colorize;
use miniquad::{debug, error, info, trace, warn};
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
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
    let filter = if std::env::var("RUST_LOG").is_ok() {
        EnvFilter::from_default_env()
    } else {
        EnvFilter::try_new("hyper=info,rustls=info,debug").unwrap()
    };
    tracing_subscriber::registry().with(CustomLayer).with(filter).init();
}

pub fn register_with_colorize(override_colorize: bool) {
    colored::control::set_override(override_colorize);
    let filter = if std::env::var("RUST_LOG").is_ok() {
        EnvFilter::from_default_env()
    } else {
        EnvFilter::try_new("hyper=info,rustls=info,debug").unwrap()
    };
    tracing_subscriber::registry().with(CustomLayer).with(filter).init();
}
