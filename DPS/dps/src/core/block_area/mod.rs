//! Phigros 4.0 `blockAreaList` —— 剧情遮挡区域（BlockArea）的解析与渲染。
//!
//! 规格来自 `BLOCKAREA_完整文档.md`，要点：
//!
//! * 归一化屏幕空间：`(0, 0)` = 左下角，`(1, 1)` = 右上角，整个可视区就是一个单位正方形；
//! * `time` 单位是**秒**，直接使用，**不做** T(拍) 换算；
//! * 事件只有 `time` 一个时间点，表示“在 `time` 时刻到达目标值”，缓动挂在**前一个**关键帧上
//!   （与 `index/` 的 `eval_xy_track` / `eval_num_track` 一致；官方 phira-docs 说的是挂在
//!   **后一个**关键帧上、且曲线是 `n = (type-1)/3 + 2` 的幂族 —— 那一版曾实现并验证过，
//!   备份在 `临时目录\blockArea_mod_doc_semantics.rs`，本项目最终选择与 `index/` 保持一致）；
//!   `easeTypeX` / `easeTypeY` 分轴独立；`Zero`(13) / `One`(14) 表示“不插值”；
//! * 生命周期 `appear -> enable -> disable -> disappear` 对应
//!   `Ready` / `Active` / `Disabled` 三段材质；Ready 仍然是「`appear` 起线性渐显」，
//!   只有 `enable` 前 `BLOCK_READY_DURATION`（0.5 s）那一小段改成
//!   **亮/原来 交替闪 `BLOCK_READY_BLINKS`（3）下**（不消失），第 4 下正好落在 `enable` 时刻、
//!   随即切成实心的 Active。
//!
//! 渲染走 [`native`]：逐字搬自 `Phira-Pro-main/prpr` 的
//! `block.rs` / `block_mask.rs` / `block_shader.rs` / `block_shader_full.{vert,frag}`。
//! 早先那套按 `index/` 逆向的 WebGL 复刻管线（`gl.rs` + `core/shaders/block_*.glsl`）
//! 已废弃并删除；`BlockAreaStyle::simplified` 的「简化噪域」也共用同一份官方材质，
//! 只是逐帧覆写 uniform 把它降级成纯色。细节见 [`native`] 与 `blockArea/NATIVE_PORT.md`。

mod native;

pub use native::{
    block_touch_blocked, collect as collect_zones, judge_paint_view, render_all as render_native_block_areas, reset_block_effects as reset_native_block_effects,
    touch_inset_world, NativeBlocks, Zone,
};

use super::Resource;
use macroquad::prelude::*;
use serde::Deserialize;

// ---------------------------------------------------------------------------
// 缓动
// ---------------------------------------------------------------------------

/// `EaseType`（16 值枚举）的序号，对应 `easeType` / `easeTypeX` / `easeTypeY` 字段。
pub const EASE_LINEAR: u8 = 0;
pub const EASE_ZERO: u8 = 13;
pub const EASE_ONE: u8 = 14;

/// Ready（闪烁）从 `enable` 之前这么久开始。
pub const BLOCK_READY_DURATION: f64 = 0.5;

/// Ready 期间闪几下；第 `BLOCK_READY_BLINKS + 1` 下正好落在 `enable` 时刻，
/// 也就是方块由闪烁的 Ready 材质切成实心的 Active 材质。
pub const BLOCK_READY_BLINKS: f64 = 3.0;

/// 闪烁的亮度增益（按需求把最大闪烁亮度降到 75%）：窗口里「亮」= 该值、
/// 「原来」= 上一版的渐显值 × 该值 —— 两个电平一起缩放，三下闪烁的对比度保持不变。
pub const BLOCK_READY_BLINK_PEAK: f32 = 0.75;

/// 求 `easeType` 在进度 `t ∈ [0, 1]` 上的缓动值。
///
/// `13 = Zero` 恒返回 0（保持起始值，到关键帧时刻才突变），
/// `14 = One` 恒返回 1（直接取目标值），二者都是“不插值”。
pub fn ease(code: u8, t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    match code {
        0 => t,
        1 => 1. - (t * std::f32::consts::FRAC_PI_2).cos(),
        2 => (t * std::f32::consts::FRAC_PI_2).sin(),
        3 => -((std::f32::consts::PI * t).cos() - 1.) / 2.,
        4 => t * t,
        5 => 1. - (1. - t) * (1. - t),
        6 => {
            if t < 0.5 {
                2. * t * t
            } else {
                1. - (-2. * t + 2.) * (-2. * t + 2.) / 2.
            }
        }
        7 => t * t * t,
        8 => 1. - (1. - t).powi(3),
        9 => {
            if t < 0.5 {
                4. * t * t * t
            } else {
                1. - (-2. * t + 2.).powi(3) / 2.
            }
        }
        10 => t.powi(4),
        11 => 1. - (1. - t).powi(4),
        12 => {
            if t < 0.5 {
                8. * t.powi(4)
            } else {
                1. - (-2. * t + 2.).powi(4) / 2.
            }
        }
        13 => 0.,
        14 => 1.,
        // 15 = AnimationCurve：谱面未提供曲线数据，退化为线性
        _ => t,
    }
}

// ---------------------------------------------------------------------------
// 数据模型
// ---------------------------------------------------------------------------

#[derive(Deserialize, Clone, Copy, Default, Debug)]
pub struct Percentage {
    pub x: f32,
    pub y: f32,
}

impl Percentage {
    pub fn vec(self) -> Vec2 {
        vec2(self.x, self.y)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BlockAreaEvent {
    /// 归一化锚点（旋转 / 缩放中心）
    pub anchor: Vec2,
    /// 到达时刻（秒）
    pub time: f64,
    /// 分轴缓动（旋转事件只用 `x`）
    pub ease_x: u8,
    pub ease_y: u8,
    /// 旋转事件：`x` 为角度；移动事件：目标中心；缩放事件：缩放系数
    pub value: Vec2,
}

impl BlockAreaEvent {
    fn new(anchor: Vec2, time: f64, ease_x: u8, ease_y: u8, value: Vec2) -> Self {
        Self {
            anchor,
            time,
            ease_x,
            ease_y,
            value,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRotateEvent {
    #[serde(default)]
    anchor: Percentage,
    time: f64,
    #[serde(default)]
    ease_type: u8,
    #[serde(default)]
    rotation: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMoveEvent {
    #[serde(default)]
    end_position: Percentage,
    time: f64,
    #[serde(default)]
    ease_type_x: u8,
    #[serde(default)]
    ease_type_y: u8,
}

fn one() -> Percentage {
    Percentage { x: 1., y: 1. }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawScaleEvent {
    #[serde(default)]
    anchor: Percentage,
    time: f64,
    #[serde(default)]
    ease_type_x: u8,
    #[serde(default)]
    ease_type_y: u8,
    #[serde(default = "one")]
    scale: Percentage,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawBlockArea {
    top_right_percentage: Percentage,
    bottom_left_percentage: Percentage,
    appear_time: f64,
    enable_time: f64,
    disable_time: f64,
    disappear_time: f64,
    /// `false` = Normal，`true` = Subtract（挖洞）
    #[serde(default)]
    pub is_subtract: bool,
    #[serde(default)]
    rotate_events: Vec<RawRotateEvent>,
    #[serde(default)]
    move_events: Vec<RawMoveEvent>,
    #[serde(default)]
    scale_events: Vec<RawScaleEvent>,
}

/// 4 个时间点驱动的生命周期相位。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockPhase {
    HiddenBefore,
    /// Ready：`appear` 起渐显，`enable` 前 0.5 s 转为闪烁（Ready 材质）
    Disabled,
    Active,
    /// 淡出中（Disabled 材质）
    DisabledOut,
    HiddenAfter,
}

#[derive(Clone, Debug)]
pub struct BlockArea {
    pub bottom_left: Vec2,
    pub top_right: Vec2,
    pub appear_time: f64,
    pub enable_time: f64,
    pub disable_time: f64,
    pub disappear_time: f64,
    pub is_subtract: bool,
    pub rotate_events: Vec<BlockAreaEvent>,
    pub move_events: Vec<BlockAreaEvent>,
    pub scale_events: Vec<BlockAreaEvent>,
    /// 它在 `blockAreaList` 里的原始序号（从 0 起）
    pub index: usize,
    /// 「噪域优化」归组后的**形状族**（从 0 起）：几何 + 减算标记 + 相对动画都相同的条目共享它。
    ///
    /// 它只用来给「同一个噪域」挑号（见 `slot`），不是印在画面上的号。
    pub group: usize,
    /// 「噪域优化」下**真正印出来的复用号**（从 0 起）。
    ///
    /// 规则：**同一个噪域（形状族）的每一次出现都印同一个号**，而一条噪域彻底消失之后它的号
    /// 立刻可以交给下一个噪域——这就是「复用」。所以号的总数 ≈ 同时活着的噪域数的峰值，而不是
    /// 条目数、也不是噪域总数（`example/pez/ハテ…AT.pez`：14355 条 / 2649 个噪域 → 432 个号，
    /// 同时最多 268 个噪域在屏上 ⇒ 任何「同时不同号」的方案都不可能需要少于 268 个号）。
    ///
    /// 关掉「噪域优化」时印的号是 `index`，打开后印的是这个。
    pub slot: usize,
}

/// `t` 时刻的方块状态（归一化空间）。
#[derive(Clone, Copy, Debug)]
pub struct BlockState {
    pub center: Vec2,
    pub size: Vec2,
    pub rotation: f32,
    pub alpha: f32,
}

#[inline]
fn ratio(v: f64, span: f64) -> f32 {
    if span <= 0. {
        1.
    } else {
        (v / span).clamp(0., 1.) as f32
    }
}

impl BlockArea {
    /// 「强制定位」用的**合成全屏噪域**：谱面里没有（或不想要）任何噪域时，整屏铺一块。
    ///
    /// 几何取归一化空间的整块 `(0,0)-(1,1)`（= chart 空间的 `[-1,1] x [-1/aspect,1/aspect]`，
    /// 正好是可见区域）；四个时间取 `-∞ / -∞ / +∞ / +∞`，于是
    /// [`BlockArea::phase`] 对任何有限时刻都返回 `Active`、[`BlockArea::alpha`] 恒为 1，
    /// 也不会被 `Zone::from_area` 的 `HiddenBefore / HiddenAfter` 过滤掉。
    /// 三类动画事件全空 ⇒ 缩放 `(1,1)`、旋转 0、中心不动。
    ///
    /// `index` / `group` / `slot` 全填 `usize::MAX`：`draw_zone_numbers` 会把 `usize::MAX`
    /// 当成「这块不是谱面条目」而跳过（合成块本来也没有号可印），于是开着「简化噪域文字」
    /// 时不会在屏幕正中顶着一个巨大的「1」。
    pub fn force_position_fullscreen() -> Self {
        Self {
            bottom_left: vec2(0., 0.),
            top_right: vec2(1., 1.),
            appear_time: f64::NEG_INFINITY,
            enable_time: f64::NEG_INFINITY,
            disable_time: f64::INFINITY,
            disappear_time: f64::INFINITY,
            is_subtract: false,
            rotate_events: Vec::new(),
            move_events: Vec::new(),
            scale_events: Vec::new(),
            index: usize::MAX,
            group: usize::MAX,
            slot: usize::MAX,
        }
    }

    pub fn from_raw(raw: RawBlockArea, index: usize) -> Self {
        let mut rotate_events = raw
            .rotate_events
            .into_iter()
            .map(|e| BlockAreaEvent::new(e.anchor.vec(), e.time, e.ease_type, e.ease_type, vec2(e.rotation, 0.)))
            .collect::<Vec<_>>();
        let mut move_events = raw
            .move_events
            .into_iter()
            .map(|e| BlockAreaEvent::new(vec2(0.5, 0.5), e.time, e.ease_type_x, e.ease_type_y, e.end_position.vec()))
            .collect::<Vec<_>>();
        let mut scale_events = raw
            .scale_events
            .into_iter()
            .map(|e| BlockAreaEvent::new(e.anchor.vec(), e.time, e.ease_type_x, e.ease_type_y, e.scale.vec()))
            .collect::<Vec<_>>();
        rotate_events.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
        move_events.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
        scale_events.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
        Self {
            bottom_left: raw.bottom_left_percentage.vec(),
            top_right: raw.top_right_percentage.vec(),
            appear_time: raw.appear_time,
            enable_time: raw.enable_time,
            disable_time: raw.disable_time,
            disappear_time: raw.disappear_time,
            is_subtract: raw.is_subtract,
            rotate_events,
            move_events,
            scale_events,
            index,
            // 归组之前「每条就是它自己的那个噪域」；`group_block_areas` 再把同形状的改写掉。
            group: index,
            slot: index,
        }
    }

    pub fn phase(&self, t: f64) -> BlockPhase {
        if t < self.appear_time {
            BlockPhase::HiddenBefore
        } else if t < self.enable_time {
            BlockPhase::Disabled
        } else if t < self.disable_time {
            BlockPhase::Active
        } else if t < self.disappear_time {
            BlockPhase::DisabledOut
        } else {
            BlockPhase::HiddenAfter
        }
    }

    pub fn alpha(&self, t: f64) -> f32 {
        if t < self.appear_time || t >= self.disappear_time {
            0.
        } else if t < self.enable_time {
            // 上一版行为：`appear -> enable` 整段线性渐显。
            // 唯一改动：最后 `BLOCK_READY_DURATION` 秒换成闪烁（不是「到这时才出现」）。
            if t >= self.enable_time - BLOCK_READY_DURATION {
                self.ready_alpha(t)
            } else {
                ratio(t - self.appear_time, self.enable_time - self.appear_time)
            }
        } else if t < self.disable_time {
            1.
        } else {
            1. - ratio(t - self.disable_time, self.disappear_time - self.disable_time)
        }
    }

    /// `enable` 前 `BLOCK_READY_DURATION` 秒内的闪烁：**不消失**，而是在「亮」与
    /// 「上一版在这时刻的原值（`appear -> enable` 的线性渐显值）」之间交替 ——
    /// 亮 → 原来 → 亮 → 原来 → 亮 → 原来 → `enable`，共三下亮，每一个「亮 + 原来」占 1/6 s。
    /// 两个电平都乘 `BLOCK_READY_BLINK_PEAK`（0.75）：最大亮度比全不透明低 25%，
    /// 同时三下闪烁的对比度与之前完全一致。
    pub fn ready_alpha(&self, t: f64) -> f32 {
        let base = ratio(t - self.appear_time, self.enable_time - self.appear_time);
        let start = (self.enable_time - BLOCK_READY_DURATION).max(self.appear_time);
        if t < start {
            return base;
        }
        let period = (self.enable_time - start) / BLOCK_READY_BLINKS;
        if period <= 0. {
            return BLOCK_READY_BLINK_PEAK;
        }
        let f = ((t - start) / period).fract() as f32;
        if f < 0.5 {
            BLOCK_READY_BLINK_PEAK
        } else {
            base * BLOCK_READY_BLINK_PEAK
        }
    }

    /// 归一化空间下的原始矩形 `(x0, y0, x1, y1)`。
    pub fn base_rect(&self) -> (f32, f32, f32, f32) {
        (
            self.bottom_left.x.min(self.top_right.x),
            self.bottom_left.y.min(self.top_right.y),
            self.bottom_left.x.max(self.top_right.x),
            self.bottom_left.y.max(self.top_right.y),
        )
    }

    /// `t` 时刻的 `(中心, 尺寸, 旋转角, alpha)`；不在生命周期内返回 `None`。
    ///
    /// 与 `index/main.js` 的 `draw_block_mask` 一致：中心取 moveEvents（缺省是矩形中心），
    /// 尺寸 = base size × |scale|，绕自身中心旋转。
    pub fn state(&self, t: f64) -> Option<BlockState> {
        let alpha = self.alpha(t);
        if alpha <= 0. {
            return None;
        }
        let (x0, y0, x1, y1) = self.base_rect();
        let base_center = vec2((x0 + x1) / 2., (y0 + y1) / 2.);
        let base_size = vec2(x1 - x0, y1 - y0);
        let center = eval_events(&self.move_events, t, base_center);
        let scale = eval_events(&self.scale_events, t, Vec2::ONE);
        let rotation = eval_events(&self.rotate_events, t, Vec2::ZERO).x;
        let size = vec2(base_size.x * scale.x.abs(), base_size.y * scale.y.abs());
        if size.x <= 1e-6 || size.y <= 1e-6 {
            return None;
        }
        Some(BlockState {
            center,
            size,
            rotation,
            alpha,
        })
    }

    /// 归一化空间下的轴对齐包围盒，已裁到 `[0, 1]²`。
    pub fn bounds(&self, t: f64) -> Option<(f32, f32, f32, f32)> {
        let st = self.state(t)?;
        let half = st.size / 2.;
        let x0 = (st.center.x - half.x).max(0.);
        let y0 = (st.center.y - half.y).max(0.);
        let x1 = (st.center.x + half.x).min(1.);
        let y1 = (st.center.y + half.y).min(1.);
        if x1 - x0 <= 1e-5 || y1 - y0 <= 1e-5 {
            return None;
        }
        Some((x0, y0, x1, y1))
    }
}

/// 在关键帧序列上求值。事件只有“到达时刻”，缓动挂在**起点**关键帧上
/// （与当前 `index/main.js` 的 `eval_xy_track` / `eval_num_track` 一致：取 `evs[i]`）；
/// 第一个关键帧之前取默认值。
fn eval_events(events: &[BlockAreaEvent], t: f64, default: Vec2) -> Vec2 {
    let Some(first) = events.first() else {
        return default;
    };
    if t < first.time {
        return default;
    }
    let mut i = 0;
    while i + 1 < events.len() && events[i + 1].time <= t {
        i += 1;
    }
    let cur = &events[i];
    let Some(next) = events.get(i + 1) else {
        return cur.value;
    };
    let span = next.time - cur.time;
    let p = if span <= 0. { 1. } else { ((t - cur.time) / span).clamp(0., 1.) as f32 };
    vec2(cur.value.x + (next.value.x - cur.value.x) * ease(cur.ease_x, p), cur.value.y + (next.value.y - cur.value.y) * ease(cur.ease_y, p))
}

// ---------------------------------------------------------------------------
// 外观参数
// ---------------------------------------------------------------------------

/// BlockArea 这一层的对外开关。
///
/// 上游材质参数（`_DisplaceStrength` / `_FillColor` / `_EdgeOpacity` … 那一整组）
/// 现在住在 `native/shader.rs` 的 `FLOATS` / `COLORS` 里，不再由这里持有。
#[derive(Clone)]
pub struct BlockAreaStyle {
    /// 整个 BlockArea 层
    pub enabled: bool,
    /// 「简化噪域」：共用同一份原生材质，只把 uniform 覆写成纯色填充
    pub simplified: bool,
    /// 「噪域优化」：把几何与动画形状相同的条目认成**同一个噪域的多次出现**，
    /// 编号共享（`BlockArea::group`）。只改编号的含义，画面逐像素不变
    pub reuse: bool,
}

impl Default for BlockAreaStyle {
    fn default() -> Self {
        Self {
            enabled: true,
            simplified: false,
            reuse: false,
        }
    }
}

// ---------------------------------------------------------------------------
// 渲染器
// ---------------------------------------------------------------------------

/// BlockArea 这一层的配置持有者。
///
/// 真正的渲染在 `native::render_all`；这里只保留开关，以及 `Chart` 里那个稳定的字段位置
///（`Chart::with_block_area_style`）。
pub struct BlockAreaRenderer {
    pub style: BlockAreaStyle,
}

impl Default for BlockAreaRenderer {
    fn default() -> Self {
        Self { style: BlockAreaStyle::default() }
    }
}

// ---------------------------------------------------------------------------
// 便捷入口
// ---------------------------------------------------------------------------

/// 从谱面 JSON 的 `blockAreaList` 字段解析。
pub fn parse_block_areas(value: Option<&serde_json::Value>) -> Vec<BlockArea> {
    let Some(serde_json::Value::Array(list)) = value else {
        return Vec::new();
    };
    list.iter()
        .enumerate()
        .filter_map(|(i, v)| {
            serde_json::from_value::<RawBlockArea>(v.clone())
                .ok()
                .map(|raw| BlockArea::from_raw(raw, i))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 「噪域优化」：把「同一个噪域」的多次出现归成一组
// ---------------------------------------------------------------------------

/// 一个**可复用的噪域**。「噪域优化」打开后，「简化噪域文字」印的就是它的编号。
///
/// 官方 `blockAreaList` 里每一条都是「方块的某一次出现」——`disappearTime` 一过就彻底没了，
/// 想让它再出现只能再写一条。于是同一张谱常常堆着大量**几何与动画完全一样、只有 4 个时间
/// 不同**的条目（例：`example/pez/ハテ…AT.pez` 有 14355 条，归组后只剩几百个真正不同的噪域）。
///
/// `windows` 是它各次出现的下标（升序，指向 `Chart::block_areas`）。渲染**逐像素不变**：
/// 每个窗口照旧按自己的 4 个时间独立显隐、各自算掩码；变的只是编号的含义——关掉「噪域优化」
/// 时编号是「谱面里的第几条」，打开后是「第几个噪域」。
#[derive(Clone, Debug)]
pub struct BlockGroup {
    /// 噪域编号（从 0 起，按该噪域**首次出现**的先后排）
    pub index: usize,
    /// 各次出现的下标（升序，指向 `Chart::block_areas`）
    pub windows: Vec<usize>,
}

/// 「同一个噪域」的量化粒度：坐标 / 相对时间都乘它再取整，量化成可哈希的整数。
///
/// 归一化空间里坐标是 0~1（可以超出），1e-6 远小于任何可见差异；时间取**相对 `appearTime`**
/// 的秒数，1e-6 s = 1 µs。复制出来的副本通常逐位相同，量化只是为了抹掉 f32 存储的末位噪声——
/// 代价是「只差一两微秒、却正好跨过取整边界」的两条会被算成两个噪域（编号多一个，画面不受影响）。
const SHAPE_QUANT: f64 = 1_000_000.;

#[inline]
fn quantize(v: f64) -> i64 {
    (v * SHAPE_QUANT).round() as i64
}

/// 把一类事件的关键帧按「相对 `appear_time`」写进键。
#[inline]
fn push_event_key(key: &mut Vec<i64>, events: &[BlockAreaEvent], appear_time: f64) {
    key.push(events.len() as i64);
    for e in events {
        key.push(quantize(e.time - appear_time));
        key.push(e.ease_x as i64);
        key.push(e.ease_y as i64);
        key.push(quantize(e.anchor.x as f64));
        key.push(quantize(e.anchor.y as f64));
        key.push(quantize(e.value.x as f64));
        key.push(quantize(e.value.y as f64));
    }
}

impl BlockArea {
    /// 这个方块「长什么样」＝ 几何 + 是否减算 + **以 `appearTime` 归一后的动画关键帧**。
    ///
    /// 刻意**不含** 4 个时间：同一份方块整体挪一段时间再复制一份，4 个时间全变了、
    /// 相对动画却一模一样，那正是「同一个噪域又出现了一次」。
    fn push_shape_key(&self, key: &mut Vec<i64>) {
        key.push(quantize(self.bottom_left.x as f64));
        key.push(quantize(self.bottom_left.y as f64));
        key.push(quantize(self.top_right.x as f64));
        key.push(quantize(self.top_right.y as f64));
        key.push(self.is_subtract as i64);
        push_event_key(key, &self.rotate_events, self.appear_time);
        push_event_key(key, &self.move_events, self.appear_time);
        push_event_key(key, &self.scale_events, self.appear_time);
    }
}

/// 「噪域优化」的两步：先按形状归族（写回 `BlockArea::group`），再给每个噪域分一个可复用的号
/// （写回 `BlockArea::slot`，同一族的每条出现都拿到同一个号）。返回形状族（按首次出现的先后）。
///
/// 形状族编号只能是「首次出现的顺序」（谱面里没有编号字段，官方格式也没有）；真正印出来的是
/// `slot`——一条噪域死掉之后它的号立刻能被后来者接手。
pub fn group_block_areas(areas: &mut [BlockArea]) -> Vec<BlockGroup> {
    use std::collections::hash_map::DefaultHasher;
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};

    // 每个噪域留一份完整键：哈希桶里还要复核一遍，免得碰撞把两个方块并成一个。
    // 遍历时复用同一个 `key` 缓冲区，省得给上万条各分配一遍。
    let mut groups: Vec<BlockGroup> = Vec::new();
    let mut group_keys: Vec<Vec<i64>> = Vec::new();
    let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut key: Vec<i64> = Vec::new();

    for (i, area) in areas.iter_mut().enumerate() {
        key.clear();
        area.push_shape_key(&mut key);
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let candidates = buckets.entry(hasher.finish()).or_default();
        let group = match candidates.iter().copied().find(|g| group_keys[*g] == key) {
            Some(g) => g,
            None => {
                let g = groups.len();
                groups.push(BlockGroup {
                    index: g,
                    windows: Vec::new(),
                });
                group_keys.push(key.clone());
                candidates.push(g);
                g
            }
        };
        groups[group].windows.push(i);
        area.group = group;
    }

    // —— 编号复用：给每个噪域（形状族的全部出现）分一个号，号用完就能被下一个噪域接手 ——
    // 一条噪域 `disappearTime` 一过就彻底没用了（它不会再出现第二次），所以它的号可以交给下一个
    // 噪域用；同一个噪域的多次出现永远共享一个号（哪怕两三次出现同时在屏上——它们本来就是一个
    // 噪域）。于是**号的总数 ≈ 同时存在的噪域数的峰值**，而不是条目数、也不是噪域总数。
    // 约束只有一条「同一时刻活着的两个噪域不同号」，所以按「首次出现的先后」贪心取最小空号即可
    //（噪域的一生是 [最早出现, 最晚消失) 这段区间，区间图的色数 = 最大重叠数）。
    // 时间先量化成微秒，这样「空出来的时刻」能用整数比较，不受浮点抖动影响（与形状键同一把尺子）。
    let mut firsts: Vec<i64> = Vec::with_capacity(groups.len());
    let mut lasts: Vec<i64> = Vec::with_capacity(groups.len());
    for group in &groups {
        firsts.push(
            group
                .windows
                .iter()
                .map(|w| quantize(areas[*w].appear_time))
                .min()
                .unwrap_or(0),
        );
        lasts.push(
            group
                .windows
                .iter()
                .map(|w| quantize(areas[*w].disappear_time))
                .max()
                .unwrap_or(0),
        );
    }

    // 贪心必须**按时间顺序**推进：族本来是按它们在 `blockAreaList` 里第一次被看到的顺序建的，
    // 而谱面列表并不保证时间有序（ハテ AT 里有 281 处 `appearTime` 比上一条更早），照列表顺序扫
    // 就会把编号小的号先发给后来才出现的噪域，先出现的反倒拿到大号。实测 1665/2649 个族的号与
    // 时间顺序下的答案不同（差得最多的一族：列表顺序给 430，时间顺序给 18）——而「复用优先用编号
    // 最小的空号」正是这个功能要说的事，所以先把族按首次出现的先后排好，再顺序扫。
    // 排序键里的 `*g` 保证先后并列（同一时刻首次出现）时结果与谱面顺序一致、可复现。
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by_key(|g| (firsts[*g], *g));
    if order.iter().enumerate().any(|(pos, g)| pos != *g) {
        let mut new_pos = vec![0usize; groups.len()];
        for (pos, g) in order.iter().enumerate() {
            new_pos[*g] = pos;
        }
        let mut sorted: Vec<BlockGroup> = Vec::with_capacity(groups.len());
        for g in &order {
            sorted.push(BlockGroup {
                index: sorted.len(),
                windows: std::mem::take(&mut groups[*g].windows),
            });
        }
        groups = sorted;
        firsts = order.iter().map(|g| firsts[*g]).collect();
        lasts = order.iter().map(|g| lasts[*g]).collect();
        // 族换了位置，`BlockArea::group` 也得跟着改指，否则 `reuse` 时印出来的号会指错族。
        for area in areas.iter_mut() {
            area.group = new_pos[area.group];
        }
    }

    let mut free_at: Vec<i64> = Vec::new();
    for (pos, group) in groups.iter().enumerate() {
        let appear = firsts[pos];
        let disappear = lasts[pos];
        let slot = free_at
            .iter()
            .position(|f| *f <= appear)
            // 一个空号都没有才开新号（此时所有号都被别的噪域占着，非开不可）。
            .unwrap_or_else(|| {
                free_at.push(i64::MIN);
                free_at.len() - 1
            });
        free_at[slot] = disappear;
        for w in &group.windows {
            areas[*w].slot = slot;
        }
    }

    groups
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 示例谱面目录（`example/`）：**只在本机仓库根，不随仓库分发**。
    /// 2026-10-16 工作区下移到 `repository/` 之后它就在 `CARGO_MANIFEST_DIR` 上面若干级，
    /// 所以这里逐级向上找；也可以直接用环境变量 `DPS_EXAMPLE_DIR` 指过去。
    /// 返回 `None` 表示这台机器上没有示例谱面，调用方按「skip」处理。
    fn example_dir() -> Option<std::path::PathBuf> {
        let mut candidates: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(dir) = std::env::var("DPS_EXAMPLE_DIR") {
            if !dir.is_empty() {
                candidates.push(std::path::PathBuf::from(dir));
            }
        }
        let mut cur = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        while let Some(parent) = cur.parent() {
            candidates.push(parent.join("example"));
            cur = parent;
        }
        candidates.into_iter().find(|dir| dir.is_dir())
    }

    /// `example/` 下的一个文件；不存在时返回 `None`。
    fn example_path(rel: &str) -> Option<std::path::PathBuf> {
        let path = example_dir()?.join(rel);
        path.is_file().then_some(path)
    }

    #[test]
    fn ease_zero_one_are_not_interpolation() {
        for t in [0., 0.25, 0.5, 0.75, 1.] {
            assert_eq!(ease(EASE_ZERO, t), 0.);
            assert_eq!(ease(EASE_ONE, t), 1.);
            assert!((ease(EASE_LINEAR, t) - t).abs() < 1e-6);
        }
        // out(t) == 1 - in(1 - t)
        for code in [1u8, 4, 7, 10] {
            for t in [0., 0.13, 0.5, 0.87, 1.] {
                assert!((ease(code + 1, t) - (1. - ease(code, 1. - t))).abs() < 1e-5, "code={code} t={t}");
            }
        }
    }

    #[test]
    fn phase_boundaries() {
        let raw: RawBlockArea = serde_json::from_str(
            r#"{"topRightPercentage":{"x":0.8,"y":0.8},"bottomLeftPercentage":{"x":0.2,"y":0.2},
                "appearTime":1.0,"enableTime":2.0,"disableTime":4.0,"disappearTime":5.0,
                "isSubtract":false,"rotateEvents":[],"moveEvents":[],"scaleEvents":[]}"#,
        )
        .unwrap();
        let b = BlockArea::from_raw(raw, 0);
        assert_eq!(b.phase(0.5), BlockPhase::HiddenBefore);
        assert_eq!(b.phase(1.5), BlockPhase::Disabled);
        assert_eq!(b.phase(3.0), BlockPhase::Active);
        assert_eq!(b.phase(4.5), BlockPhase::DisabledOut);
        assert_eq!(b.phase(5.0), BlockPhase::HiddenAfter);
        // 生命周期之外取不到状态
        assert!(b.state(0.5).is_none());
        assert!(b.state(5.0).is_none());
        // 中心 / 尺寸取自矩形本身
        let st = b.state(3.0).unwrap();
        assert!((st.center - vec2(0.5, 0.5)).length() < 1e-6);
        assert!((st.size - vec2(0.6, 0.6)).length() < 1e-6);
    }

    #[test]
    fn ready_blink_peak() {
        // appear 3.0 → enable 4.0（1 s 渐显，闪过窗里 base 只有 0.5~1.0，峰值可见）
        let raw: RawBlockArea = serde_json::from_str(
            r#"{"topRightPercentage":{"x":0.8,"y":0.8},"bottomLeftPercentage":{"x":0.2,"y":0.2},
                "appearTime":3.0,"enableTime":4.0,"disableTime":8.0,"disappearTime":10.0,
                "isSubtract":false,"rotateEvents":[],"moveEvents":[],"scaleEvents":[]}"#,
        )
        .unwrap();
        let b = BlockArea::from_raw(raw, 0);
        let start = 4.0 - BLOCK_READY_DURATION;
        let period = BLOCK_READY_DURATION / BLOCK_READY_BLINKS;
        // 闪烁前：与上一版一致（线性渐显）
        assert!((b.alpha(3.25) - 0.25).abs() < 1e-4);
        // 「亮」半个周期：alpha = 亮度增益
        for i in 0..3 {
            let t = start + period * i as f64 + 1e-3;
            assert!((b.alpha(t) - BLOCK_READY_BLINK_PEAK).abs() < 1e-4, "第 {} 下亮 t={t}", i + 1);
        }
        // 「原来」半个周期：回到该时刻的渐显值 × 增益，且 > 0（不消失）
        for i in 0..3 {
            let t = start + period * (i as f64 + 0.75);
            let base = (t - 3.0) as f32;
            assert!((b.alpha(t) - base * BLOCK_READY_BLINK_PEAK).abs() < 1e-4, "原来 t={t}");
            assert!(b.alpha(t) > 0.);
        }
        // 三下都要有亮度差（越靠近 enable，「原来」越接近峰值，差自然变小）
        for i in 0..3 {
            let bright = b.alpha(start + period * i as f64 + 1e-3);
            let origin = b.alpha(start + period * (i as f64 + 0.75));
            assert!(bright > origin + 0.02, "第 {} 下没有亮度差 {bright} vs {origin}", i + 1);
        }
        // 第 4 下就是 enable 之后的实心 Active
        assert!((b.alpha(4.001) - 1.).abs() < 1e-6);
    }

    #[test]
    fn parse_example_chart() {
        let Some(path) = example_path("True Home True World/IN.json") else {
            eprintln!("skip: example chart not found");
            return;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!("skip: example chart not found");
            return;
        };
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let areas = parse_block_areas(json.get("blockAreaList"));
        assert_eq!(areas.len(), 48);
        assert_eq!(areas.iter().filter(|a| a.is_subtract).count(), 4);
        assert!(areas.iter().all(|a| a.appear_time > 58. && a.disappear_time < 74.));
        assert!(areas.iter().any(|a| a.alpha(65.) > 0.));
        for a in &areas {
            for i in 0..=150 {
                let t = 58. + i as f64 * 0.1;
                let _ = a.phase(t);
                let _ = a.bounds(t);
                let alpha = a.alpha(t);
                assert!((0. ..=1.).contains(&alpha));
                if let Some(st) = a.state(t) {
                    assert!(alpha > 0.);
                    assert!(st.size.x > 0. && st.size.y > 0.);
                }
            }
        }
    }

    /// 一条 `RawBlockArea` 的 JSON 字面量，方便下面的归组测试拼。
    fn area_json(
        left: f32,
        bottom: f32,
        right: f32,
        top: f32,
        appear: f64,
        enable: f64,
        disable: f64,
        disappear: f64,
        subtract: bool,
        move_events: &str,
    ) -> serde_json::Value {
        serde_json::from_str(&format!(
            r#"{{"topRightPercentage":{{"x":{right},"y":{top}}},"bottomLeftPercentage":{{"x":{left},"y":{bottom}}},
                "appearTime":{appear},"enableTime":{enable},"disableTime":{disable},"disappearTime":{disappear},
                "isSubtract":{subtract},"rotateEvents":[],"moveEvents":[{move_events}],"scaleEvents":[]}}"#
        ))
        .unwrap()
    }

    /// 「噪域优化」：几何与相对动画相同的条目并成一个噪域（共享编号、各有自己的窗口），
    /// 几何 / 相对动画 / 减算标记任一不同就各自成组。
    #[test]
    fn reuse_groups_same_shape_across_windows() {
        let json = serde_json::Value::Array(vec![
            // 0 / 1：同一块方块，只是整体挪了 10 s，动画也整体挪了 10 s ⇒ 同一个噪域
            area_json(
                0.1,
                0.1,
                0.3,
                0.3,
                1.,
                2.,
                3.,
                4.,
                false,
                r#"{"endPosition":{"x":0.7,"y":0.5},"time":1.5,"easeTypeX":1,"easeTypeY":1}"#,
            ),
            area_json(
                0.1,
                0.1,
                0.3,
                0.3,
                11.,
                12.,
                13.,
                14.,
                false,
                r#"{"endPosition":{"x":0.7,"y":0.5},"time":11.5,"easeTypeX":1,"easeTypeY":1}"#,
            ),
            // 2：几何一样，但动画的相对时刻不同 ⇒ 另一个噪域
            area_json(
                0.1,
                0.1,
                0.3,
                0.3,
                21.,
                22.,
                23.,
                24.,
                false,
                r#"{"endPosition":{"x":0.7,"y":0.5},"time":21.9,"easeTypeX":1,"easeTypeY":1}"#,
            ),
            // 3：几何不同 ⇒ 另一个噪域
            area_json(0.1, 0.1, 0.35, 0.3, 31., 32., 33., 34., false, ""),
            // 4：几何相同、无动画，但被标成了减算块 ⇒ 另一个噪域
            area_json(0.1, 0.1, 0.3, 0.3, 41., 42., 43., 44., true, ""),
        ]);
        let mut areas = parse_block_areas(Some(&json));
        assert_eq!(areas.len(), 5);
        assert!(areas.iter().enumerate().all(|(i, a)| a.group == i && a.index == i));

        let groups = group_block_areas(&mut areas);
        assert_eq!(groups.len(), 4);
        assert_eq!(areas[0].group, 0);
        assert_eq!(areas[1].group, 0);
        assert_eq!(groups[0].windows, vec![0, 1]);
        assert_eq!(groups[0].index, 0);
        assert_eq!(areas[2].group, 1);
        assert_eq!(groups[1].windows, vec![2]);
        assert_eq!(areas[3].group, 2);
        assert_eq!(areas[4].group, 3);
        // 编号复用：5 条的出现窗口（1~4 / 11~14 / 21~24 / 31~34 / 41~44）两两不重叠，
        // 所以后一条能直接接手前一条的号 ⇒ 5 次出现只占 1 个号。
        assert_eq!(areas.iter().map(|a| a.slot).collect::<Vec<_>>(), vec![0, 0, 0, 0, 0]);
        // 归组不动原始序号：关掉「噪域优化」时印的还是 1…5
        assert_eq!(areas.iter().map(|a| a.index).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
    }

    /// 归组的键必须与「4 个时间」无关：两份窗口在时间上完全不重叠也能并成一个噪域。
    #[test]
    fn reuse_key_ignores_absolute_time() {
        let json = serde_json::Value::Array(vec![
            area_json(0.2, 0.2, 0.4, 0.4, 0., 1., 2., 3., false, ""),
            area_json(0.2, 0.2, 0.4, 0.4, 100., 101., 102., 103., false, ""),
        ]);
        let mut areas = parse_block_areas(Some(&json));
        let groups = group_block_areas(&mut areas);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].windows, vec![0, 1]);
        // 两个窗口各按自己的时间显隐，互不影响
        assert!(areas[0].alpha(1.5) > 0. && areas[0].alpha(101.5) == 0.);
        assert!(areas[1].alpha(101.5) > 0. && areas[1].alpha(1.5) == 0.);
        // 窗口 0~3 和 100~103 隔得远 ⇒ 共用同一个复用号
        assert_eq!(areas.iter().map(|a| a.slot).collect::<Vec<_>>(), vec![0, 0]);
    }

    /// 复用号的规则只有一条：**同一时刻存在的噪域绝不共号**；一条消失之后号立刻被下一条接手。
    #[test]
    fn reuse_slots_are_recycled_after_disappear() {
        let json = serde_json::Value::Array(vec![
            // A：0~10，占住第一个号
            area_json(0.1, 0.1, 0.2, 0.2, 0., 1., 9., 10., false, ""),
            // B：2~5，与 A 重叠 ⇒ 只能拿另一个号
            area_json(0.3, 0.3, 0.4, 0.4, 2., 3., 4., 5., false, ""),
            // C：11~14，A 已经死透了 ⇒ 捡回 A 的号
            area_json(0.5, 0.5, 0.6, 0.6, 11., 12., 13., 14., false, ""),
            // D：20~23，长得和 A 一样又隔了很久 ⇒ 还是 A 那个号
            area_json(0.1, 0.1, 0.2, 0.2, 20., 21., 22., 23., false, ""),
        ]);
        let mut areas = parse_block_areas(Some(&json));
        group_block_areas(&mut areas);
        // A 和 D 是同一个噪域（形状相同）⇒ 无论隔多久都印同一个号；B 与 A 重叠、C 落在 A 之后但
        // B 的号先空出来 ⇒ C 接手 B 的号（A 的号一直要为 D 留着）。
        assert_eq!(areas.iter().map(|a| a.slot).collect::<Vec<_>>(), vec![0, 1, 1, 0]);

        // 两条噪域同时活着就必须不同号 ⇒ 号的总数 = 最大重叠数（这里是 A+B）
        let slots = areas.iter().map(|a| a.slot).max().unwrap() + 1;
        assert_eq!(slots, 2);
        // 同一个号里的不同噪域，一生（[最早出现, 最晚消失)）不得重叠
        for s in 0..slots {
            let mut spans: Vec<(f64, f64)> = (0..areas.len())
                .filter(|i| areas[*i].slot == s)
                // 同一形状族算一个噪域，先族内合并
                .fold(Vec::<(usize, f64, f64)>::new(), |mut acc, i| {
                    let g = areas[i].group;
                    match acc.iter_mut().find(|(ag, _, _)| *ag == g) {
                        Some(slot) => {
                            slot.1 = slot.1.min(areas[i].appear_time);
                            slot.2 = slot.2.max(areas[i].disappear_time);
                        }
                        None => acc.push((g, areas[i].appear_time, areas[i].disappear_time)),
                    }
                    acc
                })
                .into_iter()
                .map(|(_, a, d)| (a, d))
                .collect();
            spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            for pair in spans.windows(2) {
                assert!(
                    pair[0].1 <= pair[1].0,
                    "第 {} 号被两个噪域同时占着：{:?} 与 {:?}",
                    s,
                    pair[0],
                    pair[1]
                );
            }
        }
    }

    /// 「强制定位」的合成全屏噪域必须「永远 Active、alpha 恒 1、不印编号」——
    /// 否则它会在屏幕上凭空冒出一个巨大的「1」，或者在 `disable_time` 之后整块消失。
    #[test]
    fn force_position_fullscreen_is_always_active_and_unnumbered() {
        let area = BlockArea::force_position_fullscreen();
        assert_eq!(area.bottom_left, vec2(0., 0.));
        assert_eq!(area.top_right, vec2(1., 1.));
        assert!(!area.is_subtract);
        assert!(area.rotate_events.is_empty() && area.move_events.is_empty() && area.scale_events.is_empty());
        for t in [-1e9, -1., 0., 12.5, 1e9] {
            assert_eq!(area.phase(t), BlockPhase::Active);
            assert_eq!(area.alpha(t), 1.);
            assert!(area.bounds(t).is_some());
        }
        // `draw_zone_numbers` 认这个哨兵值 ⇒ 合成块不会印号
        assert_eq!(area.index, usize::MAX);
        assert_eq!(area.group, usize::MAX);
        assert_eq!(area.slot, usize::MAX);
    }

    /// 分号必须**按时间顺序**推进，也就是每个噪域都拿「当时编号最小的空号」——谱面列表
    /// 并不保证时间有序（ハテ AT 里有 281 处 `appearTime` 比上一条更早），照列表顺序扫会把
    /// 小号先发给后来才出现的噪域。这个测试故意把列表顺序倒着写（后出现的排在前面）。
    #[test]
    fn reuse_slots_prefer_smallest_free_number_in_time_order() {
        let json = serde_json::Value::Array(vec![
            // 列表里的第 0 条其实是**最晚**出现的（30~40），挤在 A 的一生里面
            area_json(0.5, 0.5, 0.6, 0.6, 30., 31., 39., 40., false, ""),
            // 第 1 条：10~20，也在 A 的一生里面
            area_json(0.3, 0.3, 0.4, 0.4, 10., 11., 19., 20., false, ""),
            // 第 2 条：0~100，出现最早、活得最久 ⇒ 必须拿 1 号
            area_json(0.1, 0.1, 0.2, 0.2, 0., 1., 99., 100., false, ""),
        ]);
        let mut areas = parse_block_areas(Some(&json));
        let groups = group_block_areas(&mut areas);

        // 族按「首次出现的先后」重排：0~100 的排第一，10~20 的排第二，30~40 的排第三
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].windows, vec![2]);
        assert_eq!(groups[1].windows, vec![1]);
        assert_eq!(groups[2].windows, vec![0]);
        assert_eq!(groups.iter().map(|g| g.index).collect::<Vec<_>>(), vec![0, 1, 2]);
        // 族换了位置，条目的 `group` 必须跟着改指（否则 `reuse` 时印的号会指错族）
        assert_eq!(areas.iter().map(|a| a.group).collect::<Vec<_>>(), vec![2, 1, 0]);

        // 分号：0~100 拿 0 号；10~20 拿 1 号；30~40 出现时 1 号已经空出来了（20 ≤ 30），
        // 所以它拿 1 号而不是接着开 2 号 —— 这就是「优先用编号最小的空号」。
        assert_eq!(areas.iter().map(|a| a.slot).collect::<Vec<_>>(), vec![1, 1, 0]);
        assert_eq!(areas.iter().map(|a| a.slot).max().unwrap() + 1, 2);
    }

    /// 「噪域优化」的实弹检验：示例谱面 `example/pez/ハテ.rNFrums.0.AT.pez` 的 `blockAreaList`
    /// 有 14355 条，归组后「噪域用量」必须掉下来，而且窗口一个不多、一个不少。
    /// （谱面在本机仓库根、不随仓库分发；见 `example_dir`。）
    #[test]
    fn reuse_shrinks_real_chart_block_area_count() {
        let Some(path) = example_path("pez/ハテ.rNFrums.0.AT.pez") else {
            eprintln!("skip: example chart not found");
            return;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skip: example chart not found");
            return;
        };
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let text = {
            use std::io::Read;
            let mut s = String::new();
            zip.by_name("AT.json").unwrap().read_to_string(&mut s).unwrap();
            s
        };
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let mut areas = parse_block_areas(json.get("blockAreaList"));
        assert_eq!(areas.len(), 14355);

        let groups = group_block_areas(&mut areas);
        eprintln!("block areas = {}, groups = {}", areas.len(), groups.len());
        // 每个条目都必须落进某个噪域，且窗口互不重叠、升序
        assert_eq!(groups.iter().map(|g| g.windows.len()).sum::<usize>(), areas.len());
        for g in &groups {
            assert!(g.windows.windows(2).all(|w| w[0] < w[1]));
            assert!(g.windows.iter().all(|w| areas[*w].group == g.index));
        }
        // 同一个噪域的窗口必须「长一个样」。注意判据是**量化后**相同（几何 1e-6 一格），
        // 所以这里只能断言到一格以内，不能要求 f32 逐位相等。
        for g in &groups {
            let first = &areas[g.windows[0]];
            for w in &g.windows {
                let a = &areas[*w];
                assert!((a.bottom_left - first.bottom_left).length() < 1.1e-6);
                assert!((a.top_right - first.top_right).length() < 1.1e-6);
                assert_eq!(a.is_subtract, first.is_subtract);
                assert_eq!(a.rotate_events.len(), first.rotate_events.len());
                assert_eq!(a.move_events.len(), first.move_events.len());
                assert_eq!(a.scale_events.len(), first.scale_events.len());
            }
        }
        // 实测：14355 条 → 2649 个噪域（用量降到 18.5%）。
        // 这个数是「几何 + 是否减算 + 以 appearTime 归一的动画关键帧」这把尺子量出来的，
        // 改尺子（`SHAPE_QUANT` / 键的内容）就得同步改它。
        assert_eq!(groups.len(), 2649);

        // 复用号：一个噪域（形状族的全部出现）永远印同一个号，噪域彻底消失后号才能被下一个接手。
        let slot_count = areas.iter().map(|a| a.slot).max().unwrap() + 1;
        eprintln!("block areas = {}, groups = {}, slots = {}", areas.len(), groups.len(), slot_count);
        for g in &groups {
            let slot = areas[g.windows[0]].slot;
            assert!(g.windows.iter().all(|w| areas[*w].slot == slot), "同一个噪域印了不同的号");
        }
        // 同一个号里的不同噪域，一生（[最早出现, 最晚消失)）不得重叠 —— 不成立就会「两个噪域撞号」
        let mut by_slot: Vec<Vec<(f64, f64)>> = vec![Vec::new(); slot_count];
        for g in &groups {
            let appear = g.windows.iter().map(|w| areas[*w].appear_time).fold(f64::INFINITY, f64::min);
            let disappear = g
                .windows
                .iter()
                .map(|w| areas[*w].disappear_time)
                .fold(f64::NEG_INFINITY, f64::max);
            by_slot[areas[g.windows[0]].slot].push((appear, disappear));
        }
        for (s, spans) in by_slot.iter_mut().enumerate() {
            spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            for pair in spans.windows(2) {
                assert!(pair[0].1 <= pair[1].0 + 1e-6, "第 {} 号被两个噪域同时占着", s);
            }
        }
        // 实测：14355 条 → 2649 个噪域 → **432 个号**（号池只有噪域总数的 16%）。
        // 同时最多有 268 个噪域活着（t≈95.65 s），所以「编号不超过 200」在这张谱上做不到：
        // 只要「同时活着的噪域不同号」，就至少需要 268 个；432 个号的额外开销来自「一个噪域在
        // 它两次出现之间的空档里也占着自己的号」（这样才能保证同一个噪域永远是同一个号）。
        assert_eq!(slot_count, 432);
    }
}
