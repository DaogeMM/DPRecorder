//! rpe-next v220 噪域：`blockAreaList` 里字段是 `bottomLeft` / `topRight` 的那一类。
//!
//! 与原生百分比噪域（父模块的 [`super::BlockArea`]）刻意分开，因为两者的输入空间不同：
//! * 时间：rpe-next 用**全局 beat**，靠谱面 `BPMList` 换算成秒（[`Tempo`]）；原生用秒。
//! * 坐标：rpe-next 用 RPE 1350x900 画布的**画布单位**，原点在画布中心，x ∈ [-675, 675]、
//!   y ∈ [-450, 450]；原生用 (0,0)~(1,1) 的归一化百分比。
//! * 形状：rpe-next 由 `bottomLeft` / `topRight` 定矩形，横移 / 纵移 / 旋转 / 横缩放 / 纵缩放
//!   是五条**独立轨道**，事件可带 1..29 号标准缓动或 bezier；原生只有三类事件。
//!
//! 换算：一个画布单位在 chart 空间里是 `UNIT = 2 / 1350`（x 半宽 1 对应 675 单位）。
//! 纵向要再乘 `1.5 / aspect`：画布纵向 450 单位、chart 空间纵向半高是 `1 / aspect`，
//! 而 `450 * UNIT * (1.5 / aspect) == 1 / aspect`。这等价于参考实现 `Zone::y_scale` 的作用，
//! 本模块把它**折进** `Zone::center.y` 与 `Zone::half.y` —— DPS 的 [`Zone`] 没有 `y_scale`，
//! 掩码与着色器都只认 chart 空间里绕中心旋转的矩形。
//!
//! 精度：解析、缓动、bezier 求根、仿射矩阵合成全部在 `f64` 里做（与参考实现逐位一致），
//! 只在最后产出 [`Zone`] 时舍入到 `f32`；这个舍入在 chart 空间里远小于一个像素。
//!
//! 已知偏差：参考实现是「先在画布空间旋转、再整体纵向压扁」，`aspect != 1.5` 时那是平行四边形；
//! [`Zone`] 只能表达旋转矩形，所以这里按「chart 空间里绕中心旋转的矩形」处理。轴对齐矩形在
//! 任何 aspect 下都完全一致，`aspect == 1.5` 时与参考实现完全一致，其余情况旋转块有
//! `O(1 - 1.5 / aspect)` 的二阶偏差；渲染与遮挡判定用同一套几何，二者始终自洽。

use super::native::touch_inset_world;
use crate::core::{Vector, Zone};
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use tracing::warn;

/// 时间比较容差。beat 换算成秒会有累加误差，边界判定一律用 `time + EPS` 比较，
/// 免得同一时刻因为误差被判成「还没到」。
const EPS: f64 = 1e-9;
/// RPE 1350x900 画布单位 → chart 空间的换算系数（x 方向）。
const UNIT: f64 = 2. / 1350.;

/// 画布单位下的一个点 / 一角的坐标。
#[derive(Clone, Copy, Debug, Deserialize)]
struct Position {
    x: f64,
    y: f64,
}

/// RPE 用 `[整数, 分子, 分母]` 表示 beat，`value() = 整数 + 分子 / 分母`，允许负数。
#[derive(Clone, Copy, Deserialize)]
struct Beat([i64; 3]);

impl Beat {
    fn value(self) -> Result<f64> {
        ensure!(
            self.0[2] > 0 && self.0.iter().all(|v| v.unsigned_abs() <= 9_007_199_254_740_991),
            "invalid beat fraction"
        );
        Ok(self.0[0] as f64 + self.0[1] as f64 / self.0[2] as f64)
    }
}

/// 谱面 BPM 表 → beat 到秒的换算（`beat, bpm, 该段起点的秒` 三元组）。
///
/// 独立于判定线自己的 BPM（那条要除 `bpmFactor`，这里不除），也不叠谱面音频 offset：
/// 谱面 offset 已经在 `res.time` 里减掉了，所以第一条 BPM 从 beat 0 起算即可。
pub struct Tempo(Vec<(f64, f64, f64)>);

impl Tempo {
    pub fn new(entries: impl IntoIterator<Item = (f64, f64)>) -> Result<Self> {
        let mut entries: Vec<_> = entries.into_iter().collect();
        ensure!(!entries.is_empty(), "empty BPMList");
        ensure!(
            entries.iter().all(|(beat, bpm)| beat.is_finite() && bpm.is_finite() && *bpm > 0.),
            "invalid BPMList"
        );
        entries.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut result = Vec::with_capacity(entries.len());
        // 第一段从 beat 0 起算，所以第一项之前若还有负 beat，秒数自然是负的。
        let mut seconds = entries[0].0 * 60. / entries[0].1;
        for (i, &(beat, bpm)) in entries.iter().enumerate() {
            if i != 0 {
                seconds += (beat - entries[i - 1].0) * 60. / entries[i - 1].1;
            }
            result.push((beat, bpm, seconds));
        }
        Ok(Self(result))
    }

    /// `beat` 时刻在第几秒（相对谱面起点）。
    pub fn seconds(&self, beat: f64) -> f64 {
        let i = self.0.partition_point(|e| e.0 <= beat).saturating_sub(1);
        let (b, bpm, sec) = self.0[i];
        sec + (beat - b) * 60. / bpm
    }

    fn time(&self, beat: Beat) -> Result<f64> {
        Ok(self.seconds(beat.value()?))
    }
}

fn one() -> f64 {
    1.
}

fn linear() -> u8 {
    1
}

fn points() -> [f64; 4] {
    [0., 0., 1., 1.]
}

/// 布尔开关，允许写成 `true` / `false` 或 `0` / `1`，缺省为假。
#[derive(Clone, Copy, Default, Deserialize)]
#[serde(untagged)]
enum Flag {
    #[default]
    Absent,
    Bool(bool),
    Number(u8),
}

impl Flag {
    fn get(self) -> Result<bool> {
        match self {
            Self::Absent => Ok(false),
            Self::Bool(v) => Ok(v),
            Self::Number(v) => {
                ensure!(v <= 1, "flag must be 0/1 or boolean");
                Ok(v == 1)
            }
        }
    }
}

/// `readyFlash`：`true` / 缺省 = 0.5 秒，`false` = 0，数字 = 该秒数（取到毫秒）。
#[derive(Default, Deserialize)]
#[serde(untagged)]
enum Ready {
    #[default]
    Absent,
    Bool(bool),
    Seconds(f64),
}

impl Ready {
    fn seconds(self) -> f64 {
        match self {
            Self::Absent | Self::Bool(true) => 0.5,
            Self::Bool(false) => 0.,
            Self::Seconds(v) => (v.max(0.) * 1000.).round() / 1000.,
        }
    }
}

/// 一条动画事件：在 `startTime..endTime` 之间把某个量从 `start` 变到 `end`。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Event {
    start_time: Beat,
    end_time: Beat,
    start: f64,
    end: f64,
    /// 1..29，见 [`curve`]；缺省线性。
    #[serde(default = "linear")]
    easing_type: u8,
    /// 缓动只在这个归一化区间 `[easing_left, easing_right]` 内取值，其余部分夹住。
    #[serde(default)]
    easing_left: f64,
    #[serde(default = "one")]
    easing_right: f64,
    #[serde(default)]
    bezier: Flag,
    /// `[x1, y1, x2, y2]`，三次 bezier 的两个控制点（端点固定为 (0,0)/(1,1)）。
    #[serde(default = "points")]
    bezier_points: [f64; 4],
    /// 瞬变事件：要求 `start == end`。
    #[serde(default)]
    inst: Flag,
    /// 旋转 / 缩放绕哪个画布点做；横移纵移轨道不允许带。
    anchor: Option<Position>,
    /// 合成链用：本事件的变换从哪个值「接着」算，缺省用 `start` / `end`。
    transform_start: Option<f64>,
    transform_end: Option<f64>,
    /// 零时长的累积步进，同一时刻可链多条。
    #[serde(default)]
    transform_step: bool,
}

/// 一个生效区间：只有落在它里面才计入遮挡，区间开始前 `readyFlash` 秒是闪烁窗口。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Interval {
    start_time: Beat,
    end_time: Beat,
    #[serde(default)]
    ready_flash: Ready,
}

/// 一条 rpe-next 噪域的原始数据（`blockAreaList` 里的一个元素）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AreaData {
    bottom_left: Position,
    top_right: Position,
    appear_time: Beat,
    disappear_time: Beat,
    /// 减算块：不画外观、只从遮挡里「减掉」一块（奇偶规则里算一次反转）。
    #[serde(default)]
    is_invert: bool,
    #[serde(default)]
    active_intervals: Vec<Interval>,
    #[serde(default)]
    move_x_events: Vec<Event>,
    #[serde(default)]
    move_y_events: Vec<Event>,
    #[serde(default)]
    rotate_events: Vec<Event>,
    #[serde(default)]
    scale_x_events: Vec<Event>,
    #[serde(default)]
    scale_y_events: Vec<Event>,
}

/// 齐次仿射矩阵（`f64`，与参考实现一致）。
type Affine = nalgebra::Matrix3<f64>;

#[derive(Clone, Copy)]
enum Axis {
    /// 绝对位移（横移 / 纵移）：不做矩阵合成，只把采样值当成坐标。
    Move,
    X,
    Y,
    Rotate,
}

impl Axis {
    /// 旋转 / 缩放要绕 `anchor` 做，所以必须有锚点；位移不需要。
    fn anchored(self) -> bool {
        !matches!(self, Self::Move)
    }

    /// 从 `from` 变到 `to` 这一步的仿射（绕 `anchor`）。
    fn relative(self, from: f64, to: f64, anchor: Position) -> Affine {
        let mut m = Affine::identity();
        match self {
            Self::Move => {}
            Self::Rotate => {
                let (s, c) = (to - from).to_radians().sin_cos();
                m[(0, 0)] = c;
                m[(0, 1)] = s;
                m[(1, 0)] = -s;
                m[(1, 1)] = c;
                m[(0, 2)] = anchor.x - c * anchor.x - s * anchor.y;
                m[(1, 2)] = anchor.y + s * anchor.x - c * anchor.y;
            }
            _ => {
                // 缩放用前后比值：`from == 0` 或结果非有限时退化成不变（不做除法）。
                let ratio = to / from;
                let scale = if from == 0. || !ratio.is_finite() { 1. } else { ratio };
                let i = usize::from(matches!(self, Self::Y));
                m[(i, i)] = scale;
                m[(i, 2)] = (if i == 0 { anchor.x } else { anchor.y }) * (1. - scale);
            }
        }
        m
    }
}

struct TimedEvent {
    data: Event,
    /// `startTime` / `endTime` 换算成秒后的结果。
    start: f64,
    end: f64,
    /// 本事件开始前的累积仿射（前面的锚定事件都已结算进去）。
    prefix: Affine,
}

/// 一条轨道：同一类事件按时序排好，采样出某个时刻的「值 + 正在跑的事件」。
struct Track {
    events: Vec<TimedEvent>,
    axis: Axis,
    fallback: f64,
}

impl Track {
    fn new(events: Vec<Event>, axis: Axis, fallback: f64, center: Position, tempo: &Tempo) -> Result<Self> {
        let mut result = Self { events: Vec::with_capacity(events.len()), axis, fallback };
        for data in events {
            let start = tempo.time(data.start_time)?;
            let end = tempo.time(data.end_time)?;
            ensure!(start <= end, "event starts after its end");
            ensure!(data.start.is_finite() && data.end.is_finite(), "non-finite event value");
            ensure!((1..=29).contains(&data.easing_type), "easingType must be 1..29");
            ensure!(
                data.easing_left >= 0. && data.easing_right <= 1. && data.easing_left < data.easing_right,
                "invalid easing bounds"
            );
            ensure!(!data.inst.get()? || data.start == data.end, "inst requires equal start/end values");
            if data.bezier.get()? {
                ensure!(
                    data.bezier_points.iter().all(|v| v.is_finite())
                        && (0. ..=1.).contains(&data.bezier_points[0])
                        && (0. ..=1.).contains(&data.bezier_points[2]),
                    "invalid bezierPoints"
                );
            }
            if axis.anchored() {
                let a = data.anchor.context("missing transform anchor")?;
                ensure!(a.x.is_finite() && a.y.is_finite(), "invalid anchor");
            } else {
                ensure!(
                    data.transform_start.is_none() && data.transform_end.is_none() && !data.transform_step,
                    "transform metadata on movement track"
                );
            }
            ensure!(
                data.transform_start.into_iter().chain(data.transform_end).all(|v| v.is_finite()),
                "invalid transform reference"
            );
            ensure!(!data.transform_step || start == end, "transformStep requires zero duration");
            result.events.push(TimedEvent { data, start, end, prefix: Affine::identity() });
        }
        // 稳定排序：同一时刻的瞬变步进要保持文件里的先后顺序，累积仿射才对得上。
        result.events.sort_by(|a, b| a.start.total_cmp(&b.start));
        for i in 1..result.events.len() {
            let (before, after) = result.events.split_at_mut(i);
            let prev = &before[i - 1];
            let next = &mut after[0];
            // 同一时刻的零时长 transformStep 允许首尾相接，其余情况必须留出间隔。
            let chain = axis.anchored() && prev.start == next.start && prev.start == prev.end && prev.data.transform_step;
            ensure!(next.start >= prev.end && (next.start != prev.start || chain), "overlapping events");
            if axis.anchored() {
                let anchor = prev.data.anchor.unwrap_or(center);
                let from = prev.data.transform_start.unwrap_or(prev.data.start);
                let end = prev.data.transform_end.unwrap_or(prev.data.end);
                let next_from = next.data.transform_start.unwrap_or(next.data.start);
                next.prefix = axis.relative(end, next_from, anchor) * axis.relative(from, end, anchor) * prev.prefix;
            }
        }
        Ok(result)
    }

    /// 采样：`(值, 正在跑的事件)`。一个事件都没命中的时候用 `fallback`。
    fn sample(&self, time: f64) -> (f64, Option<&TimedEvent>) {
        let n = self.events.partition_point(|e| e.start <= time + EPS);
        if n == 0 {
            return (self.fallback, None);
        }
        let entry = &self.events[n - 1];
        let e = &entry.data;
        if e.start == e.end {
            return (e.start, Some(entry));
        }
        let p = if entry.end == entry.start {
            1.
        } else {
            ((time - entry.start) / (entry.end - entry.start)).clamp(0., 1.)
        };
        let eased = if e.bezier.get().unwrap_or(false) {
            // 解 x(t) = p 再取 y：不能直接对 y 用进度，否则和导出器的曲线形状对不上。
            // 控制点不量化，40 次二分足够把 t 压到 1e-12。
            let [x1, y1, x2, y2] = e.bezier_points;
            let component =
                |t: f64, a, b| 3. * t * (1. - t).powi(2) * a + 3. * t * t * (1. - t) * b + t.powi(3);
            let (mut lo, mut hi) = (0., 1.);
            for _ in 0..40 {
                let mid = (lo + hi) / 2.;
                if component(mid, x1, x2) < p {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            component((lo + hi) / 2., y1, y2)
        } else {
            // easingLeft / easingRight 把缓动曲线的一段区间拉伸到整个事件时长。
            let f = |v: f64| curve(e.easing_type, v);
            let base = f(e.easing_left);
            let range = f(e.easing_right) - base;
            if range.abs() < 1e-12 {
                p
            } else {
                (f(e.easing_left + (e.easing_right - e.easing_left) * p) - base) / range
            }
        };
        (e.start + (e.end - e.start) * eased, Some(entry))
    }

    /// 把事件带来的仿射作用到点上（没命中事件就是恒等）。
    fn transform(&self, point: nalgebra::Vector3<f64>, sample: (f64, Option<&TimedEvent>), center: Position) -> nalgebra::Vector3<f64> {
        if let Some(e) = sample.1 {
            self.axis.relative(
                e.data.transform_start.unwrap_or(e.data.start),
                sample.0,
                e.data.anchor.unwrap_or(center),
            ) * e.prefix
                * point
        } else {
            point
        }
    }
}

/// 标准缓动曲线（`easingType` 1..29），全部在 `f64` 里算。
///
/// 不能用判定线那套 `RPE_TWEEN_MAP` 表：那套的指数端点与 Back / Elastic 的 InOut 参数都不一样，
/// 换过来会让噪域几何和导出器画出来的对不上。
fn curve(id: u8, p: f64) -> f64 {
    use std::f64::consts::PI;
    // (族, 方向)：方向 0 = In、1 = Out、2 = InOut。
    let (family, direction) = match id {
        1 => return p,
        2 => (1, 1),
        3 => (1, 0),
        4 => (2, 1),
        5 => (2, 0),
        6 => (1, 2),
        7 => (2, 2),
        8 => (3, 1),
        9 => (3, 0),
        10 => (4, 1),
        11 => (4, 0),
        12 => (3, 2),
        13 => (4, 2),
        14 => (5, 1),
        15 => (5, 0),
        16 => (6, 1),
        17 => (6, 0),
        18 => (7, 1),
        19 => (7, 0),
        20 => (8, 1),
        21 => (8, 0),
        22 => (7, 2),
        23 => (8, 2),
        24 => (9, 1),
        25 => (9, 0),
        26 => (10, 1),
        27 => (10, 0),
        28 => (10, 2),
        29 => (9, 2),
        _ => return p,
    };
    // Bounce 的四个落地段（位移 / 抬升）。
    let bounce_out = |x: f64| {
        let (shift, offset) = if x < 4. / 11. {
            (0., 0.)
        } else if x < 8. / 11. {
            (6. / 11., 0.75)
        } else if x < 10. / 11. {
            (9. / 11., 0.9375)
        } else {
            (21. / 22., 0.984375)
        };
        121. / 16. * (x - shift).powi(2) + offset
    };
    let inside = |x: f64| match family {
        1 => 1. - (x * PI / 2.).cos(),
        2..=5 => x.powi(family),
        6 => {
            if x == 0. {
                0.
            } else {
                2_f64.powf(10. * (x - 1.))
            }
        }
        7 => 1. - (1. - x * x).abs().sqrt(),
        8 => x * x * ((1.70158 + 1.) * x - 1.70158),
        9 => {
            if x == 0. || x == 1. {
                x
            } else {
                -2_f64.powf(10. * x - 10.) * ((10. * x - 10.75) * 2. * PI / 3.).sin()
            }
        }
        10 => 1. - bounce_out(1. - x),
        _ => x,
    };
    match direction {
        0 => inside(p),
        1 => 1. - inside(1. - p),
        // Back / Elastic 的 InOut 不能靠 `1 - inside(1 - p)` 拼，各有自己的分段参数。
        _ if family == 8 => {
            let k = 1.70158 * 1.525;
            if p < 0.5 {
                let x = 2. * p;
                x * x * ((k + 1.) * x - k) / 2.
            } else {
                let x = 2. * p - 2.;
                (x * x * ((k + 1.) * x + k) + 2.) / 2.
            }
        }
        _ if family == 9 => {
            if p == 0. || p == 1. {
                p
            } else {
                let wave = ((20. * p - 11.125) * 2. * PI / 4.5).sin() / 2.;
                if p < 0.5 {
                    -2_f64.powf(20. * p - 10.) * wave
                } else {
                    2_f64.powf(10. - 20. * p) * wave + 1.
                }
            }
        }
        _ => {
            if p < 0.5 {
                inside(2. * p) / 2.
            } else {
                1. - inside(2. - 2. * p) / 2.
            }
        }
    }
}

/// 解析并校验好的一条 rpe-next 噪域。
pub struct NextArea {
    /// 矩形中心（画布单位）。
    center: Position,
    /// 矩形半宽半高（画布单位，忽略符号）。
    half: Position,
    /// `appearTime` / `disappearTime` 换算成秒后的可见窗口（左闭右开）。
    appear: f64,
    disappear: f64,
    /// 生效区间 `(start, end, readyFlash 秒)`，已按 start 排序、互不重叠、并裁到可见窗口内。
    intervals: Vec<(f64, f64, f64)>,
    /// 减算块标记（见 `AreaData::is_invert`）。
    invert: bool,
    /// 横移、纵移、旋转、横缩放、纵缩放。
    tracks: [Track; 5],
}

/// 某一时刻解算出来的结果。
struct Sample {
    zone: Zone,
    /// 1 = ready（紧接着的生效区间开始前那段闪烁窗口），2 = active（计入遮挡）。
    state: u8,
}

impl NextArea {
    fn from_value(value: &Value, tempo: &Tempo) -> Result<Self> {
        let data: AreaData = serde_json::from_value(value.clone())?;
        Self::new(data, tempo)
    }

    fn new(data: AreaData, tempo: &Tempo) -> Result<Self> {
        let center = Position {
            x: (data.bottom_left.x + data.top_right.x) / 2.,
            y: (data.bottom_left.y + data.top_right.y) / 2.,
        };
        // 上右角可能给到左下角的另一侧，取绝对值当半尺寸。
        let half = Position {
            x: (data.top_right.x - data.bottom_left.x).abs() / 2.,
            y: (data.top_right.y - data.bottom_left.y).abs() / 2.,
        };
        ensure!([center.x, center.y, half.x, half.y].iter().all(|v| v.is_finite()), "invalid rectangle");
        let appear = tempo.time(data.appear_time)?;
        let disappear = tempo.time(data.disappear_time)?;
        ensure!(appear <= disappear, "appearTime is after disappearTime");
        let mut raw_intervals = Vec::new();
        for i in data.active_intervals {
            let start = tempo.time(i.start_time)?;
            let end = tempo.time(i.end_time)?;
            ensure!(start <= end, "invalid active interval");
            // 零长度区间不生效（也就没有 ready 闪烁）。
            if start < end {
                raw_intervals.push((start, end, i.ready_flash.seconds()));
            }
        }
        raw_intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
        ensure!(raw_intervals.windows(2).all(|w| w[0].1 <= w[1].0), "overlapping active intervals");
        let intervals = raw_intervals
            .into_iter()
            .map(|(a, b, r)| (a.max(appear), b.min(disappear), r))
            .filter(|(a, b, _)| a < b)
            .collect();
        let track = |events, axis, fallback, name: &str| {
            Track::new(events, axis, fallback, center, tempo).with_context(|| name.to_owned())
        };
        Ok(Self {
            center,
            half,
            appear,
            disappear,
            intervals,
            invert: data.is_invert,
            tracks: [
                track(data.move_x_events, Axis::Move, center.x, "moveXEvents")?,
                track(data.move_y_events, Axis::Move, center.y, "moveYEvents")?,
                track(data.rotate_events, Axis::Rotate, 0., "rotateEvents")?,
                track(data.scale_x_events, Axis::X, 1., "scaleXEvents")?,
                track(data.scale_y_events, Axis::Y, 1., "scaleYEvents")?,
            ],
        })
    }

    /// 解算 `time` 时刻的矩形；不在可见窗口内（或退化成零面积）返回 `None`。
    fn sample(&self, time: f64, aspect: f32) -> Option<Sample> {
        if time + EPS < self.appear || time + EPS >= self.disappear {
            return None;
        }
        let n = self.intervals.partition_point(|i| i.0 <= time + EPS);
        let active = n != 0 && time + EPS < self.intervals[n - 1].1;
        let next = self.intervals.get(n);
        let ready = !active && next.is_some_and(|&(a, _, r)| r > 0. && time + EPS >= a - r);
        let values = [
            self.tracks[0].sample(time),
            self.tracks[1].sample(time),
            self.tracks[2].sample(time),
            self.tracks[3].sample(time),
            self.tracks[4].sample(time),
        ];
        // 变换顺序：先绕锚点缩放、旋转，最后才整体平移到采样出来的位置。
        let mut p = nalgebra::Vector3::new(self.center.x, self.center.y, 1.);
        for i in [3, 4, 2] {
            p = self.tracks[i].transform(p, values[i], self.center);
        }
        p.x += values[0].0 - self.center.x;
        p.y += values[1].0 - self.center.y;
        // 缩放后的半尺寸仍在画布单位里，先判零面积再换算到 chart 空间。
        let half_x = (self.half.x * values[3].0).abs();
        let half_y = (self.half.y * values[4].0).abs();
        if half_x == 0. || half_y == 0. {
            return None;
        }
        // `aspect` 折进纵向：见模块文档。
        let k = 1.5 / aspect as f64;
        let zone = Zone {
            center: Vector::new((p.x * UNIT) as f32, (p.y * UNIT * k) as f32),
            half: Vector::new((half_x * UNIT) as f32, (half_y * UNIT * k) as f32),
            angle: -(values[2].0.to_radians() as f32),
            invert: self.invert,
            active,
            ready,
            // 没有 `enableTime`，「没生效」就是「没在生效区间里」。
            before_enable: !active,
            opacity: 1.,
            // 不参与「简化噪域文字」编号。
            index: usize::MAX,
            judge_cover: 0,
            // rpe-next 噪域在自己的块层里带颜色，这条管线不接色：固定原生红。
            color: None,
        };
        Some(Sample { zone, state: if active { 2 } else if ready { 1 } else { 0 } })
    }
}

/// 这个条目是不是 rpe-next 噪域（`bottomLeft` / `topRight` 是它的独有字段）。
pub fn is_next_block_area(value: &Value) -> bool {
    value.get("bottomLeft").is_some() || value.get("topRight").is_some()
}

/// 这个条目是不是原生百分比噪域（`bottomLeftPercentage` / `topRightPercentage`）。
pub fn is_native_block_area(value: &Value) -> bool {
    value.get("bottomLeftPercentage").is_some() || value.get("topRightPercentage").is_some()
}

/// 逐条解析 `blockAreaList` 里的 rpe-next 条目，不是 next 形状的跳过。
///
/// 容错口径与原生 [`super::parse_block_areas`] 一致：单条坏数据只丢这一条。
pub fn parse_next(list: &[Value], tempo: &Tempo) -> Vec<NextArea> {
    list.iter()
        .enumerate()
        .filter(|(_, value)| is_next_block_area(value) && !is_native_block_area(value))
        .filter_map(|(index, value)| match NextArea::from_value(value, tempo) {
            Ok(area) => Some(area),
            Err(error) => {
                warn!("blockAreaList[{index}]: {error:#}");
                None
            }
        })
        .collect()
}

/// `time` 时刻所有可见噪域解算出来的 [`Zone`]，直接交给原生渲染那一趟一起画。
pub fn zones(areas: &[NextArea], time: f64, aspect: f32) -> Vec<Zone> {
    areas.iter().filter_map(|area| area.sample(time, aspect)).map(|sample| sample.zone).collect()
}

/// 点是否落在矩形内（`inset` 是 chart 空间的内缩，负值可以当外扩）。
///
/// 几何与掩码栅格化保持一致：`Rotation2::new(-angle) * (p - center)` 之后比半宽半高。
/// 减算块（`invert`）反过来向外扩，与原生噪域同一套语义。
fn contains(zone: &Zone, p: Vector, inset: f32) -> bool {
    let (s, c) = zone.angle.sin_cos();
    let d = p - zone.center;
    let local = Vector::new(c * d.x + s * d.y, -s * d.x + c * d.y);
    let sign = if zone.invert { 1. } else { -1. };
    let ix = inset.min(zone.half.x * 0.5);
    let iy = inset.min(zone.half.y * 0.5);
    local.x.abs() <= zone.half.x + sign * ix && local.y.abs() <= zone.half.y + sign * iy
}

/// 触摸点是否被 rpe-next 噪域挡住：与原生那套一样按奇偶规则算。
///
/// 只统计 `active` 的块；普通块算「被盖住」，减算块算一次反转。要求零内缩与外扩内缩两次都成立，
/// 这样贴着边缘的手指不会漏过去。
pub fn blocked(areas: &[NextArea], point: Vector, time: f64, aspect: f32) -> bool {
    let samples: Vec<_> = areas.iter().filter_map(|area| area.sample(time, aspect)).collect();
    let coverage = |inset: f32| {
        let (mut normal, mut inversions) = (false, 0u32);
        for sample in samples.iter().filter(|sample| sample.state == 2) {
            if contains(&sample.zone, point, inset) {
                if sample.zone.invert {
                    inversions += 1;
                } else {
                    normal = true;
                }
            }
        }
        normal ^ (inversions % 2 == 1)
    };
    coverage(0.) && coverage(touch_inset_world(aspect))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tempo_120() -> Tempo {
        Tempo::new([(0., 120.)]).unwrap()
    }

    /// 整块画布：画布中心在 (0, 0)，半宽半高 (675, 450) 画布单位。
    fn full_canvas(extra: Value) -> Value {
        let mut value = json!({
            "bottomLeft": {"x": -675., "y": -450.},
            "topRight": {"x": 675., "y": 450.},
            "appearTime": [0, 0, 1],
            "disappearTime": [6, 0, 1],
            "activeIntervals": [{"startTime": [2, 0, 1], "endTime": [4, 0, 1]}],
        });
        for (key, item) in extra.as_object().unwrap() {
            value[key.as_str()] = item.clone();
        }
        value
    }

    #[test]
    fn tempo_maps_beats_to_seconds() {
        // 0..4 beat @120（2 秒），之后 @60（每 beat 1 秒）。
        let tempo = Tempo::new([(0., 120.), (4., 60.)]).unwrap();
        assert!((tempo.seconds(0.) - 0.).abs() < 1e-9);
        assert!((tempo.seconds(2.) - 1.).abs() < 1e-9);
        assert!((tempo.seconds(4.) - 2.).abs() < 1e-9);
        assert!((tempo.seconds(6.) - 4.).abs() < 1e-9);
        assert!(Tempo::new([]).is_err());
        assert!(Tempo::new([(0., 0.)]).is_err());
    }

    #[test]
    fn parses_rectangle_into_chart_space() {
        let tempo = tempo_120();
        let areas = parse_next(&[full_canvas(json!({}))], &tempo);
        assert_eq!(areas.len(), 1);
        // 120 bpm：1 beat = 0.5 秒，生效区间 [2, 4) beat = [1.0, 2.0) 秒。
        let sample = areas[0].sample(1.5, 1.5).unwrap();
        assert_eq!(sample.state, 2);
        let zone = sample.zone;
        assert!(zone.center.x.abs() < 1e-6 && zone.center.y.abs() < 1e-6);
        // 675 * 2/1350 = 1；450 * 2/1350 * (1.5/1.5) = 2/3。
        assert!((zone.half.x - 1.).abs() < 1e-6);
        assert!((zone.half.y - 2. / 3.).abs() < 1e-6);
        assert_eq!(zone.angle, 0.);
        assert!(zone.active && !zone.ready && !zone.invert);
        assert_eq!(zone.index, usize::MAX);
        // 生效前是 ready 闪烁窗口（readyFlash 缺省 0.5 秒），生效区间外不算 active。
        assert_eq!(areas[0].sample(0.7, 1.5).unwrap().state, 1);
        assert_eq!(areas[0].sample(2.5, 1.5).unwrap().state, 0);
        assert!(areas[0].sample(3.1, 1.5).is_none());
    }

    #[test]
    fn aspect_scales_vertical_extent() {
        let tempo = tempo_120();
        let areas = parse_next(&[full_canvas(json!({}))], &tempo);
        // aspect 3 时 chart 空间纵向半高只有 1/3，画布 450 单位要压到那里。
        let zone = areas[0].sample(1.5, 3.).unwrap().zone;
        assert!((zone.half.x - 1.).abs() < 1e-6);
        assert!((zone.half.y - 1. / 3.).abs() < 1e-6);
    }

    #[test]
    fn blocked_follows_odd_even_rule_with_inset() {
        let tempo = tempo_120();
        let areas = parse_next(&[full_canvas(json!({}))], &tempo);
        let t = 1.5;
        let aspect = 1.5;
        // 正中心一定挡住。
        assert!(blocked(&areas, Vector::new(0., 0.), t, aspect));
        // y = 0.5 在两条边界之内。
        assert!(blocked(&areas, Vector::new(0., 0.5), t, aspect));
        // y = 0.65 落在原始矩形里、但落在内缩后的边界外 ⇒ 不挡。
        // （touch_inset_world(1.5) = 0.05 * 2 / 1.5 = 1/15，内缩后半高 2/3 - 1/15 = 0.6）
        assert!(!blocked(&areas, Vector::new(0., 0.65), t, aspect));
        // 横向同理：1 - 1/15 = 0.9333。
        assert!(!blocked(&areas, Vector::new(0.95, 0.), t, aspect));
        // 完全在外面。
        assert!(!blocked(&areas, Vector::new(1.2, 0.), t, aspect));
        // 不在生效区间里就不挡。
        assert!(!blocked(&areas, Vector::new(0., 0.), 2.5, aspect));
        assert!(!blocked(&areas, Vector::new(0., 0.), 0.7, aspect));
    }

    #[test]
    fn inversion_cancels_normal_coverage() {
        let tempo = tempo_120();
        let normal = parse_next(&[full_canvas(json!({}))], &tempo);
        let inverted = parse_next(&[full_canvas(json!({"isInvert": true}))], &tempo);
        assert!(inverted[0].invert);
        let point = Vector::new(0., 0.);
        // 单块减算：普通块 0 次、反转 1 次 ⇒ 仍然算挡住（奇偶规则）。
        assert!(blocked(&inverted, point, 1.5, 1.5));
        // 普通 + 减算重叠：反转一次把覆盖抵消掉。
        let both: Vec<_> = normal.into_iter().chain(inverted).collect();
        assert!(!blocked(&both, point, 1.5, 1.5));
    }

    #[test]
    fn move_x_event_shifts_center() {
        let tempo = tempo_120();
        let events = json!({"moveXEvents": [{"startTime": [0, 0, 1], "endTime": [2, 0, 1], "start": 0., "end": 675.}]});
        let areas = parse_next(&[full_canvas(events)], &tempo);
        // 1 beat = 0.5 秒，事件跑到一半 ⇒ 画布 x = 337.5 ⇒ chart x = 0.5。
        let zone = areas[0].sample(0.5, 1.5).unwrap().zone;
        assert!((zone.center.x - 0.5).abs() < 1e-6, "{:?}", zone.center);
        assert!(zone.center.y.abs() < 1e-6);
    }

    #[test]
    fn rotate_event_spins_rectangle() {
        let tempo = tempo_120();
        let events = json!({"rotateEvents": [{"startTime": [0, 0, 1], "endTime": [2, 0, 1], "start": 0., "end": 90., "anchor": {"x": 0., "y": 0.}}]});
        let areas = parse_next(&[full_canvas(events)], &tempo);
        let zone = areas[0].sample(0.5, 1.5).unwrap().zone;
        // 事件跑到一半 ⇒ 45 度；`angle` 取反号，与参考实现的 `Zone` 约定一致。
        assert!((zone.angle + std::f32::consts::FRAC_PI_4).abs() < 1e-5, "{}", zone.angle);
    }

    #[test]
    fn scale_event_resizes_half_extent() {
        let tempo = tempo_120();
        let events = json!({"scaleXEvents": [{"startTime": [0, 0, 1], "endTime": [2, 0, 1], "start": 1., "end": 0.5, "anchor": {"x": 0., "y": 0.}}]});
        let areas = parse_next(&[full_canvas(events)], &tempo);
        let zone = areas[0].sample(0.5, 1.5).unwrap().zone;
        // 1.0 → 0.5 跑到一半是 0.75，半宽 1 * 0.75。
        assert!((zone.half.x - 0.75).abs() < 1e-6, "{}", zone.half.x);
        assert!((zone.half.y - 2. / 3.).abs() < 1e-6);
    }

    #[test]
    fn mixed_list_separates_native_and_next() {
        let tempo = tempo_120();
        let native = json!({
            "topRightPercentage": {"x": 1., "y": 1.},
            "bottomLeftPercentage": {"x": 0., "y": 0.},
            "appearTime": 0., "enableTime": 0., "disableTime": 1., "disappearTime": 2.,
            "isSubtract": false,
            "rotateEvents": [], "moveEvents": [], "scaleEvents": [],
        });
        let next = full_canvas(json!({}));
        let list = vec![native.clone(), next.clone()];
        assert!(is_native_block_area(&native) && !is_next_block_area(&native));
        assert!(is_next_block_area(&next) && !is_native_block_area(&next));
        let parsed = super::super::parse_block_areas(Some(&Value::Array(list.clone())));
        assert_eq!(parsed.len(), 1);
        assert_eq!(parse_next(&list, &tempo).len(), 1);
        // 两套字段同时出现的条目两边都不收，避免字段撞车解析出半截数据。
        let ambiguous = json!({"bottomLeft": {"x": 0., "y": 0.}, "bottomLeftPercentage": {"x": 0., "y": 0.}});
        assert!(is_next_block_area(&ambiguous) && is_native_block_area(&ambiguous));
        assert!(parse_next(&[ambiguous], &tempo).is_empty());
    }

    #[test]
    fn invalid_data_is_rejected() {
        let tempo = tempo_120();
        let bad_beat = json!({
            "bottomLeft": {"x": 0., "y": 0.},
            "topRight": {"x": 10., "y": 10.},
            "appearTime": [0, 0, 0],
            "disappearTime": [4, 0, 1],
        });
        assert!(NextArea::from_value(&bad_beat, &tempo).is_err());
        let no_anchor = json!({
            "bottomLeft": {"x": 0., "y": 0.},
            "topRight": {"x": 10., "y": 10.},
            "appearTime": [0, 0, 1],
            "disappearTime": [4, 0, 1],
            "rotateEvents": [{"startTime": [0, 0, 1], "endTime": [1, 0, 1], "start": 0., "end": 90.}],
        });
        assert!(NextArea::from_value(&no_anchor, &tempo).is_err());
        // 坏条目只丢自己，不影响同一份列表里的好条目。
        let good = full_canvas(json!({}));
        assert_eq!(parse_next(&[bad_beat, good], &tempo).len(), 1);
    }

    #[test]
    fn easing_curves_hit_both_endpoints() {
        for id in 1..=29u8 {
            assert!(curve(id, 0.).abs() < 1e-3, "id={id}");
            assert!((curve(id, 1.) - 1.).abs() < 1e-3, "id={id}");
        }
        assert!((curve(1, 0.25) - 0.25).abs() < 1e-12);
    }
}
