use crate::{
    render::{build_player, generate_resource},
    ASSET_PATH,
};
use anyhow::{Context, Result};
use macroquad::prelude::*;
use dps::{
    config::{Config, Mods},
    scene::{show_error, GameMode, LoadingScene, NextScene, Scene, DIALOG},
    time::TimeManager,
    ui::{FontArc, TextPainter, Ui},
    Main,
};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

struct BaseScene(Option<NextScene>, bool, Rc<RefCell<Option<f32>>>);
impl Scene for BaseScene {
    fn on_result(&mut self, _tm: &mut TimeManager, result: Box<dyn std::any::Any>) -> Result<()> {
        match result.downcast::<Option<f32>>() {
            Ok(result_offset) => {
                if let Some(offset) = *result_offset {
                    *self.2.borrow_mut() = Some(offset);
                }
                Ok(())
            }
            Err(result_err) => match result_err.downcast::<anyhow::Error>() {
                Ok(error) => {
                    show_error(error.context("加载谱面失败"));
                    self.1 = true;
                    Ok(())
                }
                Err(_) => Ok(()),
            },
        }
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
    fn next_scene(&mut self, _tm: &mut TimeManager) -> dps::scene::NextScene {
        if self.1 {
            let has_dialog = DIALOG.with(|it| it.borrow().is_some());
            if !has_dialog {
                return NextScene::Exit;
            }
            return NextScene::None;
        }
        self.0.take().unwrap_or_default()
    }
}

pub async fn main(cmd: bool, tweak_offset: bool, autoplay: bool) -> Result<()> {
    let (fs, _, config, info) = generate_resource(cmd, false)
        .await
        .context("failed to prepare preview resources")?;

    // 分屏游玩时这个进程是主窗口还是副窗口；不是分屏就是 `None`。
    let split = crate::split::Role::from_env();
    let secondary = split.as_ref().is_some_and(|role| role.secondary);

    set_pc_assets_folder(ASSET_PATH.get().unwrap().to_str().unwrap());
    let mut prpr_config: Config = config.to_config();
    // 资源包：跟着 config 里选的那份走，没设置就用 assets/respack/new
    let respack_root = ASSET_PATH.get().unwrap().join("respack");
    prpr_config.res_pack_path = Some(
        config
            .res_pack_path
            .clone()
            .unwrap_or_else(|| respack_root.join("new").to_string_lossy().into_owned()),
    );
    if autoplay {
        prpr_config.mods |= Mods::AUTOPLAY;
    }
    prpr_config.volume_bgm = prpr_config.volume_music;
    if secondary {
        // 副窗口只负责画面：声音由主窗口放，两个都放会叠成重音。
        prpr_config.volume_music = 0.;
        prpr_config.volume_bgm = 0.;
    }
    // 谱面信息里的「预览开始时间 / 结束时间」原来只喂选歌列表那 15 秒试听，试玩一律从 0 开始；
    // 这里把它落到播放参数上：从开始时间起播，到结束时间收尾（结束时间为 0 就播到谱面结尾）。
    // 引擎侧只有 `play_start_time` / `play_end_time` 两个入口（`GameScene::new` 的试玩区间、
    // `Resource::new` 的 `track_length`），所以不需要改引擎。
    if info.preview_start > 0. || info.preview_end.unwrap_or_default() > 0. {
        prpr_config.play_start_time = info.preview_start.max(0.);
        prpr_config.play_end_time = info
            .preview_end
            .filter(|end| *end > info.preview_start && *end > 0.);
    }
    // 是否全屏由「设置 → 预览全屏」决定。
    // 读不到配置时按 true 兜底。
    let preview_fullscreen = crate::common::read_config()
        .map(|config| config.preview_fullscreen)
        .unwrap_or(true);
    match &split {
        // 分屏要两个窗口并排：位置和大小由 `crate::split::place_window` 指定，这里先退全屏。
        Some(role) => {
            macroquad::window::set_fullscreen(false);
            crate::split::place_window(role.secondary);
        }
        None => macroquad::window::set_fullscreen(preview_fullscreen),
    }

    let font = FontArc::try_from_vec(load_file("font.ttf").await?)?;
    let mut painter = TextPainter::new(font);

    let player = if secondary {
        None
    } else {
        Some(build_player(&config).await?)
    };

    // 主窗口每帧把播放时刻发给副窗口，副窗口每帧把时刻拨过去。
    let mut time_sender = split
        .as_ref()
        .filter(|role| !role.secondary)
        .and_then(|role| crate::split::TimeSender::new(role.port));
    let mut time_receiver = split
        .as_ref()
        .filter(|role| role.secondary)
        .and_then(|role| crate::split::TimeReceiver::new(role.port));

    let tm = TimeManager::default();
    let ctm = TimeManager::from_config(&prpr_config); // strange variable name...
    let offset = Rc::new(RefCell::new(None));
    let mut main = Main::new(
        Box::new(BaseScene(
            Some(NextScene::Overlay(Box::new(
                LoadingScene::new(
                    None,
                    if tweak_offset {
                        GameMode::TweakOffset
                    } else {
                        GameMode::Exercise
                    },
                    info,
                    &prpr_config,
                    fs,
                    player,
                    None,
                    None,
                )
                .await
                .context("failed to load the chart")?,
            ))),
            false,
            Rc::clone(&offset),
        )),
        ctm,
        None,
    )
    .await
    .context("failed to create the preview window")?;

    let mut frame_times: VecDeque<(f64, u32)> = VecDeque::new(); // (time, fps)
    let mut fps_last_update_sec: u32 = 0;

    'app: loop {
        let frame_start = tm.real_time();

        // 副窗口：把时刻对齐到主窗口，主窗口关掉就跟着退。
        if let Some(receiver) = time_receiver.as_mut() {
            if let Some(time) = receiver.latest() {
                main.seek_time(time);
            }
            if receiver.closed() {
                break 'app;
            }
        }

        main.update()?;
        main.render(&mut painter)?;
        if main.should_exit() {
            break 'app;
        }

        // 主窗口：把这一刻发出去，副窗口下一帧跟到这个时刻。
        if let Some(sender) = time_sender.as_mut() {
            sender.send(main.time());
        }

        let frame_end = tm.real_time();
        let frame_end_during = frame_end - frame_start;
        let now_fps = (1. / frame_end_during) as u32;
        frame_times.push_back((frame_end, now_fps));
        while frame_times.front().is_some_and(|it| frame_end - it.0 > 1.0) {
            frame_times.pop_front();
        }

        next_frame().await;
        let flash_end = tm.real_time();
        let flash_end_during = flash_end - frame_start;

        let fps_now_sec = frame_start as u32;
        if fps_last_update_sec != fps_now_sec {
            fps_last_update_sec = fps_now_sec;
            let real_avg_fps = frame_times.len() as u32;
            let real_now_fps = (1. / flash_end_during) as u32;
            let avg_fps = frame_times.iter().map(|(_, fps)| fps).sum::<u32>() / real_avg_fps;
            let min_fps = frame_times.iter().map(|(_, fps)| fps).min().unwrap_or(&0);
            eprintln!(
                "| AVG: {}|{} NOW: {}({:.4}ms)|{}({:.4}ms), MIN: {}",
                real_avg_fps,
                avg_fps,
                real_now_fps,
                flash_end_during * 1000.,
                now_fps,
                frame_end_during * 1000.,
                min_fps
            );
        }
    }

    if tweak_offset {
        if let Some(result_offset) = *offset.borrow() {
            let result_json = serde_json::to_string(&result_offset)?;
            println!("{}", result_json);
        }
    }

    Ok(())
}
