//! DPE —— 谱面预览进程（DPEdit 的「实时看谱面」那一半）。
//!
//! 一个进程只开一个窗口（见 `DPS/dps/src/scene/split.rs` 的注释），所以编辑器不自己建窗口，
//! 而是把 DPE 当子进程拉起来：stdin 收命令（seek / play / pause / reload），stdout 每帧回一条
//! 播放状态，编辑器据此驱动时间轴与「改一下就刷新」的热重载。协议见 `protocol.rs`。

mod cmd;
/// 窗口图标：与 DPR 的预览窗口用同一份（`DPR/src-tauri/src/icon.rs` 由
/// `DPR/src/assets/favicon.png` 生成，直接引用，避免这里再复制一份几千行字节）。
#[path = "../../DPR/src-tauri/src/icon.rs"]
mod icon;
mod preview;
mod protocol;
mod temp;

use std::process::exit;

use anyhow::Result;

fn main() {
    // dps 的文件接口内部走 `tokio::task::spawn_blocking`，调用时必须在 runtime 上下文里；
    // guard 要活到进程结束 —— macroquad 的主循环在同一线程里 poll，加载谱面就发生在其中。
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("创建 tokio runtime 失败");
    let _guard = rt.enter();

    let args = match cmd::Args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("[命令行] {err}\n\n{}", cmd::USAGE);
            exit(2);
        }
    };

    // 临时目录要在开窗口前定好：之后 dps 与 ffmpeg 之类都读 `TMP` / `TEMP`。
    temp::init();

    let conf = preview::window_conf(&args);
    macroquad::Window::from_config(conf, async move {
        if let Err(err) = preview::run(args).await {
            eprintln!("[预览] 失败：{err:#}");
            exit(1);
        }
    });
    exit(0);
}

/// 让 `Result` 在 main 里只出现一次，方便各模块统一用 anyhow。
pub type AnyResult<T> = Result<T>;
