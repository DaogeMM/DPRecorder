crate::tl_file!("parser" ptl);

use super::process_lines;
use crate::{
    core::{
        Anim, AnimFloat, AnimFloatF64, AnimVector, BpmList, Chart, ChartExtra, ChartSettings, JudgeLine, JudgeLineCache, JudgeLineKind, Keyframe, Note, NoteKind, Object, TweenId, HEIGHT_RATIO
    },
    ext::NotNanExt,
    judge::{HitSound, JudgeStatus},
};
use anyhow::{Context, Result};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::{cell::RefCell};
use tracing::warn;
use macroquad::prelude::{Color, WHITE};

#[derive(Clone, Deserialize, Serialize)]
pub struct RGBColor(u8, u8, u8);

impl Default for RGBColor {
    fn default() -> Self {
        Self(255, 255, 255)
    }
}

impl From<RGBColor> for Color {
    fn from(RGBColor(r, g, b): RGBColor) -> Self {
        Self::from_rgba(r, g, b, 255)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PgrEvent {
    pub start_time: f64,
    pub end_time: f64,
    pub start: f32,
    pub end: f32,
    #[serde(default)]
    pub start2: f32,
    #[serde(default)]
    pub end2: f32,
    /// fv2 才写的缓动码（x 轴 / 单轴）。取值见 [`fv2_tween`]，fv3 的事件不带这两个字段。
    #[serde(default)]
    pub ease_type: f32,
    /// fv2 move 事件的 y 轴缓动码。
    #[serde(default)]
    pub ease_type2: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PgrSpeedEvent {
    pub start_time: f64,
    pub end_time: f64,
    pub value: f64,
    /// fv2 才写的累计高度：该事件起点处按键已经走过的 floor position。
    /// fv1 / fv3 没有这个字段，用 `Option` 才能区分「没写」和「写了 0」（写了 0 表示起点确实是 0）。
    /// 保留 `f64`：谱面里的高度能到几万，先降到 f32 会丢末位，按键高度跟着差几个 0.001。
    #[serde(default)]
    pub floor_position: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PgrNote {
    #[serde(rename = "type")]
    kind: u8,
    time: f64,
    position_x: f32,
    hold_time: f64,
    speed: f64,
    /// fv2 才写的「判定线在这一刻的滚动速度」：v2 里每个按键的 `speed` 恒为 1.0，真实速度写在这里。
    /// v3 的烘焙版把同一个值写进**长条的** `speed`（其余按键仍然是 1.0），所以只有 fv2 需要回退到它。
    #[serde(default)]
    head_speed: Option<f64>,
    #[allow(unused)] floor_position: f32,
    color: Option<RGBColor>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PgrJudgeLine {
    bpm: f64,
    #[serde(rename = "judgeLineDisappearEvents")]
    alpha_events: Vec<PgrEvent>,
    #[serde(rename = "judgeLineRotateEvents")]
    rotate_events: Vec<PgrEvent>,
    #[serde(rename = "judgeLineMoveEvents")]
    move_events: Vec<PgrEvent>,
    speed_events: Vec<PgrSpeedEvent>,

    notes_above: Vec<PgrNote>,
    notes_below: Vec<PgrNote>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PgrChart {
    format_version: u32,
    offset: f64,
    judge_line_list: Vec<PgrJudgeLine>,
    /// Phigros 4.0 剧情遮挡区域。`blockAreaList` 的 `time` 单位是**秒**，
    /// 不做 T(拍) 换算，所以这里原样保留 JSON，交给 `block_area` 模块解析。
    #[serde(default)]
    block_area_list: Option<serde_json::Value>,
}

macro_rules! validate_events {
    ($pgr:expr) => {
        $pgr.retain(|it| {
            if it.start_time > it.end_time {
                warn!("invalid time range, ignoring");
                false
            } else {
                true
            }
        });
        /*
        for i in 0..($pgr.len() - 1) {
            if $pgr[i].end_time != $pgr[i + 1].start_time {
                ptl!(bail "event-not-contiguous");
            }
        }*/
        // if $pgr.last().unwrap().end_time <= 900000000.0 {
        // bail!("End time is not great enough ({})", $pgr.last().unwrap().end_time);
        // }
    };
}

/// fv2 的事件只写起点：`endTime` 恒为 0，真正的终点是下一条的 `startTime`。
/// 不补终点的话，这些条目会先被 `validate_events!` 当成「起点晚于终点」丢掉 ——
/// 判定线只会剩第一帧、整条线冻在原地，按键高度也全是 0。
macro_rules! fill_missing_end_times {
    ($events:expr) => {
        for i in 0..$events.len() {
            if $events[i].end_time <= $events[i].start_time {
                let start = $events[i].start_time;
                // 最后一条没有下一条可接，收成零长（起点那一帧仍然有效），否则会被校验丢掉。
                $events[i].end_time = $events.get(i + 1).map_or(start, |next| next.start_time);
            }
        }
    };
}

fn parse_speed_events(r: f64, mut pgr: Vec<PgrSpeedEvent>, max_time: f64) -> Result<(AnimFloatF64, AnimFloatF64)> {
    // 空的关键帧动画是合法的（`Anim::now` 返回默认值），而这类线通常只是装饰：fv2 官谱里有 8 条
    // 既没有速度事件也没有按键的线，先挡住再谈首条。
    if pgr.is_empty() {
        return Ok((AnimFloatF64::default(), AnimFloatF64::default()));
    }
    validate_events!(pgr);
    // fv2 的首条事件起点不一定是 0，它的 `floorPosition` 就是那一刻的累计高度：
    // 这种情况既能用它当积分的起点，也不能把起点挪到 0（那会把整条线的按键高度整体挪错）。
    let first_offset = pgr[0].floor_position.unwrap_or(0.);
    if pgr[0].floor_position.is_none() && pgr[0].start_time != 0. {
        pgr[0].start_time = 0.;
    }
    let mut kfs = Vec::new();
    let mut pos = first_offset;
    kfs.extend(pgr[..pgr.len().saturating_sub(1)].iter().map(|it| {
        let from_pos = pos;
        pos += (it.end_time - it.start_time) * r * it.value;
        Keyframe::new(it.start_time * r, from_pos, 2)
    }));
    let last = pgr.last().unwrap();
    kfs.push(Keyframe::new(last.start_time * r, pos, 2));
    kfs.push(Keyframe::new(max_time, pos + (max_time - last.start_time * r) * last.value, 0));
    for kf in &mut kfs {
        kf.value /= HEIGHT_RATIO;
    }
    Ok((
        AnimFloatF64::new(pgr.iter().map(
            |it| Keyframe::new(it.start_time * r, it.value, 0)
        ).collect()), 
        AnimFloatF64::new(kfs)
    ))
}

fn parse_float_events(r: f64, mut pgr: Vec<PgrEvent>) -> Result<AnimFloat> {
    validate_events!(pgr);
    let mut kfs = Vec::<Keyframe<f32>>::new();
    for e in pgr {
        if !kfs.last().map_or(false, |it| it.value == e.start) {
            kfs.push(Keyframe::new((e.start_time * r).max(0.), e.start, 2));
        }
        kfs.push(Keyframe::new(e.end_time * r, e.end, 2));
    }
    kfs.pop();
    Ok(AnimFloat::new(kfs))
}

fn parse_move_events(r: f64, mut pgr: Vec<PgrEvent>) -> Result<AnimVector> {
    validate_events!(pgr);
    let mut kf1 = Vec::<Keyframe<f32>>::new();
    let mut kf2 = Vec::<Keyframe<f32>>::new();
    for e in pgr {
        let st = (e.start_time * r).max(0.);
        let en = e.end_time * r;
        if !kf1.last().map_or(false, |it| it.value == e.start) {
            kf1.push(Keyframe::new(st, e.start, 2));
        }
        if !kf2.last().map_or(false, |it| it.value == e.start2) {
            kf2.push(Keyframe::new(st, e.start2, 2));
        }
        kf1.push(Keyframe::new(en, e.end, 2));
        kf2.push(Keyframe::new(en, e.end2, 2));
    }
    kf1.pop();
    kf2.pop();
    for kf in &mut kf1 {
        kf.value = -1. + kf.value * 2.;
    }
    for kf in &mut kf2 {
        kf.value = -1. + kf.value * 2.;
    }
    Ok(AnimVector(AnimFloat::new(kf1), AnimFloat::new(kf2)))
}

fn parse_move_events_fv1(r: f64, mut pgr: Vec<PgrEvent>) -> Result<AnimVector> {
    validate_events!(pgr);
    let mut kf1 = Vec::<Keyframe<f32>>::new();
    let mut kf2 = Vec::<Keyframe<f32>>::new();
    for e in pgr {
        let st = (e.start_time * r).max(0.);
        let en = e.end_time * r;
        if !kf1.last().map_or(false, |it| it.value == e.start) {
            let start = (e.start - e.start % 1000.) / 1000.;
            kf1.push(Keyframe::new(st, start, 2));
        }
        // 注意：fv1 的 y 是从 start/end 里解包出来的（`v % 1000`），
        // `e.start2` 这种独立字段在 fv1 里不存在（serde 默认恒为 0.0），
        // 拿它和已解包的 y 比大小会误判成「同值」而丢掉这一帧，
        // 于是 y 会从 -1 一路缓动回 0（判定线/音符纵向错位）。
        let start2 = e.start % 1000.;
        if !kf2.last().map_or(false, |it| it.value == start2) {
            kf2.push(Keyframe::new(st, start2, 2));
        }
        let end = (e.end - e.end % 1000.) / 1000.;
        let end2 = e.end % 1000.;
        kf1.push(Keyframe::new(en, end, 2));
        kf2.push(Keyframe::new(en, end2, 2));
    }
    kf1.pop();
    kf2.pop();
    for kf in &mut kf1 {
        kf.value = (-880. + kf.value * 2.) / 880.;
    }
    for kf in &mut kf2 {
        kf.value = (-520. + kf.value * 2.) / 520.;
    }
    Ok(AnimVector(AnimFloat::new(kf1), AnimFloat::new(kf2)))
}

/// fv2 的 `easeType` → `TWEEN_FUNCTIONS` 下标。
///
/// 数值顺序是「线性 → 二次 / 三次 / 四次 / 五次的 in / out / inout → 正弦三档 → 恒 0 / 恒 1」，
/// 与 `TWEEN_FUNCTIONS`（线性 2、sine 3/4/5、quad 6/7/8、cubic 9/10/11、quart 12/13/14、
/// quint 15/16/17）**不是同一顺序**，所以不能直接拿码当下标：`1..=9` 是 `code + 5`，
/// 但 `10/11/12` 是 sine（3/4/5）而不是 quint。
fn fv2_tween(code: f32) -> TweenId {
    match code as i32 {
        0 => 2,
        1..=9 => code as u8 + 5,
        10 => 3,
        11 => 4,
        12 => 5,
        13 => 0,
        14 => 1,
        _ => 2,
    }
}

/// 往 fv2 的关键帧表里追加一个节点。
///
/// fv2 的事件按时间有序，但相邻两条可能落在同一拍上（同一时刻连跳两次）：同刻只保留后一条，
/// 否则 `Anim::now` 会在零长区间上 `(t - kf1.time) / (kf2.time - kf1.time)` 除零得到 NaN。
fn push_fv2_keyframe<T>(kfs: &mut Vec<Keyframe<T>>, time: f64, value: T, tween: TweenId) {
    if let Some(last) = kfs.last_mut() {
        if time <= last.time {
            *last = Keyframe::new(last.time, value, tween);
            return;
        }
    }
    kfs.push(Keyframe::new(time, value, tween));
}

/// fv2 的判定线事件只写一个节点：每条事件从自己的 `startTime` 出发，按 `easeType`
/// 缓动到下一条事件的 `start`，整段就是 `[startTime, 下一条 startTime)`。
///
/// `endTime`（恒 0）与 `end` / `end2` 都是废字段：同一张谱的 fv3 版本正是按上面这个语义
/// 烘焙出来的（逐帧比对平均误差 0.002），照 fv3 的写法去读 `end` 会把每段都压成瞬间跳变。
/// 值相同的事件也不能省 —— 它可能是下一段缓动的起点（官方谱里就有「先停在 0，下一拍再
/// 转 360°」这种：省掉中间那一条，整段旋转会退回上一个关键帧的缓动）。
fn parse_move_events_fv2(r: f64, mut pgr: Vec<PgrEvent>) -> Result<AnimVector> {
    validate_events!(pgr);
    let mut kf1 = Vec::<Keyframe<f32>>::new();
    let mut kf2 = Vec::<Keyframe<f32>>::new();
    for e in pgr {
        let st = (e.start_time * r).max(0.);
        push_fv2_keyframe(&mut kf1, st, e.start, fv2_tween(e.ease_type));
        push_fv2_keyframe(&mut kf2, st, e.start2, fv2_tween(e.ease_type2));
    }
    for kf in &mut kf1 {
        kf.value = -1. + kf.value * 2.;
    }
    for kf in &mut kf2 {
        kf.value = -1. + kf.value * 2.;
    }
    Ok(AnimVector(AnimFloat::new(kf1), AnimFloat::new(kf2)))
}

/// 单轴事件（alpha / rotate）的 fv2 版本，语义同 [`parse_move_events_fv2`]。
fn parse_float_events_fv2(r: f64, mut pgr: Vec<PgrEvent>) -> Result<AnimFloat> {
    validate_events!(pgr);
    let mut kfs = Vec::<Keyframe<f32>>::new();
    for e in pgr {
        push_fv2_keyframe(&mut kfs, (e.start_time * r).max(0.), e.start, fv2_tween(e.ease_type));
    }
    Ok(AnimFloat::new(kfs))
}

fn parse_notes(r: f64, mut pgr: Vec<PgrNote>, _speed: &mut AnimFloatF64, height: &mut AnimFloatF64, above: bool) -> Result<Vec<Note>> {
    // is_sorted is unstable...
    if pgr.is_empty() {
        return Ok(Vec::new());
    }
    pgr.sort_unstable_by_key(|it| it.time.not_nan());
    pgr.into_iter()
        .map(|pgr| {
            let time = pgr.time * r;
            let height = {
                height.set_time(time);
                height.now()
            };
            let kind = match pgr.kind {
                1 => NoteKind::Click,
                2 => NoteKind::Drag,
                3 => {
                    // 长条长度 = 按住期间走过的 floor 距离。官方把「那一刻的判定线速度」烘焙进了
                    // hold 的 `speed`（fv3 就是这样），fv2 则把同一个值放在 `headSpeed` 里、`speed`
                    // 恒为 1.0 —— 只看 `speed` 会让 fv2 的长条短成实际的一半（速度 2.0 的段）。
                    let speed = pgr.head_speed.unwrap_or(pgr.speed);
                    let end_time = (pgr.time + pgr.hold_time) * r;
                    let end_height = height + (pgr.hold_time * speed * r / HEIGHT_RATIO);
                    let end_speed = Some(speed);
                    NoteKind::Hold { end_time, end_height, end_speed }
                }
                4 => NoteKind::Flick,
                _ => ptl!(bail "unknown-note-type", "type" => pgr.kind),
            };
            let hitsound = HitSound::default_from_kind(&kind);
            
            let note_color = match pgr.color {
                Some(RGBColor(r, g, b)) => Color::from_rgba(r, g, b, 255),
                None => WHITE
            };
            
            let color_anim = if matches!(note_color, WHITE) {
                Anim::default()
            } else {
                Anim::fixed(note_color)
            };

            Ok(Note {
                object: Object {
                    translation: AnimVector(AnimFloat::fixed(pgr.position_x * (2. * 9. / 160.)), AnimFloat::default()),
                    ..Default::default()
                },
                kind,
                hitsound,
                time,
                speed: if pgr.kind == 3 {
                    1.
                } else {
                    pgr.speed
                },
                height,

                above,
                multiple_hint: false,
                fake: false,
                judge: JudgeStatus::NotJudged,
                judge_scale: 1.0,
                color: color_anim.clone(),
                hit_fx_color: color_anim,
                protected: false,
                hidden: false,
                pure_config_noise: false,
                water: Default::default(),
            })
        })
        .collect()
}

fn parse_judge_line(mut pgr: PgrJudgeLine, max_time: f64, format_version: u32) -> Result<JudgeLine> {
    let r = 60. / 32. / pgr.bpm;
    // fv2 的事件只写起点（`endTime` 恒为 0），先把真正的终点补上，后面三条解析路径才能共用。
    if format_version == 2 {
        fill_missing_end_times!(pgr.alpha_events);
        fill_missing_end_times!(pgr.rotate_events);
        fill_missing_end_times!(pgr.move_events);
        fill_missing_end_times!(pgr.speed_events);
    }
    let (mut speed, mut height) = parse_speed_events(r, pgr.speed_events, max_time).context("Failed to parse speed events")?;
    let notes_above = parse_notes(r, pgr.notes_above, &mut speed, &mut height, true).context("Failed to parse notes above")?;
    let mut notes_below = parse_notes(r, pgr.notes_below, &mut speed, &mut height, false).context("Failed to parse notes below")?;
    let mut notes = notes_above;
    notes.append(&mut notes_below);
    let cache = JudgeLineCache::new(&mut notes);
    Ok(JudgeLine {
        object: Object {
            alpha: match format_version {
                2 => parse_float_events_fv2(r, pgr.alpha_events),
                _ => parse_float_events(r, pgr.alpha_events),
            }.with_context(|| ptl!("alpha-events-parse-failed"))?,
            scale: AnimVector(AnimFloat::fixed(5.75 / 6.0), AnimFloat::default()),
            rotation: match format_version {
                2 => parse_float_events_fv2(r, pgr.rotate_events),
                _ => parse_float_events(r, pgr.rotate_events),
            }.with_context(|| ptl!("rotate-events-parse-failed"))?,
            translation: {
                match format_version {
                    1 => parse_move_events_fv1(r, pgr.move_events).with_context(|| ptl!("move-events-parse-failed"))?,
                    // fv2 的 `start` / `start2` 已经是归一化后的 x / y（不是 fv1 那种
                    // `x * 1000 + y` 打包），但事件语义与 fv3 不同：只写起点 + 缓动码。
                    2 => parse_move_events_fv2(r, pgr.move_events).with_context(|| ptl!("move-events-parse-failed"))?,
                    3 => parse_move_events(r, pgr.move_events).with_context(|| ptl!("move-events-parse-failed"))?,
                    _ => ptl!(bail "unknown-format-version"),
                }
            }
        },
        color: Anim::default(),
        ctrl_obj: RefCell::default(),
        kind: JudgeLineKind::Normal,
        height,
        incline: AnimFloat::default(),
        notes,
        parent: None,
        rotate_with_parent: false,
        anchor: [0.5, 0.5],
        z_index: 0,
        show_below: false,
        attach_ui: None,

        cache,
    })
}

/// 解析完整 Phigros 官方 JSON 谱面文件，转换为引擎通用 Chart 谱面对象
/// # 参数
/// * source: &str - 完整的 JSON 谱面文本字符串
/// * extra: ChartExtra - 谱面附加资源信息（音频、封面、文件路径等扩展数据）
/// # 返回值
/// Result<Chart>：解析成功返回引擎标准谱面结构；失败携带对应上下文错误信息
pub fn parse_phigros(source: &str, extra: ChartExtra) -> Result<Chart> {
    // 将 JSON 字符串反序列化为 Phigros 原始谱面结构体，JSON 语法错误时抛出自定义上下文
    let pgr: PgrChart = serde_json::from_str(source).with_context(|| ptl!("json-parse-failed"))?;
    // 记录谱面格式版本，供判定线、音符解析时做版本兼容分支判断
    let format_version = pgr.format_version;

    // 收集所有判定线索引与对应BPM，用于构建全局BPM时间轴
    let mut bpm_values = Vec::new();
    for (index, judge_line) in pgr.judge_line_list.iter().enumerate() {
        bpm_values.push((index as f64, judge_line.bpm));
    }

    // 计算整首谱面的最大时长基准值（末尾额外+1秒预留收尾）
    // 逻辑：遍历每条判定线所有上下层音符，换算成真实秒数，取全局最大值
    let max_time = *pgr
        .judge_line_list
        .iter()
        .map(|line| {
            // 合并当前判定线上层、下层所有音符
            line.notes_above
                .iter()
                .chain(line.notes_below.iter())
                // 取出音符原始时间戳，过滤NaN非法值
                .map(|note| note.time.not_nan())
                // 获取当前判定线最晚音符时间，无音符则取默认0
                .max()
                .unwrap_or_default()
                // 原始节拍单位转真实秒：time * (60 / BPM / 32)
                * (60. / line.bpm / 32.)
        })
        // 取所有判定线里最大的结束时间
        .max()
        .unwrap_or_default()
        + 1.0; // 额外增加1秒缓冲，防止末尾动画、音效被截断

    // 遍历所有原始判定线，逐条转换为引擎内部 JudgeLine 结构
    // 每条线绑定唯一ID，解析出错时附带线ID定位错误来源
    let mut lines = pgr
        .judge_line_list
        .into_iter()
        .enumerate()
        .map(|(id, pgr_line)| {
            parse_judge_line(pgr_line, max_time, format_version)
                .with_context(|| ptl!("judge-line-location", "jlid" => id))
        })
        // 收集所有判定线，任意一条解析失败则整体返回Err
        .collect::<Result<Vec<_>>>()?;

    // 后处理判定线：统一修正层级、动画、渲染顺序、偏移等全局逻辑
    process_lines(&mut lines);

    // Phigros 4.0 剧情遮挡区域：注意 time 单位就是秒，不走 _time_coverter
    let mut block_areas = crate::core::parse_block_areas(pgr.block_area_list.as_ref());
    // 官方格式没有全局 beat 表（`BPMList` 只按判定线索引给），rpe-next v220 噪域的时间就是
    // 全局 beat，换算不出秒，所以这里不解析它们：`parse_block_areas` 会跳过 next 形状的条目，
    // 只提示一句，免得以为是解析器漏了。
    if pgr
        .block_area_list
        .as_ref()
        .and_then(|it| it.as_array())
        .is_some_and(|list| list.iter().any(crate::core::is_next_block_area))
    {
        warn!("blockAreaList: 官方格式没有全局 BPM 表，rpe-next 噪域已忽略");
    }
    // 「噪域优化」：解析时无条件把同形状的条目归成同一个噪域（`BlockArea::group`），
    // 各条成为它的多个出现窗口。用不用这个编号由 config.block_area_reuse 决定，
    // 所以这里不必等配置——归一次组只是几百微秒的一次哈希。
    let block_groups = crate::core::group_block_areas(&mut block_areas);

    // 组装并返回引擎标准 Chart 谱面对象
    let mut chart = Chart::new(
        pgr.offset,                // 谱面全局音频偏移量
        lines,                     // 解析+后处理完成的判定线列表
        BpmList::from_time(bpm_values), // 基于判定线索引构建的BPM时间管理器
        ChartSettings {
            line_reference_y_axis: true, // 使用判定线Y轴作为坐标基准
            ..Default::default()         // 其余渲染配置使用默认值
        },
        extra,                      // 外部传入的谱面附加资源信息
        FxHashMap::default(),       // 空特效缓存映射（解析阶段暂未加载特效）
        Vec::new(),                 // 空特效动画列表
    );
    if !block_areas.is_empty() {
        chart.block_areas = block_areas;
        chart.block_groups = block_groups;
    }
    Ok(chart)
}

#[cfg(test)]
mod fv2_tests {
    use super::*;

    /// `example/` 在仓库上面若干级（与 `core::block_area` 里的同名工具同一套找法）；
    /// 没有示例谱面时返回 `None`，调用方按「skip」处理。
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

    /// `example/` 下的一个目录；不存在时返回 `None`。
    fn example_subdir(rel: &str) -> Option<std::path::PathBuf> {
        let path = example_dir()?.join(rel);
        path.is_dir().then_some(path)
    }

    /// fv2 官谱：事件只写起点（`endTime` 恒为 0），`floorPosition` 是那一刻的累计高度，
    /// 坐标与缓动跟 fv3 是同一套。
    ///
    /// 这里拿真谱面钉三件事：解析能过、按键数与噪域数对、**每个按键的高度**都对得上官方
    /// `floorPosition / HEIGHT_RATIO` —— 速度段只要错一段、或者把首条事件的起点偏移
    /// （该谱第 5 条线从 8320 beat 才开始，起点高度 70.909）丢掉，这条断言就会挂。
    #[test]
    fn fv2_official_chart_matches_floor_position() {
        let Some(path) = example_path("pez2/Message.くるぶっこちゃん.0.SP.pez") else {
            eprintln!("skip: example chart not found");
            return;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skip: example chart not found");
            return;
        };
        let text = {
            use std::io::Read;
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
            let mut text = String::new();
            zip.by_name("SP.json").unwrap().read_to_string(&mut text).unwrap();
            text
        };

        let chart = parse_phigros(&text, ChartExtra::default()).expect("fv2 chart should parse");
        assert_eq!(chart.lines.len(), 24);
        assert_eq!(chart.block_areas.len(), 15);
        assert_eq!(chart.lines.iter().map(|line| line.notes.len()).sum::<usize>(), 600);

        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let pgr_lines = json["judgeLineList"].as_array().unwrap();
        let mut checked = 0;
        for (index, line) in chart.lines.iter().enumerate() {
            let pgr_line = &pgr_lines[index];
            let r = 60. / 32. / pgr_line["bpm"].as_f64().unwrap();
            let mut expected: Vec<(f64, f64)> = pgr_line["notesAbove"]
                .as_array()
                .unwrap()
                .iter()
                .chain(pgr_line["notesBelow"].as_array().unwrap())
                .map(|note| {
                    (
                        note["time"].as_f64().unwrap() * r,
                        note["floorPosition"].as_f64().unwrap() / HEIGHT_RATIO,
                    )
                })
                .collect();
            expected.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
            let mut actual: Vec<(f64, f64)> =
                line.notes.iter().map(|note| (note.time, note.height)).collect();
            actual.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
            assert_eq!(actual.len(), expected.len(), "line {index} note count");
            for (actual, expected) in actual.iter().zip(expected.iter()) {
                assert!(
                    (actual.0 - expected.0).abs() < 1e-6,
                    "line {index}: time {} vs {}",
                    actual.0,
                    expected.0
                );
                assert!(
                    (actual.1 - expected.1).abs() < 1e-3 + expected.1.abs() * 1e-6,
                    "line {index}: height at {} is {} but floorPosition says {}",
                    actual.0,
                    actual.1,
                    expected.1
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 600);
    }

    /// fv2 的长条要用 `headSpeed`（那一刻的判定线速度），不是 `speed`（v2 里恒为 1.0）。
    ///
    /// 同一张谱的 fv3 烘焙版把那个速度写进了长条的 `speed`：两边各解析一遍、按
    /// (判定线, 时间) 把长条对上，`end_height` 必须一致。只读 `speed` 时长条会按 1 倍速算，
    /// 速度 1.5–2.5 的段全部短掉，这条断言立刻挂。
    #[test]
    fn fv2_hold_length_matches_baked_fv3_chart() {
        use std::io::Read;
        let read = |rel: &str| -> Option<String> {
            let path = example_path(rel)?;
            let bytes = std::fs::read(&path).ok()?;
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
            let mut text = String::new();
            zip.by_name("SP.json").ok()?.read_to_string(&mut text).ok()?;
            Some(text)
        };
        let (Some(text2), Some(text3)) = (
            read("pez2/Message.くるぶっこちゃん.0.SP.pez"),
            read("pez/Message.くるぶっこちゃん.0.SP.pez"),
        ) else {
            eprintln!("skip: example charts not found");
            return;
        };
        let fv2 = parse_phigros(&text2, ChartExtra::default()).expect("fv2 chart should parse");
        let fv3 = parse_phigros(&text3, ChartExtra::default()).expect("fv3 chart should parse");
        let holds = |chart: &Chart| {
            let mut out = Vec::new();
            for (index, line) in chart.lines.iter().enumerate() {
                for note in &line.notes {
                    if let NoteKind::Hold { end_time, end_height, .. } = &note.kind {
                        // 比的是**长度**（`end_height - height`）：两个版本的按键高度来自各自的
                        // 速度积分，本身会差 1e-6 量级，直接比绝对值会把这层噪声也算进去。
                        out.push((index, note.time, *end_time, *end_height - note.height));
                    }
                }
            }
            out
        };
        let (a, b) = (holds(&fv2), holds(&fv3));
        assert_eq!(a.len(), 108, "fv2 hold count");
        assert_eq!(a.len(), b.len(), "fv2 / fv3 hold count");
        for (left, right) in a.iter().zip(b.iter()) {
            let (index, time, end_time, length) = *left;
            let (index3, time3, end_time3, length3) = *right;
            assert_eq!((index, time), (index3, time3), "hold at line {index} time {time}");
            assert!((end_time - end_time3).abs() < 1e-6, "hold end_time at {time}");
            assert!(
                (length - length3).abs() < 1e-6,
                "line {index} hold at {time}: length {length} vs baked fv3 {length3}"
            );
        }
    }

    /// `example/pez2` 这个目录里 fv1 / fv2 / fv3 三种谱面混着放：改版本分支时最容易在这里
    /// 「修好一种、弄坏另一种」，所以整目录过一遍解析就算守住了。
    #[test]
    fn every_pez2_example_chart_parses() {
        let Some(dir) = example_subdir("pez2") else {
            eprintln!("skip: example charts not found");
            return;
        };
        let mut charts = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|it| it.to_str()) != Some("pez") {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
            let name = (0..zip.len())
                .map(|index| zip.by_index(index).unwrap().name().to_owned())
                .find(|name| name.ends_with(".json"))
                .unwrap_or_else(|| panic!("{}: no chart json", path.display()));
            let text = {
                use std::io::Read;
                let mut text = String::new();
                zip.by_name(&name).unwrap().read_to_string(&mut text).unwrap();
                text
            };
            parse_phigros(&text, ChartExtra::default())
                .unwrap_or_else(|err| panic!("{}: {err:?}", path.display()));
            charts += 1;
        }
        assert!(charts >= 5, "only {charts} charts found");
    }

    /// 同一张谱的 fv2 / fv3 两版逐帧对齐。
    ///
    /// fv3 那一版是官方按 fv2 的事件与缓动**烘焙**出来的（事件加密到 8 tick、需要时 1 tick），
    /// 所以它可以直接当真值：两边的判定线 x / y 在整首曲子上的平均偏差必须小到 0.01 以内；
    /// 缓动码映射错一档（比如把 QuadIn 当成 SineIn）、或者把 `end` 当终点用，这里立刻就会崩。
    #[test]
    fn fv2_matches_baked_fv3_chart() {
        let (Some(fv2_path), Some(fv3_path)) = (
            example_path("pez2/Message.くるぶっこちゃん.0.SP.pez"),
            example_path("pez/Message.くるぶっこちゃん.0.SP.pez"),
        ) else {
            eprintln!("skip: example charts not found");
            return;
        };
        let read_chart = |path: &std::path::Path| -> Chart {
            let bytes = std::fs::read(path).unwrap();
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
            let mut text = String::new();
            {
                use std::io::Read;
                zip.by_name("SP.json").unwrap().read_to_string(&mut text).unwrap();
            }
            parse_phigros(&text, ChartExtra::default()).expect("chart should parse")
        };
        let fv2 = read_chart(&fv2_path);
        let fv3 = read_chart(&fv3_path);
        assert_eq!(fv2.lines.len(), fv3.lines.len());

        // 三路分开统计：位置是归一化坐标，旋转是角度（差 360° 视觉上等于没转），透明度是 0..1。
        let mut sum = [0.0f64; 3];
        let mut count = [0usize; 3];
        let mut nan = 0usize;
        let mut bad = [0usize; 3];
        let mut worst = [(0.0f32, 0usize, 0.0f64); 3];
        for (index, (line2, line3)) in fv2.lines.iter().zip(fv3.lines.iter()).enumerate() {
            let mut trans2 = AnimVector(line2.object.translation.0.clone(), line2.object.translation.1.clone());
            let mut trans3 = AnimVector(line3.object.translation.0.clone(), line3.object.translation.1.clone());
            let mut rot2 = line2.object.rotation.clone();
            let mut rot3 = line3.object.rotation.clone();
            let mut alpha2 = line2.object.alpha.clone();
            let mut alpha3 = line3.object.alpha.clone();
            let mut time = 0.0;
            // 谱面到 112 秒左右结束；末尾之后两边都停在最后一个关键帧上。
            while time < 120.0 {
                trans2.set_time(time);
                trans3.set_time(time);
                rot2.set_time(time);
                rot3.set_time(time);
                alpha2.set_time(time);
                alpha3.set_time(time);
                let (p2, p3) = (trans2.now(), trans3.now());
                // 旋转按 360 取模：一整圈和没转是同一个姿态。
                let spin = (rot2.now() - rot3.now()).rem_euclid(360.0);
                // 判定线被甩到屏幕外（|x| > 1.2）的样本不参与位置比较：官方在把 fv2 转成 fv3 时
                // 会把 x 夹回 [-0.1, 1.1]（`-1.0` 烘焙成 `-0.1`、`2.0` 变成 `1.1`），两边都看不见。
                let on_screen = [p2.x, p2.y, p3.x, p3.y].iter().all(|it| it.abs() <= 1.2);
                let errs: [Option<f32>; 3] = [
                    on_screen.then(|| (p2.x - p3.x).abs().max((p2.y - p3.y).abs())),
                    Some(spin.min(360.0 - spin)),
                    Some((alpha2.now() - alpha3.now()).abs()),
                ];
                for (channel, err) in errs.into_iter().enumerate() {
                    let Some(err) = err else { continue };
                    // fv3 谱面开头带一条 `startTime: -999999` 的哨兵事件，它和紧随其后的
                    // 那一条会落在同一个时刻上，零长区间在被采样的那一帧算出 NaN —— 肉眼看不见，
                    // 这里跳过它们，但数量不能多，否则说明整条线真出问题了。
                    if !err.is_finite() {
                        nan += 1;
                        continue;
                    }
                    sum[channel] += err as f64;
                    count[channel] += 1;
                    if err > [0.05, 1.0, 0.01][channel] {
                        bad[channel] += 1;
                    }
                    if err > worst[channel].0 {
                        worst[channel] = (err, index, time);
                    }
                }
                time += 1.0 / 60.0;
            }
        }
        let means = [0, 1, 2].map(|it| sum[it] / count[it].max(1) as f64);
        eprintln!(
            "fv2 vs fv3 mean deviation — position {:.5}, rotation {:.4}°, alpha {:.5}; samples {:?}, nan {nan}; worst {:?}",
            means[0], means[1], means[2], count, worst
        );
        assert!(means[0] < 0.01, "position mean deviation is {}", means[0]);
        assert!(means[1] < 1.0, "rotation mean deviation is {}°", means[1]);
        assert!(means[2] < 0.01, "alpha mean deviation is {}", means[2]);
        assert!(nan * 1000 < count[1], "{nan}/{count:?} samples were not finite");
        // 尖峰（官方烘焙里的单 tick 事件）允许存在，但不能成片。
        for channel in 0..3 {
            assert!(
                bad[channel] * 200 < count[channel],
                "channel {channel}: {}/{count:?} samples deviate beyond tolerance",
                bad[channel]
            );
        }
    }
}
