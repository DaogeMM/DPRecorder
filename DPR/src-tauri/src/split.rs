//! 游玩分屏：主窗口跑原谱面、副窗口跑改过的谱面，两个窗口是两个进程。
//!
//! 渲染是 macroquad（一个进程只能有一个窗口），所以「分屏」= 父进程一次开两个子进程。
//! 同步只有一条线：主进程每帧把播放时刻经本机 TCP 发给副进程，副进程每帧把时刻拨过去。
//! 暂停不用单独同步 —— 主进程暂停时 `TimeManager` 不再前进，副进程每帧被拨回同一个时刻，
//! 画面自然停住；恢复播放后时刻继续往前走，副进程跟着走。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

/// 连/绑端口的重试次数与间隔：两个进程几乎同时起来，谁先谁后不一定。
const RETRY: usize = 125;
const RETRY_INTERVAL: Duration = Duration::from_millis(40);

/// 本进程在分屏里扮演的角色（不在分屏里就是 `None`）。
pub struct Role {
    /// `true` 是副进程：渲染改过的谱面、静音、时刻跟着主进程。
    pub secondary: bool,
    /// 主进程监听、副进程连上来的本机端口。
    pub port: u16,
}

impl Role {
    /// 从 `DPR_SPLIT_ROLE` / `DPR_SPLIT_PORT` 读角色，这两个变量由父进程在拉起子进程时设好。
    pub fn from_env() -> Option<Self> {
        let name = std::env::var("DPR_SPLIT_ROLE").ok()?;
        let port = std::env::var("DPR_SPLIT_PORT").ok()?.parse().ok()?;
        Some(Self {
            secondary: name == "secondary",
            port,
        })
    }
}

/// 窗口标题：两个进程都叫 `DPRecorder` 的话，按标题找窗口会找错，所以分屏时带角色后缀。
pub fn window_title(secondary: bool) -> &'static str {
    if secondary {
        "DPRecorder 副谱面"
    } else {
        "DPRecorder 主谱面"
    }
}

/// 把本进程的窗口摆到屏幕左半边（主）或右半边（副）。
///
/// macroquad / miniquad 的 `Conf` 没有窗口位置这一项（本仓库用的 fork 里翻了 `Conf` 与
/// macroquad 的 `window.rs` 都没有），只能等窗口建好后用 winapi 按标题找到它再 `SetWindowPos`。
pub fn place_window(secondary: bool) {
    #[cfg(target_os = "windows")]
    {
        use winapi::um::winuser::{
            FindWindowW, GetSystemMetrics, SetWindowPos, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE,
            SWP_NOZORDER,
        };

        let title: Vec<u16> = window_title(secondary)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            // 窗口刚建好时可能还查不到，重试几次。
            let mut hwnd = std::ptr::null_mut();
            for _ in 0..50 {
                hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
                if !hwnd.is_null() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            if hwnd.is_null() {
                return;
            }
            let width = GetSystemMetrics(SM_CXSCREEN);
            let height = GetSystemMetrics(SM_CYSCREEN);
            let x = if secondary { width / 2 } else { 0 };
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                x,
                0,
                width / 2,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = secondary;
    }
}

/// 主进程这一侧的发送端：接住副进程的连接，每帧把时刻写过去。
pub struct TimeSender {
    listener: TcpListener,
    clients: Vec<TcpStream>,
}

impl TimeSender {
    /// 绑定端口；副进程起得比主进程早时会连不上，所以副进程那边也要重试，这里只管绑。
    pub fn new(port: u16) -> Option<Self> {
        let mut listener = None;
        for _ in 0..RETRY {
            if let Ok(bound) = TcpListener::bind(("127.0.0.1", port)) {
                listener = Some(bound);
                break;
            }
            std::thread::sleep(RETRY_INTERVAL);
        }
        let listener = listener?;
        let _ = listener.set_nonblocking(true);
        Some(Self {
            listener,
            clients: Vec::new(),
        })
    }

    /// 收下新连接并把时刻发给所有已连上的副进程；写失败的连接直接丢掉。
    pub fn send(&mut self, time: f64) {
        while let Ok((stream, _)) = self.listener.accept() {
            let _ = stream.set_nodelay(true);
            self.clients.push(stream);
        }
        let line = format!("t {time:.4}\n");
        self.clients
            .retain_mut(|client| client.write_all(line.as_bytes()).is_ok());
    }
}

/// 副进程这一侧的接收端：把已经到达的时刻都读掉，只留最后一个（中间帧没有价值）。
pub struct TimeReceiver {
    stream: TcpStream,
    buffer: Vec<u8>,
    closed: bool,
}

impl TimeReceiver {
    pub fn new(port: u16) -> Option<Self> {
        let mut stream = None;
        for _ in 0..RETRY {
            if let Ok(connected) = TcpStream::connect(("127.0.0.1", port)) {
                stream = Some(connected);
                break;
            }
            std::thread::sleep(RETRY_INTERVAL);
        }
        let stream = stream?;
        let _ = stream.set_nodelay(true);
        let _ = stream.set_nonblocking(true);
        Some(Self {
            stream,
            buffer: Vec::new(),
            closed: false,
        })
    }

    /// 主窗口那边的连接断了：副窗口该跟着退出，免得只剩它一个还在放。
    pub fn closed(&self) -> bool {
        self.closed
    }

    /// 返回本轮读到的最新时刻；没新消息时是 `None`（此时保持当前时刻不动）。
    pub fn latest(&mut self) -> Option<f64> {
        let mut chunk = [0u8; 512];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    self.closed = true;
                    break;
                }
                Ok(count) => self.buffer.extend_from_slice(&chunk[..count]),
                Err(ref err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.closed = true;
                    break;
                }
            }
        }
        // 主进程掉线 / 卡住时缓冲不该无限涨。
        if self.buffer.len() > 4096 {
            self.buffer.clear();
        }
        let mut latest = None;
        while let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            let Ok(text) = std::str::from_utf8(&line) else {
                continue;
            };
            if let Some(rest) = text.trim_end().strip_prefix("t ") {
                if let Ok(time) = rest.parse::<f64>() {
                    latest = Some(time);
                }
            }
        }
        latest
    }
}
