//! 谱面自带的渲染配置：谱面包（文件夹或 `.pez`）根上放一份 `config.toml`。
//!
//! 只有 DPR 读这个文件，引擎（`dps`）完全不认识它。渲染页导入谱面时读出来，合并到
//! 当前的工作配置（`RenderConfig`）上 —— 于是「这张谱面自己决定怎么被渲染」，不受用户
//! 当时的配置影响。典型用途是工具页的「噪域播放器」：它要求只留噪域、偏移 0、总分 1。
//!
//! 只放行 [`ALLOWED`] 里的键，其余键一律忽略。除了「谱面长什么样」，还放行
//! `resolution` / `fps`：工具页生成的噪域谱面要按原视频的尺寸与帧率出片。
//! 编码器、硬件加速这类「怎么跑」的参数仍然不许谱面改。
//!
//! 键名与程序的 `config.toml` 一致（camelCase，见 `render.rs::RenderConfig`）。

use std::path::Path;

use anyhow::{Context, Result};
use dps::info::ChartInfo;
use serde_json::{Map, Value};
use tracing::warn;

/// 谱面根上的配置文件：与 `info.yml` / `chart.json` 平级。
pub const CONFIG_FILE: &str = "config.toml";

/// 谱面信息文件：`load_info` 优先读它，所以换掉难度时写这一份就够（`.txt` 那套会被它盖住）。
pub const INFO_FILE: &str = "info.yml";

/// 键允许的取值形状。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Bool,
    Number,
    /// 单个字符串，例如按键替换里的目标键型（`tap` / `drag` / `hold` / `flick`）。
    String,
    /// 定长数字数组，目前只有 `resolution = [宽, 高]` 用得上。
    Array,
    /// 变长字符串数组：`chartModOrder` 里的修改名顺序。
    Strings,
}

/// 允许谱面强制设置的键：画面构成、谱面侧参数与出片尺寸。
///
/// - 谱面侧参数（`offset` / `scoreTotal` / `aspectRatio` …）会被合并进 `info`，与
///   `src/config/chartSide.ts` 的 `CHART_SIDE_KEYS` 同一套键；
/// - 开关（`renderLine` / `renderNote` / `renderUi*` / `renderBg` …）直接盖到渲染配置上；
/// - `resolution` / `fps`：噪域谱面按原视频的尺寸与帧率出片。
const ALLOWED: &[(&str, Kind)] = &[
    // 谱面侧参数
    ("offset", Kind::Number),
    ("scoreTotal", Kind::Number),
    ("aspectRatio", Kind::Number),
    ("forceAspectRatio", Kind::Bool),
    ("lineLength", Kind::Number),
    ("previewStart", Kind::Number),
    ("previewEnd", Kind::Number),
    ("backgroundDim", Kind::Number),
    ("foldAnimation", Kind::Bool),
    ("noteUniformScale", Kind::Bool),
    ("holdPartialCover", Kind::Bool),
    ("negativeLengthHold", Kind::Bool),
    ("holdParticleIntervalRatio", Kind::Number),
    // 出片尺寸：跟原视频一致
    ("resolution", Kind::Array),
    ("fps", Kind::Number),
    // 画面构成
    ("renderBg", Kind::Bool),
    ("renderBgDim", Kind::Bool),
    ("renderLine", Kind::Bool),
    ("renderLineExtra", Kind::Bool),
    ("renderNote", Kind::Bool),
    ("renderDoubleHint", Kind::Bool),
    ("renderPendingNote", Kind::Bool),
    ("renderUiPause", Kind::Bool),
    ("renderUiName", Kind::Bool),
    ("renderUiLevel", Kind::Bool),
    ("renderUiDifficulty", Kind::Bool),
    ("renderUiScore", Kind::Bool),
    ("renderUiCombo", Kind::Bool),
    ("renderUiBar", Kind::Bool),
    // 噪域
    ("renderBlockArea", Kind::Bool),
    ("blockAreaHover", Kind::Bool),
    ("blockAreaReuse", Kind::Bool),
    ("blockAreaForcePosition", Kind::Bool),
    ("blockAreaAlpha", Kind::Number),
    ("blockAreaDensity", Kind::Number),
    ("blockAreaPixelScale", Kind::Number),
    // 谱面修改：DPE 的「生成谱面」把这份配置写进新的 `.pez`，导入时合并回工作配置，
    // 于是修改跟着谱面走，不依赖用户当时的设置。
    ("pureConfigChart", Kind::Bool),
    ("pureConfigChartLineY", Kind::Number),
    ("pureConfigChartSpeed", Kind::Number),
    ("pureConfigChartSideLines", Kind::Bool),
    ("pureConfigChartSideX", Kind::Number),
    ("pureConfigChartAngle", Kind::Number),
    ("pureConfigChartNoise", Kind::Bool),
    ("pureConfigChartNoiseHeight", Kind::Number),
    ("pureConfigChartNoiseWidth", Kind::Number),
    ("pureConfigChartNoiseTime", Kind::Number),
    ("pureConfigChartNoiseDelay", Kind::Number),
    ("fixedLaneChart", Kind::Bool),
    ("fixedLaneGap", Kind::Number),
    ("fixedLaneSpeed", Kind::Number),
    ("fixedLaneLineY", Kind::Number),
    ("fixedLaneInterval", Kind::Number),
    ("fixedLaneHoldLength", Kind::Bool),
    ("fixedLaneThird", Kind::Bool),
    ("chartModOrder", Kind::Strings),
    ("chartModHide", Kind::Bool),
    ("chartModArrange", Kind::Bool),
    ("chartModArrangeGap", Kind::Number),
    ("chartModArrangeLineY", Kind::Number),
    ("chartModArrangeSpeed", Kind::Number),
    ("chartModReplace", Kind::Bool),
    ("chartModReplaceTap", Kind::String),
    ("chartModReplaceDrag", Kind::String),
    ("chartModReplaceHold", Kind::String),
    ("chartModReplaceFlick", Kind::String),
    ("chartModRandom", Kind::Bool),
    ("chartModRandomSpeed", Kind::Bool),
    ("chartModRandomSpeedMin", Kind::Number),
    ("chartModRandomSpeedMax", Kind::Number),
    ("chartModFixedSpeed", Kind::Bool),
    ("chartModFixedSpeedValue", Kind::Number),
    ("chartModSpreadHold", Kind::Bool),
    ("chartModSpreadHoldNoteType", Kind::String),
    ("chartModSpreadHoldInterval", Kind::Number),
    ("chartModExtendHold", Kind::Bool),
    ("chartModExtendHoldBlank", Kind::Number),
    ("chartModCopy", Kind::Bool),
    ("chartModCopyTimes", Kind::Number),
    ("chartModCopyOffset", Kind::Number),
    ("chartModRandomPos", Kind::Bool),
    ("chartModRandomPosRange", Kind::Number),
];

/// 读出谱面根上的 `config.toml`，只保留放行的键。
///
/// - 谱面里没有这个文件：`Ok(None)`（常态，不是错误）；
/// - 文件在但读不出来 / 不是合法 TOML：`Err`，调用方决定怎么提示 —— 静默忽略会让
///   写错键名的谱面看起来「配置没生效」，用户查不出原因；
/// - 单个键不认识或类型不对：跳过它并记一条警告，其它键照常生效。
pub async fn read(path: &Path) -> Result<Option<Map<String, Value>>> {
    let mut fs = dps::fs::fs_from_file(path)
        .with_context(|| format!("读取谱面失败：{}", path.display()))?;
    let bytes = match fs.load_file(CONFIG_FILE).await {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };
    let text = String::from_utf8(bytes)
        .with_context(|| format!("谱面里的 {CONFIG_FILE} 不是 UTF-8"))?;
    let table: toml::Table = toml::from_str(&text)
        .with_context(|| format!("谱面里的 {CONFIG_FILE} 不是合法 TOML"))?;

    let mut out = Map::new();
    for (key, value) in table {
        let Some((_, kind)) = ALLOWED.iter().find(|(name, _)| *name == key) else {
            warn!("[配置] {CONFIG_FILE}：不支持键 {key}，已忽略");
            continue;
        };
        let converted = match (kind, value) {
            (Kind::Bool, toml::Value::Boolean(value)) => Value::Bool(value),
            (Kind::Number, toml::Value::Integer(value)) => Value::from(value),
            (Kind::Number, toml::Value::Float(value)) => Value::from(value),
            // `resolution`：两个非负整数，多一个少一个都不认（否则渲染那边取下标会 panic）
            (Kind::Array, toml::Value::Array(values))
                if values.len() == 2
                    && values.iter().all(|it| matches!(it, toml::Value::Integer(n) if *n >= 0)) =>
            {
                Value::Array(
                    values
                        .iter()
                        .map(|it| Value::from(it.as_integer().unwrap_or(0)))
                        .collect(),
                )
            }
            (Kind::String, toml::Value::String(value)) => Value::String(value),
            (Kind::Strings, toml::Value::Array(values)) => Value::Array(
                values
                    .iter()
                    .filter_map(|it| it.as_str().map(|text| Value::String(text.to_owned())))
                    .collect(),
            ),
            (_, value) => {
                warn!("[配置] {CONFIG_FILE}：键 {key} 的类型不对（{}），已忽略", value.type_str());
                continue;
            }
        };
        out.insert(key, converted);
    }
    Ok(Some(out))
}

/// 把一份配置写进新的 `.pez`：原包的条目照抄，`config.toml` 换成 `entries`。
///
/// 只在「生成谱面」时用。源包里的旧 `config.toml` 会被丢掉 —— 调用方已经把旧配置
/// （经 [`read`] 过滤）与新修改合并好了，再叠一层会写进重复键。
///
/// `info` 给定时连包里的谱面信息一起换掉（生成谱面要把难度改成修改链）。旧的那份必须
/// 跳过：同名条目在 zip 里留两份时读到的是先写进去的那份。只有 `info.txt` 的包也能这么
/// 盖住 —— [`INFO_FILE`] 的优先级比它高。
pub fn write_generated(
    source: &Path,
    dest: &Path,
    entries: &Map<String, Value>,
    info: Option<&ChartInfo>,
) -> Result<()> {
    let file = std::fs::File::open(source)
        .with_context(|| format!("打不开谱面 {}", source.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("{} 不是合法的 .pez", source.display()))?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("建不出输出目录 {}", parent.display()))?;
    }
    let out = std::fs::File::create(dest).with_context(|| format!("写不出 {}", dest.display()))?;
    let mut writer = zip::ZipWriter::new(out);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // 原包的条目逐个搬运：目录项、旧配置与（要重写的）旧谱面信息跳过，其余保持原路径。
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_owned();
        if name == CONFIG_FILE || name.ends_with('/') || (info.is_some() && name == INFO_FILE) {
            continue;
        }
        writer.start_file(name, options)?;
        std::io::copy(&mut entry, &mut writer)?;
    }

    let mut table = toml::Table::new();
    for (key, value) in entries {
        if let Some(value) = to_toml_value(value) {
            table.insert(key.clone(), value);
        }
    }
    writer.start_file(CONFIG_FILE, options)?;
    std::io::Write::write_all(&mut writer, toml::to_string(&table)?.as_bytes())?;

    if let Some(info) = info {
        writer.start_file(INFO_FILE, options)?;
        std::io::Write::write_all(&mut writer, serde_yaml::to_string(info)?.as_bytes())?;
    }
    writer.finish().context("收尾 .pez 时失败")?;
    Ok(())
}

/// `serde_json::Value` → `toml::Value`：只认白名单里会出现的那几种形状。
fn to_toml_value(value: &Value) -> Option<toml::Value> {
    match value {
        Value::Bool(it) => Some(toml::Value::Boolean(*it)),
        Value::Number(it) => it
            .as_i64()
            .map(toml::Value::Integer)
            .or_else(|| it.as_f64().map(toml::Value::Float)),
        Value::String(it) => Some(toml::Value::String(it.clone())),
        Value::Array(items) => Some(toml::Value::Array(
            items.iter().filter_map(to_toml_value).collect(),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 放行的键照收、类型不对的键与不在白名单的键跳过；没有这个文件时返回 `None`。
    #[test]
    fn filters_to_the_whitelist() {
        let dir = std::env::temp_dir().join(format!("dpr-chart-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(CONFIG_FILE),
            "offset = 0.5\nrenderLine = false\nrenderNote = 1\nresolution = [640, 360]\nfps = 30\naspectRatio = [1, 2]\n",
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let map = runtime.block_on(read(&dir)).unwrap().unwrap();
        assert_eq!(map.get("offset"), Some(&serde_json::json!(0.5)));
        assert_eq!(map.get("renderLine"), Some(&serde_json::json!(false)));
        assert_eq!(map.get("resolution"), Some(&serde_json::json!([640, 360])));
        assert_eq!(map.get("fps"), Some(&serde_json::json!(30)));
        // `renderNote` 是开关，写成数字就不认；`aspectRatio` 要数字，写成数组也不认
        assert!(!map.contains_key("renderNote"));
        assert!(!map.contains_key("aspectRatio"));

        std::fs::remove_file(dir.join(CONFIG_FILE)).unwrap();
        assert!(runtime.block_on(read(&dir)).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 生成时换掉 `info.yml`：包里的旧那份不能留成重复条目，难度改完后仍然读得出来。
    #[test]
    fn write_generated_replaces_info() {
        use std::ops::DerefMut;

        let dir = std::env::temp_dir().join(format!("dpr-chart-config-gen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("src.pez");
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&source).unwrap());
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("chart.json", options).unwrap();
            std::io::Write::write_all(&mut zip, br#"{"formatVersion":3,"judgeLineList":[]}"#)
                .unwrap();
            zip.start_file(INFO_FILE, options).unwrap();
            std::io::Write::write_all(
                &mut zip,
                b"name: test\nchart: chart.json\nmusic: song.mp3\nillustration: background.png\n",
            )
            .unwrap();
            zip.finish().unwrap();
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut pack = dps::fs::fs_from_file(&source).unwrap();
        let mut info = runtime.block_on(dps::fs::load_info(pack.deref_mut())).unwrap();
        let text = "（随机流速 → 随机按键）".to_string();
        info.diff = text.clone();
        info.level = text.clone();
        info.difficulty = text.clone();

        let dest = dir.join("out.pez");
        write_generated(&source, &dest, &Map::new(), Some(&info)).unwrap();

        let mut read_back = dps::fs::fs_from_file(&dest).unwrap();
        let got = runtime.block_on(dps::fs::load_info(read_back.deref_mut())).unwrap();
        assert_eq!(got.diff, text);
        assert_eq!(got.difficulty, text);

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&dest).unwrap()).unwrap();
        assert_eq!(archive.file_names().filter(|name| *name == INFO_FILE).count(), 1);
        assert!(archive.by_name("chart.json").is_ok());
        assert!(archive.by_name(CONFIG_FILE).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
