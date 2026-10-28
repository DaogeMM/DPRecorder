//! 「定轨谱面」：把整张谱面拍平成**一条横向基准线 + 若干条固定轨道**，每个按键在判定时刻落到某条
//! 轨道的横坐标上，纵向照旧沿共用的速度积分落下来。
//!
//! 与「纯配置谱面」同一套求值口径：按键在自己时刻的屏幕位置由判定线那一刻的位移 / 旋转 / 缩放决定
//! （见 [`super::pure_config::LineEvaluator`]），区别只在落点 —— 纯配置保留按键原来的横向坐标，这里
//! 把横向坐标**吸附到轨道**上，于是同一时刻的按键不会堆在同一处。
//!
//! 轨道以屏幕正中那条为 `0` 号，向左右各 [`LANE_PER_SIDE_MAX`] 条，相邻两条相距 `gap`（屏宽的几成）。
//! 正中那条 **固定留给** [`Flick`](NoteKind::Flick) 与 [`Drag`](NoteKind::Drag)：这两个键型一律钉在
//! 横坐标为 `0` 的位置，不看原落点；点按与长按永远不进正中那条。同一轨道上两个按键之间至少留
//! `interval` 秒（长按算到尾巴）。
//!
//! 侧别（左 / 右）按**左右交替**排：上一个事件落在左侧，这一个事件就优先放右侧，反之亦然 —— 这样连续
//! 按键是「左一下右一下」，两侧手压力相当。**同一时刻**的按键（双押、三押）算一个事件，整组先往同一侧排
//! （本侧从内到外），塞满了才轮到另一侧，所以双押基本都落在同一只手上。
//!
//! 同一侧内按**手指顺序**由内到外挑：[`LANE_PER_SIDE`] 是食指（`±1`），其次中指（`±2`），最外侧
//! [`LANE_PER_SIDE_MAX`] 是必要时才用的无名指（`±3`）。某一侧里内层轨道还留着 `< interval` 的按键就顺延到
//! 外一层；一侧三条全满才轮到另一侧（这是唯一换侧的理由，否则按键会左右乱窜）。关掉 `third` 时无名指那一
//! 条不参与，排不下的叠回本侧最近的一条（允许重叠）。
//!
//! 这个模式不画噪域（方块与「强制定位」的挖洞都关掉，见 `Chart::render_block_area`）：谱面拍平之后
//! 那些几何都对不上画面。
//!
//! 这个模式不画噪域（方块与「强制定位」的挖洞都关掉，见 `Chart::render_block_area`）：谱面拍平之后
//! 那些几何都对不上画面。

use crate::core::{
    AnimFloat, AnimVector, JudgeLine, JudgeLineCache, NoteKind, Vector,
};

use super::pure_config::{flatten_note, speed_curve, LineEvaluator};

/// 单侧先排这么多条轨道（食指、中指）；关掉第三轨道时这就是单侧上限。
const LANE_PER_SIDE: i32 = 2;

/// 单侧最多这么多条：内层都排不下（或按键间隔不够）才用到最外侧这条（无名指）。
const LANE_PER_SIDE_MAX: i32 = 3;

/// 轨道下标换成 `lane_end` 里的位置：`-LANE_PER_SIDE_MAX` 落在 0。
fn slot(index: i32) -> usize {
    (index + LANE_PER_SIDE_MAX) as usize
}

/// 一个按键在判定时刻的落点，以及挑轨道需要的信息。
struct Entry {
    line: usize,
    note: usize,
    time: f64,
    end: f64,
    center_ok: bool,
    x: f32,
}

/// 在一侧（`sign` 为 `1` 是右、`-1` 是左）把这一组按键全部排下，返回各自轨道下标。
///
/// 排不下（那侧还留着不到间隔的按键、或轨道条数不够）返回 `None`；`lane_end` 只在副本上试，
/// 失败不会污染已有排布。
fn fit_on_side(
    sign: i32,
    per_side: i32,
    notes: &[&Entry],
    lane_end: &[f64],
    interval: f64,
) -> Option<Vec<i32>> {
    let mut ends = lane_end.to_vec();
    let mut placed = Vec::with_capacity(notes.len());
    for entry in notes {
        let lane = (1..=per_side)
            .map(|lane| sign * lane)
            .find(|index| entry.time >= ends[slot(*index)] + interval)?;
        ends[slot(lane)] = entry.end;
        placed.push(lane);
    }
    Some(placed)
}

/// 轨道参数（齿轮配置里那几项）。
#[derive(Clone, Copy)]
pub struct Lanes {
    /// 相邻轨道的间隔，屏宽的几成：`0.1` 就是相邻两条相距屏幕宽度的十分之一。
    pub gap: f32,
    /// 判定线钉在屏幕纵向的哪个高度：`0` 是屏幕底边、`1` 是屏幕顶边、`0.5` 是正中。
    pub line_y: f32,
    /// 按键流速：越大落得越快（判定线高度积分与长按尾端都按它算）。
    pub speed: f64,
    /// 同一轨道上两个按键之间至少留几秒（长按按尾巴算）。
    pub interval: f64,
    /// 是否启用单侧第 3 条轨道：关掉时常规 4 条排不下的按键会叠在最近的轨道上。
    pub third: bool,
}

/// 把整张谱面拍平到轨道上。
///
/// 挂 UI 元件的判定线不参与（它们不画、只给连击数这类元件提供变换）。谱面文件不动，只改内存里刚
/// 解析出来的 [`JudgeLine`]，所以同一个开关随时可以关掉、照原样再渲染。
///
/// `aspect` 是屏幕纵横比：求按键的纵向落点必须折它（`Object::now_translation` 就是这么折的），
/// 而转换发生在 `GameScene::load_chart` 里、那里还没有 `Resource`，所以要调用方给。
pub fn apply(lines: &mut [JudgeLine], lanes: &Lanes, aspect: f32) {
    let end_time = lines
        .iter()
        .flat_map(|line| line.notes.iter())
        .map(|note| match note.kind {
            NoteKind::Hold { end_time, .. } => end_time.max(note.time),
            _ => note.time,
        })
        .fold(0.0_f64, f64::max)
        + 1.0;

    // 世界坐标里横向 1 单位 = 半个屏幕宽，所以「屏宽的几成」换成世界坐标要乘 2。
    let step = lanes.gap.clamp(0., 1.) * 2.;
    let interval = lanes.interval.max(0.);
    // 轨道下标从 `-LANE_PER_SIDE_MAX` 到 `+LANE_PER_SIDE_MAX`，正中那条（0）夹在中间。
    let span = LANE_PER_SIDE_MAX as usize;

    // 1. 先求每个按键在**判定时刻**的落点。横向位置只用来挑轨道；纵向交给基准线与速度积分。
    let mut entries = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.attach_ui.is_some() {
            continue;
        }
        let mut evaluator = LineEvaluator::new(line);
        for (slot, note) in line.notes.iter().enumerate() {
            let time = note.time;
            let mut off_x = note.object.translation.0.clone();
            let mut off_y = note.object.translation.1.clone();
            off_x.set_time(time);
            off_y.set_time(time);
            let (x, _, _) = evaluator.eval(time, off_x.now(), off_y.now(), aspect);
            entries.push(Entry {
                line: index,
                note: slot,
                time,
                end: match note.kind {
                    NoteKind::Hold { end_time, .. } => end_time.max(time),
                    _ => time,
                },
                center_ok: matches!(note.kind, NoteKind::Flick | NoteKind::Drag),
                x,
            });
        }
    }
    // 按时间先后挑轨道，这样「同轨道间隔」只需要看这条轨道上一个按键的结束时刻。
    entries.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));

    // 2. 逐个事件挑轨道。同一时刻的按键算一组（双押、三押），整组先往同一侧排。
    let lane_x = |index: i32| index as f32 * step;
    let mut lane_end = vec![f64::NEG_INFINITY; span * 2 + 1];
    let mut places: Vec<i32> = Vec::with_capacity(entries.len());
    // 一侧能用到第几条：关掉第三轨道时两侧都只有食指、中指两条。
    let per_side = if lanes.third { LANE_PER_SIDE_MAX } else { LANE_PER_SIDE };
    // 上一个事件落在哪一侧：交替排的游标，`true` 是右。
    let mut previous_right = false;
    let mut started = false;
    let mut start = 0;
    while start < entries.len() {
        // 同一帧的按键（时间差在一个浮点容差内）合成一组：整组尽量塞进同一只手。
        let time = entries[start].time;
        let mut end = start + 1;
        while end < entries.len() && (entries[end].time - time).abs() < 1e-4 {
            end += 1;
        }
        // 第一条按原落点定侧别，之后每个事件左右交替：连续按键「左一下右一下」。
        let preferred = if started { !previous_right } else { entries[start].x >= 0. };
        started = true;
        // 候选顺序：本侧由内到外（食指 → 中指 → 无名指），排不下了才轮到另一侧。
        let sign = if preferred { 1 } else { -1 };
        let mut candidates: Vec<i32> = (1..=per_side).map(|lane| sign * lane).collect();
        candidates.extend((1..=per_side).map(|lane| -sign * lane));
        // 先试「整组塞进一只手」：本侧优先，其次另一侧；两边都塞不下才拆开。
        let hand_notes: Vec<&Entry> =
            entries[start..end].iter().filter(|entry| !entry.center_ok).collect();
        let mut whole: Vec<i32> = Vec::new();
        if !hand_notes.is_empty() {
            if let Some(fit) = fit_on_side(sign, per_side, &hand_notes, &lane_end, interval) {
                whole = fit;
            } else if let Some(fit) =
                fit_on_side(-sign, per_side, &hand_notes, &lane_end, interval)
            {
                whole = fit;
            }
        }
        let mut hand_used = false;
        for entry in &entries[start..end] {
            let free = |index: &i32| entry.time >= lane_end[slot(*index)] + interval;
            let index = if entry.center_ok {
                // Flick / Drag 一律钉在正中那条轨道（`positionX` 为 0），不看原落点也不看间隔。
                0
            } else {
                hand_used = true;
                if whole.is_empty() {
                    // 一只手塞不下整组：本侧由内到外，排不下了才轮到另一侧，最后叠本侧食指。
                    candidates.iter().copied().find(free).unwrap_or_else(|| sign)
                } else {
                    whole.remove(0)
                }
            };
            lane_end[slot(index)] = entry.end;
            places.push(index);
        }
        if hand_used {
            previous_right = preferred;
        }
        start = end;
    }

    // 3. 写回落点，再把判定线拍平。
    for (entry, index) in entries.iter().zip(&places) {
        flatten_note(&mut lines[entry.line].notes[entry.note], lane_x(*index), lanes.speed);
    }
    let base = -1. + lanes.line_y.clamp(0., 1.) * 2.;
    for line in lines.iter_mut() {
        if line.attach_ui.is_some() {
            continue;
        }
        line.object.translation = AnimVector::fixed(Vector::new(0., base));
        line.object.rotation = AnimFloat::fixed(0.);
        // 缩放归 1：它会再乘一遍按键相对判定线的位移，吸附到轨道上的横坐标就不准了。
        line.object.scale = AnimVector::fixed(Vector::new(1., 1.));
        // 恒定 1：空动画同样会被 `now_alpha()` 取成 1.0，但写死一个关键帧可以避开那条回落分支。
        line.object.alpha = AnimFloat::fixed(1.);
        line.height = speed_curve(end_time, lanes.speed);
        // 父级（含「随父旋转」）会把拍平后的线再一次带走。
        line.parent = None;
        line.rotate_with_parent = false;
        // `above` 与按键流速全变过，缓存里按 `above` / `speed` 分的桶全部失效，必须重建。
        line.cache = JudgeLineCache::new(&mut line.notes);
    }
}
