//! 「纯配置谱面」：把谱面就地拍平成「判定线全部静止、按键保留原来的屏幕横向位置」的形态，画面上
//! 于是只剩谱面配置本身 —— 按键的键型、横向站位与时机。
//!
//! 转换分两步，顺序与谱面修改工具 modify 的 `core/position.py::screen_positions` +
//! `features/layout.py::rebuild_positions` 一致：
//!
//! 1. 先在**原谱面**上求值：每个按键在自己时刻的屏幕坐标 = 判定线位移 + 按键相对判定线的偏移
//!    （判定线转了角度，偏移也跟着转），以及那一时刻判定线的角度。
//! 2. 再把求出来的坐标反过来写进按键自己的位移，并把判定线的移动 / 旋转 / 消失 / 速度事件全部
//!    换成常量。
//!
//! 纵向上按键不再沿用原谱面的 `floorPosition`，而是和判定线共用同一条常量速度的积分曲线
//! (`height(t) = t * speed / HEIGHT_RATIO`)，所以按键在自己时刻的位置只由时间和流速决定 ——
//! 这正是参考实现把 `speedEvents` 归并成一条常量后，读回来的引擎会算出的结果。
//!
//! 判定时角度接近**竖直**的按键（判定线在那一刻转了 ±90° 附近）另算：它们被分到屏幕左右两条竖
//! 判定线上 —— 落到哪一条看它**朝哪个方向落**（朝左那半圈 `-90°~90°` 贴左边、朝右那半圈
//! `90°~270°` 贴右边，判定线**下方**那一趟反着落，左右对调），在竖线上站多高用它原来的纵向坐标
//! （Y），横判定线只留横向的按键（见 [`SideLines`] 与 [`is_side_note`]）。参考实现把每条记录算出来的
//! `angle` 求了值却没往下用，这里补上。
//!
//! 拍平后屏幕上一整层噪域（`blockAreaList`）与画面对不上，所以纯配置模式下那一层不再绘制；作为
//! 替代，**判定时刻落在噪域里的按键**会被标上 [`Note::pure_config_noise`]（判定在**原谱面**的坐标上
//! 做，见 [`in_noise`]），渲染时在它周围长出一块噪域矩形 —— 也就是「反向强制定位」那个观感。
//!
//! 谱面文件不动，只改内存里刚解析出来的 [`JudgeLine`]（可能**新增**两条竖判定线），所以同一个开关
//! 随时可以关掉、照原样再渲染。拍平只看 `z_index` 与 `attach_ui` 之外的字段，这两样都不变，但新增
//! 判定线要进 `Chart` 的绘制顺序 —— 这一步由 `Chart::apply_chart_mods` 负责。

use crate::core::{
    Anim, AnimFloat, AnimFloatF64, AnimVector, BlockArea, JudgeLine, JudgeLineCache, JudgeLineKind, Keyframe, Note, NoteKind, Object, Vector,
    HEIGHT_RATIO,
};
use std::cell::RefCell;

/// 左右两条竖判定线的参数：给 `None` 就是关掉这个功能（竖着的按键照旧留在横判定线上）。
#[derive(Clone, Copy)]
pub struct SideLines {
    /// 判定时刻的角度与水平方向相差多少度以内仍算「横向按键」（`0`~`90`，默认 45）。落在
    /// `[-angle, angle]` 或 `[180 - angle, 180]` 里的按键留在横判定线上，其余分到两条竖判定线
    /// （落到左还是右看按键朝哪个方向落：朝左的贴左边、朝右的贴右边）。
    pub angle: f32,
    /// 竖判定线距**最近的那条屏幕竖边**的距离，轴单位：`0.5` 是屏幕正中、`0.1` 是离边一成。
    /// 超出 `0`~`0.5` 的部分会被夹掉（否则左右两条线会互相越过）。
    pub x: f32,
}

/// 按键噪域判定的输入：谱面的方块列表。
///
/// 方块几何在 chart 空间里（x 向右、y 向上、全屏 = `[-1, 1] x [-1/aspect, 1/aspect]`），
/// 而 [`apply`] 本来就是用 chart 空间的坐标求值的，所以这里只要方块本身；纵横比由 `apply` 统一收。
pub struct Noise<'a> {
    pub areas: &'a [BlockArea],
}

/// 对整张谱面的判定线做一次转换，`side` 打开时再往 `lines` 末尾追加左右两条竖判定线。
///
/// * `line_y` —— 判定线钉在屏幕纵向的哪个高度：`0` 是屏幕底边、`1` 是屏幕顶边、`0.5` 是正中。
///   世界坐标里纵向 1 单位是半个屏幕高、`0` 是正中，所以写进判定线位移的是 `-1 + line_y * 2`：
///   `0.1` 正是参考实现的 `start2 = 0.1`（贴近屏幕下沿）。
/// * `speed` —— 按键流速：判定线高度积分与长按尾端都按它算，点按 / 拖动 / 滑动固定 1。
/// * `noise` —— 给 `Some` 时，逐个按键看它判定时刻的屏幕位置是否落在谱面噪域里，落在里面的按键
///   被标上 `Note::pure_config_noise`，渲染时在它周围长出一块噪域矩形。
/// * `aspect` —— 屏幕纵横比。chart 空间的纵轴按 `1 / aspect` 缩放过（`Object::now_translation` 就是
///   这么折的），求按键的纵向落点必须用它，否则竖判定线上的按键会按纵横比的倍率跑偏。
///
/// 追加出来的判定线不在 `Chart` 的绘制顺序里，调用方要自己补（用返回前后的 `len()` 差就能定位）。
pub fn apply(lines: &mut Vec<JudgeLine>, line_y: f32, speed: f64, side: Option<SideLines>, noise: Option<Noise>, aspect: f32) {
    // 判定线高度积分要用谱面结束时刻当终点（与 `parse::pgr::parse_speed_events` 造这条积分时取
    // 的 `max_time` 同一个口径）；积分是线性的，所以只要不早于最后一个按键，取多少都不影响谱面
    // 内的取值。
    let end_time = lines
        .iter()
        .flat_map(|line| line.notes.iter())
        .map(|note| match note.kind {
            NoteKind::Hold { end_time, .. } => end_time.max(note.time),
            _ => note.time,
        })
        .fold(0.0_f64, f64::max)
        + 1.0;

    // 竖判定线要接走的按键：`[0]` 是左侧那条（判定线转 -90°）、`[1]` 是右侧那条（+90°）。
    let mut side_notes: [Vec<Note>; 2] = [Vec::new(), Vec::new()];
    for line in lines.iter_mut() {
        // 挂 UI 元件的判定线不参与绘制（`Chart::new` 把它们排除在 `order` 之外），只用来给连击数 /
        // 分数这类元件提供变换 —— 拍平它们会把那些元件也钉到基准线上。
        if line.attach_ui.is_none() {
            bake_line(line, line_y, speed, end_time, side.as_ref(), noise.as_ref(), &mut side_notes, aspect);
        }
    }

    let Some(side) = side else {
        return;
    };
    // 距最近竖边的距离：世界坐标里横坐标 ±1 是屏幕两条竖边、0 是正中。
    let distance = side.x.clamp(0., 0.5);
    for (index, (x, rotation)) in [(-1. + distance * 2., -90.), (1. - distance * 2., 90.)].into_iter().enumerate() {
        lines.push(side_line(x, rotation, std::mem::take(&mut side_notes[index]), end_time, speed));
    }
}

/// 把一条判定线拍平，并把每个按键的屏幕坐标**固化**进按键自己的位移里。
///
/// 横判定线上的按键：屏幕横坐标 = 判定线的横向位移 + 按键相对判定线的偏移投影到 x 轴上的那一份，
/// 所以在**按键自己的时刻**把这两项求值、加起来存成常量，之后拍平判定线就不会再挪动它的横向位置。
/// 按键相对自身判定线的纵向偏移（RPE 的 `yOffset`）不参与这个投影 —— 参考实现只用 `positionX`。
///
/// 判定时角度竖直的按键（`side` 打开时）改成沿竖判定线方向固化：它朝哪个方向落就贴哪条竖线
/// （朝左贴左边、朝右贴右边，判定线**下方**那一趟左右对调），纵向坐标原样留作它在竖线上的位置，
/// 垂直于判定线的那份交给共用的速度积分 —— 于是它和横判定线上的按键一样，在自己的时刻落在判定线上，
/// 落点方向则从屏幕中间朝两侧。
fn bake_line(
    line: &mut JudgeLine,
    line_y: f32,
    speed: f64,
    end_time: f64,
    side: Option<&SideLines>,
    noise: Option<&Noise>,
    side_notes: &mut [Vec<Note>; 2],
    aspect: f32,
) {
    let mut evaluator = LineEvaluator::new(line);
    let mut kept = Vec::with_capacity(line.notes.len());
    for mut note in line.notes.drain(..) {
        let time = note.time;
        let mut off_x = note.object.translation.0.clone();
        let mut off_y = note.object.translation.1.clone();
        off_x.set_time(time);
        off_y.set_time(time);
        let (x, y, angle) = evaluator.eval(time, off_x.now(), off_y.now(), aspect);
        // 侧别只用到落点朝哪个方向的那一份，也就是 `sinθ`。
        let sin = angle.to_radians().sin();
        if let Some(noise) = noise {
            note.pure_config_noise = in_noise(noise, time, x, y, aspect);
        }
        if side.is_some_and(|side| is_side_note(angle, side.angle)) {
            // 侧别按按键的**朝向**定，也就是它落下来时朝哪边走：判定线转 θ 后局部 -y 指向世界的
            // `(sinθ, -cosθ)`。把朝向按「0° = 朝左、90° = 朝上、180° = 朝右、270°（= -90°）= 朝下」
            // 记角度，则 `-90°~90°`（朝左那半圈）贴左边那条竖线、`90°~270°`（朝右那半圈）贴右边
            // 那条 —— 取横向分量的符号就够了。竖直这一类的 `|θ|` 在 45°~135° 之间，`sinθ` 不会
            // 落在 0 附近，不存在两侧都在的临界值。
            //
            // 竖判定线的轴就是屏幕纵轴，所以这个高度原样就是它在竖线上的偏移；局部横轴在 ±90° 旋转后
            // 指向世界纵轴，两条线的符号相反，按侧别各取一次。
            //
            // 判定线**下方**那一趟（`above` 为假）的按键是反着落的，侧别对调：原本要贴左的改贴右、
            // 原本贴右的改贴左，它才会从自己那一侧落向判定线。
            let below = !note.above;
            let index = usize::from((sin > 0.) != below);
            note.object.translation.0 = AnimFloat::fixed(if index == 1 { y } else { -y });
            finish_note(&mut note, speed);
            side_notes[index].push(note);
            continue;
        }
        // 横判定线上的按键只要横向坐标：判定线已经拍平、转角归零，位移直接就是屏幕横坐标。
        flatten_note(&mut note, x, speed);
        kept.push(note);
    }
    line.notes = kept;

    // 世界坐标里 1 单位 = 半个屏幕宽（横向）/ 半个屏幕高（纵向），`(0., 0.)` 是屏幕正中央。
    line.object.translation = AnimVector::fixed(Vector::new(0., -1. + line_y * 2.));
    line.object.rotation = AnimFloat::fixed(0.);
    // 缩放也归 1：它会把按键相对判定线的位移再乘一遍，固化下来的横向坐标就不落在该在的位置上。
    line.object.scale = AnimVector::fixed(Vector::new(1., 1.));
    // 恒定 1：空动画同样会被 `now_alpha()` 取成 1.0，但写死一个关键帧可以避开那条回落分支。
    line.object.alpha = AnimFloat::fixed(1.);
    line.height = speed_curve(end_time, speed);
    // 父级（含「随父旋转」）会把拍平后的线再一次带走。
    line.parent = None;
    line.rotate_with_parent = false;
    // `above` 与按键流速全变过，缓存里按 `above` / `speed` 分的桶全部失效，必须重建。
    line.cache = JudgeLineCache::new(&mut line.notes);
}

/// 把按键钉在横向坐标 `x` 上，纵向按共用的速度积分拍平（「定轨谱面」也走这一份）。
pub(crate) fn flatten_note(note: &mut Note, x: f32, speed: f64) {
    note.object.translation.0 = AnimFloat::fixed(x);
    finish_note(note, speed);
}

/// 按键离开所属判定线的那一份距离：与判定线共用同一条常量速度积分。
fn finish_note(note: &mut Note, speed: f64) {
    // 按键自己的纵向偏移跟着判定线一起拍掉，只留下离开判定线的那一份横向偏移。
    note.object.translation.1 = AnimFloat::fixed(0.);
    // 纵向上与判定线共用同一条常量速度积分：按键在自己的时刻正好在判定线高度上。
    note.height = height_at(note.time, speed);
    // 点按 / 拖动 / 滑动固定 1（参考实现写回谱面的就是这个值），长按另算。
    note.speed = 1.;
    if let NoteKind::Hold { end_time, end_height, end_speed } = &mut note.kind {
        *end_height = height_at(*end_time, speed);
        *end_speed = Some(speed);
    }
    // 判定线下方那一趟是镜像绘制的，拍平后没有「上方 / 下方」之分，全部并到上方。
    note.above = true;
}

/// 造一条竖判定线：判定线本体与谱面里没贴图的那种一样（`Normal`），按键在它上面沿判定线方向铺开。
///
/// `rotation` 取 ±90°、`translation` 的 y 取屏幕正中：横判定线上「离开判定线」的方向是局部 +y，
/// 转 -90° 后它指向 +x、转 +90° 后指向 -x，于是左侧那条线上的按键从屏幕中间向左落下、右侧那条线
/// 上的按键从屏幕中间向右落下，判定的那一下正好落在线上。
fn side_line(x: f32, rotation: f32, notes: Vec<Note>, end_time: f64, speed: f64) -> JudgeLine {
    let mut notes = notes;
    let cache = JudgeLineCache::new(&mut notes);
    JudgeLine {
        object: Object {
            alpha: AnimFloat::fixed(1.),
            scale: AnimVector::fixed(Vector::new(1., 1.)),
            rotation: AnimFloat::fixed(rotation),
            translation: AnimVector::fixed(Vector::new(x, 0.)),
        },
        // 空颜色动画 ⇒ 用配置里的判定线颜色，与谱面里没写颜色事件的判定线一致。
        color: Anim::default(),
        ctrl_obj: RefCell::default(),
        kind: JudgeLineKind::default(),
        height: speed_curve(end_time, speed),
        incline: AnimFloat::default(),
        notes,
        parent: None,
        rotate_with_parent: false,
        z_index: 0,
        show_below: true,
        attach_ui: None,
        cache,
        anchor: [0.5, 0.5],
    }
}

/// 判定线高度积分：只有一条常量速度事件时就是 `height(t) = t * speed / HEIGHT_RATIO`，与
/// `parse::pgr::parse_speed_events` 对单条速度事件的算法逐字一致（中点取线性补间，末帧的补间函数
/// 用不上）。
pub(crate) fn speed_curve(end_time: f64, speed: f64) -> AnimFloatF64 {
    AnimFloatF64::new(vec![Keyframe::new(0., 0., 2), Keyframe::new(end_time, height_at(end_time, speed), 0)])
}

/// 判定时刻的角度是不是「竖直」那一类：是就贴到某条竖判定线上（贴哪条由 `bake_line` 按落下的方向定）。
///
/// 判定线转 θ 与转 θ ± 180° 画出来是同一条线，所以角度先折算到 `(-180, 180]`；于是「与水平方向
/// 相差 `angle` 度以内」就是 `[-angle, angle]` 与 `[180 - angle, 180]` 两段（默认 45 度，
/// 也就是 -45~45 与 135~225），留在横判定线上，中间那两段是竖着的。这里只判「是不是竖的」，
/// 侧别交给 `sinθ` 的符号（朝向的横向分量）。
fn is_side_note(angle: f32, threshold: f32) -> bool {
    let threshold = threshold.clamp(0., 90.);
    let mut normalized = angle % 360.;
    if normalized > 180. {
        normalized -= 360.;
    } else if normalized <= -180. {
        normalized += 360.;
    }
    let magnitude = normalized.abs();
    magnitude > threshold && magnitude < 180. - threshold
}

/// 判定线高度积分在 `time` 时刻的值。
fn height_at(time: f64, speed: f64) -> f64 {
    time * speed / HEIGHT_RATIO
}

/// 按键在**判定时刻**的屏幕位置是不是落在谱面的噪域里。
///
/// 是否「被噪域盖住」直接问 `block_area` 自己那套判定（与触摸遮挡同一条通路：奇偶规则、减算方块
/// 记数），所以判定范围让位、模板遮挡这些语义都与正常渲染一致。传进来的 `(x, y)` 已经是 chart
/// 空间的坐标（纵向在 [`bake_line`] 里就折过 `1 / aspect`），方块几何正是那个空间里解算的。
fn in_noise(noise: &Noise, time: f64, x: f32, y: f32, aspect: f32) -> bool {
    // 「纯配置谱面」只处理原生百分比噪域，rpe-next 噪域（第二个参数）这里不参与。
    !noise.areas.is_empty() && crate::core::block_touch_blocked(noise.areas, &[], Vector::new(x, y), time, aspect)
}

/// 一条判定线在任意时刻的取值器：按「按键自己的时刻」求判定线的横向位移、旋转与缩放。
///
/// `Anim` 自带游标，从原动画上按任意时刻取值会把游标推到别处，所以先把要用到的动画克隆出来；克隆
/// 同时保留了 `next` 叠加链（RPE 的嵌套事件就靠它）。`AnimVector` 没有实现 `Clone`，缩放只能拆成
/// 两个 `AnimFloat` 各自克隆。
pub(crate) struct LineEvaluator {
    x: AnimFloat,
    y: AnimFloat,
    rotation: AnimFloat,
    scale_x: AnimFloat,
    scale_y: AnimFloat,
    incline: AnimFloat,
}

impl LineEvaluator {
    pub(crate) fn new(line: &JudgeLine) -> Self {
        Self {
            x: line.object.translation.0.clone(),
            y: line.object.translation.1.clone(),
            rotation: line.object.rotation.clone(),
            scale_x: line.object.scale.0.clone(),
            scale_y: line.object.scale.1.clone(),
            incline: line.incline.clone(),
        }
    }

    /// 按键在 `time` 时刻的 chart 空间坐标 `(x, y)`，以及那一刻判定线的角度（度）。
    ///
    /// 判定线这一步的矩阵是 `R(θ)·T(pos)` 之后叠一层 `Object::now_scale`（见 `JudgeLine::render`），
    /// 按键自己那一步是 `S(note.scale)·T(tr)`；判定时刻 `base` 正好为 0，所以
    /// `tr = (off * incline_val * ctrl_obj.pos, off_v / aspect)`（`ctrl_obj.pos` / `size` 这些动画
    /// 没人写过，恒等于 `unwrap_or(1.)`）。三个容易漏的量：
    ///
    /// - 判定线的缩放 `line.object.scale`：谱面会拿它做整条线的放大缩小，漏了位置就按倍率偏；
    /// - `translation.1 / aspect`：纵向在 `Object::now_translation` 里是折过纵横比的；
    /// - `incline_val`：RPE 的旧特性，判定线带倾角时按键的横向偏移按它在判定线上方的高度收窄，
    ///   只有 `translation.1`（RPE 的 `yOffset`）不是 0 时才起作用。
    pub(crate) fn eval(&mut self, time: f64, off: f32, off_v: f32, aspect: f32) -> (f32, f32, f32) {
        self.x.set_time(time);
        self.y.set_time(time);
        self.rotation.set_time(time);
        self.scale_x.set_time(time);
        self.scale_y.set_time(time);
        self.incline.set_time(time);
        let angle = self.rotation.now();
        let radians = angle.to_radians();
        let (cos, sin) = (radians.cos(), radians.sin());
        let incline_sin = self.incline.now_opt().map(|it| it.to_radians().sin()).unwrap_or_default();
        let incline_val = 1. - incline_sin * off_v * crate::parse::RPE_HEIGHT / 2. / 360.;
        let local_x = off * incline_val * self.scale_x.now_opt().unwrap_or(1.);
        let local_y = off_v / aspect * self.scale_y.now_opt().unwrap_or(1.);
        let x = self.x.now() + local_x * cos - local_y * sin;
        let y = self.y.now() / aspect + local_x * sin + local_y * cos;
        (x, y, angle)
    }
}
