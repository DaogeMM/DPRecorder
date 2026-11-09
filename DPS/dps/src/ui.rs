mod billboard;
pub use billboard::{BillBoard, Message, MessageHandle, MessageKind};

mod chart_info;
pub use chart_info::*;

mod dialog;
pub use dialog::Dialog;

mod scroll;
pub use scroll::Scroll;

mod shading;
pub use shading::*;

mod shadow;
pub use shadow::*;

pub mod icon;
pub mod theme;

mod text;
pub use text::{DrawText, TextPainter};

pub use glyph_brush::ab_glyph::FontArc;

use crate::{
    core::{Matrix, Point, Vector},
    ext::{get_viewport, nalgebra_to_glm, round_to_step, semi_black, semi_white, source_of_image, RectExt, SafeTexture, ScaleType},
    judge::Judge,
    scene::{request_input_full, return_input, take_input},
};
use lyon::{
    lyon_tessellation::{
        BuffersBuilder, FillOptions, FillTessellator, FillVertex, FillVertexConstructor, StrokeOptions, StrokeTessellator, StrokeVertex,
        StrokeVertexConstructor, VertexBuffers,
    },
    math as lm,
    path::{builder::BorderRadii, Path, PathEvent, Winding},
};
use macroquad::prelude::*;
use miniquad::PassAction;
use sasa::{AudioManager, PlaySfxParams, Sfx};
use std::{borrow::Cow, cell::RefCell, collections::HashMap, ops::Range};

#[derive(Default, Clone, Copy)]
pub struct Gravity(u8);

impl Gravity {
    pub const LEFT: u8 = 0;
    pub const HCENTER: u8 = 1;
    pub const RIGHT: u8 = 2;
    pub const TOP: u8 = 0;
    pub const VCENTER: u8 = 4;
    pub const BOTTOM: u8 = 8;

    pub const BEGIN: u8 = Self::LEFT | Self::TOP;
    pub const CENTER: u8 = Self::HCENTER | Self::VCENTER;
    pub const END: u8 = Self::RIGHT | Self::BOTTOM;

    fn value(mode: u8) -> f32 {
        match mode {
            0 => 0.,
            1 => 0.5,
            2 => 1.,
            _ => unreachable!(),
        }
    }

    pub fn offset(&self, total: (f32, f32), content: (f32, f32)) -> (f32, f32) {
        (Self::value(self.0 & 3) * (total.0 - content.0), Self::value((self.0 >> 2) & 3) * (total.1 - content.1))
    }

    pub fn from_point(&self, point: (f32, f32), content: (f32, f32)) -> (f32, f32) {
        (point.0 - content.0 * Self::value(self.0 & 3), point.1 - content.1 * Self::value((self.0 >> 2) & 3))
    }
}

impl From<u8> for Gravity {
    fn from(val: u8) -> Self {
        Self(val)
    }
}

struct ShadedConstructor<T: Shading>(Matrix, pub T);
impl<T: Shading> FillVertexConstructor<Vertex> for ShadedConstructor<T> {
    fn new_vertex(&mut self, vertex: FillVertex) -> Vertex {
        let pos = vertex.position();
        self.1.new_vertex(&self.0, &Point::new(pos.x, pos.y))
    }
}
impl<T: Shading> StrokeVertexConstructor<Vertex> for ShadedConstructor<T> {
    fn new_vertex(&mut self, vertex: StrokeVertex) -> Vertex {
        let pos = vertex.position();
        self.1.new_vertex(&self.0, &Point::new(pos.x, pos.y))
    }
}

pub struct VertexBuilder<T: Shading> {
    matrix: Matrix,
    vertices: Vec<Vertex>,
    indices: Vec<u16>,
    shading: T,
}

impl<T: Shading> VertexBuilder<T> {
    fn new(matrix: Matrix, shading: T) -> Self {
        Self {
            matrix,
            vertices: Vec::new(),
            indices: Vec::new(),
            shading,
        }
    }

    pub fn add(&mut self, x: f32, y: f32) {
        self.vertices.push(self.shading.new_vertex(&self.matrix, &Point::new(x, y)))
    }

    pub fn triangle(&mut self, x: u16, y: u16, z: u16) {
        self.indices.push(x);
        self.indices.push(y);
        self.indices.push(z);
    }

    pub fn commit(&self) {
        let gl = unsafe { get_internal_gl() }.quad_gl;
        gl.texture(self.shading.texture());
        gl.draw_mode(DrawMode::Triangles);
        gl.geometry(&self.vertices, &self.indices);
    }
}

#[derive(Clone, Copy)]
pub struct RectButton {
    pub rect: Rect,
    id: Option<u64>,
}

impl Default for RectButton {
    fn default() -> Self {
        Self::new()
    }
}

impl RectButton {
    pub fn new() -> Self {
        Self {
            rect: Rect::default(),
            id: None,
        }
    }

    pub fn touching(&self) -> bool {
        self.id.is_some()
    }

    pub fn set(&mut self, ui: &mut Ui, rect: Rect) {
        self.rect = ui.rect_to_global(rect);
    }

    pub fn touch(&mut self, touch: &Touch) -> bool {
        let inside = self.rect.contains(touch.position);
        match touch.phase {
            TouchPhase::Started => {
                if inside {
                    self.id = Some(touch.id);
                }
            }
            TouchPhase::Moved | TouchPhase::Stationary => {
                if self.id == Some(touch.id) && !inside {
                    self.id = None;
                }
            }
            TouchPhase::Cancelled => {
                self.id = None;
            }
            TouchPhase::Ended => {
                if self.id.take() == Some(touch.id) && inside {
                    return true;
                }
            }
        }
        false
    }
}

#[derive(Clone)]
pub struct DRectButton {
    inner: RectButton,
    last_touching: bool,
    start_time: Option<f32>,
    pub config: ShadowConfig,
    delta: f32,
    play_sound: bool,
}
impl Default for DRectButton {
    fn default() -> Self {
        Self::new()
    }
}
impl DRectButton {
    pub const TIME: f32 = 0.2;

    pub fn new() -> Self {
        Self {
            inner: RectButton::new(),
            last_touching: false,
            start_time: None,
            config: ShadowConfig::default(),
            delta: -0.004,
            play_sound: true,
        }
    }

    pub fn build(&mut self, ui: &mut Ui, t: f32, r: Rect) -> (Rect, Path) {
        let r = r.feather((1. - self.progress(t)) * self.delta);
        self.inner.set(ui, r);
        (r, r.rounded(self.config.radius))
    }

    pub fn invalidate(&mut self) {
        self.inner.rect = Rect::default();
    }

    pub fn render_shadow<T: IntoShading>(&mut self, ui: &mut Ui, r: Rect, t: f32, alpha: f32, shading: impl FnOnce(Rect) -> T) -> (Rect, Path) {
        let (r, path) = self.build(ui, t, r);
        let p = self.progress(t);
        rounded_rect_shadow(
            ui,
            r,
            &ShadowConfig {
                elevation: self.config.elevation * p,
                base: self.config.base * alpha,
                radius: self.config.radius,
                ..self.config
            },
        );
        ui.fill_path(&path, shading(r).into_shading());
        (r, path)
    }

    pub fn render_text<'a>(
        &mut self,
        ui: &mut Ui,
        r: Rect,
        t: f32,
        alpha: f32,
        text: impl Into<Cow<'a, str>>,
        size: f32,
        chosen: bool,
    ) -> (Rect, Path) {
        let oh = r.h;
        let (r, path) = self.build(ui, t, r);
        let ct = r.center();
        // 选中态用抬一层的面板配强调色文字，未选中是压暗的控件底配次级文字。
        ui.fill_path(&path, theme::with_alpha(if chosen { theme::SURFACE_2 } else { theme::INPUT }, alpha));
        ui.border(r, self.config.radius, alpha);
        ui.text(text)
            .pos(ct.x, ct.y)
            .anchor(0.5, 0.5)
            .no_baseline()
            .size(size * (1. - (1. - r.h / oh).powf(1.3)))
            .max_width(r.w)
            .color(theme::with_alpha(if chosen { theme::ACCENT } else { theme::TEXT_SECONDARY }, alpha))
            .draw();
        (r, path)
    }

    /// 侧栏导航项：无边框的整行，选中时铺一层 `NAV_ACTIVE`，文字按选中与否换色。
    pub fn render_nav<'a>(&mut self, ui: &mut Ui, r: Rect, t: f32, alpha: f32, text: impl Into<Cow<'a, str>>, chosen: bool) {
        let (r, _) = self.build(ui, t, r);
        if chosen {
            ui.fill_path(&r.rounded(theme::RADIUS_SM), theme::with_alpha(theme::NAV_ACTIVE, alpha));
        }
        ui.text(text)
            .pos(r.x + 0.012, r.center().y)
            .anchor(0., 0.5)
            .no_baseline()
            .size(0.32)
            .max_width(r.w - 0.024)
            .color(theme::with_alpha(if chosen { theme::TEXT } else { theme::TEXT_TERTIARY }, alpha))
            .draw();
    }

    /// 把按钮当开关用：位置和按压反馈仍旧走按钮，画面交给 [`Ui::switch`]。
    pub fn render_switch(&mut self, ui: &mut Ui, r: Rect, t: f32, alpha: f32, on: bool) {
        let (r, _) = self.build(ui, t, r);
        ui.switch(r, on, alpha);
    }

    pub fn render_text_left<'a>(
        &mut self,
        ui: &mut Ui,
        r: Rect,
        t: f32,
        alpha: f32,
        text: impl Into<Cow<'a, str>>,
        size: f32,
        chosen: bool,
    ) -> (Rect, Path) {
        let oh = r.h;
        let (r, path) = self.build(ui, t, r);
        // 选中态用抬一层的面板配强调色文字，未选中是压暗的控件底配次级文字。
        ui.fill_path(&path, theme::with_alpha(if chosen { theme::SURFACE_2 } else { theme::INPUT }, alpha));
        ui.border(r, self.config.radius, alpha);
        ui.text(text)
            .pos(r.x + 0.02, r.center().y)
            .anchor(0., 0.5)
            .max_width(r.w - 0.04)
            .no_baseline()
            .size(size * r.h / oh)
            .color(theme::with_alpha(if chosen { theme::ACCENT } else { theme::TEXT_SECONDARY }, alpha))
            .draw();
        (r, path)
    }

    #[inline]
    pub fn render_input<'a>(
        &mut self,
        ui: &mut Ui,
        r: Rect,
        t: f32,
        alpha: f32,
        text: impl Into<Cow<'a, str>>,
        hint: impl Into<Cow<'a, str>>,
        size: f32,
    ) {
        let text = text.into();
        if text.trim().is_empty() {
            self.render_text_left(ui, r, t, alpha * 0.7, hint, size, false);
        } else {
            self.render_text_left(ui, r, t, alpha, text, size, false);
        }
    }

    #[inline]
    pub fn no_sound(mut self) -> Self {
        self.play_sound = false;
        self
    }

    #[inline]
    pub fn with_radius(mut self, radius: f32) -> Self {
        self.config.radius = radius;
        self
    }

    #[inline]
    pub fn with_elevation(mut self, elevation: f32) -> Self {
        self.config.elevation = elevation;
        self
    }

    #[inline]
    pub fn with_base(mut self, base: f32) -> Self {
        self.config.base = base;
        self
    }

    #[inline]
    pub fn with_delta(mut self, delta: f32) -> Self {
        self.delta = delta;
        self
    }

    pub fn progress(&mut self, t: f32) -> f32 {
        if self.start_time.as_ref().map_or(false, |it| t > *it + Self::TIME) {
            self.start_time = None;
        }
        let p = if let Some(time) = &self.start_time {
            (t - time) / Self::TIME
        } else {
            1.
        };
        if self.inner.touching() {
            1. - p
        } else {
            p
        }
    }

    pub fn touch(&mut self, touch: &Touch, t: f32) -> bool {
        let res = self.inner.touch(touch);
        let touching = self.inner.touching();
        if self.last_touching != touching {
            self.last_touching = touching;
            self.start_time = Some(t);
        }
        if res && self.play_sound {
            button_hit();
        }
        res
    }
}

pub struct Slider {
    range: Range<f32>,
    step: f32,

    touch: Option<(u64, f32, bool)>,
    rect: Rect,
    pos: f32,
}

impl Slider {
    const THRESHOLD: f32 = 0.05;

    pub fn new(range: Range<f32>, step: f32) -> Self {
        Self {
            range,
            step,

            touch: None,
            rect: Rect::default(),
            pos: f32::INFINITY,
        }
    }

    pub fn touch(&mut self, touch: &Touch, _t: f32, dst: &mut f32) -> Option<bool> {
        if let Some((id, start_pos, unlocked)) = &mut self.touch {
            if touch.id == *id {
                match touch.phase {
                    TouchPhase::Started | TouchPhase::Moved | TouchPhase::Stationary => {
                        if (touch.position.x - *start_pos).abs() >= Self::THRESHOLD {
                            *unlocked = true;
                        }
                        if *unlocked {
                            let p = (touch.position.x - self.rect.x) / self.rect.w;
                            let p = p.clamp(0., 1.);
                            let p = self.range.start + (self.range.end - self.range.start) * p;
                            let val = (p / self.step).round() * self.step;
                            *dst = round_to_step(val, self.step);
                            return Some(true);
                        }
                    }
                    TouchPhase::Cancelled | TouchPhase::Ended => {
                        self.touch = None;
                    }
                }
                return Some(false);
            }
        } else if touch.phase == TouchPhase::Started {
            let pos = (self.pos, self.rect.center().y);
            if (touch.position.x - pos.0).hypot(touch.position.y - pos.1) <= 0.03 {
                self.touch = Some((touch.id, touch.position.x, false));
                return Some(false);
            }
        }
        None
    }

    pub fn render(&mut self, ui: &mut Ui, r: Rect, _t: f32, c: Color, p: f32, text: String) {
        // DPR 的滑条：方形轨道（已拉过的部分是灰的，其余透明）+ 方形滑块，数值贴在控件右边。
        let vw = 0.1;
        let cy = r.center().y;
        ui.text(text)
            .pos(r.right(), cy)
            .anchor(1., 0.5)
            .no_baseline()
            .color(c)
            .size(0.5)
            .draw();
        let track = Rect::new(r.x, cy - 0.0025, (r.w - vw - 0.02).max(0.02), 0.005);
        let p = ((p - self.range.start) / (self.range.end - self.range.start)).clamp(0., 1.);
        let pos = track.x + track.w * p;
        self.rect = ui.rect_to_global(track);
        self.pos = ui.to_global((pos, cy)).0;
        // 没拉过的那段不填色（DPR 的滑条轨道是透明的），只画出已拉过的一段。
        ui.fill_rect(Rect::new(track.x, track.y, track.w * p, track.h), theme::with_alpha(theme::TRACK_FILL, c.a));
        let s = 0.016;
        ui.fill_rect(
            Rect::new(pos - s / 2., cy - s / 2., s, s),
            theme::with_alpha(theme::TRACK_THUMB, c.a),
        );
    }
}

thread_local! {
    static STATE: RefCell<HashMap<String, Option<u64>>> = RefCell::new(HashMap::new());
}

pub struct InputParams<'a> {
    changed: Option<&'a mut bool>,
    password: bool,
    length: f32,
}

impl From<()> for InputParams<'_> {
    fn from(_: ()) -> Self {
        Self {
            changed: None,
            password: false,
            length: 0.3,
        }
    }
}

impl From<bool> for InputParams<'_> {
    fn from(password: bool) -> Self {
        Self { password, ..().into() }
    }
}

impl From<f32> for InputParams<'_> {
    fn from(length: f32) -> Self {
        Self { length, ..().into() }
    }
}

impl<'a> From<(f32, &'a mut bool)> for InputParams<'a> {
    fn from((length, changed): (f32, &'a mut bool)) -> Self {
        Self {
            changed: Some(changed),
            password: false,
            length,
        }
    }
}

pub struct Ui<'a> {
    pub top: f32,
    pub viewport: (i32, i32, i32, i32),

    /// 当前页面文字的缩放。菜单页统一调小，游玩界面保持原尺寸；
    /// 由页面系统在渲染每个页面前设置。
    pub text_scale: f32,

    pub text_painter: &'a mut TextPainter,

    model_stack: Vec<Matrix>,
    touches: Option<Vec<Touch>>,

    vertex_buffers: VertexBuffers<Vertex, u16>,
    fill_tess: FillTessellator,
    fill_options: FillOptions,
    stroke_tess: StrokeTessellator,
    pub stroke_options: StrokeOptions,
}

impl<'a> Ui<'a> {
    pub fn new(text_painter: &'a mut TextPainter, viewport: Option<(i32, i32, i32, i32)>) -> Self {
        unsafe { get_internal_gl() }.quad_context.begin_default_pass(PassAction::Nothing);
        let viewport = viewport.unwrap_or_else(|| (0, 0, screen_width() as i32, screen_height() as i32));
        Self {
            top: viewport.3 as f32 / viewport.2 as f32,
            viewport,

            text_scale: 1.,

            text_painter,

            model_stack: vec![Matrix::identity()],
            touches: None,

            vertex_buffers: VertexBuffers::new(),
            fill_tess: FillTessellator::new(),
            fill_options: FillOptions::default(),
            stroke_tess: StrokeTessellator::new(),
            stroke_options: StrokeOptions::default(),
        }
    }

    pub fn camera(&self) -> Camera2D {
        Camera2D {
            zoom: vec2(1., -self.viewport.2 as f32 / self.viewport.3 as f32),
            viewport: Some(self.viewport),
            ..Default::default()
        }
    }

    pub fn ensure_touches(&mut self) -> &mut Vec<Touch> {
        if self.touches.is_none() {
            self.touches = Some(Judge::get_touches(1.0, false));
        }
        self.touches.as_mut().unwrap()
    }

    pub(crate) fn set_touches(&mut self, touches: Vec<Touch>) {
        self.touches = Some(touches);
    }

    pub fn builder<T: IntoShading>(&self, shading: T) -> VertexBuilder<T::Target> {
        VertexBuilder::new(self.get_matrix(), shading.into_shading())
    }

    pub fn fill_rect(&mut self, rect: Rect, shading: impl IntoShading) {
        let mut b = self.builder(shading);
        b.add(rect.x, rect.y);
        b.add(rect.x + rect.w, rect.y);
        b.add(rect.x, rect.y + rect.h);
        b.add(rect.x + rect.w, rect.y + rect.h);
        b.triangle(0, 1, 2);
        b.triangle(1, 2, 3);
        b.commit();
    }

    fn set_tolerance(&mut self) {
        let tol = 0.15 / (self.model_stack.last().unwrap().transform_vector(&Vector::new(1., 0.)).norm() * screen_width() / 2.);
        self.fill_options.tolerance = tol;
        self.stroke_options.tolerance = tol;
    }

    pub fn fill_path(&mut self, path: impl IntoIterator<Item = PathEvent>, shading: impl IntoShading) {
        self.set_tolerance();
        let shaded = ShadedConstructor(self.get_matrix(), shading.into_shading());
        let tex = shaded.1.texture();
        self.fill_tess
            .tessellate(path, &self.fill_options, &mut BuffersBuilder::new(&mut self.vertex_buffers, shaded))
            .unwrap();
        self.emit_lyon(tex);
    }

    pub fn fill_circle(&mut self, x: f32, y: f32, radius: f32, shading: impl IntoShading) {
        self.set_tolerance();
        let shaded = ShadedConstructor(self.get_matrix(), shading.into_shading());
        let tex = shaded.1.texture();
        self.fill_tess
            .tessellate_circle(lm::point(x, y), radius, &self.fill_options, &mut BuffersBuilder::new(&mut self.vertex_buffers, shaded))
            .unwrap();
        self.emit_lyon(tex);
    }

    pub fn stroke_circle(&mut self, x: f32, y: f32, radius: f32, width: f32, shading: impl IntoShading) {
        self.set_tolerance();
        let shaded = ShadedConstructor(self.get_matrix(), shading.into_shading());
        let tex = shaded.1.texture();
        self.stroke_options.line_width = width;
        self.stroke_tess
            .tessellate_circle(lm::point(x, y), radius, &self.stroke_options, &mut BuffersBuilder::new(&mut self.vertex_buffers, shaded))
            .unwrap();
        self.emit_lyon(tex);
    }

    pub fn stroke_path(&mut self, path: &Path, width: f32, shading: impl IntoShading) {
        self.set_tolerance();
        let shaded = ShadedConstructor(self.get_matrix(), shading.into_shading());
        let tex = shaded.1.texture();
        self.stroke_options.line_width = width;
        self.stroke_tess
            .tessellate_path(path, &self.stroke_options, &mut BuffersBuilder::new(&mut self.vertex_buffers, shaded))
            .unwrap();
        self.emit_lyon(tex);
    }

    /// 画一块 DPR 风格的面板：`SURFACE_1` 底、1 px `BORDER_2` 边框、圆角。
    ///
    /// 返回路径，调用方要在面板上再画东西（列表项、分隔线）时直接用。
    pub fn panel(&mut self, r: Rect, radius: f32, alpha: f32) -> Path {
        let path = r.rounded(radius);
        self.fill_path(&path, theme::with_alpha(theme::SURFACE_1, alpha));
        self.border(r, radius, alpha);
        path
    }

    /// 给一块圆角矩形画 1 px 边框。
    ///
    /// 边框用「外圈减内圈」两道路径填充，不走描边：lyon 的描边在闭合的圆角矩形路径上
    /// 会往画面外拉出贯穿整屏的退化线段，填充没有这个问题。
    pub fn border(&mut self, r: Rect, radius: f32, alpha: f32) {
        self.outline(r, radius, theme::HAIRLINE, theme::with_alpha(theme::BORDER_2, alpha));
    }

    /// 同 [`Ui::border`]，但线宽与颜色由调用方给。
    pub fn outline(&mut self, r: Rect, radius: f32, w: f32, color: Color) {
        let inner = r.feather(-w);
        let mut builder = Path::builder();
        builder.add_rounded_rectangle(&r.to_euclid(), &BorderRadii::new(radius), Winding::Positive);
        builder.add_rounded_rectangle(&inner.to_euclid(), &BorderRadii::new((radius - w).max(0.)), Winding::Negative);
        self.fill_path(&builder.build(), color);
    }

    /// 画一个 DPR 那种方形开关：直角外框、左右各半格，选中那半填灰、记号上语义色。
    ///
    /// 只负责画；点击还是由调用方的 `DRectButton` 管，开关状态存在配置里。
    pub fn switch(&mut self, r: Rect, on: bool, alpha: f32) {
        let w = theme::HAIRLINE;
        let half = r.w / 2.;
        let left = Rect::new(r.x, r.y, half, r.h);
        let right = Rect::new(r.x + half, r.y, half, r.h);
        let cy = r.center().y;
        self.fill_rect(if on { left } else { right }, theme::with_alpha(theme::ACTIVE, alpha));
        self.fill_rect(Rect::new(r.x + half - w / 2., r.y, w, r.h), theme::with_alpha(theme::BORDER_2, alpha));
        self.outline(r, 0., w, theme::with_alpha(theme::BORDER_3, alpha));
        let s = r.h * 0.16;
        let mark = |ui: &mut Ui, cx: f32, color: Color, cross: bool| {
            glyph(ui, cx, cy, s, color, cross);
        };
        mark(self, left.center().x, theme::with_alpha(if on { theme::SUCCESS } else { theme::TEXT_CAPTION }, alpha), false);
        mark(self, right.center().x, theme::with_alpha(if on { theme::TEXT_CAPTION } else { theme::DANGER }, alpha), true);
    }

    fn emit_lyon(&mut self, texture: Option<Texture2D>) {
        let gl = unsafe { get_internal_gl() }.quad_gl;
        gl.texture(texture);
        gl.draw_mode(DrawMode::Triangles);
        gl.geometry(&std::mem::take(&mut self.vertex_buffers.vertices), &std::mem::take(&mut self.vertex_buffers.indices));
    }

    pub fn get_matrix(&self) -> Matrix {
        *self.model_stack.last().unwrap()
    }

    pub fn screen_rect(&self) -> Rect {
        Rect::new(-1., -self.top, 2., self.top * 2.)
    }

    pub fn dialog_rect() -> Rect {
        let hw = 0.45;
        let hh = 0.34;
        Rect::new(-hw, -hh, hw * 2., hh * 2.)
    }

    pub fn rect_to_global(&self, rect: Rect) -> Rect {
        let pt = self.to_global((rect.x, rect.y));
        let vec = self.vec_to_global((rect.w, rect.h));
        Rect::new(pt.0, pt.1, vec.0, vec.1)
    }

    pub fn vec_to_global(&self, vec: (f32, f32)) -> (f32, f32) {
        let r = self.model_stack.last().unwrap().transform_vector(&Vector::new(vec.0, vec.1));
        (r.x, r.y)
    }

    pub fn to_global(&self, pt: (f32, f32)) -> (f32, f32) {
        let r = self.model_stack.last().unwrap().transform_point(&Point::new(pt.0, pt.1));
        (r.x, r.y)
    }

    pub fn to_local(&self, pt: (f32, f32)) -> (f32, f32) {
        let r = self
            .model_stack
            .last()
            .unwrap()
            .try_inverse()
            .unwrap()
            .transform_point(&Point::new(pt.0, pt.1));
        (r.x, r.y)
    }

    pub fn dx(&mut self, x: f32) {
        self.model_stack.last_mut().unwrap().append_translation_mut(&Vector::new(x, 0.));
    }

    pub fn dy(&mut self, y: f32) {
        self.model_stack.last_mut().unwrap().append_translation_mut(&Vector::new(0., y));
    }

    #[inline]
    pub fn with<R>(&mut self, model: Matrix, f: impl FnOnce(&mut Self) -> R) -> R {
        let model = self.model_stack.last().unwrap() * model;
        self.model_stack.push(model);
        let res = f(self);
        self.model_stack.pop();
        res
    }

    #[inline]
    pub fn scope<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let model = *self.model_stack.last().unwrap();
        self.model_stack.push(model);
        let res = f(self);
        self.model_stack.pop();
        res
    }

    /// 同 `scope`，但闭包里可以写 `?`：出错时先恢复变换再往上抛。
    #[inline]
    pub fn scope_result<R>(&mut self, f: impl FnOnce(&mut Self) -> Result<R, anyhow::Error>) -> Result<R, anyhow::Error> {
        let model = *self.model_stack.last().unwrap();
        self.model_stack.push(model);
        let res = f(self);
        self.model_stack.pop();
        res
    }

    #[inline]
    pub fn abs_scope<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.model_stack.push(Matrix::identity());
        let res = f(self);
        self.model_stack.pop();
        res
    }

    #[inline]
    pub fn apply<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        unsafe { get_internal_gl() }
            .quad_gl
            .push_model_matrix(nalgebra_to_glm(self.model_stack.last().unwrap()));
        let res = f(self);
        unsafe { get_internal_gl() }.quad_gl.pop_model_matrix();
        res
    }

    pub fn scissor(&mut self, rect: Option<Rect>) {
        let igl = unsafe { get_internal_gl() };
        let gl = igl.quad_gl;
        if let Some(rect) = rect {
            let rect = self.rect_to_global(rect);
            let vp = get_viewport();
            let screen_height = gl
                .get_active_render_pass()
                .map(|it| it.texture(igl.quad_context).height as f32)
                .unwrap_or_else(screen_height);
            let pt = (
                vp.0 as f32 + (rect.x + 1.) / 2. * vp.2 as f32,
                (screen_height - (vp.1 + vp.3) as f32) + (rect.y * vp.2 as f32 / vp.3 as f32 + 1.) / 2. * vp.3 as f32,
            );
            gl.scissor(Some((pt.0 as _, pt.1 as _, (rect.w * vp.2 as f32 / 2.) as _, (rect.h * vp.2 as f32 / 2.) as _)));
        } else {
            gl.scissor(None);
        }
    }

    pub fn text<'s, 'ui>(&'ui mut self, text: impl Into<Cow<'s, str>>) -> DrawText<'a, 's, 'ui> {
        DrawText::new(self, text.into())
    }

    fn clicked(&mut self, rect: Rect, entry: &mut Option<u64>) -> bool {
        let rect = self.rect_to_global(rect);
        let mut exists = false;
        let mut any = false;
        let old_entry = *entry;
        let mut res = false;
        self.ensure_touches().retain(|touch| {
            exists = exists || old_entry == Some(touch.id);
            if !rect.contains(touch.position) {
                return true;
            }
            any = true;
            match touch.phase {
                TouchPhase::Started => {
                    *entry = Some(touch.id);
                    false
                }
                TouchPhase::Moved | TouchPhase::Stationary => {
                    if *entry != Some(touch.id) {
                        *entry = None;
                        true
                    } else {
                        false
                    }
                }
                TouchPhase::Cancelled => {
                    *entry = None;
                    true
                }
                TouchPhase::Ended => {
                    if entry.take() == Some(touch.id) {
                        res = true;
                        false
                    } else {
                        true
                    }
                }
            }
        });
        if res {
            return true;
        }
        if !any && exists {
            *entry = None;
        }
        false
    }

    pub fn accent(&self) -> Color {
        theme::ACCENT
    }

    pub fn background(&self) -> Color {
        theme::SURFACE_1
    }

    pub fn button(&mut self, id: &str, rect: Rect, text: impl Into<String>) -> bool {
        let text = text.into();
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let entry = state.entry(id.to_owned()).or_default();
            self.fill_path(
                &rect.rounded(theme::RADIUS_SM),
                Color {
                    a: if entry.is_some() { 0.5 } else { 1. },
                    ..self.background()
                },
            );
            let ct = rect.center();
            self.text(text)
                .pos(ct.x, ct.y)
                .anchor(0.5, 0.5)
                .max_width(rect.w)
                .size(0.42)
                .color(theme::TEXT)
                .no_baseline()
                .draw();
            self.clicked(rect, entry)
        })
    }

    pub fn checkbox(&mut self, text: impl Into<String>, value: &mut bool) -> Rect {
        let text = text.into();
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let entry = state.entry(format!("chkbox#{text}")).or_default();
            let w = 0.08;
            let s = 0.03;
            let text = self.text(text).pos(w, 0.).size(0.5).no_baseline().draw();
            let r = Rect::new(w / 2. - s, text.center().y - s, s * 2., s * 2.);
            self.fill_rect(r, if *value { self.accent() } else { self.background() });
            let r = Rect::new(r.x, r.y, text.right() - r.x, (text.bottom() - r.y).max(w));
            if self.clicked(r, entry) {
                *value ^= true;
            }
            r
        })
    }

    pub fn input<'b>(&mut self, label: impl Into<String>, value: &mut String, params: impl Into<InputParams<'b>>) -> Rect {
        let label = label.into().to_string();
        let params = params.into();
        let id = format!("input#{label}");
        let r = self.text(label.clone()).anchor(1., 0.).size(0.47).draw();
        let lf = r.x;
        let r = Rect::new(0.02, r.y - 0.01, params.length, r.h + 0.02);
        if if params.password {
            self.button(&id, r, &"*".repeat(value.chars().count()))
        } else {
            self.button(&id, r, value.as_str())
        } {
            request_input_full(&id, value, params.password, label.as_str(), "");
        }
        if let Some((its_id, text)) = take_input() {
            if its_id == id {
                if let Some(changed) = params.changed {
                    *changed = true;
                }
                *value = text;
            } else {
                return_input(its_id, text);
            }
        }
        Rect::new(lf, r.y, r.right() - lf, r.h)
    }

    pub fn slider(&mut self, text: impl Into<String>, range: Range<f32>, step: f32, value: &mut f32, length: Option<f32>) -> Rect {
        let text = text.into();
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let entry = state.entry(text.clone()).or_default();

            let len = length.unwrap_or(0.3);
            let s = 0.002;
            let tr = self.text(format!("{text}: {value:.3}")).size(0.4).draw();
            let cy = tr.h + 0.03;
            let r = Rect::new(0., cy - s, len, s * 2.);
            let p = (*value - range.start) / (range.end - range.start);
            let p = p.clamp(0., 1.);
            self.fill_path(&r.rounded(s), theme::SURFACE_2);
            self.fill_path(&Rect::new(r.x, r.y, len * p, r.h).rounded(s), self.accent());
            self.fill_circle(len * p, cy, 0.015, theme::TEXT);
            let r = r.feather(0.015 - s);
            let r = self.rect_to_global(r);
            self.ensure_touches();
            if let Some(id) = entry {
                if let Some(touch) = self.touches.as_ref().unwrap().iter().rfind(|it| it.id == *id) {
                    let Vec2 { x, y } = touch.position;
                    let (x, _) = self.to_local((x, y));
                    let p = (x / len).clamp(0., 1.);
                    *value = range.start + (range.end - range.start) * p;
                    *value = (*value / step).round() * step;
                    if matches!(touch.phase, TouchPhase::Cancelled | TouchPhase::Ended) {
                        *entry = None;
                    }
                }
            } else if let Some(touch) = self.touches.as_ref().unwrap().iter().find(|it| r.contains(it.position)) {
                if touch.phase == TouchPhase::Started {
                    *entry = Some(touch.id);
                }
            }

            let s = 0.025;
            let mut x = len + 0.02;
            let r = Rect::new(x, cy - s, s * 2., s * 2.);
            self.fill_path(&r.rounded(0.008), self.background());
            self.text("-")
                .pos(r.center().x, r.center().y)
                .anchor(0.5, 0.5)
                .size(0.4)
                .color(theme::TEXT)
                .no_baseline()
                .draw();
            if self.clicked(r, state.entry(format!("{text}:-")).or_default()) {
                *value = (*value - step).max(range.start);
            }
            x += s * 2. + 0.01;
            let r = Rect::new(x, cy - s, s * 2., s * 2.);
            self.fill_path(&r.rounded(0.008), self.background());
            self.text("+")
                .pos(r.center().x, r.center().y)
                .anchor(0.5, 0.5)
                .size(0.4)
                .color(theme::TEXT)
                .no_baseline()
                .draw();
            if self.clicked(r, state.entry(format!("{text}:+")).or_default()) {
                *value = (*value + step).min(range.end);
            }

            Rect::new(0., 0., x + s * 2., cy + s)
        })
    }

    pub fn hgrids(&mut self, width: f32, height: f32, row_num: u32, count: u32, mut content: impl FnMut(&mut Self, u32)) -> (f32, f32) {
        let mut sh = 0.;
        let w = width / row_num as f32;
        for i in (0..count).step_by(row_num as usize) {
            let mut sw = 0.;
            for j in 0..(count - i).min(row_num) {
                content(self, i + j);
                self.dx(w);
                sw += w;
            }
            self.dx(-sw);
            self.dy(height);
            sh += height;
        }
        self.dy(-sh);
        (width, sh)
    }

    pub fn avatar(&mut self, cx: f32, cy: f32, r: f32, c: Color, t: f32, avatar: Result<Option<SafeTexture>, SafeTexture>) -> Rect {
        rounded_rect_shadow(
            self,
            Rect::new(cx - r, cy - r, r * 2., r * 2.),
            &ShadowConfig {
                radius: r,
                base: c.a,
                ..Default::default()
            },
        );
        let rect = Rect::new(cx - r, cy - r, r * 2., r * 2.);
        match avatar {
            Ok(Some(avatar)) => {
                self.fill_circle(cx, cy, r, (*avatar, rect, ScaleType::CropCenter, c));
            }
            Ok(None) => {
                self.loading(
                    cx,
                    cy,
                    t,
                    c,
                    LoadingParams {
                        radius: r * 0.6,
                        width: 0.008,
                        ..Default::default()
                    },
                );
            }
            Err(icon) => {
                self.fill_circle(cx, cy, r, semi_black(c.a * 0.2));
                self.fill_circle(cx, cy, r, (*icon, rect.feather(-0.025), ScaleType::CropCenter, c));
            }
        }
        self.stroke_circle(cx, cy, r, 0.004, c);
        rect
    }

    pub fn loading_path(start: f32, len: f32, r: f32) -> Path {
        use lyon::math::{point, vector, Angle};
        let mut path = Path::svg_builder();
        let pt = |a: f32| {
            let (sin, cos) = a.sin_cos();
            point(sin * r, cos * r)
        };
        path.move_to(pt(-start));
        path.arc(point(0., 0.), vector(r, r), Angle::radians(len), Angle::radians(0.));
        path.build()
    }

    const LOADING_SCALE: f32 = 0.74;
    const LOADING_CHANGE_SPEED: f32 = 3.5;
    const LOADING_ROTATE_SPEED: f32 = 4.1;

    pub fn loading<'b>(&mut self, cx: f32, cy: f32, t: f32, shading: impl IntoShading, params: impl Into<LoadingParams<'b>>) {
        use std::f32::consts::PI;

        let params = params.into();
        let (st, mut len) = if let Some(p) = params.progress {
            (t * Self::LOADING_ROTATE_SPEED, p * PI * 2.)
        } else {
            let ct = t * Self::LOADING_CHANGE_SPEED;
            let round = (ct / (PI * 2.)).floor();
            let st = round * Self::LOADING_SCALE + {
                let t = ct - round * PI * 2.;
                if t < PI {
                    0.
                } else {
                    ((t - PI * 3. / 2.).sin() + 1.) * Self::LOADING_SCALE / 2.
                }
            };
            let st = st * PI * 2. + t * Self::LOADING_ROTATE_SPEED;
            let len = (-ct.cos() * Self::LOADING_SCALE / 2. + 0.5) * PI * 2.;
            (st, len)
        };
        if let Some(last) = params.last {
            len = (*last * 5. + len) / 6.;
            *last = len;
        }
        self.scope(|ui| {
            ui.dx(cx);
            ui.dy(cy);
            ui.stroke_path(&Self::loading_path(st, len, params.radius), params.width, shading);
        });
    }

    #[inline]
    pub fn back_rect(&self) -> Rect {
        Rect::new(-0.965, -self.top + 0.03, 0.07, 0.07)
    }

    #[inline]
    pub fn tab_rects<'b>(&mut self, c: Color, t: f32, it: impl IntoIterator<Item = (&'b mut DRectButton, Cow<'b, str>, bool)>) {
        // 左侧导航栏：比页面底色暗一档，右边一条 1 px 分界（DPR 的侧栏就是这个层次）。
        let rail = Rect::new(-1., -self.top, 0.3, self.top * 2.);
        self.fill_rect(rail, theme::with_alpha(theme::NAV, c.a));
        self.fill_rect(Rect::new(rail.right() - theme::HAIRLINE, rail.y, theme::HAIRLINE, rail.h), theme::with_alpha(theme::BORDER_1, c.a));
        // 整组按钮竖向居中，不贴着顶部排。
        let items: Vec<_> = it.into_iter().collect();
        const H: f32 = 0.032;
        const GAP: f32 = 0.002;
        let total = items.len() as f32 * H + items.len().saturating_sub(1) as f32 * GAP;
        let mut y = -total / 2.;
        for (btn, text, chosen) in items {
            btn.render_nav(self, Rect::new(-0.98, y, 0.26, H), t, c.a, text, chosen);
            y += H + GAP;
        }
    }

    #[inline]
    /// 内容面板：非主页不画大标题，直接占满导航栏右侧的整块屏幕。
    pub fn content_rect(&self) -> Rect {
        Rect::new(-0.7, -self.top, 1.7, self.top * 2.)
    }

    pub fn full_loading<'b>(&mut self, text: impl Into<Cow<'b, str>>, t: f32) {
        self.fill_rect(self.screen_rect(), theme::with_alpha(theme::BG_BASE, 0.72));
        self.loading(0., -0.03, t, theme::ACCENT, ());
        self.text(text.into()).pos(0., 0.05).anchor(0.5, 0.).size(0.6).color(theme::TEXT).draw();
    }

    pub fn full_loading_simple(&mut self, t: f32) {
        self.fill_rect(self.screen_rect(), theme::with_alpha(theme::BG_BASE, 0.72));
        self.loading(0., 0., t, theme::ACCENT, ());
    }

    pub fn main_sub_colors(use_black: bool, alpha: f32) -> (Color, Color) {
        if use_black {
            (semi_black(alpha), Color::new(0.3, 0.3, 0.3, alpha))
        } else {
            (semi_white(alpha), Color::new(0.8, 0.8, 0.8, alpha))
        }
    }
}

pub struct LoadingParams<'a> {
    pub radius: f32,
    pub width: f32,
    pub progress: Option<f32>,
    pub last: Option<&'a mut f32>,
}
impl Default for LoadingParams<'_> {
    fn default() -> Self {
        Self {
            radius: 0.05,
            width: 0.012,
            progress: None,
            last: None,
        }
    }
}
impl From<()> for LoadingParams<'_> {
    fn from(_: ()) -> Self {
        Self::default()
    }
}
impl From<f32> for LoadingParams<'_> {
    fn from(progress: f32) -> Self {
        Self {
            progress: Some(progress),
            ..Self::default()
        }
    }
}
impl<'a> From<(Option<f32>, &'a mut f32)> for LoadingParams<'a> {
    fn from((progress, last): (Option<f32>, &'a mut f32)) -> Self {
        Self {
            progress,
            last: Some(last),
            ..Self::default()
        }
    }
}

fn build_audio() -> AudioManager {
    #[cfg(target_os = "android")]
    {
        use sasa::backend::oboe::*;
        AudioManager::new(OboeBackend::new(OboeSettings {
            performance_mode: PerformanceMode::None,
            sharing_mode: SharingMode::Shared,
            usage: Usage::Media,
            ..Default::default()
        }))
        .unwrap()
    }
    #[cfg(not(target_os = "android"))]
    {
        use sasa::backend::cpal::*;
        AudioManager::new(CpalBackend::new(CpalSettings::default()))
        .unwrap()
        //.expect("Failed to play sound")
    }
}

thread_local! {
    pub static UI_AUDIO: RefCell<AudioManager> = RefCell::new(build_audio());
    pub static UI_BTN_HITSOUND_LARGE: RefCell<Option<Sfx>> = RefCell::new(None);
    pub static UI_BTN_HITSOUND: RefCell<Option<Sfx>> = RefCell::new(None);
    pub static UI_SWITCH_SOUND: RefCell<Option<Sfx>> = RefCell::new(None);
}

pub fn button_hit() {
    if crate::ext::audio_silent() {
        return;
    }
    UI_BTN_HITSOUND.with(|it| {
        if let Some(sfx) = it.borrow_mut().as_mut() {
            let _ = sfx.play(PlaySfxParams::default());
        }
    });
}

pub fn button_hit_large() {
    if crate::ext::audio_silent() {
        return;
    }
    UI_BTN_HITSOUND_LARGE.with(|it| {
        if let Some(sfx) = it.borrow_mut().as_mut() {
            let _ = sfx.play(PlaySfxParams::default());
        }
    });
}

/// 在 `(cx, cy)` 处画一个勾或叉，`s` 是半个笔画的跨度。
fn glyph(ui: &mut Ui, cx: f32, cy: f32, s: f32, color: Color, cross: bool) {
    use lyon::math::point;
    let mut b = Path::builder();
    if cross {
        b.begin(point(cx - s, cy - s));
        b.line_to(point(cx + s, cy + s));
        b.end(false);
        b.begin(point(cx + s, cy - s));
        b.line_to(point(cx - s, cy + s));
        b.end(false);
    } else {
        b.begin(point(cx - s, cy));
        b.line_to(point(cx - s * 0.3, cy + s * 0.7));
        b.line_to(point(cx + s, cy - s * 0.65));
        b.end(false);
    }
    ui.stroke_path(&b.build(), s * 0.42, color);
}

pub fn list_switch() {
    if crate::ext::audio_silent() {
        return;
    }
    UI_SWITCH_SOUND.with(|it| {
        if let Some(sfx) = it.borrow_mut().as_mut() {
            let _ = sfx.play(PlaySfxParams::default());
        }
    });
}
