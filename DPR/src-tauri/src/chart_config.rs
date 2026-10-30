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
use serde_json::{Map, Value};
use tracing::warn;

/// 谱面根上的配置文件：与 `info.yml` / `chart.json` 平级。
pub const CONFIG_FILE: &str = "config.toml";

/// 键允许的取值形状。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Bool,
    Number,
    /// 定长数字数组，目前只有 `resolution = [宽, 高]` 用得上。
    Array,
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
];

/// 读出谱面根上的 `config.toml`，只保留放行的键。
///
/// - 谱面里没有这个文件：`Ok(None)`（常态，不是错误）；
/// - 文件在但读不出来 / 不是合法 TOML：`Err`，调用方决定怎么提示 —— 静默忽略会让
///   写错键名的谱面看起来「配置没生效」，用户查不出原因；
/// - 单个键不认识或类型不对：跳过它并记一条警告，其它键照常生效。
pub async fn read(path: &Path) -> Result<Option<Map<String, Value>>> {
    let mut fs = dps::fs::fs_from_file(path)?;
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
            warn!("{CONFIG_FILE}: 不支持键 {key}，已忽略");
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
            (_, value) => {
                warn!("{CONFIG_FILE}: 键 {key} 的类型不对（{}），已忽略", value.type_str());
                continue;
            }
        };
        out.insert(key, converted);
    }
    Ok(Some(out))
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
}
