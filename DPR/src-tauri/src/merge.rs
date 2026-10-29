//! 「谱面融合」：把同一首歌的几个难度摊成一张谱面。
//!
//! 做法就是「照搬」：判定线（`judgeLineList`）与噪域（`blockAreaList`）按条目接在后面，
//! 按键在判定线里，跟着一起过来；包里的音乐、曲绘、演出用的 shader 都取自基谱。
//! 融合结果只是一次性的东西 —— 落在临时目录里，每次融合前整个清空。

use anyhow::{bail, Context, Result};
use dps::{
    fs::{self, open_zip, update_zip},
    info::{ChartFormat, ChartInfo},
};
use serde_json::Value;
use std::{
    collections::HashMap,
    ops::DerefMut,
    path::{Path, PathBuf},
};
use tauri::ipc::InvokeError;
use tracing::info;

use crate::common::{collect_chart_files, create_zip};

/// 融合结果的临时子目录（`std::env::temp_dir()` 已被 `lib.rs` 指到 `<程序目录>/tmp`）。
const MERGE_DIR: &str = "merge";

/// 融合后的难度（难度标识与难度文本都写这一份），曲名照旧：只看难度就知道这张是拼出来的。
const DIFF_TEXT: &str = "（谱面融合）";

/// 融合包的文件名后缀。路径会显示在渲染页底部，用 ASCII 免得全角括号看着别扭。
const FILE_TAG: &str = "JOIN";

/// 融合所选谱面，返回融合结果（一个临时 `.pez`）的绝对路径。
///
/// 「是不是同一首歌」「格式对不对」前端已经先判过一遍（那里的提示能说清是哪两张对不上），
/// 这里再兜底判一次。
#[tauri::command]
pub async fn merge_charts(paths: Vec<String>) -> Result<String, InvokeError> {
    crate::wrap_async(async move {
        let paths = paths.into_iter().map(PathBuf::from).collect::<Vec<_>>();
        let out = merge(&paths).await?;
        Ok(out.display().to_string())
    })
    .await
}

async fn merge(paths: &[PathBuf]) -> Result<PathBuf> {
    if paths.len() < 2 {
        bail!("merge needs at least two charts");
    }

    let mut sources = Vec::with_capacity(paths.len());
    for path in paths {
        sources.push(Source::load(path).await?);
    }

    let base = &sources[0];
    for source in &sources[1..] {
        if source.format != base.format {
            bail!("charts must be the same format");
        }
        if !same_song(&base.info, &source.info) {
            bail!("charts must be the same song");
        }
    }

    let dir = reset_merge_dir()?;
    let merged = merge_json(&sources)?;
    let info = merged_info(&sources);

    let bytes = base_zip_bytes(&base.path, &dir).await?;
    let mut zip = open_zip(bytes).context("base chart is not a zip archive")?;
    let mut patches = HashMap::new();
    // 谱面文件名沿用基谱的，`info.chart` 与包里其它文件的相对位置就不用动；info 一律写成
    // `info.yml`，它排在 `info.txt` 前面，基谱原来是 txt 的也能盖掉。
    patches.insert(base.info.chart.clone(), serde_json::to_vec(&merged)?);
    patches.insert("info.yml".to_string(), serde_yaml::to_string(&info)?.into_bytes());
    let patched = update_zip(&mut zip, patches)?;

    let out = dir.join(format!("{}.{}.pez", file_stem(&base.info.name), FILE_TAG));
    std::fs::write(&out, patched).with_context(|| format!("cannot write {}", out.display()))?;
    info!("merged chart: {}", out.display());
    Ok(out)
}

/// 一个待融合的谱面：谱面信息、谱面文本（原样，融合时直接搬 JSON 条目）、格式。
struct Source {
    path: PathBuf,
    info: ChartInfo,
    text: String,
    format: Format,
}

impl Source {
    async fn load(path: &Path) -> Result<Self> {
        let mut fs = fs::fs_from_file(path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        let info = fs::load_info(fs.deref_mut())
            .await
            .with_context(|| format!("cannot read info of {}", path.display()))?;
        let bytes = fs
            .load_file(&info.chart)
            .await
            .with_context(|| format!("cannot read {} in {}", info.chart, path.display()))?;
        let text = String::from_utf8(bytes).context("chart is not utf-8")?;
        let format = detect_format(&info, &text)?;
        Ok(Self {
            path: path.to_path_buf(),
            info,
            text,
            format,
        })
    }
}

/// 只支持 JSON 的两套格式：`pec` / `pbc` 的按键与判定线不是同一张表，没法按条目照搬。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Rpe,
    Pgr,
}

fn detect_format(info: &ChartInfo, text: &str) -> Result<Format> {
    match info.format {
        Some(ChartFormat::Rpe) => Ok(Format::Rpe),
        Some(ChartFormat::Pgr) => Ok(Format::Pgr),
        Some(ChartFormat::Pec) | Some(ChartFormat::Pbc) => {
            bail!("only rpe and pgr charts can be merged")
        }
        None => sniff_format(text),
    }
}

/// 与 `GameScene` 载入谱面时的判别一致：rpe 的谱面 JSON 有 `META`，官方格式没有。
fn sniff_format(text: &str) -> Result<Format> {
    if !text.starts_with('{') {
        bail!("only rpe and pgr charts can be merged");
    }
    Ok(if text.contains("\"META\"") {
        Format::Rpe
    } else {
        Format::Pgr
    })
}

/// 「同一首歌」= 曲名与音乐文件都对得上：同一首曲子的几个难度，这两样是一样的。
fn same_song(a: &ChartInfo, b: &ChartInfo) -> bool {
    a.name.trim().eq_ignore_ascii_case(b.name.trim())
        && a.music.trim().eq_ignore_ascii_case(b.music.trim())
}

/// 判定线与噪域都按条目照搬。
///
/// 判定线接在基谱的后面而不是插到前面：基谱的判定线索引不变，`extra.json` 里按索引引用
/// 判定线的演出效果才不会被挪位。噪域是谱面级字段（不在判定线里），原生百分比与 rpe-next
/// 两种条目都原样搬。
fn merge_json(sources: &[Source]) -> Result<Value> {
    let mut lines = Vec::new();
    let mut areas = Vec::new();
    let mut has_areas = false;

    for source in sources {
        let value: Value = serde_json::from_str(&source.text).context("chart is not valid json")?;
        let Some(list) = value.get("judgeLineList").and_then(Value::as_array) else {
            bail!("chart has no judgeLineList");
        };
        lines.extend(list.iter().cloned());
        if let Some(list) = value.get("blockAreaList").and_then(Value::as_array) {
            has_areas = true;
            areas.extend(list.iter().cloned());
        }
    }

    let mut base: Value =
        serde_json::from_str(&sources[0].text).context("chart is not valid json")?;
    let object = base.as_object_mut().context("chart is not a json object")?;
    object.insert("judgeLineList".to_string(), Value::Array(lines));
    if has_areas {
        object.insert("blockAreaList".to_string(), Value::Array(areas));
    }
    Ok(base)
}

/// 融合后的谱面信息：曲名照旧、谱师把各难度的拼在一起、难度写 `（谱面融合）`、定数留空。
fn merged_info(sources: &[Source]) -> ChartInfo {
    let base = &sources[0].info;
    let mut info = base.clone();

    let mut charter = String::new();
    for source in sources {
        charter.push_str(&format!(" [{}] {}", diff_tag(&source.info), source.info.charter.trim()));
    }
    info.charter = charter.trim().to_string();

    // 难度标识与难度文本都写同一份：界面上的「难度标识」取自 `diff`、「难度文本」取自
    // `difficulty`，只改一处会留下一个 `JOIN`。
    info.diff = DIFF_TEXT.to_string();
    // `info.yml` 的约定是 `level` 放难度文本、`difficulty` 放定数，`load_info` 会拿前者
    // 覆盖后者（见 `dps::fs::info_from_yaml`），所以两处写同一份文本，定数靠 `lvl` 留空。
    info.level = DIFF_TEXT.to_string();
    info.difficulty = DIFF_TEXT.to_string();
    info.lvl = 0.;
    info
}

/// 难度代号：`diff`（`"IN Lv.15.3"` 解析出来的档位）优先，没有就看难度文本的第一个词。
fn diff_tag(info: &ChartInfo) -> String {
    let diff = info.diff.trim();
    if !diff.is_empty() {
        return diff.to_string();
    }
    info.difficulty
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
}

/// 融合结果放在临时目录里：它只用一次，每次融合前整个清掉，不留历史。
fn reset_merge_dir() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(MERGE_DIR);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("cannot clear {}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    Ok(dir)
}

/// 基谱的包内容：`.pez` 直接用它的字节，文件夹先打成一份临时 zip，后面都走 `update_zip`。
async fn base_zip_bytes(path: &Path, dir: &Path) -> Result<Vec<u8>> {
    if path.is_file() {
        return Ok(std::fs::read(path)?);
    }
    let files = collect_chart_files(path.to_path_buf(), path.to_path_buf())?;
    let temp = dir.join("base.zip");
    create_zip(temp.clone(), files).await?;
    let bytes = std::fs::read(&temp)?;
    let _ = std::fs::remove_file(&temp);
    Ok(bytes)
}

/// 输出文件名里的曲名：只留字母数字（含中文）与几个不会惹麻烦的符号。
fn file_stem(name: &str) -> String {
    let stem = name
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.' | ' ') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let stem = stem.trim().to_string();
    if stem.is_empty() {
        "merged".to_string()
    } else {
        stem
    }
}
