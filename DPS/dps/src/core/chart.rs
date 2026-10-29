crate::tl_file!("parser");

use super::{
    BlockArea, BlockAreaRenderer, BlockGroup, BpmList, Effect, JudgeLine, JudgeLineKind, Matrix, NextArea, Resource, UIElement, Vector,
};
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
    /// rpe-next v220 噪域（同一份 `blockAreaList` 里另一套字段的条目），时间单位是**全局 beat**，
    /// 解析时就靠谱面 BPM 表换算成秒了。
    pub next_block_areas: Vec<NextArea>,
    /// 这一帧被噪域吞掉的触摸（`(手指 id, chart 坐标)`）：处于 `Active` 相位噪域里的手指。
    ///
    /// 官方语义：这些触点**不参与判定**（下面的按键打不中），同时噪域上会以它们为圆心
    /// 亮一圈 hover 光晕，手指不离开就一直亮。判定每帧重算，所以松手即清空。
    pub blocked_touches: Vec<(u64, Vector)>,
    /// 「噪域复用」的归组结果：每个「噪域」以及它各次出现的窗口（`BlockGroup`）。
    /// 解析时无条件算好；用不用它编号由 `config.block_area_reuse` 决定。
    pub block_groups: Vec<BlockGroup>,
    block_area_renderer: RefCell<BlockAreaRenderer>,

    order: Vec<usize>,
    attach_ui: [Option<usize>; 7],
    trs: Vec<Matrix>,
    /// 「谱面修改」是否已经落到这份谱面上。
    ///
    /// 渲染器为了先拿到改过的按键（离线打击音轨按它铺），会在交给 `GameScene` 之前先改一遍；
    /// 之后 `GameScene` 按同一份配置再改一遍就成了改两次（复制音符会变成平方）。
    /// 所以改过一次就在这里记下，`apply_chart_mods` 自己挡掉第二次。
    ///
    /// 配置里的改谱开关在这一趟之后**仍然保持打开**：纯配置谱面 / 定轨谱面的画面（拍平、噪域、
    /// 判定范围）渲染时还要读它们，清掉就没画面了。
    chart_mods_applied: bool,
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
            next_block_areas: Vec::new(),
            blocked_touches: Vec::new(),
            block_groups: Vec::new(),
            block_area_renderer: RefCell::new(BlockAreaRenderer::default()),
            trs,
            chart_mods_applied: false,
        }
    }

    /// 「谱面修改」的统一入口（见 `core::chart_mod`）：纯配置谱面、定轨谱面、隐藏按键与按键那一层都在
    /// 这里落到谱面上。**四类互斥**，同一时刻只有一类生效（见 `chart_mod::Mods::from_config`）；
    /// 按键那一层的落地顺序就是用户勾选的先后（`mods.steps`）。
    ///
    /// 其中三步要改判定线布局，得在这里落地：「纯配置谱面」还会**新增**左右两条竖判定线，而
    /// `render` / `render_pending_tint` 只遍历 `order`，所以新判定线要按 `new` 里同一套规则补进去；
    /// 「陈列按键」会把整张谱面收到一条基准线上、其余判定线删掉，所以落地之后要按同一条规则重建
    /// 绘制顺序与 `attach_ui` 索引表 —— 界面判定线与贴图 / 文字 / 画笔判定线一并保留。其余各项只动
    /// 按键自己，交给 `chart_mod::apply_step`。
    ///
    /// 结尾统一收尾一次（重算双押提示、重建判定线缓存，见 `chart_mod::finish`）：缓存按 `above` /
    /// `speed` / 长按尾端分桶，这几样都可能被前面的功能改过。
    ///
    /// `aspect` 是屏幕纵横比（转换发生在 `GameScene` 里，`Resource` 已经建好，由调用方给）；
    /// `track_end` 是音乐结束的时刻（秒），长条延长把它前面的留白处当长按尾端。
    ///
    /// 同一份谱面只改一次：渲染器为了先拿到改过的按键（离线打击音轨按它铺）会提前改一遍，
    /// 之后 `GameScene` 再按同一份配置调一次就直接返回，不会改两遍。
    pub fn apply_chart_mods(&mut self, mods: &super::chart_mod::Mods, aspect: f32, track_end: f64) {
        if mods.is_empty() {
            return;
        }
        // 渲染器会先改一遍再交给 `GameScene`（见 `chart_mods_applied` 的说明），第二次直接跳过。
        if self.chart_mods_applied {
            return;
        }
        self.chart_mods_applied = true;
        let mut rng = super::chart_mod::rng();
        // 定轨谱面与纯配置谱面互斥，所以它出现时一定是单独一类，直接当第一步。
        if let Some(lanes) = &mods.lane {
            super::fixed_lane::apply(&mut self.lines, lanes, aspect);
        }
        for step in &mods.steps {
            match step {
                super::chart_mod::Step::Pure(pure) => {
                    let before = self.lines.len();
                    // 方块列表要借给转换（判按键在不在噪域里），转换又要可变借用 `lines`，所以先搬出来再放回。
                    let areas = std::mem::take(&mut self.block_areas);
                    let noise = pure.noise.then(|| super::pure_config::Noise { areas: &areas });
                    super::pure_config::apply(&mut self.lines, pure.line_y, pure.speed, pure.side, noise, aspect);
                    self.block_areas = areas;
                    if self.lines.len() > before {
                        self.order.extend(before..self.lines.len());
                        self.order.sort_unstable_by_key(|it| (self.lines[*it].z_index, *it));
                    }
                }
                super::chart_mod::Step::Arrange(arrange) => {
                    if let Some(base) = super::chart_mod::arrange_notes(&mut self.lines, arrange) {
                        self.keep_line(base);
                    }
                }
                step => {
                    let mut bpm_list = self.bpm_list.borrow_mut();
                    super::chart_mod::apply_step(&mut self.lines, &mut bpm_list, step, track_end, &mut rng);
                }
            }
        }
        if mods.hide {
            super::chart_mod::hide_notes(&mut self.lines);
        }
        super::chart_mod::finish(&mut self.lines);
    }

    /// 只留 `base` 这一条判定线（「陈列按键」之后）：其余可改判定线连同按键一起丢掉，界面判定线与
    /// 贴图 / 文字 / 画笔判定线保留，再按 `new` 里那套规则重建 `order` 与 `attach_ui`。
    fn keep_line(&mut self, base: usize) {
        let mut index = 0;
        self.lines.retain(|line| {
            let keep = index == base || line.attach_ui.is_some() || !matches!(line.kind, JudgeLineKind::Normal);
            index += 1;
            keep
        });

        let mut attach_ui = [None; 7];
        let mut order = Vec::with_capacity(self.lines.len());
        for (index, line) in self.lines.iter().enumerate() {
            match line.attach_ui {
                Some(element) => attach_ui[element as usize - 1] = Some(index),
                None => order.push(index),
            }
        }
        order.sort_unstable_by_key(|it| (self.lines[*it].z_index, *it));
        self.attach_ui = attach_ui;
        self.order = order;

        // 视频挂载记的是判定线下标（`VideoAttach::line`），删过判定线之后要收回到范围内
        #[cfg(feature = "video")]
        for (_, attach) in self.extra.videos.iter_mut() {
            if let Some(attach) = attach {
                attach.line = attach.line.min(self.lines.len().saturating_sub(1));
            }
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

    /// 播放头被往回拖到 `time` 之后调用：等价于「从 `time` 处重新开局」——`time` 之前的音符
    /// 保持已经判定的样子（不参与判定、也不再出现在屏幕上），之后的音符回到「还没到来」。
    ///
    /// ⚠️ 不能改用 `reset()`：它把每个音符都改成 `NotJudged`、动画时刻归零，判定器重扫时会
    /// 把回退点之前的音符**一口气全判掉**，音符也会以开局那一刻的样子堆在判定线上。
    pub fn rewind_to(&mut self, time: f64) {
        self.lines.iter_mut().flat_map(|it| it.notes.iter_mut()).for_each(|note| {
            if note.time >= time {
                note.judge = JudgeStatus::NotJudged;
            } else if matches!(note.judge, JudgeStatus::Hold(..)) {
                // 回退点落在一根长条中间：那根长条已经按不下去了，直接结束掉，
                // 免得它带着「正在按住」的状态挂在屏幕上。
                note.judge = JudgeStatus::Judged;
            }
            note.protected = false;
            note.object.set_time(time);
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

            // 「痉挛SP」：正中的插画垫在判定线与按键下面。
            crate::core::jingluan_sp::render(res);

            // 「もぺもぺSP」：整屏红色蒙板同样垫在判定线与按键下面（只压背景）。
            crate::core::mopemope_sp::render(res);

            // 「Clock Paradox」：正中的白线大表盘同样垫在判定线与按键下面。
            crate::core::clock_paradox_sp::render(res);

            let mut guard = self.bpm_list.borrow_mut();
            for id in &self.order {
                // 「痉挛SP」：谱面里摆在正中、没有音符、只用来被插画顶替的判定线不画
                //（不管谱面把它们的 `alpha` 写成多少）；按键所在那根线照常显示。
                if res.config.jingluan_sp_mode && self.lines[*id].notes.is_empty() {
                    continue;
                }
                self.lines[*id].render(ui, res, &self.lines, &mut guard, &self.settings, *id);
            }
            drop(guard);

            res.note_buffer.borrow_mut().draw_all();

            // 「落点模拟」：每根固定落点画成一个半透明的蓝色球，压在按键之上 —— 它模拟的是手指按在
            // 屏幕上的触点，半径就是「落点宽度」，所以球边到落点中心的距离就是断连判据。
            for &(x, y, half, _) in &res.sim_touch_points {
                // 一圈柔光晕 + 半透明球体 + 偏左上的高光，模拟按在屏幕上的触点（`draw_circle` 没有渐变）。
                draw_circle(x, y, half * 1.16, Color::new(0.3, 0.7, 1., 0.10));
                draw_circle(x, y, half, Color::new(0.3, 0.7, 1., 0.30));
                draw_circle(x, y, half * 0.74, Color::new(0.42, 0.78, 1., 0.14));
                draw_circle(x - half * 0.30, y - half * 0.30, half * 0.24, Color::new(0.88, 0.96, 1., 0.30));
            }
            if res.config.aggressive_note {
                res.note_pos_map.clear();
            }

            // 拖影不建 `chart_target`（见 `Resource::update_size`），这里也就没有
            // 多重采样需要解析。
            if res.config.sample_count > 1 && !res.keep_frame() {
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
        // 「定轨谱面」与「陈列按键」都不画噪域：谱面被拍平到轨道、或整张收成一条基准线之后，
        // 方块与「强制定位」的挖洞都失去几何意义，判定范围也一并让开（它要
        // `block_area_paint` / `judge_range_mat` 两个都非空才画）。开关只看配置，所以
        // 与是否真的转换过无关 —— 同时打开时同样不画。按键那边照样会往
        // `force_position_quads` 里推矩形，这里顺手清掉，免得越攒越多。
        if res.config.fixed_lane_chart || res.config.chart_mod_arrange {
            res.force_position_quads.borrow_mut().clear();
            return;
        }
        // 「强制定位」：**不理谱面自带的 `blockAreaList`**（谱面一条噪域都没有也照样生效），
        // 改铺一块合成的全屏噪域，再由每个按键周围的长方形把它挖出洞来。
        //
        // 「反向强制定位」（`block_area_force_invert`）正好反过来：不铺全屏噪域，只把那圈
        // 矩形**本身**当噪域画出来 ⇒ 这里传**空**方块列表，矩形由 `render_all` 直接从
        // `Resource::force_position_quads` 推成可绘制的 zone（不是合成块）。
        let force = res.config.block_area_force_position;
        let force_invert = force && res.config.block_area_force_invert;
        // 「简化噪域」单独勾选也要能出画面，所以两个开关任一打开就渲染
        //
        // 「纯配置谱面」下谱面自带的 `blockAreaList` 整层不画：判定线被拍平后那些方块都是按判定线原先
        // 的坐标定位的，画出来与画面上的按键完全对不上。本函数开头已经把 `block_area_paint` /
        // `judge_range_mat` 清成 `None`，所以判定范围（要这两个同时非空才画，见 `Note::render`）也跟着
        // 回到原样。
        //
        // 例外是标了噪域的按键：它们会在自己周围推出四边形（`Note::pure_config_noise`），这里改成传
        // **空**方块列表往下走 —— 与「反向强制定位」同一条通路，屏幕上于是只有那些矩形是噪域。没有这种
        // 按键时 `render_all` 会因为 `zones` 为空立刻返回，所以照旧等于不画。
        let pure_config_noise = res.config.pure_config_chart && res.config.pure_config_chart_noise;
        if pure_config_noise {
            let time = res.time;
            let flip_x = res.config.flip_x();
            let simplified = res.config.simplified_block_area || self.block_area_renderer.borrow().style.simplified;
            let reuse = res.config.block_area_reuse || self.block_area_renderer.borrow().style.reuse;
            super::block_area::render_native_block_areas(res, &[], &[], time, flip_x, simplified, reuse, &self.blocked_touches);
            return;
        }
        if res.config.pure_config_chart
            || (!res.config.render_block_area && !res.config.simplified_block_area)
            || (!force && self.block_areas.is_empty() && self.next_block_areas.is_empty())
        {
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
        // 「强制定位」把整层换成合成噪域（反向时只留下按键矩形），谱面自带的 rpe-next 噪域
        // 同样要忽略，所以这两种情况都传空列表。
        let next: &[NextArea] = if force { &[] } else { &self.next_block_areas };
        let flip_x = res.config.flip_x();
        // 噪域现在只有一条管线：Phira-Pro 的原生实现（掩码 + 原版 Unity 面元着色器）。
        // 「简化噪域」**不是另一条路**，而是让同一份官方材质退化成「纯色 × 不透明度」
        //（`block_area::native::shader::set_fill_style` 覆写一组 uniform），所以这里
        // 只是把开关往下传，界面开关与 API 的 `BlockAreaStyle::simplified` 任一为真即算简化。
        let simplified = res.config.simplified_block_area || self.block_area_renderer.borrow().style.simplified;
        // 「噪域复用」同理：只换「简化噪域文字」印的号（条目序号 → 噪域编号），不碰画面。
        let reuse = res.config.block_area_reuse || self.block_area_renderer.borrow().style.reuse;
        super::block_area::render_native_block_areas(res, blocks, next, time, flip_x, simplified, reuse, &self.blocked_touches);
    }
}
