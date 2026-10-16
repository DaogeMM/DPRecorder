use super::{chart::ChartSettings, object::CtrlObject, Anim, AnimFloat, BpmList, Matrix, Note, Object, Point, RenderConfig, Resource, Vector};
use crate::{
    config::Mods,
    core::{anim::AnimFloatF64, NoteKind},
    ext::{get_viewport, parse_alpha, NotNanExt, SafeTexture},
    judge::{JudgeStatus, LIMIT_BAD},
    ui::Ui,
};
use macroquad::prelude::*;
use miniquad::{RenderPass, Texture, TextureParams, TextureWrap};
use nalgebra::Rotation2;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum UIElement {
    Pause = 1,
    ComboNumber = 2,
    Combo = 3,
    Score = 4,
    Bar = 5,
    Name = 6,
    Dif = 7,
}

impl UIElement {
    pub fn from_u8(val: u8) -> Option<Self> {
        Some(match val {
            1 => Self::Pause,
            2 => Self::ComboNumber,
            3 => Self::Combo,
            4 => Self::Score,
            5 => Self::Bar,
            6 => Self::Name,
            7 => Self::Dif,
            _ => return None,
        })
    }
}

pub struct GifFrames {
    /// time of each frame in milliseconds
    frames: Vec<(u128, SafeTexture)>,
    /// milliseconds
    total_time: u128,
}

impl GifFrames {
    pub fn new(frames: Vec<(u128, SafeTexture)>) -> Self {
        let total_time = frames.iter().map(|(time, _)| *time).sum();
        Self { frames, total_time }
    }

    pub fn get_time_frame(&self, time: u128) -> &SafeTexture {
        let mut time = time % self.total_time;
        for (t, frame) in &self.frames {
            if time < *t {
                return frame;
            }
            time -= t;
        }
        &self.frames.last().unwrap().1
    }

    pub fn get_prog_frame(&self, prog: f32) -> &SafeTexture {
        let time = (prog * self.total_time as f32) as u128;
        self.get_time_frame(time)
    }

    pub fn total_time(&self) -> u128 {
        self.total_time
    }
}

#[derive(Clone)]
pub struct TextData {
    pub text: String,
    pub font_id: Option<usize>,
}

impl Default for TextData {
    fn default() -> Self {
        Self {
            text: String::new(),
            font_id: None,
        }
    }
}

#[derive(Default)]
pub enum JudgeLineKind {
    #[default]
    Normal,
    Texture(SafeTexture, String),
    TextureGif(Anim<f32>, GifFrames, String),
    Text(Anim<TextData>),
    Paint(Anim<f32>, RefCell<(Option<RenderPass>, bool)>),
}

#[derive(Clone)]
pub struct JudgeLineCache {
    update_order: Vec<u32>,
    above_indices: Vec<usize>,
    below_indices: Vec<usize>,
}

impl JudgeLineCache {
    pub fn new(notes: &mut Vec<Note>) -> Self {
        notes.sort_unstable_by_key(|it| {
            (
                !it.above,
                it.speed.not_nan(),
                (match it.kind {
                    NoteKind::Hold { end_height, .. } => it.height.min(end_height),
                    _ => it.height,
                } + it.object.translation.1.now() as f64 * it.speed)
                    .not_nan(),
            )
        });

        let mut res = Self {
            update_order: Vec::with_capacity(notes.len()),
            above_indices: Vec::new(),
            below_indices: Vec::new(),
        };
        res.reset(notes);
        res
    }

    pub(crate) fn reset(&mut self, notes: &mut Vec<Note>) {
        self.update_order.clear();
        self.update_order.extend(0..notes.len() as u32);
        self.above_indices.clear();
        self.below_indices.clear();
        let mut index = 0;
        while notes.get(index).map_or(false, |it| it.above) {
            self.above_indices.push(index);
            let speed = notes[index].speed;
            loop {
                index += 1;
                if !notes.get(index).map_or(false, |it| it.above && it.speed == speed) {
                    break;
                }
            }
        }
        while index != notes.len() {
            self.below_indices.push(index);
            let speed = notes[index].speed;
            loop {
                index += 1;
                if !notes.get(index).map_or(false, |it| it.speed == speed) {
                    break;
                }
            }
        }
    }
}

pub struct JudgeLine {
    pub object: Object,
    pub color: Anim<Color>,
    pub ctrl_obj: RefCell<CtrlObject>,
    pub kind: JudgeLineKind,
    pub height: AnimFloatF64,
    pub incline: AnimFloat,
    pub notes: Vec<Note>,
    pub parent: Option<usize>,
    pub rotate_with_parent: bool,
    pub z_index: i32,
    pub show_below: bool,
    pub attach_ui: Option<UIElement>,

    pub cache: JudgeLineCache,
    pub anchor: [f32; 2],
}

unsafe impl Sync for JudgeLine {}
unsafe impl Send for JudgeLine {}

impl JudgeLine {
    pub fn update(&mut self, res: &mut Resource, tr: Matrix, bpm_list: &mut BpmList, index: usize) {
        // self.object.set_time(res.time); // this is done by chart, chart has to calculate transform for us
        let rot = self.object.rotation.now();
        self.height.set_time(res.time);
        let line_height = self.height.now();
        let mut ctrl_obj = self.ctrl_obj.borrow_mut();

        //   self.cache.update_order.retain(|id| {
        //       let note = &mut self.notes[*id as usize];
        //       note.update(...);
        //       !note.dead()
        //   });
        //   retain 在删除元素时需要将后续所有元素前移（memmove），开销为 O(n)

        let mut i = 0;
        while i < self.cache.update_order.len() {
            let id = self.cache.update_order[i];
            let note = &mut self.notes[id as usize];
            note.update(res, rot, &tr, &mut ctrl_obj, line_height, bpm_list, index);
            if note.dead() {
                self.cache.update_order.swap_remove(i); // update_order 顺序不影响功能
            } else {
                i += 1;
            }
        }
        // 「屏幕进水」：普通按键（Click / Drag / Flick）不需要逐帧更新，所以第一次 `note.update`
        // 之后就因为 `Note::dead()` 被移出 `update_order`（只有长度没走完的长条会留下）—— 溺水
        // 扣血必须在这里单独补一遍，否则只有长条会掉血。`water_last == res.time` 表示这一帧上面
        // 那圈已经更新过它（长条），跳过。
        //
        // 扣血条件是「按键画在屏幕里」（由 `Note::update_water` 自己判断），**不**限制离判定
        // 还有多久：慢慢飘下来的按键可以在水里泡很久，早该掉血了。这里只用一个很宽的下界把
        // 遥远未来的音符挡掉，省掉无谓的坐标变换。
        if res.config.water_screen {
            for note in self.notes.iter_mut() {
                if note.water.last == res.time {
                    continue;
                }
                // 只要按键的坐标还画在屏幕里就一直算溺水时间（由 `Note::update_water`
                // 自己判断）—— 既不限制判定过去多久，也**不跳过未来音符**：表演用的 Drag
                // 常常很晚才被判定（本谱 line9 上就有一条 2:32 的），却从 0:12 起就画在水里；
                // 它一旦提前离开 `update_order`（例如 object 提前 dead），以前那条
                // `res.time < note.time - 60.` 的下界会让它永远拿不到扣血。
                crate::core::water::update(note, res, &tr, &mut ctrl_obj, line_height);
            }
        }
        drop(ctrl_obj);
        match &mut self.kind {
            JudgeLineKind::Text(anim) => {
                anim.set_time(res.time);
            }
            JudgeLineKind::Paint(anim, ..) => {
                anim.set_time(res.time);
            }
            JudgeLineKind::TextureGif(anim, ..) => {
                anim.set_time(res.time);
            }
            _ => {}
        }
        self.color.set_time(res.time);

        // 「强制定位」的延后视窗（`block_area_force_delay`，默认 0.3s）：判定掉的按键本身
        // 不再绘制，但「强制定位」挖出来的洞要再停留一会儿，所以延迟窗口内的音符**不能**
        // 在这里被判成「可以跳过」—— 否则游标一推进，`Line::render` 的循环就再也扫不到
        // 它们，`Note::render_force_linger` 也就无从补洞（洞会跟着按键一起立刻消失）。
        let linger = res.config.block_area_force_position && res.config.block_area_force_delay > 0.;
        let delay = res.config.block_area_force_delay as f64;
        let not_judge = |index: usize| {
            let note = &self.notes[index];
            // 「屏幕进水」：溺水死亡动画播放期间（`water_death > 0`）不能被当成「可以跳过」
            // 而移出渲染列表 —— 否则普通按键（Click / Drag / Flick）的常红 + 倾斜 + 淡出
            // 只会闪一帧就没了（这个游标一推进，`Line::render` 的循环就再也扫不到它）。
            if note.water.death > 0. {
                return false;
            }
            if note.water.death_total > 0. {
                // 「屏幕进水」：溺水死亡的动画已经播完 ⇒ 直接移出渲染列表。否则长条要一直画到
                // 自己的 `end_time`（原版「判定后长条淡化」那条路），屏幕上会留一根发白的半透明
                // 长条（去掉长条死亡后的淡色效果）。`Note::dead()` 同一帧也会把它从
                // `update_order` 里拿掉。
                return true;
            }
            let judged = matches!(note.judge, JudgeStatus::Judged);
            let force_end = match note.kind {
                // Hold 的判定时刻取长条末尾，与 `Note::render_force_linger` 一致。
                NoteKind::Hold { end_time, .. } => {
                    if !(judged && res.time > end_time) {
                        return false;
                    }
                    end_time
                }
                _ => {
                    if !judged {
                        return false;
                    }
                    note.time
                }
            };
            !(linger && res.time - force_end <= delay)
        };

        //   self.cache.above_indices.retain_mut(|index| {
        //       while not_judge(*index) { ... }
        //       true/false
        //   });
        //   retain_mut 在删除元素时需要将后续元素前移，产生内存拷贝开销

        let mut write_idx = 0;
        for i in 0..self.cache.above_indices.len() {
            let mut index = self.cache.above_indices[i];
            while not_judge(index) {
                if self
                    .notes
                    .get(index + 1)
                    .map_or(false, |it| it.above && it.speed == self.notes[index].speed)
                {
                    index += 1;
                } else {
                    index = usize::MAX; // 标记删除
                    break;
                }
            }
            if index != usize::MAX {
                self.cache.above_indices[write_idx] = index;
                write_idx += 1;
            }
        }
        self.cache.above_indices.truncate(write_idx);

        let mut write_idx = 0;
        for i in 0..self.cache.below_indices.len() {
            let mut index = self.cache.below_indices[i];
            while not_judge(index) {
                if self.notes.get(index + 1).map_or(false, |it| it.speed == self.notes[index].speed) {
                    index += 1;
                } else {
                    index = usize::MAX;
                    break;
                }
            }
            if index != usize::MAX {
                self.cache.below_indices[write_idx] = index;
                write_idx += 1;
            }
        }
        self.cache.below_indices.truncate(write_idx);
    }

    pub fn fetch_pos(&self, res: &Resource, lines: &[JudgeLine]) -> Vector {
        if let Some(parent) = self.parent {
            let parent = &lines[parent];
            let parent_translation = parent.fetch_pos(res, lines);
            return parent_translation + Rotation2::new(parent.fetch_rot(lines).to_radians()) * self.object.now_translation(res);
        }
        self.object.now_translation(res)
    }

    pub fn fetch_rot(&self, lines: &[JudgeLine]) -> f32 {
        let mut rot = self.object.rotation.now();
        if self.rotate_with_parent {
            if let Some(parent) = self.parent {
                rot += lines[parent].fetch_rot(lines);
            }
        }
        rot
    }

    pub fn now_transform(&self, res: &Resource, lines: &[JudgeLine]) -> Matrix {
        Rotation2::new(self.fetch_rot(lines).to_radians())
            .to_homogeneous()
            .append_translation(&self.fetch_pos(res, lines))
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

    pub fn render(&self, ui: &mut Ui, res: &mut Resource, lines: &[JudgeLine], bpm_list: &mut BpmList, settings: &ChartSettings, id: usize) {
        let alpha = self.object.now_alpha();
        let color = self.color.now_opt();
        res.with_model(self.now_transform(res, lines), |res| {
            res.with_model(self.object.now_scale(), |res| {
                // 「待判按键」染色那一趟只重画那一个按键，判定线本体（以及贴图/gif 判定线）不重画。
                if !res.pending_tint.get() {
                    res.apply_model(|res| match &self.kind {
                    JudgeLineKind::Normal => {
                        if res.config.render_line {
                            let hex_str = &res.config.linecolor;
                            let global_default_color = Self::hex_to_color(hex_str, res.judge_line_color);
                            // 旧版本UI：判定线强制为白色
                            let mut color = color.unwrap_or(global_default_color);
                            color.a = parse_alpha(
                                color.a * alpha.max(0.0),
                                if res.info.fold_animation { 1.0 } else { res.alpha },
                                0.15,
                                res.config.chart_debug_line > 0.,
                            );
                            if color.a == 0.0 {
                                return;
                            }
                            let len = if settings.line_reference_y_axis {
                                res.info.line_length / res.aspect_ratio
                            } else {
                                res.info.line_length
                            };
                            let thickness = if settings.line_reference_y_axis {
                                0.0150 / res.aspect_ratio
                            } else {
                                0.0100
                            };
                            draw_line(-len, 0., len, 0., thickness, color);
                        }
                    }
                    JudgeLineKind::Texture(texture, _) => {
                        if res.config.render_line_extra {
                            let mut color = color.unwrap_or(WHITE);
                            color.a = parse_alpha(alpha.max(0.0), res.alpha, 0.15, res.config.chart_debug_line > 0.);
                            if color.a == 0.0 {
                                return;
                            }
                            // let hf = vec2(texture.width() / res.aspect_ratio, texture.height() / res.aspect_ratio);
                            let hf = vec2(texture.width(), texture.height()); // Sync RPE
                            draw_texture_ex(
                                **texture,
                                -hf.x * self.anchor[0],
                                -hf.y * self.anchor[1],
                                color,
                                DrawTextureParams {
                                    dest_size: Some(hf),
                                    flip_y: true,
                                    ..Default::default()
                                },
                            );
                        }
                    }
                    JudgeLineKind::TextureGif(anim, frames, _) => {
                        if res.config.render_line_extra {
                            let t = anim.now_opt().unwrap_or(0.0);
                            let frame = frames.get_prog_frame(t);
                            let mut color = color.unwrap_or(WHITE);
                            color.a = parse_alpha(alpha.max(0.0), res.alpha, 0.15, res.config.chart_debug_line > 0.);
                            if color.a == 0.0 {
                                return;
                            }
                            let hf = vec2(frame.width(), frame.height());
                            draw_texture_ex(
                                **frame,
                                -hf.x * self.anchor[0],
                                -hf.y * self.anchor[1],
                                color,
                                DrawTextureParams {
                                    dest_size: Some(hf),
                                    flip_y: true,
                                    ..Default::default()
                                },
                            );
                        }
                    }
                    JudgeLineKind::Text(anim) => {
                        if res.config.render_line_extra {
                            let mut color = color.unwrap_or(WHITE);
                            color.a = parse_alpha(alpha.max(0.0), res.alpha, 0.15, res.config.chart_debug_line > 0.);
                            if color.a == 0.0 {
                                return;
                            }
                            res.apply_model_of(&Matrix::identity().append_nonuniform_scaling(&Vector::new(1., -1.)), |res| {
                                let now = anim.now();
                                let mut painter = now.font_id.and_then(|id| res.fonts.get(id)).map(|cell| cell.borrow_mut());
                                ui.text(&now.text)
                                    .pos(0., 0.)
                                    .anchor(self.anchor[0], -self.anchor[1] + 1.)
                                    .size(1.)
                                    .color(color)
                                    .multiline()
                                    .draw_with_font(painter.as_deref_mut());
                            });
                        }
                    }
                    JudgeLineKind::Paint(anim, state) => {
                        if res.config.render_line_extra {
                            let mut color = color.unwrap_or(WHITE);
                            color.a = parse_alpha(alpha.max(0.0) * 2.55, res.alpha, 0.15, res.config.chart_debug_line > 0.);
                            let mut gl = unsafe { get_internal_gl() };
                            let mut guard = state.borrow_mut();
                            let vp = get_viewport();
                            let pass = *guard.0.get_or_insert_with(|| {
                                let ctx = &mut gl.quad_context;
                                let tex = Texture::new_render_texture(
                                    ctx,
                                    TextureParams {
                                        width: vp.2 as _,
                                        height: vp.3 as _,
                                        format: miniquad::TextureFormat::RGBA8,
                                        filter: FilterMode::Linear,
                                        wrap: TextureWrap::Clamp,
                                    },
                                );
                                RenderPass::new(ctx, tex, None)
                            });
                            gl.flush();
                            let old_pass = gl.quad_gl.get_active_render_pass();
                            gl.quad_gl.render_pass(Some(pass));
                            gl.quad_gl.viewport(None);
                            let size = anim.now();
                            if size <= 0. {
                                if guard.1 {
                                    clear_background(Color::default());
                                    guard.1 = false;
                                }
                            } else {
                                ui.fill_circle(0., 0., size / vp.2 as f32 * 2., color);
                                guard.1 = true;
                            }
                            gl.flush();
                            gl.quad_gl.render_pass(old_pass);
                            gl.quad_gl.viewport(Some(vp));
                        }
                    }
                })
                }
            });
            if let JudgeLineKind::Paint(_, state) = &self.kind {
                let guard = state.borrow_mut();
                if guard.1 && res.config.render_line_extra && !res.pending_tint.get() {
                    let ctx = unsafe { get_internal_gl() }.quad_context;
                    let tex = guard.0.as_ref().unwrap().texture(ctx);
                    let top = 1. / res.aspect_ratio;
                    draw_texture_ex(
                        Texture2D::from_miniquad_texture(tex),
                        -1.,
                        -top,
                        WHITE,
                        DrawTextureParams {
                            dest_size: Some(vec2(2., top * 2.)),
                            ..Default::default()
                        },
                    );
                }
            }
            let mut config = RenderConfig {
                settings,
                ctrl_obj: &mut self.ctrl_obj.borrow_mut(),
                line_height: self.height.now(),
                appear_before: f64::INFINITY,
                invisible_time: f64::INFINITY,
                draw_below: self.show_below,
                incline_sin: self.incline.now_opt().map(|it| it.to_radians().sin()).unwrap_or_default(),
            };
            if res.config.has_mod(Mods::FADE_OUT) {
                config.invisible_time = LIMIT_BAD;
            }
            let mut line_set_debug_alpha = false;
            if alpha < 0.0 {
                // `w == 0` 就是 `-1 < alpha < 0`：谱面转换器把「这条判定线全透明」写成
                // `-1.7881394e-08` 这类浮点误差值时落在这里。官方语义只是**线自己不可见**，
                // 线上的音符照常显示；所以这种情况不能按下面的 PE 负 alpha 扩展整条跳过
                //（参考实现 Phira 在这里直接 `return`，会把整条线的音符一起吞掉，
                //  `心の記憶` 的 HD 谱面就是这么被吞光的）。
                let w = (-alpha).floor() as u32;
                if w != 0 {
                    if !settings.pe_alpha_extension {
                        if res.config.chart_debug_note > 0. {
                            line_set_debug_alpha = true;
                        } else {
                            return;
                        }
                    }
                    match w {
                        1 => {
                            if res.config.chart_debug_note > 0. {
                                line_set_debug_alpha = true;
                            } else {
                                return;
                            }
                        }
                        2 => {
                            config.draw_below = false;
                        }
                        w if (100..1000).contains(&w) => {
                            config.appear_before = (w as f64 - 100.) / 10.;
                        }
                        w if (1000..2000).contains(&w) => {
                            // TODO unsupported
                        }
                        _ => {}
                    }
                }
            }
            let vw = 1.01 / res.config.chart_ratio;
            let (vw, vh) = if res.config.rotation_mode {
                (vw, vw.max(vw / res.aspect_ratio))
            } else {
                (vw, vw / res.aspect_ratio)
            };
            let p = [
                res.screen_to_world(Point::new(-vw, -vh)),
                res.screen_to_world(Point::new(-vw, vh)),
                res.screen_to_world(Point::new(vw, -vh)),
                res.screen_to_world(Point::new(vw, vh)),
            ];
            let height_above = p[0].y.max(p[1].y.max(p[2].y.max(p[3].y))) as f64;
            let height_below = p[0].y.min(p[1].y.min(p[2].y.min(p[3].y))) as f64;
            let agg = res.config.aggressive_chart;
            let mut height = self.height.clone();
            let aspect_ratio = res.aspect_ratio as f64;
            if res.config.note_scale > 0. && res.config.render_note {
                for index in &self.cache.above_indices {
                    let speed = self.notes[*index].speed;
                    for note in self.notes[*index..].iter() {
                        if !note.above || speed != note.speed {
                            break;
                        }
                        // 判定掉的按键本身不再绘制（已松手的 Hold 也算：改动前它们同样会被
                        // `not_judge` 的游标跳过），但「强制定位」採出来的洞还要再留
                        // `block_area_force_delay` 秒（默认 0.3）—— 见 `Note::render_force_linger`，
                        // 那里只推洞、不着色。
                        let hold_running = match note.kind {
                            NoteKind::Hold { end_time, .. } => res.time <= end_time,
                            _ => false,
                        };
                        if matches!(note.judge, JudgeStatus::Judged) && !hold_running && note.water.death <= 0. {
                            note.render_force_linger(res, &mut config);
                            continue;
                        }
                        if agg {
                            let line_height = match note.kind {
                                NoteKind::Hold { end_time, .. } => {
                                    let time = if res.time < end_time { res.time.min(note.time) } else { res.time };
                                    height.set_time(time);
                                    height.now()
                                }
                                _ => config.line_height,
                            };
                            let note_height = ((note.height - line_height) * speed + note.object.translation.1.now() as f64) / aspect_ratio;
                            match note.kind {
                                NoteKind::Hold { end_height, .. } => {
                                    let end_height = ((end_height - line_height) * speed + note.object.translation.1.now() as f64) / aspect_ratio;
                                    if note_height < height_below && end_height < height_below {
                                        continue;
                                    }
                                    if note_height > height_above && end_height > height_above {
                                        break;
                                    }
                                }
                                _ => {
                                    if note_height < height_below {
                                        continue;
                                    }
                                    if note_height > height_above {
                                        break;
                                    }
                                }
                            }
                        }
                        note.render(ui, res, &mut config, bpm_list, line_set_debug_alpha, id, height_above, height_below);
                    }
                }

                res.with_model(Matrix::identity().append_nonuniform_scaling(&Vector::new(1.0, -1.0)), |res| {
                    for index in &self.cache.below_indices {
                        let speed = self.notes[*index].speed;
                        for note in self.notes[*index..].iter() {
                            if speed != note.speed {
                                break;
                            }
                            // 同上（判定线下方那一趟，方向已经镜像）：判定掉的音符（含已松手的 Hold）
                            // 不再绘制，但延后视窗里还要补推洞。
                            let hold_running = match note.kind {
                                NoteKind::Hold { end_time, .. } => res.time <= end_time,
                                _ => false,
                            };
                            if matches!(note.judge, JudgeStatus::Judged) && !hold_running && note.water.death <= 0. {
                                note.render_force_linger(res, &mut config);
                                continue;
                            }
                            if agg {
                                let line_height = match note.kind {
                                    NoteKind::Hold { end_time, .. } => {
                                        let time = if res.time < end_time { res.time.min(note.time) } else { res.time };
                                        height.set_time(time);
                                        height.now()
                                    }
                                    _ => config.line_height,
                                };
                                let note_height = ((note.height - line_height) * speed + note.object.translation.1.now() as f64) / aspect_ratio;
                                match note.kind {
                                    NoteKind::Hold { end_height, .. } => {
                                        let end_height = ((end_height - line_height) * speed + note.object.translation.1.now() as f64) / aspect_ratio;
                                        if note_height < -height_above && end_height < -height_above {
                                            continue;
                                        }
                                        if note_height > -height_below && end_height > -height_below {
                                            break;
                                        }
                                    }
                                    _ => {
                                        if note_height < -height_above {
                                            continue;
                                        }
                                        if note_height > -height_below {
                                            break;
                                        }
                                    }
                                }
                            }
                            note.render(ui, res, &mut config, bpm_list, line_set_debug_alpha, id, -height_below, -height_above);
                        }
                    }
                });
            }
            if res.config.chart_debug_line > 0. {
                res.with_model(Matrix::identity().append_nonuniform_scaling(&Vector::new(1.0, -1.0)), |res| {
                    res.apply_model(|res| {
                        let kind = match &self.kind {
                            JudgeLineKind::Normal => {
                                if !res.config.render_line {
                                    return;
                                };
                                String::new()
                            }
                            JudgeLineKind::Text(text) => {
                                if !res.config.render_line_extra {
                                    return;
                                };
                                format!(" text:{}", text.now().text)
                            }
                            JudgeLineKind::Texture(_, name) => {
                                if !res.config.render_line_extra {
                                    return;
                                };
                                format!(" img:{}", name)
                            }
                            JudgeLineKind::TextureGif(_, frames, name) => {
                                if !res.config.render_line_extra {
                                    return;
                                };
                                format!(" gif:{}/{}", name, frames.total_time())
                            }
                            JudgeLineKind::Paint(_, _) => {
                                if !res.config.render_line_extra {
                                    return;
                                };
                                format!(" paint")
                            }
                        };

                        let parent = if let Some(parent) = self.parent {
                            format!("({})", parent)
                        } else {
                            String::new()
                        };
                        let line_height_ulp_in_f32 = {
                            if !config.line_height.is_nan() & !config.line_height.is_infinite() {
                                f32::EPSILON * config.line_height.abs() as f32
                            } else {
                                0.0
                            }
                        };
                        let line_height_ulp_string = {
                            if line_height_ulp_in_f32 > 0.0018518519 {
                                format!("(Speed too high! ULP: {:.4})", line_height_ulp_in_f32)
                            } else {
                                String::new()
                            }
                        };
                        let z_index = {
                            if self.z_index == 0 {
                                String::new()
                            } else {
                                format!(" z:{}", self.z_index)
                            }
                        };
                        let attach_ui = {
                            let num = self.attach_ui.map_or(0, |it| it as u8);
                            if num == 0 {
                                String::new()
                            } else {
                                format!(" a_ui:{}", num)
                            }
                        };
                        let anchor = if self.anchor[0] == 0.5 && self.anchor[1] == 0.5 {
                            String::new()
                        } else {
                            format!(" anc:{} {}", self.anchor[0], self.anchor[1])
                        };
                        let color = if line_height_ulp_in_f32 > 0.018518519 {
                            // 10px error in 1080P
                            Color::new(1., 0., 0., parse_alpha(alpha, res.alpha, 0.15, res.config.chart_debug_line > 0.))
                        } else if line_height_ulp_in_f32 > 0.0018518519 {
                            // 1px error in 1080P
                            Color::new(1., 1., 0., parse_alpha(alpha, res.alpha, 0.15, res.config.chart_debug_line > 0.))
                        } else {
                            Color::new(1., 1., 1., parse_alpha(alpha, res.alpha, 0.15, res.config.chart_debug_line > 0.))
                        };
                        ui.text(format!(
                            "[{}]{} h:{:.2}{}{}{}{}{}",
                            id, parent, config.line_height, line_height_ulp_string, z_index, attach_ui, anchor, kind
                        ))
                        .pos(0., -res.config.chart_debug_line * 0.1)
                        .anchor(0.5, 1.)
                        .size(res.config.chart_debug_line)
                        .color(color)
                        .draw();
                    });
                });
            }
        });
    }
}
