//! DPE —— 谱面预览（编辑器那一半的「实时看谱面」）。
//!
//! 一个进程只开一个窗口（见 `DPS/dps/src/scene/split.rs` 的注释），所以编辑器不自己建窗口，
//! 而是把主程序带 `dpe-preview` 参数再拉一份当子进程：stdin 收命令（seek / play / pause /
//! reload），stdout 每帧回一条播放状态，编辑器据此驱动时间轴与「改一下就刷新」的热重载。
//! 协议见 `protocol.rs`。
//!
//! 这条线不单独出 exe：预览窗口与 DPRecorder 是同一个程序，只是走另一个入口
//! （整份项目只有 DPRecorder 与 DPSimulator 两个程序，见 `DPR/src/parts.ts`）。

mod cmd;
mod preview;
mod protocol;
mod temp;

/// 预览入口：`dpr dpe-preview --input ...`，参数在 `cmd.rs` 里解析。
pub fn run(it: impl Iterator<Item = String>) {
    let args = match cmd::Args::parse(it) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("[命令行] {err}\n\n{}", cmd::USAGE);
            std::process::exit(2);
        }
    };

    // 临时目录要在开窗口前定好：之后 dps 与 ffmpeg 之类都读 `TMP` / `TEMP`。
    temp::init();

    let conf = preview::window_conf(&args);
    macroquad::Window::from_config(conf, async move {
        if let Err(err) = preview::run(args).await {
            eprintln!("[预览] 失败：{err:#}");
            std::process::exit(1);
        }
    });
    std::process::exit(0);
}
