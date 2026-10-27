use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 谱面信息里的文本字段宽松读取。
///
/// 曲名、曲师、难度这些字段在谱面文件里常被写成裸数字（例如曲名 `996`），
/// 按 `String` 直接反序列化会让整份 info.yml 失败，界面上只看到「加载谱面信息失败」。
/// 这里把字符串、整数、浮点、布尔都读成 `String`，`null` 读成空串（`Option` 字段仍是 `None`）。
mod text {
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer};

    /// 一个「什么标量都收」的访问器，`None` 表示 null。
    struct AnyText;

    impl<'de> Visitor<'de> for AnyText {
        type Value = Option<String>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("字符串或数字")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_any(AnyText)
        }
    }

    struct TextValue(Option<String>);

    impl<'de> Deserialize<'de> for TextValue {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            d.deserialize_any(AnyText).map(TextValue)
        }
    }

    pub fn string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        TextValue::deserialize(d).map(|v| v.0.unwrap_or_default())
    }

    pub fn option<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
        TextValue::deserialize(d).map(|v| v.0)
    }

    pub fn strings<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
        Vec::<TextValue>::deserialize(d).map(|v| v.into_iter().filter_map(|v| v.0).collect())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "lowercase")]
pub enum ChartFormat {
    Rpe = 0,
    Pec = 1,
    Pgr = 2,
    Pbc = 3,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
#[serde(rename_all = "camelCase")]
pub struct ChartInfo {
    pub id: Option<i32>,
    pub uploader: Option<i32>,

    #[serde(deserialize_with = "text::string")]
    pub name: String,
    pub lvl: f32,
    #[serde(deserialize_with = "text::string")]
    pub diff: String,
    #[serde(deserialize_with = "text::string")]
    pub level: String,
    #[serde(deserialize_with = "text::string")]
    pub difficulty: String,
    #[serde(deserialize_with = "text::string")]
    pub charter: String,
    #[serde(deserialize_with = "text::string")]
    pub composer: String,
    #[serde(deserialize_with = "text::string")]
    pub illustrator: String,

    pub chart: String,
    pub format: Option<ChartFormat>,
    pub music: String,
    pub illustration: String,
    #[serde(deserialize_with = "text::option")]
    pub unlock_video: Option<String>,

    pub preview_start: f64,
    pub preview_end: Option<f64>,
    pub aspect_ratio: f32,
    pub force_aspect_ratio: bool,
    pub background_dim: f32,
    pub line_length: f32,
    pub offset: f64,
    #[serde(deserialize_with = "text::option")]
    pub tip: Option<String>,
    #[serde(deserialize_with = "text::strings")]
    pub tags: Vec<String>,

    #[serde(deserialize_with = "text::string")]
    pub intro: String,

    pub hold_partial_cover: bool,
    pub negative_length_hold: bool,
    pub note_uniform_scale: bool,
    pub score_total: u32,
    pub hold_particle_interval_ratio: f32,
    pub fold_animation: bool,

    pub created: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    pub chart_updated: Option<DateTime<Utc>>,
}

impl Default for ChartInfo {
    fn default() -> Self {
        Self {
            id: None,
            uploader: None,

            name: "UK".to_string(),
            lvl: 1.,
            diff: "UK".to_string(),
            level: "UK  Lv.1".to_string(),
            difficulty: "UK  Lv.1".to_string(),
            charter: "UK".to_string(),
            composer: "UK".to_string(),
            illustrator: "UK".to_string(),
            unlock_video: None,

            chart: "chart.json".to_string(),
            format: None,
            music: "song.mp3".to_string(),
            illustration: "background.png".to_string(),

            preview_start: 0.,
            preview_end: None,
            aspect_ratio: 16. / 9.,
            force_aspect_ratio: false,
            background_dim: 0.1,
            line_length: 6.0,
            offset: 0.,
            tip: None,
            tags: Vec::new(),

            intro: String::new(),

            hold_partial_cover: false,
            negative_length_hold: true,
            note_uniform_scale: false,
            score_total: 1_000_000,
            hold_particle_interval_ratio: 1.0,
            fold_animation: false,

            created: None,
            updated: None,
            chart_updated: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ChartInfo;

    /// 曲名、曲师、难度写成裸数字时也要能读出来。
    #[test]
    fn numeric_text_fields_are_read_as_string() {
        let yaml = r#"
name: 996
composer: 996
charter: 996
illustrator: 996
diff: 996
level: 996
difficulty: 996
intro: 996
tip: 996
tags: [996, "IN", 15.5]
lvl: 15
"#;
        let info: ChartInfo = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(info.name, "996");
        assert_eq!(info.composer, "996");
        assert_eq!(info.charter, "996");
        assert_eq!(info.illustrator, "996");
        assert_eq!(info.diff, "996");
        assert_eq!(info.level, "996");
        assert_eq!(info.difficulty, "996");
        assert_eq!(info.intro, "996");
        assert_eq!(info.tip.as_deref(), Some("996"));
        assert_eq!(info.tags, vec!["996", "IN", "15.5"]);
        assert_eq!(info.lvl, 15.);
    }

    /// 没写这些字段时仍走默认值，`null` 读成空串 / None。
    #[test]
    fn missing_or_null_text_fields_fall_back() {
        let info: ChartInfo = serde_yaml::from_str("name: null").unwrap();
        assert_eq!(info.name, "");
        assert_eq!(info.composer, "UK");
        assert_eq!(info.tip, None);
        assert!(info.tags.is_empty());
    }
}
