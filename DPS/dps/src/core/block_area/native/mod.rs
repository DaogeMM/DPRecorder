//! Phira / Phigros 官方「噪域」（BlockArea）渲染 —— 原生实现移植版。
//!
//! 这一份是**直接从 `Phira-Pro-main`（`prpr/src/core/{block.rs, block_shader.rs,
//! block_mask.rs}` 与 `block_shader_full.{vert,frag}`）搬过来的**。
//!
//! 原生实现要点：
//!
//! * **几何**：`scale` / `rotate` 事件绕**自己的 anchor** 发生，已完成段的 anchor 偏移会
//!   逐段累积回中心（`scale_center` / `rotate_center`），move 事件给的是绝对中心、只取相对
//!   原中心的位移。
//! * **缓动**：原生用 101 点采样表 + 线性插值（`eased_progress`），指数族是
//!   `power = (ease-1)/3 + 2`；`13 = HoldStart` 恒 0、`14 = JumpToEnd` 恒 1、无 `15` 档。
//! * **掩码**：CPU 在 1/8 分辨率像素中心栅格化，再按 `EdgeMask` / `GlowMask` 做九宫格膨胀，
//!   打包成一张 `effect` 纹理 + 一张 `aux` 纹理，**不需要任何离屏 FBO**。
//! * **着色**：原版 Unity 反编译片元着色器整份照搬（`assets/blockarea/block_shader_full.frag`），
//!   材质常量用原版值（`_ShineColor` 白、`_ShineSpeed 37.9`、`_ShineBrightness 0.12`）。
//! * **相位**：`Zone.ready = !active && enable-0.5 <= t < enable`，`opacity` 只在
//!   「块本身在 appear 时还没 active」的情况下按 `(t - appear)/0.5` 渐显；这里没有 Ready 闪烁。
//!
//! 触屏光影（hover SDF）与判定吞触摸**不在移植范围**：需要的 `TouchHover.png` 与
//! `FD_Noise_00000.png` 两个素材在本仓库里没有，已按「缺失即退化」处理，见
//! [`HOVER_ASSET`] 与 [`crate::core::block_area::native::shader`]。

use super::next::NextArea;
use super::Resource;
use crate::core::{BlockArea, BlockPhase, Matrix, Vector};
use macroquad::prelude::{draw_rectangle, get_internal_gl, Color, Vec2};
use nalgebra::Rotation2;
use once_cell::sync::Lazy;

/// 官方 `TouchHover.png` 是否随仓库一起提供。
///
/// 仓库里的 `assets/blockarea/TouchHover.png` 是自生成的近似替身（44x44 白色径向衰减，
/// 见该目录 README），官方原图不在备份里。`true` 时才建第 3 张 hover 专用材质、
/// 上传 `_TouchPosCount` 并让 `hoverSample` 真正采样；`false` 时 hover 恒为 0。
const HOVER_ASSET: bool = true;

/// 官方噪域缓动枚举（0..=14）：`0` Linear、`1..=12` In/Out/InOut Quad..Quint、
/// `13` HoldStart、`14` JumpToEnd。
///
/// 与官方一致：用 101 点采样表 + 线性插值，而不是解析式求值 —— 材质里的 `AnimationCurve`
/// 本来就是按 1% 采样烘出来的，两者在中间点上有可见差别。
fn eased_progress(ease: i32, x: f64) -> f32 {
    static TABLES: Lazy<[[f32; 101]; 15]> =
        Lazy::new(|| std::array::from_fn(|ease| std::array::from_fn(|i| easing_sample(ease as i32, i as f32 / 100.))));
    let u = (x as f32).clamp(0., 1.);
    let ease = if (0..=14).contains(&ease) { ease as usize } else { 0 };
    let p = u * 100.;
    let i = (p as usize).min(100);
    if i == 100 {
        TABLES[ease][100]
    } else {
        TABLES[ease][i] + (p - i as f32) * (TABLES[ease][i + 1] - TABLES[ease][i])
    }
}

fn easing_sample(ease: i32, u: f32) -> f32 {
    match ease {
        0 => u,
        1..=12 => {
            let power = (ease - 1) / 3 + 2;
            match (ease - 1) % 3 {
                0 => u.powi(power),
                1 => 1. - (1. - u).powi(power),
                _ => {
                    if u < 0.5 {
                        (1 << (power - 1)) as f32 * u.powi(power)
                    } else {
                        1. - (2. - 2. * u).powi(power) / 2.
                    }
                }
            }
        }
        13 => 0.,
        14 => 1.,
        _ => u,
    }
}

/// 「最后一个 `time <= t` 的下标」；`t` 早于第一个关键帧时返回 `None`
/// （官方 `FindCurrentEventIndex` 返回 `-1`）。
fn last_index<T>(events: &[T], t: f64, time: impl Fn(&T) -> f64) -> Option<usize> {
    events.partition_point(|event| time(event) <= t).checked_sub(1)
}

/// 归一化百分比 → chart 坐标（x 向右、y 向上，全屏 = `[-1, 1] x [-1/aspect, 1/aspect]`）。
#[inline]
fn pct_to_chart(p: Vector, aspect: f32) -> Vector {
    Vector::new(2. * p.x - 1., (2. * p.y - 1.) / aspect)
}

/// `mod.rs` 的数据模型用 macroquad 的 `Vec2` 存归一化坐标，
/// 原生几何解算用继承自 Phira 的 `nalgebra::Vector2`。这里是两种表示之间的唯一桥梁。
#[inline]
fn vec2_to_vector(v: Vec2) -> Vector {
    Vector::new(v.x, v.y)
}

#[inline]
fn norm(cur: f64, next: f64, t: f64) -> f64 {
    if next <= cur {
        1.
    } else {
        (t - cur) / (next - cur)
    }
}

fn scale_center(center: Vector, anchor: Vector, next: Vector, current: Vector) -> Vector {
    // SafeDiv 等价于 Mathf.Approximately(denominator, 0)：缩放为 0 时比值取 1，
    // 而不是 inf / NaN。
    let div = |a: f32, b: f32| if b.abs() < f32::from_bits(8) { 1. } else { a / b };
    anchor + Vector::new(div(next.x, current.x) * (center.x - anchor.x), div(next.y, current.y) * (center.y - anchor.y))
}

fn rotate_center(center: Vector, anchor: Vector, delta: f32) -> Vector {
    anchor + Rotation2::new(delta.to_radians()) * (center - anchor)
}

/// `t` 时刻解算出来的矩形（chart 空间）。
#[derive(Debug, Clone, Copy)]
pub struct BlockTransform {
    pub center: Vector,
    pub size: Vector,
    /// 角度，逆时针为正（chart 空间 y 向上）。
    pub rotation: f32,
}

fn matrix_of(tr: &BlockTransform) -> Matrix {
    Matrix::new_translation(&tr.center)
        * Rotation2::new(tr.rotation.to_radians()).to_homogeneous()
        * Matrix::identity().append_nonuniform_scaling(&tr.size)
}

/// 给 [`BlockArea`] 挂上原生几何解算（用 trait 是为了不改动 `mod.rs` 里的状态机）。
pub trait NativeBlocks: Sized {
    fn scale_at(&self, t: f64) -> Vector;
    fn rotation_at(&self, t: f64) -> f32;
    fn move_center(&self, t: f64, aspect: f32) -> Option<Vector>;
    fn transform(&self, t: f64, aspect: f32) -> BlockTransform;
    fn matrix(&self, t: f64, aspect: f32) -> Matrix;
    fn contains(&self, p: Vector, t: f64, aspect: f32, inset_world: f32) -> bool;
}

impl NativeBlocks for super::BlockArea {
    fn scale_at(&self, t: f64) -> Vector {
        let ev = &self.scale_events;
        if ev.is_empty() {
            return Vector::new(1., 1.);
        }
        match last_index(ev, t, |e| e.time) {
            None => Vector::new(1., 1.),
            Some(i) if i + 1 >= ev.len() => vec2_to_vector(ev[i].value),
            Some(i) => {
                let (cur, next) = (&ev[i], &ev[i + 1]);
                let p = norm(cur.time, next.time, t);
                let px = eased_progress(cur.ease_x as i32, p);
                let py = eased_progress(cur.ease_y as i32, p);
                Vector::new(cur.value.x + px * (next.value.x - cur.value.x), cur.value.y + py * (next.value.y - cur.value.y))
            }
        }
    }

    fn rotation_at(&self, t: f64) -> f32 {
        let ev = &self.rotate_events;
        if ev.is_empty() {
            return 0.;
        }
        match last_index(ev, t, |e| e.time) {
            None => 0.,
            Some(i) if i + 1 >= ev.len() => ev[i].value.x,
            Some(i) => {
                let (cur, next) = (&ev[i], &ev[i + 1]);
                let p = eased_progress(cur.ease_x as i32, norm(cur.time, next.time, t));
                cur.value.x + p * (next.value.x - cur.value.x)
            }
        }
    }

    fn move_center(&self, t: f64, aspect: f32) -> Option<Vector> {
        let ev = &self.move_events;
        if ev.is_empty() {
            return None;
        }
        let p = match last_index(ev, t, |e| e.time) {
            None => return None,
            Some(i) if i + 1 >= ev.len() => vec2_to_vector(ev[i].value),
            Some(i) => {
                let (cur, next) = (&ev[i], &ev[i + 1]);
                let n = norm(cur.time, next.time, t);
                let px = eased_progress(cur.ease_x as i32, n);
                let py = eased_progress(cur.ease_y as i32, n);
                Vector::new(cur.value.x + px * (next.value.x - cur.value.x), cur.value.y + py * (next.value.y - cur.value.y))
            }
        };
        Some(pct_to_chart(p, aspect))
    }

    fn transform(&self, t: f64, aspect: f32) -> BlockTransform {
        let bl = pct_to_chart(vec2_to_vector(self.bottom_left), aspect);
        let tr = pct_to_chart(vec2_to_vector(self.top_right), aspect);
        let base_size = tr - bl;
        let c = (bl + tr) * 0.5;

        let scale = self.scale_at(t);
        let rotation = self.rotation_at(t);
        let mut center = c;
        if let Some(i) = last_index(&self.scale_events, t, |e| e.time) {
            for k in 0..i {
                let cur = &self.scale_events[k];
                center = scale_center(
                    center,
                    pct_to_chart(vec2_to_vector(cur.anchor), aspect),
                    vec2_to_vector(self.scale_events[k + 1].value),
                    vec2_to_vector(cur.value),
                );
            }
            if i + 1 < self.scale_events.len() {
                let cur = &self.scale_events[i];
                center = scale_center(center, pct_to_chart(vec2_to_vector(cur.anchor), aspect), scale, vec2_to_vector(cur.value));
            }
        }
        if let Some(i) = last_index(&self.rotate_events, t, |e| e.time) {
            for k in 0..i {
                let cur = &self.rotate_events[k];
                center = rotate_center(center, pct_to_chart(vec2_to_vector(cur.anchor), aspect), self.rotate_events[k + 1].value.x - cur.value.x);
            }
            if i + 1 < self.rotate_events.len() {
                let cur = &self.rotate_events[i];
                center = rotate_center(center, pct_to_chart(vec2_to_vector(cur.anchor), aspect), rotation - cur.value.x);
            }
        }
        if let Some(m) = self.move_center(t, aspect) {
            center += m - c;
        }

        BlockTransform {
            center,
            size: Vector::new((base_size.x * scale.x).abs(), (base_size.y * scale.y).abs()),
            rotation,
        }
    }

    fn matrix(&self, t: f64, aspect: f32) -> Matrix {
        matrix_of(&self.transform(t, aspect))
    }

    fn contains(&self, p: Vector, t: f64, aspect: f32, inset_world: f32) -> bool {
        if self.phase(t) == BlockPhase::HiddenBefore || self.phase(t) == BlockPhase::HiddenAfter {
            return false;
        }
        let tr = self.transform(t, aspect);
        let Some(inv) = matrix_of(&tr).try_inverse() else {
            return false;
        };
        let lp = inv.transform_point(&crate::core::Point::new(p.x, p.y));
        let sign = if self.is_subtract { 1. } else { -1. };
        let hx = 0.5 + sign * inset_local(tr.size.x.abs(), inset_world);
        let hy = 0.5 + sign * inset_local(tr.size.y.abs(), inset_world);
        lp.x.abs() <= hx && lp.y.abs() <= hy
    }
}

/// 官方 `JudgeControl.maxBlockTouchInsetLocal`。
pub const TOUCH_INSET_LOCAL: f32 = 0.05;
/// 官方 `JudgeControl.blockTouchInsetScreenHeightRatio`（同时也是夹取上界）。
pub const TOUCH_INSET_SCREEN_HEIGHT_RATIO: f32 = 0.25;

/// chart 空间下的触摸内缩量：`maxBlockTouchInsetLocal * screenHeight`
///（屏幕高度在 chart 单位下是 `2 / aspect`）。
#[inline]
pub fn touch_inset_world(aspect: f32) -> f32 {
    TOUCH_INSET_LOCAL * 2.0 / aspect
}

#[inline]
fn inset_local(size: f32, inset_world: f32) -> f32 {
    if size.abs() < 1e-6 {
        TOUCH_INSET_SCREEN_HEIGHT_RATIO
    } else {
        (inset_world / size).abs().clamp(0., TOUCH_INSET_SCREEN_HEIGHT_RATIO)
    }
}

/// 与官方 `JudgeControl.TryGetBlockingBlock` 等价的奇偶规则：点被遮挡
/// ⟺ 完整矩形与内缩矩形上，「有普通块」与「减算块个数为奇」都成立。
///
/// `p` 用 chart 坐标（x 右为正、y 上为正，见 `core::judge` 里把触点 y 取负的那一步）；
/// `core::judge` 每帧用它把落在 `Active` 相位噪域里的触摸从触点列表里摘掉。
///
/// `next` 是 rpe-next v220 噪域（见 [`super::next`]），它自己那套 beat / 画布单位判定
/// 已经解算成同一空间的矩形，所以这里只需在原生结果上再或一次。
pub fn block_touch_blocked(areas: &[BlockArea], next: &[NextArea], p: Vector, t: f64, aspect: f32) -> bool {
    let inset = touch_inset_world(aspect);
    let mut orig_non = false;
    let mut orig_sub = 0u32;
    let mut ins_non = false;
    let mut ins_sub = 0u32;
    for b in areas {
        if b.phase(t) != BlockPhase::Active {
            continue;
        }
        if b.contains(p, t, aspect, 0.) {
            if b.is_subtract {
                orig_sub += 1;
            } else {
                orig_non = true;
            }
        }
        if b.contains(p, t, aspect, inset) {
            if b.is_subtract {
                ins_sub += 1;
            } else {
                ins_non = true;
            }
        }
    }
    let native = (orig_non as u32) != (orig_sub & 1) && (ins_non as u32) != (ins_sub & 1);
    native || super::next::blocked(next, p, t, aspect)
}

mod mask;
mod shader;
mod touch;

use shader::{draw_disabled_zones, draw_zones_with_touches, set_simplified};
pub use shader::{judge_paint_view, reset_block_effects, Zone};

/// 一次把整帧的噪域画出来。
///
/// **层序上的有意偏离**：官方把 Disabled 层画在背景趟（判定线与音符之下）、Active 层画在
/// 精灵/HUD 之后；本仓库把方块整个压在最高层（见 `Chart::render_block_area`
/// 的注释），所以这里像官方 `draw_zones_at` 那样把两层**背靠背画在同一个最高层**：
/// 先 Disabled（`uLayer = 0`，加算），再 Active（`uLayer = 3`，source-alpha 混合）。
/// 着色器、材质、掩码、几何都与官方逐字一致，只有这一处场景层序不同。
///
/// `simplified` 是「简化噪域」：**不再另开一条管线**，而是让同一份官方材质退化
///（`shader::set_fill_style` 覆写一组 uniform）。界面开关走 `res.config.simplified_block_area`，
/// API 走 `BlockAreaStyle::simplified`，两者任一为真都算简化。
///
/// `reuse` 是「噪域优化」：只换「简化噪域文字」印的那个号——印**噪域编号**
///（同一个噪域的多次出现共享它）而不是条目序号。画面逐像素不变。界面开关走
/// `res.config.block_area_reuse`，API 走 `BlockAreaStyle::reuse`，两者任一为真都算。
///
/// `next` 是 rpe-next v220 噪域（[`super::next`]）。它们解算出来的 `Zone` 与原生那批**同层同趟**
/// 画，编号取 `usize::MAX`（不参与「简化噪域文字」的编号）。
pub fn render_all(
    res: &mut Resource,
    blocks: &[BlockArea],
    next: &[NextArea],
    time: f64,
    flip_x: bool,
    simplified: bool,
    reuse: bool,
    touches: &[(u64, Vector)],
) {
    let aspect = res.aspect_ratio;
    let mut zones = collect(blocks, time, aspect, reuse);
    // rpe-next 的块必须并进同一趟：早退条件只看 `zones`，单独渲染会让「只有 rpe-next 噪域的
    // 谱面」整层画不出来。
    zones.extend(super::next::zones(next, time, aspect));
    // 「反向强制定位」：上游（`Chart::render_block_area`）此时传的是**空**列表 —— 不铺全屏
    // 噪域；屏幕上唯一该出现噪域的地方就是每个按键周围那圈矩形，于是把它们当成**真方块**
    // 推进 `zones`（`judge_cover = 0` ⇒ 照常进层、照常被绘制；编号取 `usize::MAX` ⇒ 不印号）。
    // 必须赶在下面的 `zones.is_empty()` 早退**之前**：这时 `zones` 十有八九就是空的。
    //
    // 「纯配置谱面」勾了按键噪域时走同一条路：转换阶段把判定时刻落在噪域里的按键标了出来，上游
    // 同样传空列表，于是屏幕上只剩这些按键周围的矩形是噪域。
    //
    // 纯配置模式下「强制定位」那一支不参与：它要靠全屏噪域衬托，而纯配置模式把整层噪域关掉了，
    // 参与进来只会把四边形本身当噪域铺满画面（每个按键周围一块）。
    let pure_config_mode = res.config.pure_config_chart;
    let invert = (pure_config_mode && res.config.pure_config_chart_noise)
        || (!pure_config_mode && res.config.block_area_force_position && res.config.block_area_force_invert);
    if invert {
        let quads = res.force_position_quads.borrow();
        push_force_zones(&quads, &mut zones);
    }
    if zones.is_empty() {
        return;
    }
    set_simplified(simplified);
    // 谱面开了水平镜像时，方块也要跟着镜像。掩码栅格化与 `fieldUV` 都按 chart 空间算，
    // 所以把镜像折进 zone（x -> -x、angle -> -angle），而不是去翻场。
    if flip_x {
        for z in &mut zones {
            z.center.x = -z.center.x;
            z.angle = -z.angle;
        }
    }
    // 「强制定位」：这一帧每个按键周围的长方形同样变成**合成块**（覆盖度写满 1.0），
    // 于是全屏噪域在那里被整块挖掉 —— 「除了按键，哪里都是噪域」。消费完同样清空。
    // 反向模式下这些四边形已经在上面变成方块本体了，这里不再挖洞。
    // （判定范围**不**在这里挖洞：它由 `judge_range.glsl` 自己让位给噪域 —— 噪域那块矩形
    // 保持不透明，判定范围在重合处淡出，方向与上游一致。）
    if !invert {
        let quads = res.force_position_quads.borrow();
        push_cover_zones(&quads, &mut zones);
    }
    res.force_position_quads.borrow_mut().clear();
    draw_disabled_zones(res, aspect, &zones);
    // 「噪域打击效果」：把这一帧被噪域吞掉的触点交给 hover SDF 画光晕
    //（`block_area_hover` 关掉时上游传空列表，这里自然什么都不画）。
    let touches = if res.config.block_area_hover { touches } else { &[] };
    draw_zones_with_touches(res, aspect, &zones, touches, flip_x);
    if simplified && res.config.simplified_block_area_text {
        draw_zone_numbers(
            aspect,
            &zones,
            res.config.simplified_block_area_text_color,
            res.config.simplified_block_area_text_pale_color,
            res.block_area_chart_scale,
        );
    }
}

/// 把一批 `(四个角点, 覆盖度)` 折成**合成块**推进 `zones`（「强制定位」的
/// `force_position_quads` 走这里；判定范围不走这条 —— 它是自己让位给噪域）。
///
/// 四个角点来自 `Note::draw_force_position_hole` 的 `world_to_screen`，那只过模型栈、不含相机，
/// 所以**已经是 chart 坐标** —— 矩形怎么旋转、平移都由角点自己带过来，这里只需反解
/// 出中心 / 半尺寸 / 旋转角，不需要任何额外适配。
fn push_cover_zones(quads: &[([[f32; 2]; 4], f32)], zones: &mut Vec<Zone>) {
    push_quad_zones(quads, zones, true)
}

/// 「反向强制定位」：同样那批四边形，但折成**会画出来的**方块本身（`judge_cover = 0`）。
/// 上游此时传的是空方块列表，所以屏幕上只有这些矩形是噪域。
fn push_force_zones(quads: &[([[f32; 2]; 4], f32)], zones: &mut Vec<Zone>) {
    push_quad_zones(quads, zones, false)
}

/// 把四个角点反解成中心 / 半尺寸 / 旋转角（角点来自 `Note::draw_force_position_hole`
/// 与 `Note::draw_judge_range`，过完模型栈、已经是 chart 坐标）。`cover = true` 得到只往
/// 掩码覆盖度位图里打点的合成块（挖洞用），`false` 得到普通噪域方块（反向强制定位用）。
fn push_quad_zones(quads: &[([[f32; 2]; 4], f32)], zones: &mut Vec<Zone>, cover: bool) {
    for (p, alpha) in quads.iter() {
        let (ex, ey) = (p[1][0] - p[0][0], p[1][1] - p[0][1]);
        let (fx, fy) = (p[3][0] - p[0][0], p[3][1] - p[0][1]);
        let half = Vector::new((ex * ex + ey * ey).sqrt() * 0.5, (fx * fx + fy * fy).sqrt() * 0.5);
        if half.x <= 0. || half.y <= 0. {
            continue;
        }
        zones.push(Zone {
            center: Vector::new((p[0][0] + p[2][0]) * 0.5, (p[0][1] + p[2][1]) * 0.5),
            half,
            angle: ey.atan2(ex),
            invert: false,
            active: true,
            ready: false,
            before_enable: false,
            opacity: 1.,
            index: usize::MAX,
            // 「判定范围让位」按**整块**擦：`judge_cover` 只有 0 / 255 两个值，掩码里就
            // 不会出现中间灰度，`Masks::render_rings` 便一直走位图膨胀那条便宜路径。
            // 这一点是性能上的硬要求：只要掩码里有一个像素不是 0/255，它就会退化成
            // `render_gray_rings` 的逐像素 3×3 灰度膨胀，1080p 下实测 79 ms/帧
            //（噪域与判定范围同时打开只有 15 FPS，关掉判定范围就有 80 FPS，差的就是它）。
            // 视觉上也正是注释里写的「从噪域里擦出一条干净的矩形」：原先按 `alpha`
            // 打折（判定范围默认 alpha 0.1 ⇒ 只擦掉 10%），噪域照样糊在判定范围上。
            // 顺带避开哨兵问题：`mask.rs` 用 `> 0` 判断「这是不是合成块」，而判定范围
            // 淡入时 `alpha` 会趋近 0（`judge_range_alpha` 的 `base * (show - dt) / (show - fade)`，
            // 默认参数下 `dt > 0.9863`），取整成 0 会让整条四边形被当成**普通方块**画成噪域。
            judge_cover: if cover && *alpha > 0. { 255 } else { 0 },
            // 合成块只挖洞、不绘制，没有颜色可言。
            color: None,
        });
    }
}

/// 字模的宽 × 高（格数）。5×7 已是可读性的甜点：再高就要开始画衬线细节，
/// 而这里的字形最小只有几个像素高，多出来的行只会糊成一团。
const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;

/// 5×7 点阵数字，每行 5 个 bit，最高位是最左列（`row & (1 << (GLYPH_W - 1 - c))`）。
const DIGIT_ROWS: [[u8; GLYPH_H]; 10] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110], // 0
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // 1
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111], // 2
    [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110], // 3
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010], // 4
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110], // 5
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110], // 6
    [0b11111, 0b10001, 0b00001, 0b00010, 0b00100, 0b00100, 0b00100], // 7
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110], // 8
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100], // 9
];
/// 「简化噪域文字」：把噪域编号（从 1 开始）印在方块正中心，disable 相位白字、enable 相位黑字。
///
/// **故意不用 `ui.text()`**：实测在方块图层画完之后调文字 API（`DrawText` →
/// `TextPainter::submit` 那套裸 `glBindTexture` / `glTexSubImage2D` 字形上传），
/// 整块 non-subtract 噪域会整片消失；同一位置改用 `draw_rectangle` 则方块安然无恙
///（连帧末再画也一样抹掉，所以不是绘制顺序问题）。这里因此自己用 5×7 点阵方块拼数字。
///
/// 坐标：`draw_layer_at` 用来铺全屏四边形的那套 `chart_to_base` 把 chart 空间的
/// `(x, y)` 送到基础空间的 `(x / proj_x, y * aspect / proj_y)`，`draw_rectangle` 用的
/// 也是同一套基础空间，所以这里沿用同一个投影反解，换分辨率、换 `chartRatio` 都不必改代码。
/// `k` 是 `Resource::block_area_chart_scale`（「谱面缩放」适配系数，见 `shader::draw_layer_at`）：
/// 方块本体乘了它，编号的位置与字号也必须乘同一个系数，否则谱面一缩放编号就和方块分家。
///
/// 印的号取自 `Zone::index`：关掉「噪域优化」时是条目序号，打开后是噪域编号
///（`collect` 决定），这里不再分辨。
fn draw_zone_numbers(aspect: f32, zones: &[Zone], active_color: [f32; 3], pale_color: [f32; 3], k: [f32; 2]) {
    let (proj_x, proj_y) = {
        let gl = unsafe { get_internal_gl() };
        let proj = gl.quad_gl.get_projection_matrix();
        (proj.x_axis.x, proj.y_axis.y)
    };
    if proj_x.abs() <= f32::EPSILON || proj_y.abs() <= f32::EPSILON {
        return;
    }
    // 两个视口长宽比相同的时候 `k[0] == k[1]`（常态）；真不一致时按各自轴缩放，字模不至于被拉扁。
    let (kx, ky) = (k[0].abs().max(f32::EPSILON), k[1].abs().max(f32::EPSILON));
    let (cell_x_k, cell_y_k) = (kx, ky);
    for z in zones.iter().filter(|z| z.opacity > 0. && z.index != usize::MAX) {
        let cx = z.center.x * kx / proj_x;
        let cy = z.center.y * ky * aspect / proj_y;
        // 格子边长跟着方块高度走（数字共 5×7 格），并卡上下限。
        let height = (z.half.y * 2. * aspect / proj_y * cell_y_k).abs();
        // 字号跟着方块高度走（`height` 里已经乘过 `k`），但上下限也得跟着 `k` 缩 ——
        // 否则谱面缩到 0.53 时所有编号都被 `0.006` 这条下限顶住、不再变小。
        let kk = cell_y_k.abs().clamp(0.05, 1.);
        // 下限是「小方块也必须有编号」的保证：**不要**在这里加小方块跳过分支 ——
        // 高度不到屏幕 5% 上下的小噪域会**一个数字都不印**，玩家根本看不出它是几号。
        // 这里只把字号压到下限，照样画出来：字可能比方块本身还大，
        // 但编号看得见，比「什么都不印」有用。
        let cell = (height * 0.115).clamp(0.004 * kk, 0.02 * kk);
        let cell_x = cell * cell_x_k / cell_y_k;
        // disable 相位淡色字、enable 相位主色字。
        let rgb = if z.active { active_color } else { pale_color };
        let color = Color::new(rgb[0], rgb[1], rgb[2], 1.);
        let text = (z.index + 1).to_string();
        // 字距 1 格：每字前进 `GLYPH_W + 1` 格，整串再减掉最后一个字距。
        let advance = cell_x * (GLYPH_W + 1) as f32;
        let total_w = advance * text.len() as f32 - cell_x;
        let x0 = cx - total_w * 0.5;
        // 噪域那台相机的 `zoom.y` 是**正**的（见 `scene\game.rs` 里那段注释），也就是基础空间
        // 的 y 轴朝上，所以字模的第 0 行要画在「基础 y 更大」的一侧，否则数字整个上下镜像。
        let y_top = cy + cell * GLYPH_H as f32 * 0.5;
        for (i, ch) in text.bytes().enumerate() {
            let digit = (ch - b'0') as usize;
            let Some(rows) = DIGIT_ROWS.get(digit) else { continue };
            let dx = x0 + advance * i as f32;
            for (r, row) in rows.iter().enumerate() {
                for c in 0..GLYPH_W {
                    if row & (1 << (GLYPH_W - 1 - c)) != 0 {
                        draw_rectangle(
                            dx + c as f32 * cell_x,
                            y_top - (r + 1) as f32 * cell,
                            cell_x,
                            cell,
                            color,
                        );
                    }
                }
            }
        }
    }
}

/// 把谱面里的方块按 `t` 解算成 `Zone`。
///
/// `Zone::index` 记的是它在**谱面 `blockAreaList` 里的原始序号**（`BlockArea::index`，不是过滤后的
/// 位置），「简化噪域文字」印的就是这个号。`reuse` 是「噪域优化」：改成印 `BlockArea::slot`，
/// 也就是**复用号**——同一时刻存在的噪域一定不同号，一条消失后它的号立刻被下一条复用。
pub fn collect(blocks: &[BlockArea], t: f64, aspect: f32, reuse: bool) -> Vec<Zone> {
    // 噪域谱面一帧要扫几十万条（60 s 的 720p 噪域谱就有 85 万条），单线程扫一遍就是一个核
    // 跑满整帧。这里按条数分片并行：`par_iter` 的 `filter_map` + `collect` **保持原顺序**，
    // 所以 `zones` 的次序与串行逐条过滤完全一致，后续栅格化/合成的结果逐像素不变。
    // 条数少时（普通谱面几十到几千条）起线程池反而更贵，仍走串行的那条路。
    let keep = |b: &BlockArea| {
        let index = if reuse { b.slot } else { b.index };
        Zone::from_area(b, t, aspect).map(|zone| Zone { index, ..zone })
    };
    if blocks.len() >= PARALLEL_COLLECT_MIN {
        use rayon::prelude::*;
        blocks.par_iter().filter_map(keep).collect()
    } else {
        blocks.iter().filter_map(keep).collect()
    }
}

/// [`collect`] 从多少条起改用 rayon 并行。低于它时线程池的唤醒与合并比扫描本身还贵。
const PARALLEL_COLLECT_MIN: usize = 4096;
