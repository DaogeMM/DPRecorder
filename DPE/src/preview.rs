//! 预览窗口本体：把引擎的场景栈跑起来，并把编辑器的命令接到场景上。
//!
//! 与 DPR 的 `src-tauri/src/preview.rs` 是同一套骨架（`BaseScene` + `LoadingScene` + `Main` 循环），
//! 区别只在于这里没有 Tauri：谱面从命令行来，播放控制从 stdin 来，热重载原地换谱面。

use std::any::Any;
use std::io::IsTerminal;

use anyhow::{Context, Result};
use dps::{
    core,
    ext,
    fs::{self},
    info::{ChartFormat, ChartInfo},
    scene::{show_error, GameMode, GameScene, LoadingScene, NextScene, Scene, DIALOG},
    time::TimeManager,
    ui::{FontArc, TextPainter, Ui},
    Main,
};
use macroquad::prelude::*;

use crate::{
    cmd::Args,
    protocol::{self, Command},
};

/// 编辑器视角的播放状态。
///
/// `time` / `playing` 是「想要的位置」，命令与按键先记在这里，等 `GameScene` 真在栈里了
/// 再交给 `GameScene::scrub` 去做 —— 加载期间只有时钟可拨，场景一出现就会把时间打回 0。
struct Playback {
    time: f64,
    playing: bool,
    /// 有待生效的 seek / play / pause
    pending: bool,
    /// 谱面场景是否已经进栈（进栈前收不到 seek）
    loaded: bool,
    /// 上一次热重载解析出的格式；首次加载在 `LoadingScene` 内部完成，这里拿不到
    format: Option<ChartFormat>,
}

pub fn window_conf(args: &Args) -> macroquad::window::Conf {
    let name = args
        .input
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    macroquad::window::Conf {
        window_title: format!("DPE 预览 - {name}"),
        window_width: 1280,
        window_height: 720,
        icon: Some(macroquad::miniquad::conf::Icon {
            medium: crate::icon::ICON,
            big: crate::icon::BIG_ICON,
            small: crate::icon::SMALL_ICON,
        }),
        ..Default::default()
    }
}

pub async fn run(args: Args) -> Result<()> {
    // 资源目录（assets/）跟着 exe 走：这一步会把 cwd 切到 exe 旁边并设好 pc assets 目录
    core::init_assets();
    if args.mute {
        ext::set_audio_silent(true);
    }
    if args.fullscreen {
        macroquad::window::set_fullscreen(true);
    }

    let config = args.config.clone();
    let mut files = fs::fs_from_file(&args.input).with_context(|| format!("无法打开谱面 {}", args.input.display()))?;
    let mut info = fs::load_info(&mut *files).await.context("读取谱面信息失败")?;
    // info 里没写谱面 / 音乐 / 曲绘时按目录内容猜，RPE 谱面还会让 META 覆盖曲名等字段
    fs::fix_info(&mut *files, &mut info).await.context("补全谱面信息失败")?;

    let queue = protocol::queue();
    // 编辑器是带着管道 stdio 起的，那种情况下 stdin 关闭就等于「父进程没了」，该跟着退出；
    // 从终端 / 资源管理器直接起时 stdin 是控制台（永远不 EOF），别起读线程。
    // `--frames` 是冒烟测试模式，同样不接协议。
    if args.frames == 0 && !args.standalone && !std::io::stdin().is_terminal() {
        protocol::spawn_stdin_reader(queue.clone());
    }
    protocol::emit_ready(
        &args.input.to_string_lossy(),
        &crate::temp::dir().to_string_lossy(),
    );

    let loading = LoadingScene::new(None, GameMode::Exercise, info.clone(), &config, files, None, None, None)
        .await
        .context("加载谱面失败")?;
    let mut main = Main::new(
        Box::new(BaseScene(Some(NextScene::Overlay(Box::new(loading))), false)),
        TimeManager::from_config(&config),
        None,
    )
    .await
    .context("创建预览窗口失败")?;
    let font = FontArc::try_from_vec(load_file("font.ttf").await?)?;
    let mut painter = TextPainter::new(font);

    let mut playback = Playback {
        time: config.play_start_time,
        playing: args.play,
        pending: true,
        loaded: false,
        format: None,
    };
    let mut frames = 0u64;
    loop {
        // 1. 外部命令
        let commands: Vec<Command> = {
            let mut queue = queue.lock().unwrap();
            queue.drain(..).collect()
        };
        let mut quit = false;
        for command in commands {
            match command {
                Command::Ping => {}
                Command::Seek { t, playing } => {
                    playback.time = t;
                    if let Some(playing) = playing {
                        playback.playing = playing;
                    }
                    playback.pending = true;
                }
                Command::Play => {
                    playback.playing = true;
                    playback.pending = true;
                }
                Command::Pause => {
                    playback.playing = false;
                    playback.pending = true;
                }
                Command::Reload => {
                    let reloaded = reload(&mut main, &args, &info, &config, &mut playback.format).await;
                    match reloaded {
                        Ok(()) => playback.pending = true,
                        Err(err) => {
                            // 改谱面写到一半时解析失败是常事，报一下继续跑，别把预览关掉
                            eprintln!("[dpe] 热重载失败：{err:?}");
                            protocol::emit(serde_json::json!({
                                "ev": "error",
                                "what": "reload",
                                "message": format!("{err:#}"),
                            }));
                        }
                    }
                }
                Command::Quit => quit = true,
            }
        }
        if quit {
            break;
        }

        // 2. 手动按键（`interactive = false`，GameScene 不抢键盘，这里自己响应）
        let now = main.time();
        if is_key_pressed(KeyCode::Escape) {
            break;
        }
        if is_key_pressed(KeyCode::Space) {
            playback.playing = !playback.playing;
            playback.time = now;
            playback.pending = true;
        }
        if is_key_pressed(KeyCode::Left) {
            playback.time = (now - 2.).max(0.);
            playback.pending = true;
        }
        if is_key_pressed(KeyCode::Right) {
            playback.time = now + 5.;
            playback.pending = true;
        }

        // 3. 把「想要的位置」交给谱面场景；还在加载就只记着，等它进栈
        {
            let (scenes, tm) = main.scenes_and_time();
            if let Some(game) = find_game(scenes) {
                playback.loaded = true;
                if playback.pending {
                    playback.pending = false;
                    let length = game.res.track_length;
                    playback.time = playback.time.clamp(0., if length > 0. { length } else { f64::MAX });
                    let time = playback.time;
                    game.scrub(tm, time, playback.playing)?;
                }
            }
        }

        main.update()?;
        main.render(&mut painter)?;
        if main.should_exit() {
            break;
        }

        // 4. 结算界面压在游戏层上面，编辑器里直接弹掉；播到底就回到开头接着放
        {
            let (scenes, tm) = main.scenes_and_time();
            if playback.loaded {
                while scenes.len() > 1 && scenes.last_mut().map_or(false, |scene| scene.as_any_mut().is_none()) {
                    scenes.pop();
                }
            }
            if let Some(game) = find_game(scenes) {
                let length = game.res.track_length;
                if length > 0. && playback.playing && tm.now() >= length {
                    playback.time = 0.;
                    game.scrub(tm, 0., true)?;
                }
            }
        }

        frames += 1;
        let (loaded, length) = match main.scenes_and_time() {
            (scenes, _) => match find_game(scenes) {
                Some(game) => (true, game.res.track_length),
                None => (false, 0.),
            },
        };
        protocol::emit_frame(main.time(), playback.playing, loaded, length);
        if args.frames > 0 && frames >= args.frames {
            break;
        }
        next_frame().await;
    }
    Ok(())
}

/// 热重载：重新打开谱面 → 重新解析 → 原地换掉当前谱面。
///
/// `Resource` 在构造时接手了第一份 `FileSystem`（读音乐、曲绘用），所以这里只能重新开一份；
/// 重新解析走 `GameScene::load_chart`（内部已经读了 `extra.json` 与贴图），与首次加载同一条路。
async fn reload(
    main: &mut Main,
    args: &Args,
    info: &ChartInfo,
    config: &dps::config::Config,
    format: &mut Option<ChartFormat>,
) -> Result<()> {
    let mut files = fs::fs_from_file(&args.input).with_context(|| format!("无法重新打开谱面 {}", args.input.display()))?;
    let (chart, new_format) = GameScene::load_chart(&mut *files, info, config).await.context("重新解析谱面失败")?;
    if let Some(old) = format {
        // 换了格式的话旋律线长度这类按格式算出来的东西要重启预览才会跟着变
        if std::mem::discriminant(old) != std::mem::discriminant(&new_format) {
            eprintln!("[dpe] 谱面格式变了，旋律线长度等设置要重启预览才会生效");
        }
    }
    *format = Some(new_format);
    let (scenes, _) = main.scenes_and_time();
    let game = find_game(scenes).context("谱面还在加载，暂时不能热重载")?;
    game.reload_chart(chart)
}

/// 在场景栈里找谱面场景（`Scene::as_any_mut` 只有 `GameScene` 会返回 `Some`）。
fn find_game(scenes: &mut [Box<dyn Scene>]) -> Option<&mut GameScene> {
    scenes.iter_mut().find_map(|scene| scene.as_any_mut()?.downcast_mut::<GameScene>())
}

/// 预览栈的底：只负责把加载场景摆上去，并在加载失败时弹错误框（与 DPR 预览一致）。
struct BaseScene(Option<NextScene>, bool);

impl Scene for BaseScene {
    fn on_result(&mut self, _tm: &mut TimeManager, result: Box<dyn Any>) -> Result<()> {
        match result.downcast::<anyhow::Error>() {
            Ok(error) => {
                show_error(*error);
                self.1 = true;
            }
            Err(_) => anyhow::bail!("预览场景收到了意外的返回值"),
        }
        Ok(())
    }

    fn enter(&mut self, _tm: &mut TimeManager, _target: Option<RenderTarget>) -> Result<()> {
        if self.0.is_none() && !self.1 {
            self.0 = Some(NextScene::Exit);
        }
        Ok(())
    }

    fn update(&mut self, _tm: &mut TimeManager) -> Result<()> {
        Ok(())
    }

    fn render(&mut self, _tm: &mut TimeManager, _ui: &mut Ui) -> Result<()> {
        Ok(())
    }

    fn next_scene(&mut self, _tm: &mut TimeManager) -> NextScene {
        if self.1 {
            // 错误框关掉之后再退出，别让窗口一闪就没了
            return if DIALOG.with(|it| it.borrow().is_some()) {
                NextScene::None
            } else {
                NextScene::Exit
            };
        }
        self.0.take().unwrap_or_default()
    }
}
