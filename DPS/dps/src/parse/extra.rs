crate::tl_file!("parser" ptl);

use super::RPE_TWEEN_MAP;
#[cfg(feature = "video")]
use crate::core::{Video, VideoAttach};
use crate::{
    core::{Anim, BpmList, ChartExtra, ClampedTween, Effect, Keyframe, StaticTween, Triple, Tweenable, Uniform, EPS},
    fs::FileSystem,
};
#[cfg(feature = "video")]
use crate::ext::ScaleType;
use anyhow::{Context, Result};
use macroquad::prelude::{Color, Vec2, Vec3};
use serde::Deserialize;
use std::{collections::HashMap, rc::Rc};

// serde is weird...
fn f32_zero() -> f32 {
    0.
}

fn f32_one() -> f32 {
    1.
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtKeyframe<T> {
    #[serde(default = "f32_zero")]
    easing_left: f32,
    #[serde(default = "f32_one")]
    easing_right: f32,
    easing_type: i32,
    start: T,
    end: T,
    start_time: Triple,
    end_time: Triple,
}

#[derive(Default, Deserialize)]
#[serde(untagged)]
enum ExtAnim<V> {
    #[default]
    Default,
    Fixed(V),
    Keyframes(Vec<ExtKeyframe<V>>),
}

impl<V> ExtAnim<V> {
    fn into<T: Tweenable>(self, r: &mut BpmList, default: Option<T>) -> Anim<T>
    where
        V: Into<T>,
    {
        match self {
            ExtAnim::Default => Anim::default(),
            ExtAnim::Fixed(value) => Anim::fixed(value.into()),
            ExtAnim::Keyframes(events) => {
                let mut kfs = Vec::with_capacity(events.len() * 2 + 2);
                if let Some(default) = default {
                    if events[0].start_time.beats() != 0.0 {
                        kfs.push(Keyframe::new(0.0, default, 0));
                    }
                }
                for e in events {
                    kfs.push(Keyframe {
                        time: r.time(&e.start_time),
                        value: e.start.into(),
                        tween: {
                            let tween = RPE_TWEEN_MAP.get(e.easing_type.max(1) as usize).copied().unwrap_or(RPE_TWEEN_MAP[0]);
                            if e.easing_left.abs() < EPS as f32 && (e.easing_right - 1.0).abs() < EPS as f32 {
                                StaticTween::get_rc(tween)
                            } else {
                                Rc::new(ClampedTween::new(tween, e.easing_left..e.easing_right))
                            }
                        },
                    });
                    kfs.push(Keyframe::new(r.time(&e.end_time), e.end.into(), 0));
                }
                Anim::new(kfs)
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtBpmItem {
    time: Triple,
    bpm: f64,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum BpmForm {
    Single(f64),
    List(Vec<ExtBpmItem>),
}

impl From<BpmForm> for BpmList {
    fn from(value: BpmForm) -> Self {
        match value {
            BpmForm::Single(value) => BpmList::new(vec![(0., value)]),
            BpmForm::List(list) => BpmList::new(list.into_iter().map(|it| (it.time.beats(), it.bpm)).collect()),
        }
    }
}

/// 着色器颜色参数。
///
/// 谱面里同一份颜色有两种写法：老 RPE 谱面的 0~255，以及 rpe-next 编辑器的 0~1
/// （rpe-next 自己按「任一分量超过 1 就整组除以 255，否则原样」区分，见它的
/// `ui/shader-pipeline.mjs`）。`two_tone` 这类内置着色器的默认值本身就写在 0~1 上，
/// 只按 0~255 读会把 `1` 读成 1/255（几乎全黑），整数以外的值还会让整份
/// `extra.json` 反序列化失败、谱面直接加载不了。
#[derive(Clone, Copy, Deserialize)]
#[serde(transparent)]
struct Rgba([f32; 4]);

impl From<Rgba> for Color {
    fn from(Rgba([r, g, b, a]): Rgba) -> Self {
        // 归一化在这里做（每个关键帧值各自判断一次），插值照旧在 `Color` 上按 0~1 走。
        let scale = if [r, g, b, a].iter().any(|v| v.abs() > 1.) { 1. / 255. } else { 1. };
        Self::new(r * scale, g * scale, b * scale, a * scale)
    }
}

/// 着色器参数。
///
/// 类型面与 rpe-next 对齐（它按 uniform 声明给 `float` / `int` / `bool` 记 1 维、
/// `vecN` 记 N 维，见 `core/shader-uniforms.mjs`）：数组形状有歧义时按「2 个数 = vec2、
/// 3 个数 = vec3、4 个数 = 颜色」区分，`int` 与 `float` 共用 `Float`（JSON 里都是数字），
/// 兼容用 bool 字面量写的谱面。
#[derive(Deserialize)]
#[serde(untagged)]
enum Variable {
    Float(ExtAnim<f32>),
    Vec2(ExtAnim<(f32, f32)>),
    Vec3(ExtAnim<(f32, f32, f32)>),
    Color(ExtAnim<Rgba>),
    Bool(ExtAnim<bool>),
}

#[derive(Deserialize)]
struct ExtEffect {
    start: Triple,
    end: Triple,
    shader: String,
    #[serde(default)]
    vars: HashMap<String, Variable>,
    #[serde(default)]
    global: bool,
}

/// 视频事件。`VideoAttach` 只在 `video` feature 下存在，整个结构体跟着门控。
#[cfg(feature = "video")]
#[derive(Deserialize)]
struct ExtVideo {
    path: String,
    #[serde(default)]
    time: Triple,
    #[serde(default)]
    scale: ScaleType,
    #[serde(default)]
    alpha: ExtAnim<f32>,
    #[serde(default)]
    dim: ExtAnim<f32>,
    #[serde(default)]
    attach: Option<VideoAttach>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Extra {
    bpm: BpmForm,
    #[serde(default)]
    effects: Vec<ExtEffect>,
    #[cfg(feature = "video")]
    #[serde(default)]
    videos: Vec<ExtVideo>,
}

/// 谱面里引用的内置 RPE 着色器 → 引擎内置的那一份源码。
///
/// 引用写法有好几种：`/two_tone.glsl`、`/shaders/pr/two_tone_pr.glsl`、
/// `two_tone_pr`、`/grayscale.glsl` …… 引擎这边有两套内置表：`<名字>_pr` 那份
/// 是现代版 RPE 着色器，10 个老着色器（`grayscale`、`radialBlur`…）在更早的表里。
/// 所以取最后一段文件名、去掉 `.glsl` 之后按这三步找：
///
/// 1. 原样查 `_pr` 表；
/// 2. 补一次 `_pr` 再查 —— rpe-next 的旧谱面引用的是不带 `_pr` 的路径，
///    那在 rpe-next 里指「老版分支」，引擎这边只有现代版，但同名着色器
///    应当照常加载，不该整张谱面报 `shader-load-failed`；
/// 3. 换成驼峰名查老表（`radial_blur` → `radialBlur`、`circle_blur` → `circleBlur`）。
fn rpe_preset(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let name = name.strip_suffix(".glsl").unwrap_or(name);
    let legacy = match name {
        "radial_blur" => "radialBlur",
        "circle_blur" => "circleBlur",
        _ => name,
    };
    Effect::get_rpe_preset(name)
        .or_else(|| Effect::get_rpe_preset(&format!("{name}_pr")))
        .or_else(|| Effect::get_preset(legacy))
}

async fn parse_effect(r: &mut BpmList, rpe: ExtEffect, fs: &mut dyn FileSystem) -> Result<Effect> {
    let range = r.time(&rpe.start)..r.time(&rpe.end);
    let vars = rpe
        .vars
        .into_iter()
        .map(|(name, var)| -> Result<Box<dyn Uniform>> {
            Ok(match var {
                Variable::Float(events) => Box::new((name, events.into::<f32>(r, None))),
                Variable::Vec2(events) => Box::new((name, events.into::<Vec2>(r, None))),
                Variable::Vec3(events) => Box::new((name, events.into::<Vec3>(r, None))),
                Variable::Color(events) => Box::new((name, events.into::<Color>(r, None))),
                Variable::Bool(events) => Box::new((name, events.into::<bool>(r, None))),
            })
        })
        .collect::<Result<_>>()?;
    let chart_only = rpe.shader.starts_with('/');
    let string;
    let mut effect = Effect::new(
        range,
        if let Some(path) = rpe.shader.strip_prefix('/') {
            if let Ok(file) = fs.load_file(path).await {
                string = String::from_utf8(file).with_context(|| ptl!("shader-load-failed", "path" => path))?;
                &string
            } else if let Some(rpe) = rpe_preset(path) {
                rpe
            } else {
                return Err(ptl!(err "shader-load-failed", "path" => rpe.shader));
            }
        } else {
            Effect::get_preset(&rpe.shader).ok_or_else(|| ptl!(err "shader-not-found", "shader" => rpe.shader))?
        },
        vars,
        rpe.global,
    )?;
    effect.chart_only = chart_only;
    Ok(effect)
}

pub async fn parse_extra(source: &str, fs: &mut dyn FileSystem) -> Result<ChartExtra> {
    let ext: Extra = serde_json::from_str(source).with_context(|| ptl!("json-parse-failed"))?;
    let mut r: BpmList = ext.bpm.into();
    let mut effects = Vec::new();
    let mut global_effects = Vec::new();
    for (id, effect) in ext.effects.into_iter().enumerate() {
        (if effect.global { &mut global_effects } else { &mut effects }).push(
            parse_effect(&mut r, effect, fs)
                .await
                .with_context(|| ptl!("effect-location", "id" => id))?,
        );
    }
    #[cfg(feature = "video")]
    let mut videos = Vec::new();
    #[cfg(feature = "video")]
    for video in ext.videos {
        videos.push((
            Video::new(
                fs.load_file(&video.path)
                    .await
                    .with_context(|| ptl!("video-load-failed", "path" => video.path.clone()))?,
                r.time(&video.time),
                video.scale,
                video.alpha.into(&mut r, Some(1.)),
                video.dim.into(&mut r, Some(0.)),
            )
            .with_context(|| ptl!("video-load-failed", "path" => video.path))?,
            video.attach,
        ));
    }
    Ok(ChartExtra {
        effects,
        global_effects,
        #[cfg(feature = "video")]
        videos,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(values: [f32; 4]) -> Color {
        Rgba(values).into()
    }

    #[test]
    fn normalised_color_kept() {
        // rpe-next 里 two_tone 的默认值：color1 = 黑、color2 = 浅蓝，都写在 0~1 上。
        let black = rgba([0., 0., 0., 1.]);
        assert_eq!((black.r, black.g, black.b, black.a), (0., 0., 0., 1.));
        let blue = rgba([0.6, 0.6, 1., 1.]);
        assert_eq!((blue.r, blue.b, blue.a), (0.6, 1., 1.));
    }

    #[test]
    fn byte_color_scaled_down() {
        let color = rgba([127., 127., 255., 255.]);
        assert!((color.r - 127. / 255.).abs() < 1e-6);
        assert_eq!((color.b, color.a), (1., 1.));
    }

    /// 着色器参数：rpe-next 写 0~1 浮点、老谱面写 0~255 整数，两种都得能读进来。
    #[test]
    fn shader_vars_parse_both_scales() {
        let vars: HashMap<String, Variable> = serde_json::from_str(
            r#"{"color1":[0.6,0.6,1.0,1.0],"color2":[127,127,255,255],"threshold":0.5,"rate":[{"start":[0.0,0.0,0.0,1.0],"end":[1.0,1.0,1.0,1.0],"startTime":[0,0,1],"endTime":[4,0,1],"easingType":1}]}"#,
        )
        .expect("shader vars should parse");
        assert_eq!(vars.len(), 4);
        assert!(matches!(vars["threshold"], Variable::Float(_)));
        assert!(matches!(vars["color1"], Variable::Color(_)));
        assert!(matches!(vars["rate"], Variable::Color(_)));
    }
}
