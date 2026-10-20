//! 「痉挛SP」演出：把正中那根判定线按时间段换成插画。
//!
//! 这张谱面里除了真正打歌的那根线，还有 23 根「藏在正中、`alpha` 恒 0」的判定线；正常出画
//! 什么都不显示。打开演出的那几个时间段里，这里把插画按屏幕居中画上去，颜色取判定线的
//! 「完美」色（`ResPackInfo::line_perfect`，`line_tinted` 打开时就是官方的偏黄蒙版色），
//! 于是看起来像「那根判定线被换成了图片」。其余时间什么都不画。
//!
//! 素材在 `appendant/appendant.spas.zip` 里，不随程序分发；缺包时这段演出静默不画。

use macroquad::prelude::*;
use once_cell::sync::Lazy;
use std::sync::Mutex;

use crate::{
    config::Config,
    core::{appendant, Resource},
    ext::SafeTexture,
};

/// 第一段：`SpasmodicChan`。
static CHAN_TEX: Lazy<Option<Texture2D>> = Lazy::new(|| load(appendant::spas_chan_image()));

/// 第二段：「光」。
static GUANG_TEX: Lazy<Option<Texture2D>> = Lazy::new(|| load(appendant::spas_guang_image()));

/// 第三段：「もぺもぺ」。
static MOPEMOPE_TEX: Lazy<Option<Texture2D>> = Lazy::new(|| load(appendant::spas_mopemope_image()));

/// 第二、三段的模糊版。整幅曲绘铺在谱面上太抢眼，所以和游戏背景一样先做一遍高斯模糊
/// （半径取 `bg_blurriness`）。半径被改过就重新糊一次，其余时间命中缓存。
static GUANG_BLUR: Lazy<Mutex<Option<(f32, SafeTexture)>>> = Lazy::new(|| Mutex::new(None));
static MOPEMOPE_BLUR: Lazy<Mutex<Option<(f32, SafeTexture)>>> = Lazy::new(|| Mutex::new(None));

/// 三段演出的时间窗（秒）。
const SEGMENTS: [(f64, f64); 3] = [(6.00, 7.20), (92.40, 111.60), (130.80, 139.80)];

fn load(bytes: Option<Vec<u8>>) -> Option<Texture2D> {
    let source = image::load_from_memory(&bytes?).ok()?.to_rgba8();
    let texture = Texture2D::from_rgba8(source.width() as u16, source.height() as u16, source.as_raw());
    texture.set_filter(FilterMode::Linear);
    Some(texture)
}

/// 三段插画是否全都读到了（缺一张就照缺的那段不画，所以这里只用于提示）。
pub fn available() -> bool {
    CHAN_TEX.is_some() && GUANG_TEX.is_some() && MOPEMOPE_TEX.is_some()
}

/// 取某一档模糊半径下的模糊版贴图；`bytes` 只在需要重糊时才会被调用。
fn blurred(cache: &Mutex<Option<(f32, SafeTexture)>>, radius: f32, bytes: impl FnOnce() -> Option<Vec<u8>>) -> Option<Texture2D> {
    let mut guard = cache.lock().ok()?;
    if let Some((cached, texture)) = guard.as_ref() {
        if (*cached - radius).abs() < 0.01 {
            return Some(**texture);
        }
    }
    let image = image::load_from_memory(&bytes()?).ok()?;
    let texture = crate::ext::blur_image(image, radius).ok()?;
    let raw = *texture;
    guard.replace((radius, texture));
    Some(raw)
}

/// 每一段的（高度比例、上下偏移、淡入秒数、淡出秒数）。
fn params(config: &Config, index: usize) -> (f32, f32, f32, f32) {
    match index {
        0 => (
            config.jingluan_sp_chan_height,
            config.jingluan_sp_chan_offset_y,
            config.jingluan_sp_chan_fade_in,
            config.jingluan_sp_chan_fade_out,
        ),
        1 => (
            config.jingluan_sp_guang_height,
            config.jingluan_sp_guang_offset_y,
            config.jingluan_sp_guang_fade_in,
            config.jingluan_sp_guang_fade_out,
        ),
        _ => (
            config.jingluan_sp_mopemope_height,
            config.jingluan_sp_mopemope_offset_y,
            config.jingluan_sp_mopemope_fade_in,
            config.jingluan_sp_mopemope_fade_out,
        ),
    }
}

/// 某个时刻的透明度：时间窗外是 0，窗内按淡入 / 淡出线性升降。
fn envelope(time: f64, (start, end): (f64, f64), fade_in: f32, fade_out: f32) -> f32 {
    if time < start || time >= end {
        return 0.;
    }
    let mut alpha: f32 = 1.;
    if fade_in > 0. {
        alpha = alpha.min(((time - start) as f32 / fade_in).clamp(0., 1.));
    }
    if fade_out > 0. {
        alpha = alpha.min(((end - time) as f32 / fade_out).clamp(0., 1.));
    }
    alpha
}

/// 在谱面空间里画当前这一帧该显示的插画。基础空间是 `x ∈ [-1, 1]`、`y ∈ [-1/aspect, 1/aspect]`、
/// y 轴朝上，所以贴图要 `flip_y`，尺寸按「屏幕高度 × 比例」算。
pub fn render(res: &mut Resource) {
    if !res.config.jingluan_sp_mode {
        return;
    }
    let aspect = res.aspect_ratio;
    if !aspect.is_finite() || aspect <= 0. {
        return;
    }
    let screen_height = 2. / aspect;
    let base = res.res_pack.info.line_perfect();
    let time = res.time;
    for (index, window) in SEGMENTS.iter().enumerate() {
        let texture = match index {
            0 => *CHAN_TEX,
            1 => blurred(&GUANG_BLUR, res.config.bg_blurriness, appendant::spas_guang_image).or(*GUANG_TEX),
            _ => blurred(&MOPEMOPE_BLUR, res.config.bg_blurriness, appendant::spas_mopemope_image).or(*MOPEMOPE_TEX),
        };
        let Some(texture) = texture else { continue };
        let (height_ratio, offset_ratio, fade_in, fade_out) = params(&res.config, index);
        let alpha = envelope(time, *window, fade_in, fade_out);
        if alpha <= 0. {
            continue;
        }
        let height = screen_height * height_ratio;
        if !height.is_finite() || height <= 0. {
            continue;
        }
        let width = height * texture.width() / texture.height();
        // 压暗是「痉挛SP」自己的参数，和设置里的背景压暗无关：0 不压暗，1 全黑。
        // 第一段是带透明底的角色立绘，压暗会把它糊成一团，所以只压后两段整幅曲绘。
        let dim = if index == 0 { 1. } else { (1. - res.config.jingluan_sp_dim).clamp(0., 1.) };
        let color = Color {
            r: base.r * dim,
            g: base.g * dim,
            b: base.b * dim,
            a: base.a * alpha,
        };
        // 谱面空间 y 轴朝上，所以「向下偏移」要从中心减掉。
        let offset_y = screen_height * offset_ratio;
        draw_texture_ex(
            texture,
            -width * 0.5,
            -height * 0.5 - offset_y,
            color,
            DrawTextureParams {
                dest_size: Some(vec2(width, height)),
                flip_y: true,
                ..Default::default()
            },
        );
    }
}
