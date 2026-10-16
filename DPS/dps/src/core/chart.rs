crate::tl_file!("parser");

use super::{BlockArea, BlockAreaRenderer, BlockGroup, BpmList, Effect, JudgeLine, JudgeLineKind, Matrix, Resource, UIElement, Vector};
use crate::{
    core::Object,
    fs::FileSystem,
    judge::JudgeStatus,
    ui::{TextPainter, Ui},
};
use anyhow::{Context, Result};
use macroquad::prelude::*;
use nalgebra::Rotation2;
use rustc_hash::FxHashMap;
use sasa::AudioClip;
use std::cell::RefCell;

#[derive(Default)]
pub struct ChartExtra {
    pub effects: Vec<Effect>,
    pub global_effects: Vec<Effect>,
    #[cfg(feature = "video")]
    pub videos: Vec<(super::Video, Option<super::VideoAttach>)>,
}

#[derive(Default)]
pub struct ChartSettings {
    pub pe_alpha_extension: bool,
    pub line_reference_y_axis: bool,
}

pub type HitSoundMap = FxHashMap<String, AudioClip>;
const PROGRESS_BAR_COLOR: Color = Color::new(0.565, 0.565, 0.565, 1.0);

pub struct Chart {
    pub offset: f64,
    pub lines: Vec<JudgeLine>,
    pub bpm_list: RefCell<BpmList>,
    pub settings: ChartSettings,
    pub extra: ChartExtra,
    pub hitsounds: HitSoundMap,
    pub fonts: Vec<RefCell<TextPainter>>,
    /// Phigros 4.0 剧情遮挡区域（`blockAreaList`），时间单位是**秒**
    pub block_areas: Vec<BlockArea>,
    /// 「噪域复用」的归组结果：每个「噪域」以及它各次出现的窗口（`BlockGroup`）。
    /// 解析时无条件算好；用不用它编号由 `config.block_area_reuse` 决定。
    pub block_groups: Vec<BlockGroup>,
    block_area_renderer: RefCell<BlockAreaRenderer>,

    order: Vec<usize>,
    attach_ui: [Option<usize>; 7],
    trs: Vec<Matrix>,
}

impl Chart {
    pub fn new(
        offset: f64,
        lines: Vec<JudgeLine>,
        bpm_list: BpmList,
        settings: ChartSettings,
        extra: ChartExtra,
        hitsounds: HitSoundMap,
        fonts: Vec<RefCell<TextPainter>>,
    ) -> Self {
        let mut attach_ui = [None; 7];
        let mut order = (0..lines.len())
            .filter(|it| {
                if let Some(element) = lines[*it].attach_ui {
                    attach_ui[element as usize - 1] = Some(*it);
                    false
                } else {
                    true
                }
            })
            .collect::<Vec<_>>();
        order.sort_unstable_by_key(|it| (lines[*it].z_index, *it));
        let trs = Vec::with_capacity(lines.len());
        Self {
            offset,
            lines,
            bpm_list: RefCell::new(bpm_list),
            settings,
            extra,

            order,
            attach_ui,
            hitsounds,
            fonts,
            block_areas: Vec::new(),
            block_groups: Vec::new(),
            block_area_renderer: RefCell::new(BlockAreaRenderer::default()),
            trs,
        }
    }

    /// 设置剧情遮挡区域的外观参数。
    pub fn with_block_area_style(self, f: impl FnOnce(&mut super::BlockAreaStyle)) -> Self {
        f(&mut self.block_area_renderer.borrow_mut().style);
        self
    }

    #[inline]
    pub fn with_element<R>(
        &self,
        ui: &mut Ui,
        res: &Resource,
        element: UIElement,
        scale_point: Option<(f32, f32)>,
        rotation_point: Option<(f32, f32)>,
        f: impl FnOnce(&mut Ui, Color) -> R,
    ) -> R {
        let default_color = if matches!(element, UIElement::Bar) { PROGRESS_BAR_COLOR } else { WHITE };
        if let Some(id) = self.attach_ui[element as usize - 1] {
            let lines = &self.lines;
            let line = &lines[id];
            let object = &line.object;
            let translation = {
                let mut tr = line.fetch_pos(res, lines);
                tr.y *= -res.aspect_ratio;
                tr.x *= res.aspect_ratio;
                let sc = object.now_scale_wrt_point(scale_point.map_or_else(|| Vector::default(), |(x, y)| Vector::new(x, y)));
                let ro = Object::new_rotation_wrt_point(
                    Rotation2::new(-line.fetch_rot(lines).to_radians()),
                    rotation_point.map_or_else(|| Vector::default(), |(x, y)| Vector::new(x, y)),
                );
                Matrix::new_translation(&tr) * ro * sc
            };
            let mut color = line.color.now_opt().unwrap_or(default_color);
            color.a *= object.now_alpha().max(0.);
            ui.with(translation, |ui| f(ui, color))
        } else {
            f(ui, default_color)
        }
    }

    pub async fn load_textures(&mut self, fs: &mut dyn FileSystem) -> Result<()> {
        for line in &mut self.lines {
            if let JudgeLineKind::Texture(tex, path) = &mut line.kind {
                *tex = image::load_from_memory(&fs.load_file(path).await.with_context(|| format!("failed to load illustration {path}"))?)?.into();
            }
        }
        Ok(())
    }

    pub fn reset(&mut self) {
        self.lines.iter_mut().flat_map(|it| it.notes.iter_mut()).for_each(|note| {
            note.judge = JudgeStatus::NotJudged;
            note.protected = false;
            note.object.set_time(0.0);
        });
        for line in &mut self.lines {
            line.cache.reset(&mut line.notes);
        }
        super::block_area::reset_native_block_effects();
    }

    pub fn update(&mut self, res: &mut Resource) {
        for line in &mut self.lines {
            line.object.set_time(res.time);
        }
        self.trs.clear();
        self.trs.extend(self.lines.iter().map(|it| it.now_transform(res, &self.lines)));
        let mut guard = self.bpm_list.borrow_mut();
        for (index, (line, tr)) in self.lines.iter_mut().zip(self.trs.drain(..)).enumerate() {
            line.update(res, tr, &mut guard, index);
        }
        drop(guard);
        for effect in &mut self.extra.effects {
            effect.update(res);
        }
        #[cfg(feature = "video")]
        for (video, _) in &mut self.extra.videos {
            if let Err(err) = video.update(res.time) {
                tracing::warn!("video error: {err:?}");
            }
        }
    }

    fn hex_to_color(hex: &str, fallback: Color) -> Color {
        let hex = hex.strip_prefix('#').unwrap_or(hex);
        if hex.len() != 6 {
            return fallback;
        }
        match u8::from_str_radix(&hex[0..2], 16) {
            Ok(r) => {
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(fallback.g as u8 * 255);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(fallback.b as u8 * 255);
                Color::new(r as f32 / 255., g as f32 / 255., b as f32 / 255., 1.0)
            }
            Err(_) => fallback,
        }
    }

    pub fn render(&self, ui: &mut Ui, res: &mut Resource) {
        // 「待判按键」：每帧重新收敛一次「判定时刻最靠前的那个还没判定的按键」
        //（`Note::render` 顺手往小里更新），染色那一趟靠它认出要染色的按键。
        res.pending_note_time.set(f64::INFINITY);
        res.apply_model_of(&Matrix::identity().append_nonuniform_scaling(&Vector::new(if res.config.flip_x() { -1. } else { 1. }, -1.)), |res| {
            #[cfg(feature = "video")]
            for (video, attach) in &self.extra.videos {
                if let Some(attach) = attach {
                    let line = &self.lines[attach.line];
                    let hex_str = &res.config.linecolor;
                    let global_default_color = Self::hex_to_color(hex_str, res.judge_line_color);
                    let color = line.color.now_opt().unwrap_or(global_default_color);
                    let line_scale = line.object.scale.now_with_def(1.0, 1.0);
                    let mat = Rotation2::new(self.lines[attach.line].fetch_rot(&self.lines).to_radians() * attach.rotation_factor)
                        .to_homogeneous()
                        .append_translation(
                            &self.lines[attach.line]
                                .fetch_pos(res, &self.lines)
                                .component_mul(&Vector::new(attach.position_x_factor, attach.position_y_factor)),
                        );
                    res.apply_model_of(&mat.append_nonuniform_scaling(&Vector::new(1., -1.)), |res| {
                        video.render(res.time, res.aspect_ratio, color, Some((line_scale.x, line_scale.y, attach.scale_x_mode, attach.scale_y_mode)));
                    });
                } else {
                    res.apply_model_of(&Matrix::identity().append_nonuniform_scaling(&Vector::new(1., -1.)), |res| {
                        video.render(res.time, res.aspect_ratio, WHITE, None);
                    });
                }
            }

            let mut guard = self.bpm_list.borrow_mut();
            for id in &self.order {
                self.lines[*id].render(ui, res, &self.lines, &mut guard, &self.settings, *id);
            }
            drop(guard);

            res.note_buffer.borrow_mut().draw_all();
            if res.config.aggressive_note {
                res.note_pos_map.clear();
            }

            if res.config.sample_count > 1 {
                unsafe { get_internal_gl() }.flush();
                if let Some(target) = &res.chart_target {
                    target.blit();
                }
            }
        });
    }

    /// 「待判按键」染色：把判定时刻最靠前的那个按键**用他自己的绘制代码**再画一遍，只换颜色。
    ///
    /// 为什么照抄 `Chart::render` 这一小段、而不是自己拿矩阵重画一份贴图：按键的绘制里还有
    /// 长条本体 / 头尾、双押提示贴图、淡出、屏幕进水、旋转与移动时间等一大堆规则，自己复刻
    /// 必然对不上。这里只是把 `pending_tint` 打开，
    /// `Note::render` 就会跳过别的按键与判定范围 / 强制定位等副作用、`JudgeLine::render` 会跳过
    /// 判定线本体，于是同一段代码把目标按键按新颜色原样画一遍。
    ///
    /// 调用点在 `Game::render` 的噪域那一趟**之后**（相机与谱面那一趟完全一致），所以染色压在
    /// 噪域之上。注意这里**不能**再 `chart_target.blit()`：那次 blit 会把多重采样 FBO 解析回
    /// `output()`，正好把刚染好的按键盖掉。
    pub fn render_pending_tint(&self, ui: &mut Ui, res: &mut Resource) {
        res.pending_tint.set(true);
        res.apply_model_of(&Matrix::identity().append_nonuniform_scaling(&Vector::new(if res.config.flip_x() { -1. } else { 1. }, -1.)), |res| {
            let mut guard = self.bpm_list.borrow_mut();
            for id in &self.order {
                self.lines[*id].render(ui, res, &self.lines, &mut guard, &self.settings, *id);
            }
            drop(guard);

            res.note_buffer.borrow_mut().draw_all();
            if res.config.aggressive_note {
                res.note_pos_map.clear();
            }
        });
        res.pending_tint.set(false);
    }

    /// 方块（Phigros 4.0 剧情遮挡区域）图层。
    ///
    /// **调用点在 `Game::render` 的 UI 之后**，所以它压在所有东西上面：判定线、音符、
    /// 分数 / 连击 / 暂停键（`renderUiPause` 等）。原版 `index/` 里方块属于背景层
    /// （`render_scene(bg)` 里画、按键永远在上面），本仓库把它放在最高层。
    ///
    /// 注意必须在 MSAA `blit()` **之后**调用：blit 会把多重采样 FBO 解析成
    /// `chart_target.output()`，之后屏幕上显示的就是那张已经解析好的纹理，所以这里直接
    /// 写 `output()`，写进多重采样输入 FBO 反而会被丢掉。
    pub fn render_block_area(&self, res: &mut Resource) {
        // 判定范围（`Chart::render` 里画的，在本函数之前）读的是**上一帧**烘出来的噪域掩码
        // 画布 + 判定范围材质；这里先清掉，等这一帧的管线跑完（`native/shader.rs` 里
        // `frame.uploaded_masks` 落笔那处）再重新填上。
        // 噪域整个关掉时它就一直保持 None，判定范围恢复原样。
        res.block_area_paint = None;
        res.judge_range_mat = None;
        // 「强制定位」：**不理谱面自带的 `blockAreaList`**（谱面一条噪域都没有也照样生效），
        // 改铺一块合成的全屏噪域，再由每个按键周围的长方形把它挖出洞来。
        //
        // 「反向强制定位」（`block_area_force_invert`）正好反过来：不铺全屏噪域，只把那圈
        // 矩形**本身**当噪域画出来 ⇒ 这里传**空**方块列表，矩形由 `render_all` 直接从
        // `Resource::force_position_quads` 推成可绘制的 zone（不是合成块）。
        let force = res.config.block_area_force_position;
        let force_invert = force && res.config.block_area_force_invert;
        // 「简化噪域」单独勾选也要能出画面，所以两个开关任一打开就渲染
        if (!res.config.render_block_area && !res.config.simplified_block_area) || (!force && self.block_areas.is_empty()) {
            return;
        }
        let time = res.time;
        let forced = [BlockArea::force_position_fullscreen()];
        let blocks: &[BlockArea] = if force_invert {
            &[]
        } else if force {
            &forced
        } else {
            &self.block_areas
        };
        let flip_x = res.config.flip_x();
        // 噪域现在只有一条管线：Phira-Pro 的原生实现（掩码 + 原版 Unity 面元着色器）。
        // 「简化噪域」**不是另一条路**，而是让同一份官方材质退化成「纯色 × 不透明度」
        //（`block_area::native::shader::set_fill_style` 覆写一组 uniform），所以这里
        // 只是把开关往下传，界面开关与 API 的 `BlockAreaStyle::simplified` 任一为真即算简化。
        let simplified = res.config.simplified_block_area || self.block_area_renderer.borrow().style.simplified;
        // 「噪域复用」同理：只换「简化噪域文字」印的号（条目序号 → 噪域编号），不碰画面。
        let reuse = res.config.block_area_reuse || self.block_area_renderer.borrow().style.reuse;
        super::block_area::render_native_block_areas(res, blocks, time, flip_x, simplified, reuse);
    }
}
