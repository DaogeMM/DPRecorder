//! 编辑器 ⇄ 预览进程的协议：一行一条 JSON。
//!
//! stdin 收命令，stdout 发事件，日志一律走 stderr（stdout 只留给协议，父进程才好按行解析）。
//! 每次预览都是新进程，所以不设握手：窗口建好后先发一条 `ready`，之后每帧一条 `frame`。

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use serde::Deserialize;

/// 编辑器发给预览的命令。
#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// 探活，回一条 `frame` 之外什么也不做
    Ping,
    /// 把播放头拨到 `t` 秒；`playing` 省略则保持原来的播放状态
    Seek {
        t: f64,
        #[serde(default)]
        playing: Option<bool>,
    },
    Play,
    Pause,
    /// 谱面文件改过了：重新解析并原地换掉当前谱面（不重启进程、不丢播放位置）
    Reload,
    Quit,
}

pub type Queue = Arc<Mutex<VecDeque<Command>>>;

pub fn queue() -> Queue {
    Arc::new(Mutex::new(VecDeque::new()))
}

/// 起一条线程读 stdin。父进程关掉管道（或自己先没了）就当「关闭预览」。
pub fn spawn_stdin_reader(queue: Queue) {
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<Command>(line) {
                Ok(command) => queue.lock().unwrap().push_back(command),
                Err(err) => eprintln!("[dpe] 命令解析失败 {line:?}：{err}"),
            }
        }
        queue.lock().unwrap().push_back(Command::Quit);
    });
}

pub fn emit(event: serde_json::Value) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if writeln!(out, "{event}").is_ok() {
        let _ = out.flush();
    }
}

/// 启动就绪：把谱面路径与临时目录（`<根>/DPE`，谱面缓存放这里）一起报给编辑器。
pub fn emit_ready(input: &str, temp: &str) {
    emit(serde_json::json!({ "ev": "ready", "input": input, "temp": temp }));
}

pub fn emit_frame(time: f64, playing: bool, loaded: bool, length: f64) {
    emit(serde_json::json!({
        "ev": "frame",
        "t": time,
        "playing": playing,
        "loaded": loaded,
        "length": length,
        "fps": macroquad::time::get_fps(),
    }));
}
