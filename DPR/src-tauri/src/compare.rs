//! 采音对比：把两张同一首歌的谱面按「判定时刻」比一遍，把各自的按键染上颜色另存一份。
//!
//! 判据只看时刻：两张谱面在同一时刻（音频位置，容差 1 ms）都有按键就算「共有」，
//! 只有一张有的按键算「独有」——和弦落在同一时刻也按共有算。
//! 音频位置 = 谱面内算出的秒数 + 谱面自带的 offset + `info.txt` 的 offset。
//!
//! 结果写进谱面 JSON 本身（rpe 写音符的 `tint`、官方格式写 `color`），因为逐音符的颜色
//! 不是渲染配置里的项，只能长在谱面里；不染色的那一种颜色直接删掉字段，让引擎用默认色。
//! 产物是 `dpe/modify` 下以 `(采音对比)` 结尾的文件夹，里面放着两张改好的 `.pez`，
//! 可以直接拖进游戏或交给预览。
//!
//! 只支持 `rpe` 与官方（`pgr`）两种 JSON 格式：`pec` / `pbc` 的按键不在 JSON 里，改不动。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use dps::fs::{open_zip, update_zip};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::ipc::InvokeError;
use tracing::info;

use crate::merge::{base_zip_bytes, file_stem, same_song, Format, Source};

/// 产物文件夹名的后缀：输出表按它认出「这是采音对比的结果」。
const FOLDER_TAG: &str = " (采音对比)";

/// 两张谱面在同一时刻的判定容差（秒）：只差浮点误差才算同一时刻。
const TIME_TOLERANCE: f64 = 0.001;

/// 一个颜色：`[r, g, b]`，与谱面 JSON 里的写法一致。
type RGB = [u8; 3];

/// 齿轮里配的三种颜色。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareColors {
    /// 两张谱面都有的按键；`#FFFFFF` 或留空 = 不染色。
    common: String,
    /// 只有第一张谱面才有的按键。
    first: String,
    /// 只有第二张谱面才有的按键。
    second: String,
}

/// 输出表里的一行：`dpe/modify` 下的一个采音对比文件夹。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareOutput {
    name: String,
    path: String,
    modified: i64,
}

/// 对比勾中的两张谱面，把两张改好颜色的 `.pez` 放进 `dpe/modify` 下的新文件夹，返回该文件夹。
///
/// 前端已经判过「正好两张」与「同一首歌」，这里再兜底判一次：命令也可能被别处直接调用。
#[tauri::command]
pub async fn dpe_compare_charts(
    paths: Vec<String>,
    colors: CompareColors,
) -> Result<String, InvokeError> {
    crate::wrap_async(async move {
        let colors = (
            parse_color(&colors.common)?,
            parse_color(&colors.first)?,
            parse_color(&colors.second)?,
        );
        let paths = paths.into_iter().map(PathBuf::from).collect::<Vec<_>>();
        let dir = crate::common::dpe_output_dir(crate::common::DPE_MODIFY_DIR)?;
        let folder = compare(&paths, colors, &dir).await?;
        Ok(folder.display().to_string())
    })
    .await
}

/// 列出 `dir` 下所有采音对比文件夹（输出表用）。
#[tauri::command]
pub fn dpe_compare_outputs(dir: String) -> Result<Vec<CompareOutput>, InvokeError> {
    list_outputs(Path::new(&dir)).map_err(InvokeError::from_anyhow)
}

fn list_outputs(dir: &Path) -> Result<Vec<CompareOutput>> {
    let mut list = Vec::new();
    if !dir.is_dir() {
        return Ok(list);
    }
    for entry in std::fs::read_dir(dir).with_context(|| format!("无法读取 {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !path.is_dir() || !name.ends_with(FOLDER_TAG) {
            continue;
        }
        let modified = path
            .metadata()
            .and_then(|it| it.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|it| it.as_secs() as i64)
            .unwrap_or(0);
        list.push(CompareOutput {
            name,
            path: path.display().to_string(),
            modified,
        });
    }
    list.sort_by(|a, b| b.modified.cmp(&a.modified));
    Ok(list)
}

async fn compare(
    paths: &[PathBuf],
    (common, first, second): (RGB, RGB, RGB),
    dir: &Path,
) -> Result<PathBuf> {
    if paths.len() != 2 {
        bail!("采音对比需要正好两张谱面");
    }
    let mut sources = Vec::with_capacity(2);
    for path in paths {
        sources.push(Source::load(path).await?);
    }
    let (a, b) = (&sources[0], &sources[1]);
    if !same_song(&a.info, &b.info) {
        bail!("两张谱面不是同一首歌（曲名或音乐文件不一致）");
    }

    let Parsed {
        value: mut a_value,
        notes: a_notes,
    } = parse(a)?;
    let Parsed {
        value: mut b_value,
        notes: b_notes,
    } = parse(b)?;

    // 先各自判「对方有没有同一时刻的按键」，再按结果上色：两张谱面的颜色是对称的。
    let shared_a = shared_flags(&a_notes, &b_notes);
    let shared_b = shared_flags(&b_notes, &a_notes);
    paint(&mut a_value, a.format, &a_notes, &shared_a, common, first);
    paint(&mut b_value, b.format, &b_notes, &shared_b, common, second);

    let folder = dir.join(format!("{}{FOLDER_TAG}", file_stem(&a.info.name)));
    if folder.exists() {
        std::fs::remove_dir_all(&folder)
            .with_context(|| format!("无法清空 {}", folder.display()))?;
    }
    std::fs::create_dir_all(&folder).with_context(|| format!("无法创建 {}", folder.display()))?;

    let a_name = package_name(&a.path, 1);
    let b_name = package_name(&b.path, 2);
    write_package(a, &a_value, &folder, &a_name).await?;
    write_package(b, &b_value, &folder, &b_name).await?;
    info!("[谱面] 已生成采音对比 {}", folder.display());
    Ok(folder)
}

/// 把颜色文本读成 RGB：`#FFFFFF` / `FFFFFF` / `#FFF` 都认，空串当白色（不染色）。
fn parse_color(text: &str) -> Result<RGB> {
    let text = text.trim().trim_start_matches('#');
    if text.is_empty() {
        return Ok([255, 255, 255]);
    }
    let digits = text
        .chars()
        .map(|ch| {
            ch.to_digit(16)
                .map(|it| it as u8)
                .with_context(|| format!("颜色要写成 #RRGGBB 或 #RGB：{text}"))
        })
        .collect::<Result<Vec<u8>>>()?;
    match digits.as_slice() {
        [r, g, b] => Ok([r * 17, g * 17, b * 17]),
        [r1, r2, g1, g2, b1, b2] => Ok([r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2]),
        _ => bail!("颜色要写成 #RRGGBB 或 #RGB：{text}"),
    }
}

/// 一份谱面：可改的 JSON 值，以及逐个音符的音频时刻（只用来对比，不改）。
struct Parsed {
    value: Value,
    notes: Vec<NoteRef>,
}

/// 谱面里一个按键的位置：它挂在哪条判定线的哪个列表的第几个，以及它的音频时刻。
struct NoteRef {
    line: usize,
    above: bool,
    index: usize,
    time: f64,
}

fn parse(source: &Source) -> Result<Parsed> {
    let value: Value = serde_json::from_str(&source.text).context("谱面不是合法的 JSON")?;
    let offset = chart_offset(&value, source.format) + source.info.offset;
    let notes = match source.format {
        Format::Rpe => rpe_notes(&value, offset)?,
        Format::Pgr => pgr_notes(&value, offset)?,
    };
    Ok(Parsed { value, notes })
}

/// 谱面自带的 offset（秒）：rpe 记在 `META.offset` 里且单位是毫秒，官方格式的 `offset` 就是秒。
fn chart_offset(value: &Value, format: Format) -> f64 {
    match format {
        Format::Rpe => {
            value
                .pointer("/META/offset")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                / 1000.0
        }
        Format::Pgr => value.get("offset").and_then(Value::as_f64).unwrap_or(0.0),
    }
}

fn rpe_notes(value: &Value, offset: f64) -> Result<Vec<NoteRef>> {
    let bpms = value
        .get("BPMList")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|item| {
                    let bpm = item.get("bpm").and_then(Value::as_f64)?;
                    let beats = item.get("startTime").and_then(beats_of)?;
                    Some((beats, bpm))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let lines = value
        .get("judgeLineList")
        .and_then(Value::as_array)
        .context("谱面里没有 judgeLineList")?;

    let mut notes = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        // 每条判定线的音符时间都按「BPM / bpmfactor」算，与引擎的读法一致。
        let factor = line
            .get("bpmfactor")
            .and_then(Value::as_f64)
            .filter(|it| *it != 0.0)
            .unwrap_or(1.0);
        let table = BpmTable::new(bpms.iter().map(|(beats, bpm)| (*beats, bpm / factor)));
        let Some(list) = line.get("notes").and_then(Value::as_array) else {
            continue;
        };
        for (index, note) in list.iter().enumerate() {
            let Some(beats) = note.get("startTime").and_then(beats_of) else {
                continue;
            };
            notes.push(NoteRef {
                line: line_index,
                above: false,
                index,
                time: table.time_at(beats) + offset,
            });
        }
    }
    Ok(notes)
}

fn pgr_notes(value: &Value, offset: f64) -> Result<Vec<NoteRef>> {
    let lines = value
        .get("judgeLineList")
        .and_then(Value::as_array)
        .context("谱面里没有 judgeLineList")?;

    let mut notes = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        let bpm = line
            .get("bpm")
            .and_then(Value::as_f64)
            .filter(|it| *it != 0.0)
            .unwrap_or(120.0);
        // 官方格式的 `time` 单位是 1/32 拍。
        let ratio = 60.0 / 32.0 / bpm;
        for (key, above) in [("notesAbove", true), ("notesBelow", false)] {
            let Some(list) = line.get(key).and_then(Value::as_array) else {
                continue;
            };
            for (index, note) in list.iter().enumerate() {
                let Some(time) = note.get("time").and_then(Value::as_f64) else {
                    continue;
                };
                notes.push(NoteRef {
                    line: line_index,
                    above,
                    index,
                    time: time * ratio + offset,
                });
            }
        }
    }
    Ok(notes)
}

/// rpe 的 `startTime` 是 `[a, b, c]` 三段拍号，取 `a + b / c`。
fn beats_of(value: &Value) -> Option<f64> {
    let list = value.as_array()?;
    if list.len() != 3 {
        return None;
    }
    let a = list[0].as_f64()?;
    let b = list[1].as_f64()?;
    let c = list[2].as_f64().filter(|it| *it != 0.0)?;
    Some(a + b / c)
}

/// 拍 → 秒的换算表，与 `dps::core::BpmList` 同一套算法：
/// 按 BPM 段累加起始秒，取「最后一个起点不晚于目标的段」往后续。
struct BpmTable {
    /// (起始拍, 起始秒, 该段的 BPM)
    elements: Vec<(f64, f64, f64)>,
}

impl BpmTable {
    fn new(ranges: impl Iterator<Item = (f64, f64)>) -> Self {
        let mut elements = Vec::new();
        let mut time = 0.0;
        let mut last: Option<(f64, f64)> = None;
        for (beats, bpm) in ranges {
            if let Some((last_beats, last_bpm)) = last {
                time += (beats - last_beats) * (60.0 / last_bpm);
            }
            last = Some((beats, bpm));
            elements.push((beats, time, bpm));
        }
        Self { elements }
    }

    fn time_at(&self, beats: f64) -> f64 {
        let index = self
            .elements
            .iter()
            .rposition(|it| it.0 <= beats)
            .unwrap_or(0);
        match self.elements.get(index) {
            Some((start_beats, time, bpm)) => time + (beats - start_beats) * (60.0 / bpm),
            None => 0.0,
        }
    }
}

/// 逐个问一遍「对方谱面里这个时刻有没有按键」：有的下标为 true（共有），没有的是独有。
fn shared_flags(notes: &[NoteRef], other: &[NoteRef]) -> Vec<bool> {
    let mut times = other.iter().map(|it| it.time).collect::<Vec<_>>();
    times.sort_by(f64::total_cmp);
    notes.iter().map(|it| has_near(&times, it.time)).collect()
}

/// 有序时刻表里有没有落在 `time` 容差范围内的项。
fn has_near(sorted: &[f64], time: f64) -> bool {
    match sorted.binary_search_by(|it| it.total_cmp(&time)) {
        Ok(_) => true,
        Err(0) => sorted
            .first()
            .is_some_and(|it| (it - time).abs() <= TIME_TOLERANCE),
        Err(index) if index == sorted.len() => sorted
            .last()
            .is_some_and(|it| (time - it).abs() <= TIME_TOLERANCE),
        Err(index) => {
            (time - sorted[index - 1]).abs() <= TIME_TOLERANCE
                || (sorted[index] - time).abs() <= TIME_TOLERANCE
        }
    }
}

/// 按对比结果给音符上色；白色等于不染色，直接删掉颜色字段。
fn paint(
    value: &mut Value,
    format: Format,
    notes: &[NoteRef],
    shared: &[bool],
    common: RGB,
    own: RGB,
) {
    let key = match format {
        Format::Rpe => "tint",
        Format::Pgr => "color",
    };
    for (note, is_shared) in notes.iter().zip(shared) {
        let Some(lines) = value.get_mut("judgeLineList").and_then(Value::as_array_mut) else {
            return;
        };
        let Some(line) = lines.get_mut(note.line) else {
            continue;
        };
        let list_key = match format {
            Format::Rpe => "notes",
            Format::Pgr => {
                if note.above {
                    "notesAbove"
                } else {
                    "notesBelow"
                }
            }
        };
        let Some(entry) = line
            .get_mut(list_key)
            .and_then(Value::as_array_mut)
            .and_then(|list| list.get_mut(note.index))
        else {
            continue;
        };
        let Some(object) = entry.as_object_mut() else {
            continue;
        };
        let color = if *is_shared { common } else { own };
        if color == [255, 255, 255] {
            object.remove(key);
        } else {
            object.insert(
                key.to_string(),
                serde_json::json!([color[0], color[1], color[2]]),
            );
        }
    }
}

/// 把改好的谱面写回一份 `.pez`：其余条目（曲绘、音乐、`info.txt`）原样照搬。
async fn write_package(source: &Source, value: &Value, folder: &Path, name: &str) -> Result<()> {
    let bytes = base_zip_bytes(&source.path, folder).await?;
    let mut zip = open_zip(bytes).context("谱面包不是 zip 压缩包")?;
    let mut patches = HashMap::new();
    patches.insert(source.info.chart.clone(), serde_json::to_vec(value)?);
    let bytes = update_zip(&mut zip, patches)?;
    let output = folder.join(name);
    std::fs::write(&output, bytes).with_context(|| format!("无法写入 {}", output.display()))?;
    Ok(())
}

/// 产物文件名：`1.原名.pez` / `2.原名.pez` —— 序号与齿轮里「第一张 / 第二张」的颜色对得上；
/// 文件夹源没有文件名，用它的目录名加 `.pez`。
fn package_name(path: &Path, index: usize) -> String {
    let base = if path.is_dir() {
        let name = path
            .file_name()
            .map(|it| it.to_string_lossy().to_string())
            .unwrap_or_else(|| "chart".into());
        format!("{name}.pez")
    } else {
        path.file_name()
            .map(|it| it.to_string_lossy().to_string())
            .unwrap_or_else(|| "chart.pez".into())
    };
    format!("{index}.{base}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn note(line: usize, above: bool, index: usize, time: f64) -> NoteRef {
        NoteRef {
            line,
            above,
            index,
            time,
        }
    }

    // 换算必须与 `dps::core::BpmList` 一致：按段累加，段内按该段 BPM 续。
    #[test]
    fn bpm_table_follows_segments() {
        let table = BpmTable::new([(0.0, 120.0), (4.0, 240.0)].into_iter());
        assert!((table.time_at(0.0) - 0.0).abs() < 1e-9);
        // 前 4 拍 120 BPM = 2 秒
        assert!((table.time_at(4.0) - 2.0).abs() < 1e-9);
        // 之后 4 拍 240 BPM = 1 秒
        assert!((table.time_at(8.0) - 3.0).abs() < 1e-9);
    }

    // rpe 每个音符的时间 = 按判定线 BPM（谱面 BPM / bpmfactor）换算的拍 → 秒，再加谱面 offset。
    #[test]
    fn rpe_note_times_use_bpmfactor() {
        let value = json!({
            "META": { "RPEVersion": 150, "offset": 500 },
            "BPMList": [{ "bpm": 120.0, "startTime": [0, 0, 1] }],
            "judgeLineList": [{
                "bpmfactor": 2.0,
                "notes": [{ "type": 1, "startTime": [4, 0, 1] }, { "type": 1, "startTime": [0, 0, 1] }]
            }]
        });
        let notes = rpe_notes(&value, 0.5).unwrap();
        assert_eq!(notes.len(), 2);
        // 引擎按 `BPM / bpmfactor` 建表：120 / 2 = 60 BPM ⇒ 一拍 1 秒
        assert!((notes[0].time - 4.5).abs() < 1e-9);
        assert!((notes[1].time - 0.5).abs() < 1e-9);
        assert_eq!((notes[0].line, notes[0].index), (0, 0));
    }

    // 官方格式的 `time` 是 1/32 拍，判定线 BPM 120 ⇒ 每单位 60/32/120 秒。
    #[test]
    fn pgr_note_times_are_thirty_seconds() {
        let value = json!({
            "offset": 0.25,
            "judgeLineList": [{
                "bpm": 120.0,
                "notesAbove": [{ "time": 32.0 }],
                "notesBelow": [{ "time": 16.0 }]
            }]
        });
        let notes = pgr_notes(&value, 0.25).unwrap();
        assert_eq!(notes.len(), 2);
        assert!((notes[0].time - 0.75).abs() < 1e-9);
        assert!(notes[0].above);
        assert!((notes[1].time - 0.5).abs() < 1e-9);
        assert!(!notes[1].above);
    }

    // 判据只看时刻：差在 1 ms 以内算同一时刻，超过就不算。
    #[test]
    fn shared_flags_use_a_one_millisecond_window() {
        let a = vec![note(0, false, 0, 1.0), note(0, false, 1, 2.0)];
        let b = vec![note(0, false, 0, 1.0005), note(0, false, 1, 3.0)];
        assert_eq!(shared_flags(&a, &b), vec![true, false]);
        assert_eq!(shared_flags(&b, &a), vec![true, false]);
    }

    // 白色等于不染色：共有色留白时删掉字段（而不是写一份白），独有色照写。
    #[test]
    fn paint_writes_only_colored_notes() {
        let mut value = json!({
            "judgeLineList": [{ "notes": [{ "type": 1 }, { "type": 1 }] }]
        });
        let notes = vec![note(0, false, 0, 0.0), note(0, false, 1, 1.0)];
        paint(&mut value, Format::Rpe, &notes, &[true, false], [255, 255, 255], [255, 0, 0]);
        assert!(value.pointer("/judgeLineList/0/notes/0/tint").is_none());
        assert_eq!(
            value.pointer("/judgeLineList/0/notes/1/tint"),
            Some(&json!([255, 0, 0]))
        );
    }

    // 官方格式写的是 `color`，且上下两个列表各自按下标定位。
    #[test]
    fn paint_targets_pgr_color_fields() {
        let mut value = json!({
            "judgeLineList": [{ "notesAbove": [{ "time": 1 }], "notesBelow": [{ "time": 2 }] }]
        });
        let notes = vec![note(0, false, 0, 0.0)];
        paint(&mut value, Format::Pgr, &notes, &[false], [255, 255, 255], [0, 0, 255]);
        assert!(value.pointer("/judgeLineList/0/notesAbove/0/color").is_none());
        assert_eq!(
            value.pointer("/judgeLineList/0/notesBelow/0/color"),
            Some(&json!([0, 0, 255]))
        );
    }

    #[test]
    fn colors_read_short_and_long_forms() {
        assert_eq!(parse_color("#FFFFFF").unwrap(), [255, 255, 255]);
        assert_eq!(parse_color(" #f00 ").unwrap(), [255, 0, 0]);
        assert_eq!(parse_color("448AFF").unwrap(), [0x44, 0x8A, 0xFF]);
        assert_eq!(parse_color("").unwrap(), [255, 255, 255]);
        assert!(parse_color("#12345").is_err());
        assert!(parse_color("#GGGGGG").is_err());
    }

    // 产物文件名带序号：与齿轮里「第一张 / 第二张」的颜色对得上。
    #[test]
    fn output_names_are_numbered() {
        assert_eq!(package_name(Path::new("C:/charts/SP.pez"), 1), "1.SP.pez");
        assert_eq!(package_name(Path::new("C:/charts/my chart"), 2), "2.my chart");
    }

    /// 统计一份谱面 JSON 里带颜色的按键数（`tint` / `color` 两种写法）。
    fn colored_notes(value: &Value) -> (usize, usize) {
        let mut total = 0;
        let mut colored = 0;
        for line in value["judgeLineList"].as_array().into_iter().flatten() {
            for key in ["notes", "notesAbove", "notesBelow"] {
                for note in line[key].as_array().into_iter().flatten() {
                    total += 1;
                    if note.get("tint").is_some() || note.get("color").is_some() {
                        colored += 1;
                    }
                }
            }
        }
        (total, colored)
    }

    // 端到端：拿两张同歌的样例谱面真跑一遍（样例目录不在仓库里，缺了就跳过）。
    #[test]
    fn example_pair_end_to_end() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../example/pez");
        let a = root.join("Message.くるぶっこちゃん.0.AT.pez");
        let b = root.join("Message.くるぶっこちゃん.0.IN.pez");
        if !a.exists() || !b.exists() {
            eprintln!("skip：没有样例谱面 {}", root.display());
            return;
        }
        let out = std::env::temp_dir().join("dpr-compare-e2e");
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        let folder = tauri::async_runtime::block_on(compare(
            &[a, b],
            ([255, 255, 255], [255, 0, 0], [0, 0, 255]),
            &out,
        ))
        .unwrap();
        eprintln!("产物文件夹 = {}", folder.display());

        let mut names = Vec::new();
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            names.push(path.file_name().unwrap().to_string_lossy().to_string());
            let mut archive = zip::ZipArchive::new(std::io::Cursor::new(std::fs::read(&path).unwrap())).unwrap();
            for index in 0..archive.len() {
                let mut file = archive.by_index(index).unwrap();
                if !file.name().ends_with(".json") {
                    continue;
                }
                let mut text = String::new();
                std::io::Read::read_to_string(&mut file, &mut text).unwrap();
                let value: Value = serde_json::from_str(&text).unwrap();
                let (total, colored) = colored_notes(&value);
                eprintln!("{} :: {} 个按键，{} 个染色", path.display(), total, colored);
                assert!(colored > 0, "{} 里一个染色按键都没有", path.display());
            }
        }
        names.sort();
        // 序号前缀对应齿轮里的「第一张 / 第二张」，后缀沿用各自的谱面文件名。
        assert_eq!(names.len(), 2);
        assert!(names[0].starts_with("1."), "{names:?}");
        assert!(names[1].starts_with("2."), "{names:?}");
    }
}
