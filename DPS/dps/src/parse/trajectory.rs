//! rpe-next 的曲线轨迹（`trajectory`，RPEVersion 210 起）。
//!
//! 一条轨迹把事件的进度映射成平面上的一个点：`parametric` 用 x / y 两条表达式，
//! `polar` 用极坐标的半径表达式 + 角度区间，`shape` 用星形/随机多边形的顶点连线。
//! 表达式求值见 [`super::expr`]。
//!
//! `tangentRotation` 按切线方向给出旋转（与 rpe-next 一样用数值差分 + 1024 点解缠绕表），
//! `rotationExpression` 则直接给角度；两者都没有时轨迹不带旋转。

use super::expr::Expr;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

/// 切线旋转的解缠绕采样数，与 rpe-next 的 `angles` 表一致。
const TANGENT_SAMPLES: usize = 1024;

fn f32_zero() -> f32 {
    0.
}

fn f32_one() -> f32 {
    1.
}

fn easing_id() -> i32 {
    1
}

/// `trajectory = { version, options: {...}, domain?, offsets? }`。
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Trajectory {
    #[serde(default)]
    pub domain: Option<[f32; 2]>,
    /// 每个轴上再叠加的线性偏移：`offsets.x = [起点, 终点]`。
    #[serde(default)]
    pub offsets: Option<FxHashMap<String, [f32; 2]>>,
    #[serde(default)]
    pub options: TrajectoryOptions,
}

#[derive(Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TrajectoryOptions {
    /// `parametric` 或 `polar`。
    pub mode: Option<String>,
    /// `star` 或 `random-polygon`，有它时忽略 `mode` 与表达式。
    pub shape: Option<String>,
    pub x_expression: Option<String>,
    pub y_expression: Option<String>,
    pub radius_expression: Option<String>,
    pub rotation_expression: Option<String>,
    #[serde(default)]
    pub tangent_rotation: bool,
    pub parameter_start: Option<String>,
    pub parameter_end: Option<String>,
    pub angle_start: Option<String>,
    pub angle_end: Option<String>,
    #[serde(default = "f32_zero")]
    pub trim_start: f32,
    #[serde(default = "f32_one")]
    pub trim_end: f32,
    #[serde(default = "f32_zero")]
    pub rotation: f32,
    #[serde(default = "f32_one")]
    pub scale_x: f32,
    #[serde(default = "f32_one")]
    pub scale_y: f32,
    #[serde(default)]
    pub align_start: bool,
    #[serde(default = "f32_zero")]
    pub start_x: f32,
    #[serde(default = "f32_zero")]
    pub start_y: f32,
    #[serde(default = "easing_id")]
    pub easing_x: i32,
    #[serde(default = "easing_id")]
    pub easing_y: i32,
    #[serde(default = "f32_one")]
    pub seed: f32,
    #[serde(default = "f32_zero")]
    pub randomness: f32,
    #[serde(default)]
    pub parameters: FxHashMap<String, f32>,
}

/// 轨迹上一个采样点：坐标是 RPE 画布单位，旋转是角度。
#[derive(Clone, Copy)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub rotation: f32,
}

pub struct Compiled {
    polar: bool,
    shape: Option<Shape>,
    begin: Expr,
    finish: Expr,
    horizontal: Option<Expr>,
    vertical: Option<Expr>,
    radial: Option<Expr>,
    direction: Option<Expr>,
    tangent: Option<Vec<f32>>,
    domain: (f32, f32),
    offsets: FxHashMap<String, [f32; 2]>,
    trim_start: f32,
    trim_end: f32,
    rotation: f32,
    scale_x: f32,
    scale_y: f32,
    align_start: bool,
    start_x: f32,
    start_y: f32,
    easing_x: usize,
    easing_y: usize,
    origin: (f32, f32),
    scope: FxHashMap<String, f32>,
}

enum Shape {
    Star,
    RandomPolygon,
}

impl Trajectory {
    /// 编译成可求值的形式。缺表达式、表达式写坏都返回 None。
    pub fn compile(&self) -> Option<Compiled> {
        let o = &self.options;
        let shape = match o.shape.as_deref() {
            None => None,
            Some("star") => Some(Shape::Star),
            Some("random-polygon") => Some(Shape::RandomPolygon),
            Some(_) => return None,
        };
        let polar = o.mode.as_deref().unwrap_or("parametric") == "polar";
        let pick = |s: &Option<String>| s.as_deref().and_then(Expr::parse);
        let begin = pick(if polar { &o.angle_start } else { &o.parameter_start }).or_else(|| Expr::parse("0"))?;
        let finish = pick(if polar { &o.angle_end } else { &o.parameter_end }).or_else(|| Expr::parse("1"))?;
        let horizontal = if polar || shape.is_some() { None } else { Some(pick(&o.x_expression).or_else(|| Expr::parse("0"))?) };
        let vertical = if polar || shape.is_some() { None } else { Some(pick(&o.y_expression).or_else(|| Expr::parse("0"))?) };
        let radial = if polar { Some(pick(&o.radius_expression)?) } else { None };
        let direction = if o.tangent_rotation { None } else { pick(&o.rotation_expression) };
        let mut scope = o.parameters.clone();
        scope.insert("seed".into(), o.seed);
        scope.insert("randomness".into(), o.randomness);
        // 轨迹的 `easingX/Y` 用的是事件那套 1..30 的缓动编号，先换成 [`TWEEN_FUNCTIONS`] 的下标。
        let easing = |id: i32| super::RPE_TWEEN_MAP[(id.clamp(1, 30) as usize) - 1] as usize;
        let mut compiled = Compiled {
            polar,
            shape,
            begin,
            finish,
            horizontal,
            vertical,
            radial,
            direction,
            tangent: None,
            domain: self.domain.map(|it| (it[0], it[1])).unwrap_or((0., 1.)),
            offsets: self.offsets.clone().unwrap_or_default(),
            trim_start: o.trim_start,
            trim_end: o.trim_end,
            rotation: o.rotation,
            scale_x: o.scale_x,
            scale_y: o.scale_y,
            align_start: false,
            start_x: o.start_x,
            start_y: o.start_y,
            easing_x: easing(o.easing_x),
            easing_y: easing(o.easing_y),
            origin: (0., 0.),
            scope,
        };
        compiled.origin = compiled.raw(0.).map(|it| (it.x, it.y))?;
        compiled.align_start = o.align_start;
        if o.tangent_rotation {
            let mut angles = Vec::with_capacity(TANGENT_SAMPLES + 1);
            for index in 0..=TANGENT_SAMPLES {
                let previous = angles.last().copied().unwrap_or(0.);
                let angle = compiled.tangent(index as f32 / TANGENT_SAMPLES as f32).unwrap_or(previous);
                angles.push(if index > 0 { angle + 360. * ((previous - angle) / 360.).round() } else { angle });
            }
            compiled.tangent = Some(angles);
        }
        Some(compiled)
    }
}

impl Compiled {
    /// 轨迹是否顺带给出旋转（切线角或旋转表达式）。没有时不生成 `rotateEvents` 的采样点。
    pub fn has_rotation(&self) -> bool {
        self.tangent.is_some() || self.direction.is_some()
    }

    fn ease(&self, x: f32, id: usize) -> f32 {
        let table = crate::core::TWEEN_FUNCTIONS;
        table.get(id).copied().unwrap_or(table[0])(x)
    }

    fn random(&self, index: f32) -> f32 {
        let value = (index * 127.1 + self.scope.get("seed").copied().unwrap_or(1.) * 311.7).sin() * 43758.5453;
        value - value.floor()
    }

    fn parameter(&self, key: &str, fallback: f32) -> f32 {
        self.scope.get(key).copied().unwrap_or(fallback)
    }

    /// 事件进度（0..1）对应的点，带 `offsets` 与 `alignStart` 的位移。
    pub fn value(&self, progress: f32) -> Option<Point> {
        let progress = progress.clamp(0., 1.);
        let mapped = self.domain.0 + (self.domain.1 - self.domain.0) * progress;
        let mut point = self.raw(mapped)?;
        if self.align_start {
            point.x += self.start_x - self.origin.0;
            point.y += self.start_y - self.origin.1;
        }
        for (axis, range) in &self.offsets {
            let delta = range[0] + (range[1] - range[0]) * mapped;
            match axis.as_str() {
                "x" => point.x += delta,
                "y" => point.y += delta,
                _ => {}
            }
        }
        if let Some(angles) = &self.tangent {
            let reference = angles[(progress * TANGENT_SAMPLES as f32).round() as usize];
            let angle = self.tangent(progress).unwrap_or(reference);
            point.rotation = angle + 360. * ((reference - angle) / 360.).round();
        }
        Some(point)
    }

    /// 曲线的原始点（还没加 `alignStart` 与 `offsets`，切线角还是未解缠绕的）。
    fn raw(&self, progress: f32) -> Option<Point> {
        let unit = self.trim_start + (self.trim_end - self.trim_start) * progress;
        let begin = self.begin.eval(&self.scope)?;
        let finish = self.finish.eval(&self.scope)?;
        let coordinate = |easing: usize| begin + (finish - begin) * self.ease(unit, easing);
        let (x, y) = if let Some(shape) = &self.shape {
            // 顶点连线：`unit` 在 0..1 之间走一圈多边形，相邻顶点之间线性插值。
            let sides = self.parameter("points", 5.).round().clamp(3., 40.) as i32 * if matches!(shape, Shape::Star) { 2 } else { 1 };
            let amount = unit.rem_euclid(1.) * sides as f32;
            let index = amount.floor() as i32;
            let fraction = amount - index as f32;
            let vertex = |step: i32| {
                let wrapped = step.rem_euclid(sides);
                let radius = self.parameter("radius", 200.)
                    * if matches!(shape, Shape::Star) {
                        if wrapped % 2 == 1 { self.parameter("inner", 0.4) } else { 1. }
                    } else {
                        1. - self.scope.get("randomness").copied().unwrap_or(0.) * self.random(wrapped as f32)
                    };
                let angle = std::f32::consts::TAU * wrapped as f32 / sides as f32;
                (radius * angle.cos(), radius * angle.sin())
            };
            let first = vertex(index);
            let second = vertex(index + 1);
            (first.0 + (second.0 - first.0) * fraction, first.1 + (second.1 - first.1) * fraction)
        } else if let Some(radial) = &self.radial {
            let theta = coordinate(self.easing_x);
            let mut scope = self.scope.clone();
            scope.insert("theta".into(), theta);
            scope.insert("t".into(), unit);
            scope.insert("u".into(), progress);
            let radius = radial.eval(&scope)?;
            (radius * theta.cos(), radius * theta.sin())
        } else {
            let mut xs = self.scope.clone();
            xs.insert("t".into(), coordinate(self.easing_x));
            xs.insert("u".into(), progress);
            let mut ys = self.scope.clone();
            ys.insert("t".into(), coordinate(self.easing_y));
            ys.insert("u".into(), progress);
            (self.horizontal.as_ref()?.eval(&xs)?, self.vertical.as_ref()?.eval(&ys)?)
        };
        let angle = self.rotation.to_radians();
        let (sx, sy) = (x * self.scale_x, y * self.scale_y);
        let (x, y) = (sx * angle.cos() - sy * angle.sin(), sx * angle.sin() + sy * angle.cos());
        let rotation = match &self.direction {
            Some(expr) => {
                let mut scope = self.scope.clone();
                scope.insert("t".into(), if self.polar { unit } else { coordinate(self.easing_x) });
                scope.insert("theta".into(), coordinate(self.easing_x));
                scope.insert("u".into(), progress);
                expr.eval(&scope)?
            }
            None => 0.,
        };
        Some(Point { x, y, rotation })
    }

    /// 切线方向（角度），逐步放大差分步长直到两点分得开。
    fn tangent(&self, progress: f32) -> Option<f32> {
        for delta in [0.00001, 0.0001, 0.001, 0.01] {
            let before = self.raw((progress - delta).max(0.))?;
            let after = self.raw((progress + delta).min(1.))?;
            let (dx, dy) = (after.x - before.x, after.y - before.y);
            if dx.hypot(dy) > 1e-9 {
                return Some(-dy.atan2(dx).to_degrees());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(json: &str) -> Compiled {
        serde_json::from_str::<Trajectory>(json).unwrap().compile().unwrap()
    }

    #[test]
    fn parametric_samples_both_axes() {
        let trajectory = compile(r#"{"version":1,"options":{"mode":"parametric","xExpression":"200*t","yExpression":"-100*t"}}"#);
        let point = trajectory.value(0.25).unwrap();
        assert!((point.x - 50.).abs() < 1e-3, "x = {}", point.x);
        assert!((point.y + 25.).abs() < 1e-3, "y = {}", point.y);
    }

    #[test]
    fn polar_traces_the_angle_range() {
        let trajectory = compile(r#"{"version":1,"options":{"mode":"polar","radiusExpression":"100","angleStart":"0","angleEnd":"pi"}}"#);
        let point = trajectory.value(0.5).unwrap();
        assert!(point.x.abs() < 1e-2, "x = {}", point.x);
        assert!((point.y - 100.).abs() < 1e-2, "y = {}", point.y);
    }

    #[test]
    fn align_start_and_offsets_are_applied() {
        let trajectory = compile(
            r#"{"version":1,"options":{"xExpression":"100*t","yExpression":"0","alignStart":true,"startX":5,"startY":7},"offsets":{"x":[10,20]}}"#,
        );
        let point = trajectory.value(0.5).unwrap();
        // 起点挪到 (5, 7) 后，x 上再叠加 10→20 的线性偏移。
        assert!((point.x - 70.).abs() < 1e-2, "x = {}", point.x);
        assert!((point.y - 7.).abs() < 1e-2, "y = {}", point.y);
    }

    /// 星形：10 个顶点（points=5 的两倍），半径在内/外交替，外顶点 200、内顶点 80。
    #[test]
    fn star_shape_alternates_radius() {
        let trajectory = compile(r#"{"options":{"shape":"star","parameters":{"radius":200,"points":5,"inner":0.4}}}"#);
        let outer = trajectory.value(0.).unwrap();
        assert!((outer.x - 200.).abs() < 1., "x = {}", outer.x);
        assert!(outer.y.abs() < 1., "y = {}", outer.y);
        // 走到第二个顶点（1/10 圈）时落在内顶点上，半径 80。
        let inner = trajectory.value(0.1).unwrap();
        assert!((inner.x.hypot(inner.y) - 80.).abs() < 1., "r = {}", inner.x.hypot(inner.y));
        // 顶点之间是直线段，中点半径介于两者之间（弦的中点，不是半径中点）。
        let middle = trajectory.value(0.05).unwrap();
        let radius = middle.x.hypot(middle.y);
        assert!(radius > 80. && radius < 200., "r = {radius}");
    }

    /// 切线旋转：逆时针走圆时切线朝外，角度从 -90° 起每 1/4 圈加 90°。
    #[test]
    fn tangent_rotation_follows_the_curve() {
        let trajectory = compile(r#"{"options":{"xExpression":"200*cos(2*pi*t)","yExpression":"200*sin(2*pi*t)","tangentRotation":true}}"#);
        assert!(trajectory.has_rotation());
        let start = trajectory.value(0.).unwrap();
        assert!((start.rotation + 90.).abs() < 1., "{}", start.rotation);
        let quarter = trajectory.value(0.25).unwrap();
        assert!((quarter.rotation + 180.).abs() < 1., "{}", quarter.rotation);
    }

    #[test]
    fn rotation_expression_sets_rotation() {
        let trajectory = compile(r#"{"options":{"xExpression":"t","yExpression":"0","rotationExpression":"90*t"}}"#);
        assert!(trajectory.has_rotation());
        assert!((trajectory.value(0.5).unwrap().rotation - 45.).abs() < 1e-3);
    }

    #[test]
    fn unknown_shape_is_not_supported() {
        assert!(serde_json::from_str::<Trajectory>(r#"{"options":{"shape":"triangle"}}"#).unwrap().compile().is_none());
    }
}
