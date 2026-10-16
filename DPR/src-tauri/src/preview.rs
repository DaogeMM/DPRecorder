use crate::{
    render::{build_player, generate_resource},
    ASSET_PATH,
};
use anyhow::Result;
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
    let (fs, _, config, info) = generate_resource(cmd, false).await?;

    set_pc_assets_folder(ASSET_PATH.get().unwrap().to_str().unwrap());
    let mut prpr_config: Config = config.to_config();
    // 普通UI 默认用 assets/respack/new（旧版本UI 的 assets/respack/old 分支已移除）
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
    // 是否全屏由「设置 → 预览全屏」决定（2026-10-09 曾被写死成 true，2026-10-12 起搬回设置里）。
    // 读不到配置时按 true 兜底，也就是和以前强制全屏的行为一致。
    let preview_fullscreen = crate::common::read_config()
        .map(|config| config.preview_fullscreen)
        .unwrap_or(true);
    macroquad::window::set_fullscreen(preview_fullscreen);

    let font = FontArc::try_from_vec(load_file("font.ttf").await?)?;
    let mut painter = TextPainter::new(font);

    let player = build_player(&config).await?;

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
                    Some(player),
                    None,
                    None,
                )
                .await?,
            ))),
            false,
            Rc::clone(&offset),
        )),
        ctm,
        None,
    )
    .await?;

    let mut frame_times: VecDeque<(f64, u32)> = VecDeque::new(); // (time, fps)
    let mut fps_last_update_sec: u32 = 0;

    'app: loop {
        let frame_start = tm.real_time();

        main.update()?;
        main.render(&mut painter)?;
        if main.should_exit() {
            break 'app;
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
