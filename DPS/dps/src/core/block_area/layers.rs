//! rpe-next v230 的噪域层（`noiseAreaLayers`）。
//!
//! v230 起噪域按「层」组织：每层有自己的皮肤包（`shader/<名字>`）、参数与通道，谱面里的
//! 每条噪域用 `layerId` 指向一层。层的顺序就是绘制顺序，遮挡判定也按层各自合成
//! （见 [`super::next::blocked`]）。
//!
//! DPS 不跑皮肤包里的 GLSL 管线（`config.json` 声明的多趟 RT 依赖完整的 shader 框架），
//! 所有层都用内置官方材质画；这里只把层信息解析出来，用于分组、绘制顺序与包名诊断。

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use tracing::warn;

/// 谱面没给 `noiseAreaLayers` 时的单层 id。
pub const DEFAULT_LAYER_ID: &str = "default";

/// 层的皮肤通道：`low` 是给低性能机器准备的简化皮肤变体。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Channel {
    #[default]
    Normal,
    Low,
}

impl Channel {
    fn parse(value: Option<&str>) -> Self {
        match value {
            None | Some("normal") => Self::Normal,
            Some("low") => Self::Low,
            Some(other) => {
                warn!("noiseAreaLayers: 未知的 channel {other:?}，按 normal 处理");
                Self::Normal
            }
        }
    }
}

/// 一层噪域的定义。
#[derive(Clone, Debug)]
pub struct NoiseLayer {
    /// 层 id；噪域的 `layerId` 指向它，没写 `layerId` 的噪域归到 [`DEFAULT_LAYER_ID`]。
    pub id: String,
    /// 层名，只在编辑器与日志里用。
    pub name: String,
    /// 皮肤包目录，形如 `shader/NoiseField-builtin`；`None` 表示没配皮肤。
    pub skin: Option<String>,
    pub channel: Channel,
    /// 谱面覆盖的皮肤参数（皮肤包 `config.json` 里声明的参数名 → 数值或向量）。
    pub parameters: Value,
    /// 编辑器切皮肤时记下的各皮肤参数，渲染不读。
    pub skin_presets: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LayerData {
    id: Option<String>,
    #[serde(default)]
    name: String,
    skin: Option<String>,
    channel: Option<String>,
    #[serde(default)]
    parameters: Value,
    #[serde(default)]
    skin_presets: Value,
}

impl NoiseLayer {
    fn single() -> Self {
        Self {
            id: DEFAULT_LAYER_ID.to_owned(),
            name: String::new(),
            skin: None,
            channel: Channel::Normal,
            parameters: Value::Null,
            skin_presets: Value::Null,
        }
    }

    fn from_data(data: LayerData) -> Option<Self> {
        let id = data.id?;
        if id.is_empty() {
            warn!("noiseAreaLayers: 层的 id 为空，已忽略这一层");
            return None;
        }
        // 皮肤包只认 `shader/<名字>`：与 rpe-next 的校验一致，路径里不许有层级。
        let skin = data.skin.filter(|skin| {
            let valid = skin.starts_with("shader/") && !skin["shader/".len()..].contains('/');
            if !valid {
                warn!("noiseAreaLayers: 皮肤路径 {skin:?} 不是 shader/<名字>，按没有皮肤处理");
            }
            valid
        });
        Some(Self {
            id,
            name: data.name,
            skin,
            channel: Channel::parse(data.channel.as_deref()),
            parameters: data.parameters,
            skin_presets: data.skin_presets,
        })
    }
}

/// 一份谱面的全部噪域层，按谱面里的顺序（也就是绘制顺序）。
pub struct NoiseLayers {
    layers: Vec<NoiseLayer>,
    index: HashMap<String, usize>,
}

impl Default for NoiseLayers {
    fn default() -> Self {
        let layer = NoiseLayer::single();
        let index = HashMap::from([(layer.id.clone(), 0)]);
        Self { layers: vec![layer], index }
    }
}

impl NoiseLayers {
    /// 解析 `noiseAreaLayers`。
    ///
    /// 容错口径与 [`super::next::parse`] 一致：单层坏数据只丢这一层，不连累整份谱面。
    /// 没有可用的层时退回单层 [`DEFAULT_LAYER_ID`] —— 噪域的 `layerId` 缺省也指向它。
    pub fn parse(raw: Option<&[Value]>) -> Self {
        let raw = raw.unwrap_or_default();
        if raw.is_empty() {
            return Self::default();
        }
        let mut layers: Vec<NoiseLayer> = Vec::with_capacity(raw.len());
        let mut index = HashMap::with_capacity(raw.len());
        for (i, value) in raw.iter().enumerate() {
            let data: LayerData = match serde_json::from_value(value.clone()) {
                Ok(data) => data,
                Err(error) => {
                    warn!("noiseAreaLayers[{i}]: {error:#}");
                    continue;
                }
            };
            let Some(layer) = NoiseLayer::from_data(data) else {
                continue;
            };
            if index.contains_key(&layer.id) {
                warn!("noiseAreaLayers[{i}]: 重复的层 id {:?}，已忽略", layer.id);
                continue;
            }
            index.insert(layer.id.clone(), layers.len());
            layers.push(layer);
        }
        if layers.is_empty() {
            warn!("noiseAreaLayers: 没有可用的层，改用单层 {DEFAULT_LAYER_ID:?}");
            return Self::default();
        }
        Self { layers, index }
    }

    /// 层 id 对应的下标（`noiseAreaLayers` 里的位置，也就是绘制顺序）。
    pub fn index(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }

    pub fn layers(&self) -> &[NoiseLayer] {
        &self.layers
    }

    pub fn into_layers(self) -> Vec<NoiseLayer> {
        self.layers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> NoiseLayers {
        NoiseLayers::parse(value.as_array().map(Vec::as_slice))
    }

    #[test]
    fn missing_layers_fall_back_to_a_single_default() {
        let layers = NoiseLayers::parse(None);
        assert_eq!(layers.layers().len(), 1);
        assert_eq!(layers.layers()[0].id, DEFAULT_LAYER_ID);
        assert_eq!(layers.index(DEFAULT_LAYER_ID), Some(0));
        assert_eq!(layers.index("other"), None);
        // 空数组与缺省等价：噪域的 layerId 缺省也指向 default。
        let empty = parse(json!([]));
        assert_eq!(empty.layers().len(), 1);
        assert_eq!(empty.index(DEFAULT_LAYER_ID), Some(0));
    }

    #[test]
    fn parses_layers_in_chart_order() {
        let value = json!([
            {"id": "b", "name": "前景", "skin": "shader/NoiseField-builtin", "parameters": {"touchReleaseDuration": 0.2}},
            {"id": "a", "name": "背景", "channel": "low", "skinPresets": {"shader/x": {"parameters": {}}}},
        ]);
        let layers = parse(value);
        assert_eq!(layers.layers().len(), 2);
        // 顺序就是谱面里的顺序：绘制时也按这个顺序叠。
        assert_eq!(layers.index("b"), Some(0));
        assert_eq!(layers.index("a"), Some(1));
        assert_eq!(layers.layers()[0].skin.as_deref(), Some("shader/NoiseField-builtin"));
        assert_eq!(layers.layers()[0].parameters["touchReleaseDuration"], 0.2);
        assert_eq!(layers.layers()[1].channel, Channel::Low);
        assert_eq!(layers.layers()[1].name, "背景");
    }

    #[test]
    fn invalid_layers_are_dropped_one_by_one() {
        let value = json!([
            {"name": "没有 id"},
            {"id": ""},
            {"id": "dup"},
            {"id": "dup", "name": "重复"},
            {"id": 7},
            {"id": "ok", "channel": "high"},
        ]);
        let layers = parse(value);
        assert_eq!(layers.layers().len(), 2);
        assert_eq!(layers.index("dup"), Some(0));
        assert_eq!(layers.index("ok"), Some(1));
        assert_eq!(layers.layers()[1].channel, Channel::Normal);
    }
}
