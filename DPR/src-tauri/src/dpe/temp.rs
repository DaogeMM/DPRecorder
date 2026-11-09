//! 临时目录：DPE 自己的谱面缓存放这里。
//!
//! 根目录由外壳（DPRecorder 主程序）决定 —— 设置里填了「临时文件目录」就用它，否则是
//! `<程序目录>/tmp`；两种情况下外壳都会把根写进 `DPRECORDER_TEMP_DIR` 交给子进程，
//! 所以这里与 DPR 那条线看到的是同一个根，只是各用根下的一层子目录：
//! DPR 用 `DPR`（`DPR/src-tauri/src/common.rs` 的 `temp_root`），DPE 用 `DPE`。
//!
//! 从命令行直接起 `dpr dpe-preview`（没有外壳传环境变量）时按 `<程序目录>/tmp` 处理，
//! 那里也写不进去才退回系统临时目录。

use std::path::PathBuf;
use std::sync::OnceLock;

/// 与外壳约定的环境变量：临时目录的**根**。
const ROOT_ENV: &str = "DPRECORDER_TEMP_DIR";

/// 子目录名：DPR 用 `DPR`，这里用 `DPE`，两边的中间产物互不覆盖。
const SUBDIR: &str = "DPE";

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// 解析并建好临时目录，返回它的路径；顺带把 `TMP` / `TEMP` 指过去，
/// 让 dps 与 ffmpeg 之类直接读环境变量的地方也落在同一个盘上。
///
/// 进程刚起来时调一次即可；之后的 `dir()` 拿的是同一个值。
pub fn init() -> PathBuf {
    let root = match std::env::var_os(ROOT_ENV).map(PathBuf::from) {
        Some(root) => root,
        None => std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("tmp")))
            .unwrap_or_else(std::env::temp_dir),
    };
    let dir = root.join(SUBDIR);
    let dir = if std::fs::create_dir_all(&dir).is_ok() {
        std::env::set_var("TMP", &dir);
        std::env::set_var("TEMP", &dir);
        dir
    } else {
        // 指定目录写不进去就退回系统临时目录，别让预览因为一个缓存目录起不来
        std::env::temp_dir().join(SUBDIR)
    };
    let _ = std::fs::create_dir_all(&dir);
    DIR.get_or_init(|| dir).clone()
}

/// 临时目录（`init()` 之后才准）。
pub fn dir() -> PathBuf {
    DIR.get().cloned().unwrap_or_else(init)
}
