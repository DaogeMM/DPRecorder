//! 「Clock Paradox」演出：画面正中那只大表盘在四个时刻硬切出现、停留一下、再线性淡出。
//!
//! 表盘是一张白线贴图（`appendant.cp` 里的 `clock.png`，罗马数字 I~XII 那圈），整幅不动、
//! 位置与大小固定：1920×1080 画面里外环半径 455 px、圆心 (962, 542)。它垫在判定线与按键
//! 下面、背景上面 —— 所以画在 `Chart::render` 的判定线循环之前。
//!
//! 时刻按参考录像逐帧测出来的对比度曲线换算成 `Resource::time`：录像是从音乐第 3.02 秒
//! 开始录的（与 `music.ogg` 做包络互相关得到），所以录像秒数 + 3.02 才是谱面时间。
//! 每次都是硬切进入、退出是线性渐隐（前三次约 0.44 秒，第四次 2.2 秒）。

use macroquad::prelude::*;
use once_cell::sync::Lazy;

use crate::core::{appendant, Resource};

/// 四次出现的默认 `(亮起, 开始淡出)`，单位秒（`Resource::time`）。
///
/// 这四对时刻可以在配置里改（`clock_paradox_sp_flash{1..4}_on/_fade`）。
const FLASH_DEFAULTS: [(f64, f64); 4] = [
    (108.020, 108.453),
    (109.720, 110.170),
    (111.42, 111.870),
    (142.287, 145.077),
];

/// 前三次从「开始淡出」到「完全消失」的时长：都是一条短的线性渐隐。
const FLASH_FADE: f64 = 0.44;

/// 第四次从「开始淡出」到「完全消失」的时长：停得久、退得慢。
const LONG_FADE: f64 = 2.233;

/// 把四次 `(亮起, 开始淡出)` 补上各自的淡出时长，得到完整的时间表。
///
/// 亮起的时刻与谱面里那条转动的判定线起转对齐（108.000 / 109.714 / 111.429 / 142.286）。
fn appearances(flashes: [(f64, f64); 4]) -> [(f64, f64, f64); 4] {
    let fade = |index: usize, (on, fade_start): (f64, f64)| {
        let length = if index == 3 { LONG_FADE } else { FLASH_FADE };
        (on, fade_start, fade_start + length)
    };
    [
        fade(0, flashes[0]),
        fade(1, flashes[1]),
        fade(2, flashes[2]),
        fade(3, flashes[3]),
    ]
}

/// 提前这么多秒解码贴图，免得恰好卡在第一次出现的帧上。
const PRELOAD: f64 = 3.;

/// 表盘外环在 1920×1080 画面里的直径（像素）：录像实测外环半径 455 px。
const DIAL_DIAMETER_PX: f32 = 910.;

/// 贴图里外环直径占整张贴图宽度的比例（贴图 2956 px 宽，外环半径约 1460 px）。
///
/// 直接按这个比例缩放，表环的像素半径才与录像一致；贴图边缘还有一圈更淡的刻度，不计入。
const TEXTURE_DIAL_RATIO: f32 = 2. * 1460. / 2956.;

/// 表盘圆心在 1920×1080 画面里的位置（像素，左上角为原点）。
const CENTER_PX: (f32, f32) = (962., 542.);

/// 贴图重采样后的边长（像素）。
///
/// 原图 2956×2957、屏幕上只画到 900 出头，直接由显卡缩 3 倍去采样那圈 1~2 px 宽的细线时
/// 会漏采样，画面就一跳一跳地闪（细线粗细逐帧变、刻度像在爬）。先按这个尺寸用 Lanczos 缩一次，
/// 细线被平均成稳定的灰度，之后只剩 1.5 倍左右的缩放。
const TEXTURE_SIZE: u32 = 1400;

/// 白线表盘（整张图都是白色，形状在 alpha 里）。
static CLOCK: Lazy<Option<Texture2D>> = Lazy::new(|| {
    let bytes = appendant::cp_clock_image()?;
    let source = image::load_from_memory(&bytes).ok()?;
    let resized = if source.width().max(source.height()) > TEXTURE_SIZE {
        source.resize(TEXTURE_SIZE, TEXTURE_SIZE, image::imageops::FilterType::Lanczos3)
    } else {
        source
    };
    let rgba = resized.to_rgba8();
    let texture = Texture2D::from_rgba8(rgba.width() as u16, rgba.height() as u16, rgba.as_raw());
    texture.set_filter(FilterMode::Linear);
    Some(texture)
});

/// 资源包里的表盘贴图齐不齐（缺包时整段演出什么都不画）。
pub fn available() -> bool {
    CLOCK.is_some()
}

/// 这一时刻表盘的不透明度：时段内先满，再从 `fade_start` 线性退到 `gone`。
fn alpha_at(time: f64, appearances: &[(f64, f64, f64); 4]) -> f32 {
    for &(on, fade_start, gone) in appearances {
        if time < on {
            return 0.;
        }
        if time < fade_start {
            return 1.;
        }
        if time < gone {
            return (((gone - time) / (gone - fade_start)).clamp(0., 1.)) as f32;
        }
    }
    0.
}

/// 把表盘铺在画面正中。基础空间是 `x ∈ [-1, 1]`、`y ∈ [-1/aspect, 1/aspect]`。
pub fn render(res: &mut Resource) {
    if !res.config.clock_paradox_sp {
        return;
    }
    let aspect = res.aspect_ratio;
    if !aspect.is_finite() || aspect <= 0. {
        return;
    }
    let config = &res.config;
    let table = appearances([
        (config.clock_paradox_sp_flash1_on, config.clock_paradox_sp_flash1_fade),
        (config.clock_paradox_sp_flash2_on, config.clock_paradox_sp_flash2_fade),
        (config.clock_paradox_sp_flash3_on, config.clock_paradox_sp_flash3_fade),
        (config.clock_paradox_sp_flash4_on, config.clock_paradox_sp_flash4_fade),
    ]);
    if res.time >= table[0].0 - PRELOAD {
        Lazy::force(&CLOCK);
    }
    let alpha = alpha_at(res.time, &table);
    if alpha <= 0. {
        return;
    }
    let Some(texture) = *CLOCK else {
        return;
    };
    let scale = res.config.clock_paradox_sp_scale.max(0.);
    if scale <= 0. {
        return;
    }

    // 1 像素在相机坐标里的长度（相机高度是 2/aspect，对应 1080 px）
    let unit = 2. / (aspect * 1080.);
    let size = DIAL_DIAMETER_PX / TEXTURE_DIAL_RATIO * scale * unit;
    let center_x = CENTER_PX.0 / 960. - 1. + res.config.clock_paradox_sp_offset_x * unit;
    let center_y = (0.5 - CENTER_PX.1 / 1080.) * (2. / aspect) + res.config.clock_paradox_sp_offset_y * unit;
    draw_texture_ex(
        texture,
        center_x - size * 0.5,
        center_y - size * 0.5,
        Color::new(1., 1., 1., alpha),
        DrawTextureParams {
            dest_size: Some(vec2(size, size)),
            // 文件里的贴图 v=0 是「行 0」，而这里的相机 y 轴朝上，不翻就是上下颠倒。
            flip_y: true,
            ..Default::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认时间表（前三次用 FLASH_FADE、第四次用 LONG_FADE）。
    fn table() -> [(f64, f64, f64); 4] {
        appearances(FLASH_DEFAULTS)
    }

    #[test]
    fn appearances_follow_the_reference_recording() {
        // 表盘只在四段里出现，四段都不到 5 秒
        assert_eq!(alpha_at(0., &table()), 0.);
        assert_eq!(alpha_at(107.9, &table()), 0.);
        assert_eq!(alpha_at(108.020, &table()), 1.);
        assert_eq!(alpha_at(108.4, &table()), 1.);
        assert!(alpha_at(108.7, &table()) > 0. && alpha_at(108.7, &table()) < 1.);
        assert_eq!(alpha_at(108.99, &table()), 0.);
        assert_eq!(alpha_at(113., &table()), 0.);
        assert_eq!(alpha_at(142.287, &table()), 1.);
        // 第四次停得久：145.0 秒还满、145.077 起开始退，2.233 秒后退干净
        assert_eq!(alpha_at(145., &table()), 1.);
        assert!(alpha_at(146.2, &table()) > 0. && alpha_at(146.2, &table()) < 1.);
        assert_eq!(alpha_at(147.4, &table()), 0.);
    }

    #[test]
    fn fade_is_linear_between_the_last_full_frame_and_gone() {
        let (_, fade_start, gone) = table()[0];
        let middle = (fade_start + gone) / 2.;
        assert!((alpha_at(middle, &table()) - 0.5).abs() < 0.02);
        // 第四次渐隐明显更长
        let (_, start4, gone4) = table()[3];
        assert!(gone4 - start4 > 1.5);
        assert!(table()[..3].iter().all(|(on, start, gone)| gone - start < 0.6 && start - on > 0.4));
    }

    #[test]
    fn flash_times_come_from_the_table() {
        // 换个时间表（全部往后推 2 秒）时，同一个时刻的结果跟着变
        let shifted = FLASH_DEFAULTS.map(|(on, fade)| (on + 2., fade + 2.));
        let table = appearances(shifted);
        assert_eq!(alpha_at(108.020, &table), 0.);
        assert_eq!(alpha_at(110.020, &table), 1.);
        // 淡出时长：前三次 0.44 秒、第四次 2.233 秒
        assert!((table[0].2 - table[0].1 - FLASH_FADE).abs() < 1e-9);
        assert!((table[3].2 - table[3].1 - LONG_FADE).abs() < 1e-9);
    }
}
