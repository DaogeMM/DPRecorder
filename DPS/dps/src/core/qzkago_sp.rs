//! 「QZKAGO」演出：77.07 秒起整屏换成 7 张静图来回硬切，共 7.10 秒。
//!
//! 整段没有任何动画、转场或滤镜 —— 「动」的感觉全部来自**按 60 fps 整数帧换图**：蓝场 →
//! 歌曲卡 → 远景/放大两张来回切（越切越快）→ 一张重故障定格 → 中/重故障两张以 14 帧为周期
//! 频闪 → 末尾一张白屏盖在上面渐入再渐出。所以这里只有一张「帧号 → 图号」表，没有补间；
//! 唯一的连续变化是那张白屏的透明度。
//!
//! 白屏在频闪段的后半段就开始往上压，所以那几下故障闪的最后几次是被白色逐渐吃掉的，白屏在
//! 频闪结束的地方开始退，退完正好是整段末尾，画面还给谱面。
//!
//! 素材是游戏自己的整屏截图，这一段要**盖住背景、判定线、按键与界面**，因此调用点在整帧的
//! 最后（`scene/game.rs`，界面的待判染色之后、拷屏之前），不像「痉挛SP」「もぺもぺSP」那样
//! 垫在判定线下面。
//!
//! 七张图在演出包 `appendant.qzk.zip` 里（见 `core/appendant.rs` 的 `QZK_FILES`）；缺包时
//! `IMAGES` 全是 `None`，这一段什么都不画，谱面照常播放。
//!
//! 起点 77.07 秒取自参考录像（录像时长与音乐时长一致，两者一一对应）；整段帧数 426 帧
//! ÷ 60 = 7.10 秒。

use macroquad::prelude::*;
use once_cell::sync::Lazy;

use crate::core::{appendant, Resource};

/// 提前这么多秒开始解码贴图，免得恰好卡在起点那一帧。
const PRELOAD: f64 = 3.;

/// 六个阶段的分界（从 `START` 起算的帧号，60 fps）。
///
/// A 蓝场 0..50 / B 歌曲卡 50..153 / C 变焦抖动 153..229 / D 重故障定格 229..251 /
/// E 故障频闪 251..397 / F 只剩白屏退场 397..426。
const BLUE_END: f64 = 50.;
const CARD_END: f64 = 153.;
const ZOOM_END: f64 = 229.;
const GLITCH_END: f64 = 251.;
const STROBE_END: f64 = 397.;
/// 整段总帧数（426 帧 = 7.10 秒）。
const TOTAL: f64 = 426.;

/// 白屏（`QZK_6`）的透明度关键帧：0→1 渐入、停 20 帧、1→0 渐出到整段末尾。
///
/// 边界按 60 fps 帧号：5s11f 开始渐入、6s16f 全白、6s36f 开始退、7s6f 退完（= 整段总长）。
/// 渐入是盖在故障闪上面的；`WHITE_OUT_START` 正好是频闪结束的那一帧。
/// 渐入 / 渐出的**时长**可以由配置调，下面的默认值就是 65 帧与 30 帧。
const WHITE_IN_START: f64 = 311.;
const WHITE_OUT_START: f64 = 396.;
const WHITE_IN_DEFAULT: f64 = 65. / 60.;
const WHITE_OUT_DEFAULT: f64 = 30. / 60.;

/// C 段 16 段的帧数：远景/放大两张交替，第 0 段是远景（`QZK_2`），越切越快。
const ZOOM_FRAMES: [f64; 16] = [8., 8., 2., 6., 4., 6., 4., 6., 4., 6., 2., 2., 2., 8., 2., 6.];

/// E 段开头那一段中故障（`QZK_5`）的帧数，之后进入 14 帧循环。
const STROBE_LEAD: f64 = 2.;

/// E 段一个 14 帧循环：`(图号, 帧数)`，合起来是「重 4 → 中 2 → 重 2 → 中 2 → 重 2 → 中 2」。
const STROBE_CYCLE: [(usize, f64); 6] = [(4, 4.), (5, 2.), (4, 2.), (5, 2.), (4, 2.), (5, 2.)];

/// 七张图，第一次要画之前才解码（那时 GL 上下文已经就绪）。
///
/// 下标与 `appendant::QZK_FILES` 一致：0 蓝场、1 歌曲卡、2 远景、3 放大、4 重故障、5 中故障、
/// 6 白屏。
static IMAGES: Lazy<[Option<Texture2D>; 7]> = Lazy::new(|| {
    std::array::from_fn(|index| {
        let bytes = appendant::qzk_image(index)?;
        Some(Texture2D::from_file_with_format(&bytes, Some(ImageFormat::Png)))
    })
});

/// 从起点算起的帧号落在哪张图上（不透明）。F 段之后没有图，返回 `None`。
fn image_at(frame: f64) -> Option<usize> {
    if !(0. ..STROBE_END).contains(&frame) {
        return None;
    }
    if frame < BLUE_END {
        return Some(0);
    }
    if frame < CARD_END {
        return Some(1);
    }
    if frame < ZOOM_END {
        let mut at = CARD_END;
        for (index, len) in ZOOM_FRAMES.iter().enumerate() {
            at += *len;
            if frame < at {
                return Some(if index % 2 == 0 { 2 } else { 3 });
            }
        }
        return Some(3);
    }
    if frame < GLITCH_END {
        return Some(4);
    }
    let mut at = GLITCH_END + STROBE_LEAD;
    if frame < at {
        return Some(5);
    }
    let cycle: f64 = STROBE_CYCLE.iter().map(|(_, len)| len).sum();
    let mut rest = (frame - at) % cycle;
    for (image, len) in STROBE_CYCLE {
        if rest < len {
            return Some(image);
        }
        rest -= len;
    }
    Some(STROBE_CYCLE[0].0)
}

/// 白屏在这一帧的不透明度：0 → 1 → 0 的分段线性。
///
/// `fade_in` / `fade_out` 是从配置来的**秒数**：渐入从 `WHITE_IN_START` 起算，渐出从
/// `WHITE_OUT_START` 起算，两者都是 0 秒时就是硬切。
fn white_alpha(frame: f64, fade_in: f64, fade_out: f64) -> f32 {
    let ramp = |from: f64, to: f64, low: f64, high: f64| {
        (((frame - from) / (to - from).max(1e-6)).clamp(0., 1.) * (high - low) + low) as f32
    };
    let in_end = WHITE_IN_START + (fade_in.max(0.) * 60.);
    let out_end = WHITE_OUT_START + (fade_out.max(0.) * 60.);
    if frame < WHITE_IN_START || frame >= out_end {
        return 0.;
    }
    if frame < in_end {
        return ramp(WHITE_IN_START, in_end, 0., 1.);
    }
    if frame < WHITE_OUT_START {
        return 1.;
    }
    ramp(WHITE_OUT_START, out_end, 1., 0.)
}

/// 铺一张整屏图。基础空间是 `x ∈ [-1, 1]`、`y ∈ [-1/aspect, 1/aspect]`。
pub fn render(res: &mut Resource) {
    if !res.config.qzkago_sp {
        return;
    }
    let aspect = res.aspect_ratio;
    if !aspect.is_finite() || aspect <= 0. {
        return;
    }
    let start = res.config.qzkago_sp_start;
    if res.time >= start - PRELOAD {
        Lazy::force(&IMAGES);
    }
    let frame = (res.time - start) * 60.;
    let white_out = res.config.qzkago_sp_white_out;
    // 总长跟着渐出末尾走：渐出调长了，白屏就多退一会儿。
    let total = WHITE_OUT_START + white_out.max(0.) * 60.;
    if !(0. ..total).contains(&frame) {
        return;
    }

    let height = 1. / aspect;
    let draw = |texture: Texture2D, alpha: f32| {
        draw_texture_ex(
            texture,
            -1.,
            -height,
            Color::new(1., 1., 1., alpha),
            DrawTextureParams {
                dest_size: Some(vec2(2., height * 2.)),
                // 文件里的贴图 v=0 是「行 0」，而这里的相机 y 轴朝上，不翻就是上下颠倒。
                flip_y: true,
                ..Default::default()
            },
        );
    };

    if let Some(index) = image_at(frame) {
        if let Some(texture) = IMAGES[index] {
            draw(texture, 1.);
        }
    }
    // 白屏压在故障闪上面：渐入那一段是「故障闪被白色逐渐吃掉」，渐出退完才把画面还给谱面。
    let alpha = white_alpha(frame, res.config.qzkago_sp_white_in, white_out);
    if alpha > 0. {
        if let Some(texture) = IMAGES[6] {
            draw(texture, alpha);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 落在哪张图；只关心图号时用。
    fn image(frame: f64) -> Option<usize> {
        image_at(frame)
    }

    #[test]
    fn segments_follow_the_reference_table() {
        assert_eq!(image(-1.), None);
        assert_eq!(image(TOTAL), None);
        // A 蓝场 50 帧、B 歌曲卡 103 帧
        assert_eq!(image(0.), Some(0));
        assert_eq!(image(49.9), Some(0));
        assert_eq!(image(50.), Some(1));
        assert_eq!(image(152.9), Some(1));
        // C 变焦抖动：8 帧远景 → 8 帧放大 → 2 帧远景 …
        assert_eq!(image(153.), Some(2));
        assert_eq!(image(161.), Some(3));
        assert_eq!(image(169.), Some(2));
        // D 重故障定格 22 帧
        assert_eq!(image(229.), Some(4));
        assert_eq!(image(250.9), Some(4));
        // E 频闪：开头 2 帧中故障，之后 14 帧一循环（重 4 → 中 2 → 重 2 → 中 2 → 重 2 → 中 2）
        assert_eq!(image(251.), Some(5));
        assert_eq!(image(253.), Some(4));
        assert_eq!(image(257.), Some(5));
        assert_eq!(image(259.), Some(4));
        assert_eq!(image(267.), Some(4));
        // F 段：没有图了，只有白屏在退
        assert_eq!(image(397.5), None);
        assert_eq!(image(425.9), None);
    }

    #[test]
    fn white_screen_fades_in_then_out() {
        let fade = |frame: f64| white_alpha(frame, WHITE_IN_DEFAULT, WHITE_OUT_DEFAULT);
        // 频闪段的后半段开始渐入：从全透明到全白 65 帧
        assert_eq!(fade(310.9), 0.);
        assert!(fade(311.) < 0.01);
        assert!((fade(343.5) - 0.5).abs() < 0.02);
        assert_eq!(fade(376.), 1.);
        // 停到频闪结束，再从 1 退到 0
        assert_eq!(fade(395.9), 1.);
        assert!((fade(411.) - 0.5).abs() < 0.02);
        assert!(fade(425.9) < 0.01);
        assert_eq!(fade(426.), 0.);
        // 时长可调：渐入调成 0.5 秒时 341 帧就已经全白
        assert_eq!(white_alpha(341., 0.5, WHITE_OUT_DEFAULT), 1.);
        assert!(white_alpha(330., 0.5, WHITE_OUT_DEFAULT) < 0.7);
    }

    #[test]
    fn the_whole_segment_lasts_seven_seconds() {
        // 426 帧 @ 60 fps = 7.10 秒
        assert!((TOTAL / 60. - 7.1).abs() < 1e-9);
        // C 段的 16 个时长刚好填满 76 帧
        assert_eq!(ZOOM_FRAMES.iter().sum::<f64>(), ZOOM_END - CARD_END);
        let cycle: f64 = STROBE_CYCLE.iter().map(|(_, len)| len).sum();
        assert_eq!(cycle, 14.);
        // E 段 146 帧 = 开头 2 帧 + 10 个整循环 + 末尾一小段重故障
        let rest = STROBE_END - GLITCH_END - STROBE_LEAD;
        assert_eq!(rest % cycle, 4.);
        assert_eq!(image(STROBE_END - 0.1), Some(4));
    }
}
