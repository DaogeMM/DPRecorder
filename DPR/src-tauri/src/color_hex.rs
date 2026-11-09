//! 配置里的颜色：写得出的地方一律是 `#rrggbb`，读得回来的既认色号也认 `[r, g, b]` 数组。
//!
//! `RenderConfig` 的字段在内存里仍是 `[f32; 3]`（0~1），只有序列化时才换成文本 ——
//! 这样 `presets.toml` / `config.toml` / `tasks.json` 里看到的是能直接抄进输入框的色号，
//! 而早先写成三个浮点的旧文件也照样读得回来。反序列化只影响读，不会把数组再写出去。

use serde::de::{self, Unexpected};
use serde::{Deserialize, Deserializer, Serializer};

/// 0~1 的三分量 → `#RRGGBB`（大写，与界面上的输入框同一种写法）。
///
/// 按 8 位色深取整，用的是 [`crate::common::color8`] 的同一套口径（默认值写成十进制
/// `0.9` 时 f32 是 0.89999998，乘 255 得 229.49999，不补那一点就会取成 229）。
pub fn to_hex(rgb: &[f32; 3]) -> String {
    let mut out = String::with_capacity(7);
    out.push('#');
    for it in rgb {
        let byte = crate::common::color8((*it as f64).clamp(0., 1.)).clamp(0, 255) as u8;
        out.push_str(&format!("{byte:02X}"));
    }
    out
}

/// `#RRGGBB`（`#` 可省、大小写都认）→ 0~1 的三分量。认不出来返回 `None`。
pub fn from_hex(text: &str) -> Option<[f32; 3]> {
    let text = text.trim();
    let digits = text.strip_prefix('#').unwrap_or(text);
    if digits.len() != 6 || !digits.chars().all(|it| it.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    Some([
        ((value >> 16) & 0xff) as f32 / 255.,
        ((value >> 8) & 0xff) as f32 / 255.,
        (value & 0xff) as f32 / 255.,
    ])
}

/// 文件里能出现的两种写法。
#[derive(Deserialize)]
#[serde(untagged)]
enum Repr {
    Hex(String),
    Rgb([f32; 3]),
}

impl Repr {
    fn into_rgb<E: de::Error>(self) -> Result<[f32; 3], E> {
        match self {
            Repr::Hex(text) => from_hex(&text)
                .ok_or_else(|| E::invalid_value(Unexpected::Str(&text), &"`#RRGGBB` 或三个 0~1 的数")),
            Repr::Rgb(rgb) => Ok(rgb),
        }
    }
}

pub fn serialize<S: Serializer>(rgb: &[f32; 3], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&to_hex(rgb))
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[f32; 3], D::Error> {
    Repr::deserialize(deserializer)?.into_rgb()
}

/// `Option<[f32; 3]>` 的那一份，配合 `#[serde(with = "crate::color_hex::option")]`。
///
/// `None`（比如「纯色背景」没填）写 `null` / 不写这一行，读回来还是 `None`。
pub mod option {
    use super::*;

    pub fn serialize<S: Serializer>(rgb: &Option<[f32; 3]>, serializer: S) -> Result<S::Ok, S::Error> {
        match rgb {
            Some(rgb) => serializer.serialize_some(&to_hex(rgb)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<[f32; 3]>, D::Error> {
        match Option::<Repr>::deserialize(deserializer)? {
            Some(repr) => Ok(Some(repr.into_rgb()?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Wrapper {
        #[serde(with = "crate::color_hex")]
        color: [f32; 3],
        #[serde(
            default,
            with = "crate::color_hex::option",
            skip_serializing_if = "Option::is_none"
        )]
        maybe: Option<[f32; 3]>,
    }

    /// 写成 `#RRGGBB`（大写），读回来还是同一个 8 位色。
    #[test]
    fn writes_hex_and_reads_both_forms() {
        let value = Wrapper {
            color: [230. / 255., 26. / 255., 26. / 255.],
            maybe: None,
        };
        let toml_text = toml::to_string(&value).unwrap();
        assert!(toml_text.contains("color = \"#E61A1A\""), "{toml_text}");
        assert!(!toml_text.contains("maybe"), "{toml_text}");

        // 色号与数组两种写法都读得回来，读出来的 8 位色一致
        let from_hex: Wrapper = toml::from_str("color = \"#E61A1A\"").unwrap();
        assert_eq!(to_hex(&from_hex.color), "#E61A1A");
        let from_rgb: Wrapper = toml::from_str("color = [0.902, 0.102, 0.102]").unwrap();
        assert_eq!(to_hex(&from_rgb.color), "#E61A1A");
        // 小写、省略 `#` 也认
        let loose: Wrapper = toml::from_str("color = \"e61a1a\"").unwrap();
        assert_eq!(to_hex(&loose.color), "#E61A1A");
    }

    /// JSON（`tasks.json`）走同一套：写色号，`Option` 为 `None` 时写 `null`、读得回来。
    #[test]
    fn json_round_trip() {
        let value = Wrapper {
            color: [1., 1., 1.],
            maybe: Some([0., 0., 0.]),
        };
        let text = serde_json::to_string(&value).unwrap();
        assert_eq!(text, r##"{"color":"#FFFFFF","maybe":"#000000"}"##, "{text}");
        let back: Wrapper = serde_json::from_str(&text).unwrap();
        assert_eq!(back, value);
        let none: Wrapper = serde_json::from_str(r#"{"color":[1,1,1],"maybe":null}"#).unwrap();
        assert_eq!(none.maybe, None);
    }

    /// 256 个色阶逐个走一遍「数组 → 色号 → 数组 → 色号」，一个都不能漂。
    ///
    /// 这条是给 `to_hex` 的取整口径兜底的：`0.9` 这种写成十进制的默认值在 f32 里只有
    /// 0.89999998，乘 255 差一点，不加那 `1e-3` 就会整段错位。
    #[test]
    fn every_byte_survives_a_round_trip() {
        for value in 0..=255u32 {
            let rgb = value as f32 / 255.;
            let rgb = [rgb, rgb, rgb];
            let text = to_hex(&rgb);
            let back = from_hex(&text).unwrap();
            assert_eq!(to_hex(&back), text, "value = {value}");
            assert_eq!(back[0], rgb[0], "value = {value}");
        }
    }
}
