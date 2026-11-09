//! 浏览器里的 dps：把一个谱面用 Exercise 模式跑起来，验证「引擎 → wasm → 画面」这条链路。
//!
//! 这里跑的是「编辑器预览」的形态：不放引擎自带的开场加载页，谱面放完也不弹结算页，
//! 画面只由 `EditorScene` 里那个 `GameScene` 负责。
//!
//! 资源优先走页面用 `fw_put` 塞进内存的文件表，没塞的路径再退回 macroquad 的 HTTP 加载
//! （wasm 上 `load_file` 会变成同目录的相对请求，所以旁边摆一份 `assets/` 也能跑）。
//! 音频由引擎自己的 WebAudio 后端出声（cpal 的 wasm 后端），出不了声时退回静音。

use anyhow::{Context, Result};
use dps::{
    config::{Config, Mods},
    core::init_assets,
    fs::{self as dpsfs, fs_from_assets},
    scene::{GameMode, GameScene, LoadingScene, Main, NextScene, Scene},
    time::TimeManager,
    ui::{FontArc, TextPainter, Ui},
};
use macroquad::prelude::*;
use std::any::Any;
use std::cell::{Cell, RefCell};

thread_local! {
    /// 最近一条进度文本。页面用 `wm_status()` 轮询它，卡住时能看出停在哪个阶段。
    static STATUS: RefCell<String> = const { RefCell::new(String::new()) };
    /// 页面轮询的谱面时间与暂停态，每帧由主循环回填。
    static CLOCK: Cell<f64> = const { Cell::new(0.0) };
    static PAUSED: Cell<bool> = const { Cell::new(false) };
}

#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    #[wasm_bindgen::prelude::wasm_bindgen(js_namespace = console, js_name = log)]
    fn js_log(s: &str);
}

fn set_status(msg: impl Into<String>) {
    let msg = msg.into();
    js_log(&format!("[dps-wasm] {msg}"));
    STATUS.with(|it| *it.borrow_mut() = msg);
}

/// 页面轮询这个函数，拿到引擎当前停在哪个阶段。
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn wm_status() -> String {
    STATUS.with(|it| it.borrow().clone())
}

/// 当前谱面时间（秒）。页面用它给 `<audio>` 对时。
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn wm_time() -> f64 {
    CLOCK.with(|it| it.get())
}

/// 是否暂停。页面据此暂停 / 继续 `<audio>`。
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn wm_paused() -> bool {
    PAUSED.with(|it| it.get())
}

/// 排查用：wasm 这边看到的资源表键名（页面塞进来的谱面文件应当都在里面）。
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn wm_debug_files() -> String {
    format!("{:?}", dps::fs::debug_wasm_files())
}

/// 取走这一帧该播的按键音效名（逗号分隔，可能为空串）。
///
/// 只有在引擎自己出不了声（退回了静音后端）时才会往队列里投名字，
/// 页面每帧取一次再用 WebAudio 播 `assets/<名字>.ogg`。
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn wm_take_hitsounds() -> String {
    dps::judge::wasm_sfx::take()
}

/// 引擎是不是用浏览器的 WebAudio 直接出声（页面据此决定要不要跑自己的音效桥与 BGM）。
///
/// 音频后端在载入谱面（`Resource::new`）时才建，所以页面要等到 `wm_status()` 报告就绪之后再问。
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn wm_audio_native() -> bool {
    dps::ext::wasm_audio_native()
}

#[macroquad::main("DPS wasm player")]
async fn main() {
    if let Err(err) = run().await {
        let text = format!("{err:?}");
        set_status(format!("ERROR {text}"));
        error_screen(text).await;
    }
}

/// 起不来时把错误摊在画面上，截图就能直接看到。
async fn error_screen(text: String) {
    loop {
        clear_background(BLACK);
        let mut y = 40.0;
        for line in text.lines().take(24) {
            draw_text(line, 20.0, y, 18.0, RED);
            y += 22.0;
        }
        draw_text("dps wasm player 启动失败", 20.0, y + 24.0, 20.0, WHITE);
        next_frame().await;
    }
}

/// DPR 预览实际使用的渲染配置。
///
/// 基准是 DPR 的 `DEFAULT_RENDER_CONFIG`（`DPR/src/model.ts`，GUI 与预览共用那份），
/// 再叠上 `DPR/src-tauri/src/preview.rs` 里的覆盖项。与 DPS 的 `Config::default()`
/// 取值相同的键不重复写，这里只补两边的差异，改动时照 DPR 那份改。
///
/// 只取「预览看得到」的项：分辨率、帧率、码率、编码器、`sampleCount` 这些是成片输出的
/// 参数，网页版只做预览，不碰。
fn dpr_preview_config() -> Config {
    let mut config = Config::default();
    // 与 DEFAULT_RENDER_CONFIG 的差异（括号里是 DPS 默认值）。
    config.aggressive_chart = false; // true，激进谱面加载
    config.aspect_ratio = Some(16.0 / 9.0); // None，回落谱面自带的宽高比
    config.volume_music = 0.5; // 1.0
    config.volume_sfx = 0.4; // 0.0 —— 关着会让按键音效整条链路静音
    config.combo = "AUTOPLAY".to_owned(); // "RECALL"
    config.render_judge_range = false; // true，判定范围条
    config.judge_range_time = 1.0; // 3.0
    config.judge_range_fade_time = 0.3; // 1.8
    // 编辑器里拖到哪里就该直接看到哪里：开场的淡入加载动画（还有它那几秒等待）一律不要。
    config.enter_animation = false;
    // 编辑器预览只要谱面本身：连击数、分数、暂停按钮、曲名、难度、Lv 等级、进度条都不画。
    // 这几个是引擎的 `render_ui_*`，渲染页那边有它自己的默认值（全开），两边互不影响。
    config.render_ui_combo = false; // true，连击数
    config.render_ui_score = false; // true，分数
    config.render_ui_pause = false; // true，常驻的暂停小按钮
    config.render_ui_name = false; // true，曲名
    config.render_ui_difficulty = false; // true，难度名
    config.render_ui_level = false; // true，Lv 等级数字
    config.render_ui_bar = false; // true，谱面进度条
    // 玩家名 / RKS / 挑战段位不跟：那是本地档案，预览画面不读，页面里也不该写死别人的 ID。
    config
}

// 页面通过 `wm_set_config` 塞进来的整份渲染配置（DPDebugger 用）。
//
// 有它的时候就不再用 `dpr_preview_config()`：那份配置是为「编辑器预览只看谱面」把关掉的
// 界面元素关掉、把强度调低用的，而调试窗口要看的就是成片那一刻的画面。
thread_local! {
    static PAGE_CONFIG: RefCell<Option<Config>> = const { RefCell::new(None) };
}

/// 把一份配置修成网页播放器能跑的样子：强制 autoplay、关掉自动校时、时间轴铺满整张谱面。
///
/// 这几项不管配置从哪儿来都得压上（见 `run()` 里那段注释），
/// 所以首次建场景与 `wm_reload()` 都走这里，免得两处各写一遍。
fn prepare_config(mut config: Config) -> Config {
    config.mods.insert(Mods::AUTOPLAY);
    // 谱面时间默认会按「音乐播放位置」纠正（`TimeManager::update` 只在 `auto_tweak_offset`
    // 打开时生效），而 wasm 的静音音频后端从来没有音频线程去推进 `MusicClock::position`，
    // 那个位置永远停在最后一次 seek 的地方 ⇒ 谱面时间被摁住不动。关掉它，时间就只剩
    // `performance.now()` 一条来源；音乐由页面侧 `<audio>` 按 `wm_time()` 对齐。
    config.auto_tweak_offset = false;
    // 调试窗口要预览整张谱面，不套用谱面自带的试听区间（info.txt 的 previewS / previewE）。
    config.play_start_time = 0.0;
    config.play_end_time = None;
    // 页面地址带 `?start=<秒>` 时用那个起播点（`?start=0` 即从头放）。
    #[cfg(target_arch = "wasm32")]
    if let Some(start) = url_start_seconds() {
        config.play_start_time = start.max(0.0);
    }
    config
}

/// 编辑器预览的场景壳：只做一件事——把引擎在谱面结尾推上来的结算页（`NextScene::Overlay`）
/// 丢掉。编辑窗口里播放头由页面控制，放完停在最后一帧比弹一个「再来一次 / 继续」的
/// 结算界面更合适（那套按钮在预览里也没法用）。
struct EditorScene {
    inner: GameScene,
}

impl Scene for EditorScene {
    fn enter(&mut self, tm: &mut TimeManager, target: Option<RenderTarget>) -> Result<()> {
        self.inner.enter(tm, target)
    }

    fn pause(&mut self, tm: &mut TimeManager) -> Result<()> {
        self.inner.pause(tm)
    }

    fn resume(&mut self, tm: &mut TimeManager) -> Result<()> {
        self.inner.resume(tm)
    }

    fn foucus_pause(&mut self, tm: &mut TimeManager) -> Result<()> {
        self.inner.foucus_pause(tm)
    }

    fn foucus_resume(&mut self, tm: &mut TimeManager) -> Result<()> {
        self.inner.foucus_resume(tm)
    }

    fn update(&mut self, tm: &mut TimeManager) -> Result<()> {
        self.inner.update(tm)
    }

    fn render(&mut self, tm: &mut TimeManager, ui: &mut Ui) -> Result<()> {
        self.inner.render(tm, ui)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        self.inner.as_any_mut()
    }

    fn next_scene(&mut self, tm: &mut TimeManager) -> NextScene {
        match self.inner.next_scene(tm) {
            NextScene::Overlay(_) => NextScene::None,
            other => other,
        }
    }
}

async fn run() -> Result<()> {
    // 资源要由页面用 `fw_put` 塞进内存文件系统，而 wasm 一实例化就直接跑进 `run()`：
    // 先让出一帧把控制权交回 JS，页面才能在 `await init()` 之后把资源写完。
    next_frame().await;
    // 和桌面版一样把资源根设成 assets/。
    init_assets();
    set_status("engine started");

    let mut fs = fs_from_assets(String::new()).context("create assets fs")?;
    let mut info = dpsfs::load_info(fs.as_mut()).await.context("load info.txt")?;
    // 不给 tip 的话 LoadingScene 会去拉 tips.txt，这里直接给一句。
    info.tip = Some(String::from("DPS wasm player"));
    // 老格式的 info.txt 把预览区间写成 previewS / previewE，serde 只认 previewStart / previewEnd，
    // 读不到就自己扫一遍原文补上。
    if info.preview_start <= 0.0 {
        if let Ok(bytes) = fs.load_file("info.txt").await {
            let text = String::from_utf8_lossy(&bytes);
            for line in text.lines() {
                let Some((key, value)) = line.split_once(':') else {
                    continue;
                };
                let Ok(value) = value.trim().parse::<f64>() else {
                    continue;
                };
                match key.trim() {
                    "previewS" | "previewStart" => info.preview_start = value,
                    "previewE" | "previewEnd" => info.preview_end = Some(value),
                    _ => {}
                }
            }
        }
    }
    set_status(format!("chart {} / {}", info.name, info.chart));

    let config = prepare_config(
        PAGE_CONFIG
            .with(|it| it.borrow().clone())
            .unwrap_or_else(dpr_preview_config),
    );
    set_status(format!(
        "preview {} -> {:?}",
        config.play_start_time, config.play_end_time
    ));

    // 热重载的原料：下面 `GameScene::new` 会拿走 `fs` / `info` / `config` 本体
    RELOAD_SOURCE.with(|it| {
        *it.borrow_mut() = Some(ReloadSource {
            fs: fs.clone_box(),
            info: info.clone(),
            config: config.clone(),
        });
    });

    // 编辑器预览不要引擎自带的开场加载页：直接建 `GameScene`，资源加载期间画面是黑的，
    // 加载完立刻进谱面。结算页由下面的 `EditorScene` 拦掉。
    let mut background_fs = fs.clone_box();
    let (illustration, background) = match LoadingScene::load_background(&mut background_fs, &config, &info.illustration).await {
        Ok((illustration, background)) => (illustration.into(), background.into()),
        Err(err) => {
            set_status(format!("background unavailable: {err:?}"));
            (dps::ext::BLACK_TEXTURE.clone(), dps::ext::BLACK_TEXTURE.clone())
        }
    };
    let game = GameScene::new(
        None,
        GameMode::Exercise,
        info,
        config.clone(),
        fs,
        None,
        background,
        illustration,
        None,
        None,
    )
    .await
    .context("create GameScene")?;
    set_status("game scene created");

    // 时间源换成 `manual`：`TimeManager::from_config` 读的是 `performance.now()`，页面既停不住
    // 它、也改不了推进速率；`manual` 接的是文件末尾那个页面可控的虚拟时钟（`play_clock_now`）。
    start_play_clock();
    let mut main = Main::new(
        Box::new(EditorScene { inner: game }),
        TimeManager::manual(Box::new(play_clock_now)),
        None,
    )
    .await
    .context("create Main")?;
    set_status("main ready");

    let font = FontArc::try_from_vec(load_file("font.ttf").await.context("load font.ttf")?)
        .context("parse font.ttf")?;
    let mut painter = TextPainter::new(font);
    set_status("font ready");

    let mut frames = 0u32;
    let mut frozen_frames = 0u32;
    let mut last_time = f64::NEG_INFINITY;
    loop {
        // 页面提的 seek / 暂停 / 倍速在这一帧的一开头落地，`update` 里就按新状态走。
        apply_requests(&mut main).await;
        main.update().context("update")?;
        main.render(&mut painter).context("render")?;
        frames += 1;
        // 调试：时间不走时区分是「暂停」还是「帧停了」。浏览器里没有外部事件来 resume，
        // 这里主动调一次 `foucus_resume`（桌面版由窗口焦点事件触发）把场景从暂停里拉出来。
        let now = main.time();
        CLOCK.with(|it| it.set(now));
        let clock_running = play_clock_running();
        PAUSED.with(|it| it.set(main.paused() || !clock_running));
        if (now - last_time).abs() < 1e-4 {
            frozen_frames += 1;
            // 页面暂停时时间当然不走，别把「暂停」当成「卡死」去发 resume。
            if frozen_frames == 60 && clock_running {
                set_status(format!(
                    "time frozen at {now:.3} (wall {:.3}), sending focus resume",
                    macroquad::time::get_time()
                ));
                main.foucus_resume().context("focus resume")?;
            }
        } else {
            frozen_frames = 0;
        }
        last_time = now;
        if frames % 60 == 0 {
            set_status(format!("frame {frames} t={:.2}", now));
        }
        if main.should_exit() {
            set_status("scene exited");
            break;
        }
        next_frame().await;
    }
    Ok(())
}

// ─── 页面控制接口 ───
//
// 导出函数在 JS 的调用栈里执行，那一刻拿不到引擎状态（`Main` 只在主循环手里），所以这些
// 导出只把请求塞进 `REQUESTS` 队列，主循环每帧开头用 `apply_requests` 取出来执行。
//
// 暂停与倍速为什么要自带一个时钟、而不是直接改 `TimeManager`：`Main` 里的 `TimeManager` 是
// 私有字段（`scene.rs`），改不到 `speed`；`Main` 透出来的 `foucus_pause` / `foucus_resume`
// 在游戏处于 `Playing` 时又都不会动 `tm` 的暂停（`scene/game.rs`），拿来做页面暂停没反应。
// 于是时间源换成 `TimeManager::manual` 接下面这个页面可控的虚拟时钟：暂停就是停住它，倍速
// 就是改它的推进速率；`seek` 仍走 `Main::seek_time`（= `tm.seek_to`，往回跳时
// `scene/game.rs` 会自己 `rewind!`）。速率 1、不暂停时它与 `performance.now()` 同步推进，
// 和换成 `manual` 之前的行为一致。

/// 页面提的播放控制请求。
enum Request {
    /// 绝对谱面时间（秒），与 `info.txt` 里 previewS / previewE 同一套坐标系。
    Seek(f64),
    TogglePause,
    SetRate(f64),
    /// 音乐 / 音效音量，两者都是 0~1。
    SetVolume(f32, f32),
    /// 换一份渲染配置（DPDebugger 调参数用）：能直接落到已经在跑的场景上，不必重建。
    SetConfig(Config),
    /// 重新解析内存文件表里的谱面，原地换掉当前那张（编辑器改完参数调它）。
    Reload,
}

/// 热重载要用的原料：`GameScene::new` 会把文件系统、谱面信息与配置吃掉，这里另存一份。
///
/// 页面调 `wm_reload()` 之前已经把新谱面写进 `__dprw_files`，解析时读到的就是新字节。
struct ReloadSource {
    fs: Box<dyn dpsfs::FileSystem>,
    info: dps::info::ChartInfo,
    config: Config,
}

thread_local! {
    /// `Some` 表示可以热重载；在建 `GameScene` 之前填好，之后每次重载复制一份用。
    static RELOAD_SOURCE: RefCell<Option<ReloadSource>> = const { RefCell::new(None) };
}

thread_local! {
    static REQUESTS: RefCell<Vec<Request>> = const { RefCell::new(Vec::new()) };
    static PLAY_CLOCK: RefCell<PlayClock> = const { RefCell::new(PlayClock::STOPPED) };
}

/// 页面可控的播放时钟：`base` 是基准谱面时间，`wall_base` 是取这个基准时的墙钟秒数。
struct PlayClock {
    base: f64,
    wall_base: f64,
    rate: f64,
    running: bool,
}

impl PlayClock {
    const STOPPED: Self = Self {
        base: 0.0,
        wall_base: 0.0,
        rate: 1.0,
        running: false,
    };

    fn now(&self) -> f64 {
        if self.running {
            self.base + (wall_now() - self.wall_base).max(0.0) * self.rate
        } else {
            self.base
        }
    }

    /// 把「现在走到哪儿」落成新基准：改速率、暂停、继续之前必须先调，
    /// 否则已经走过的那段时间会按新速率重算一遍。
    fn commit(&mut self) {
        self.base = self.now();
        self.wall_base = wall_now();
    }

    /// 跳到 `time` 秒并接着走（暂停中也只是换个停住的位置）。
    fn jump(&mut self, time: f64) {
        self.base = time.max(0.0);
        self.wall_base = wall_now();
    }
}

/// 以秒为单位的墙钟。wasm 上与 `TimeManager::new` 取的是同一个 `performance.now()`。
#[cfg(target_arch = "wasm32")]
fn wall_now() -> f64 {
    web_sys::window()
        .and_then(|win| win.performance())
        .map(|perf| perf.now() / 1000.0)
        .unwrap_or_default()
}

/// 这个 bin 只编 wasm，桌面分支只是为了别的目标上也能过编译。
#[cfg(not(target_arch = "wasm32"))]
fn wall_now() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// `Main` 建起来之前先把表走起来，基准取当前墙钟：和 `TimeManager::from_config` 一样从 0 起算。
fn start_play_clock() {
    let now = wall_now();
    PLAY_CLOCK.with(|it| {
        *it.borrow_mut() = PlayClock {
            base: now,
            wall_base: now,
            rate: 1.0,
            running: true,
        };
    });
}

/// 交给 `TimeManager::manual` 的时间源。
fn play_clock_now() -> f64 {
    PLAY_CLOCK.with(|it| it.borrow().now())
}

fn play_clock_running() -> bool {
    PLAY_CLOCK.with(|it| it.borrow().running)
}

/// 借出场景栈顶的那个 `GameScene`（`EditorScene::as_any_mut` 直接把内层交出来）。
fn with_game_scene(main: &mut Main, f: impl FnOnce(&mut GameScene, &mut TimeManager)) {
    let (scenes, tm) = main.scenes_and_time();
    let Some(scene) = scenes.last_mut() else {
        return;
    };
    let Some(any) = scene.as_any_mut() else {
        return;
    };
    if let Some(game) = any.downcast_mut::<GameScene>() {
        f(game, tm);
    }
}

/// 把播放头放到 `time` 秒（`playing` 决定之后走不走）。
///
/// 走 `GameScene::scrub`：它把时钟、音乐、判定一起对齐，并给「往回跳」补上静音窗口
/// （`pause_rewind`，0.3 秒内不播打击音、不出打击粒子）。自己拼 `seek_time` + 音乐
/// `seek_to` 会漏掉这层保护，拖进度条就会炸音、按键粒子在屏幕上乱飞。
fn scrub_to(main: &mut Main, time: f64, playing: bool) {
    with_game_scene(main, |game, tm| {
        if let Err(err) = game.scrub(tm, time, playing) {
            set_status(format!("scrub failed: {err:?}"));
        }
    });
}

/// 主循环每帧开头取一次页面请求；队列空就什么都不做。
///
/// 拖进度条时页面一帧内可能排进几十个 `Seek`：在这里合并成「最后一个位置」，
/// 否则每一帧都要重新定位音乐与判定，声音会一片爆音、画面一直闪。
async fn apply_requests(main: &mut Main) {
    let requests = REQUESTS.with(|it| std::mem::take(&mut *it.borrow_mut()));
    let mut last_seek = None;
    for request in requests {
        match request {
            Request::Seek(t) => last_seek = Some(t),
            Request::TogglePause => {
                let running = PLAY_CLOCK.with(|it| {
                    let mut clock = it.borrow_mut();
                    clock.commit();
                    clock.running = !clock.running;
                    clock.running
                });
                let time = play_clock_now();
                // BGM 不会自己停：`MusicRenderer` 只要混音器还在被拉就继续出声。
                // 暂停时只掐声音、不暂停 `tm`，否则预览区里会浮出引擎自带的暂停菜单。
                if running {
                    // 继续：`scrub` 顺带把时钟、音乐、判定一起对齐（暂停期间墙钟还在走）。
                    scrub_to(main, time, true);
                    if let Err(err) = main.resume() {
                        set_status(format!("resume failed: {err:?}"));
                    }
                } else {
                    with_game_scene(main, |game, _| {
                        if let Err(err) = game.set_music_playing(time, false) {
                            set_status(format!("pause music failed: {err:?}"));
                        }
                    });
                }
            },
            Request::SetRate(rate) => PLAY_CLOCK.with(|it| {
                let mut clock = it.borrow_mut();
                clock.commit();
                clock.rate = rate;
            }),
            // 音量不碰时钟与位置，直接落到场景上。
            Request::SetVolume(music, sfx) => with_game_scene(main, |game, _| game.set_volume(music, sfx)),
            // 演出参数都是每帧现读 `res.config` 的，换掉它就立刻生效：调试时改一个数字
            // 下一帧就能看到，不必重建场景（重建会重载纹理、把播放头推回原处）。
            Request::SetConfig(config) => with_game_scene(main, |game, _| game.res.config = config),
            // 重载要重新解析谱面（异步读文件 + 建纹理），所以放在这个 async 函数里做。
            Request::Reload => reload_now(main).await,
        }
    }
    let Some(t) = last_seek else {
        return;
    };
    // 播放头先落到 `t`，再让场景按同一个时间对齐时钟与声音。
    PLAY_CLOCK.with(|it| it.borrow_mut().jump(t));
    let time = play_clock_now();
    if play_clock_running() {
        // 走着的时候跳：`scrub` 会补一层「回退静音窗口」，拖进度条不炸音、粒子不乱飞。
        scrub_to(main, time, true);
    } else {
        // 停着的时候跳：只挪播放头与音乐位置，不能让 `tm` 进入暂停（会画出引擎的暂停菜单）。
        // `prepare_at` 负责把结束态拉回游玩态，否则拖回中间之后画面停在结束帧。
        main.seek_time(time);
        with_game_scene(main, |game, tm| {
            game.prepare_at(tm);
            if let Err(err) = game.set_music_playing(time, false) {
                set_status(format!("seek music failed: {err:?}"));
            }
        });
    }
}

/// 重新解析内存文件表里的谱面，原地换掉当前场景里的那一张。
///
/// 走的是首次加载同一条 `GameScene::load_chart`：噪域、贴图、按键音效都会重新建；
/// `reload_chart` 自己会把谱面与判定退回当前播放头，所以画面不会跳回曲首。
async fn reload_now(main: &mut Main) {
    // 复制一份原料再来回 await：RefCell 的借用不能跨 await 一直挂着
    let source = RELOAD_SOURCE.with(|it| {
        it.borrow().as_ref().map(|it| ReloadSource {
            fs: it.fs.clone_box(),
            info: it.info.clone(),
            config: it.config.clone(),
        })
    });
    let Some(mut source) = source else {
        set_status("reload unavailable");
        return;
    };
    // 页面可能在上次建场景之后又改了参数：重载要以最新那份为准。
    if let Some(config) = PAGE_CONFIG.with(|it| it.borrow().clone()) {
        source.config = prepare_config(config);
    }
    match GameScene::load_chart(source.fs.as_mut(), &source.info, &source.config).await {
        Ok((chart, _format)) => with_game_scene(main, |game, _| match game.reload_chart(chart) {
            Ok(()) => set_status("chart reloaded"),
            Err(err) => set_status(format!("reload failed: {err:?}")),
        }),
        Err(err) => set_status(format!("reload parse failed: {err:?}")),
    }
}

/// 用页面给的整份渲染配置（JSON，`dps::config::Config` 的 camelCase 形式）重建场景。
///
/// 要在 `init()` 之后、引擎恢复执行之前调用才赶得上首次建场景；之后调就要配合
/// `wm_reload()` 才会生效（重载会重新读一次这份配置）。
///
/// 解析失败只记状态、不改已有配置：页面那边漏一个键就整份作废，调试窗口里看起来
/// 像「参数没反应」，比留着上一份还能看要好查。
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn wm_set_config(json: &str) {
    match serde_json::from_str::<Config>(json) {
        Ok(config) => {
            let config = prepare_config(config);
            PAGE_CONFIG.with(|it| *it.borrow_mut() = Some(config.clone()));
            REQUESTS.with(|it| it.borrow_mut().push(Request::SetConfig(config)));
        },
        Err(err) => set_status(format!("config parse failed: {err}")),
    }
}

/// 重新读一遍内存文件表里的谱面，下一帧原地换掉（编辑器的「刷新画面」走它）。
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn wm_reload() {
    REQUESTS.with(|it| it.borrow_mut().push(Request::Reload));
}

/// 跳到谱面时间 `t` 秒（绝对时间，不是预览区间内的偏移），下一帧生效。
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn wm_seek(t: f64) {
    if t.is_finite() {
        REQUESTS.with(|it| it.borrow_mut().push(Request::Seek(t.max(0.0))));
    }
}

/// 暂停 / 继续，下一帧生效；暂停态由 `wm_paused()` 回报。
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn wm_toggle_pause() {
    REQUESTS.with(|it| it.borrow_mut().push(Request::TogglePause));
}

/// 设置倍速，夹在 0.1 ~ 5 之间（NaN / 无穷按 1.0），下一帧生效。
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn wm_set_rate(r: f64) {
    let rate = if r.is_finite() { r.clamp(0.1, 5.0) } else { 1.0 };
    REQUESTS.with(|it| it.borrow_mut().push(Request::SetRate(rate)));
}

/// 设置音乐 / 音效音量，两者都夹在 0~1 之间（NaN / 无穷按 1.0），下一帧生效。
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn wm_set_volume(music: f32, sfx: f32) {
    let clamp = |v: f32| if v.is_finite() { v.clamp(0., 1.) } else { 1. };
    REQUESTS.with(|it| it.borrow_mut().push(Request::SetVolume(clamp(music), clamp(sfx))));
}

/// 从页面地址里读 `?start=<秒>`，没给或解析不出来就返回 `None`。
#[cfg(target_arch = "wasm32")]
fn url_start_seconds() -> Option<f64> {
    let window = web_sys::window()?;
    let search = window.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("start")?.trim().parse::<f64>().ok()
}
