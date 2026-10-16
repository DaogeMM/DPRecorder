crate::tl_file!("parser" ptl);

use super::process_lines;
use crate::{
    core::{
        Anim, AnimFloat, AnimFloatF64, AnimVector, BpmList, Chart, ChartExtra, ChartSettings, JudgeLine, JudgeLineCache, JudgeLineKind, Keyframe, Note, NoteKind, Object, HEIGHT_RATIO
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
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PgrSpeedEvent {
    pub start_time: f64,
    pub end_time: f64,
    pub value: f64,
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

fn parse_speed_events(r: f64, mut pgr: Vec<PgrSpeedEvent>, max_time: f64) -> Result<(AnimFloatF64, AnimFloatF64)> {
    validate_events!(pgr);
    if pgr[0].start_time != 0. { pgr[0].start_time = 0. }
    let mut kfs = Vec::new();
    let mut pos = 0.;
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
                    let end_time = (pgr.time + pgr.hold_time) * r;
                    let end_height = height + (pgr.hold_time * pgr.speed * r / HEIGHT_RATIO);
                    let end_speed = Some(pgr.speed);
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
                water: Default::default(),
            })
        })
        .collect()
}

fn parse_judge_line(pgr: PgrJudgeLine, max_time: f64, format_version: u32) -> Result<JudgeLine> {
    let r = 60. / 32. / pgr.bpm;
    let (mut speed, mut height) = parse_speed_events(r, pgr.speed_events, max_time).context("Failed to parse speed events")?;
    let notes_above = parse_notes(r, pgr.notes_above, &mut speed, &mut height, true).context("Failed to parse notes above")?;
    let mut notes_below = parse_notes(r, pgr.notes_below, &mut speed, &mut height, false).context("Failed to parse notes below")?;
    let mut notes = notes_above;
    notes.append(&mut notes_below);
    let cache = JudgeLineCache::new(&mut notes);
    Ok(JudgeLine {
        object: Object {
            alpha: parse_float_events(r, pgr.alpha_events).with_context(|| ptl!("alpha-events-parse-failed"))?,
            scale: AnimVector(AnimFloat::fixed(5.75 / 6.0), AnimFloat::default()),
            rotation: parse_float_events(r, pgr.rotate_events).with_context(|| ptl!("rotate-events-parse-failed"))?,
            translation: {
                match format_version {
                    1 => parse_move_events_fv1(r, pgr.move_events).with_context(|| ptl!("move-events-parse-failed"))?,
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
