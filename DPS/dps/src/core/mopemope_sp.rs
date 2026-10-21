//! 「もぺもぺSP」演出：整屏按「乘上加色」压一层红，按谱面时间切换成两种红。
//!
//! 蒙板只压背景，判定线、按键、粒子与界面都在它上面（调用点在 `Chart::render` 里判定线循环之前，
//! 见 `core/chart.rs`），所以它是一层「背景被染红」的效果，不是盖住一切的色片。
//!
//! 混合方式是**乘算**（`dst = src * dst`），不是普通的 alpha 混合：原演出里背景的明暗结构一直被
//! 保留着 —— 亮的地方仍然亮、暗的地方仍然暗，只是整体被染暗成红。用 alpha 混合（`dst = 色 * α +
//! dst * (1-α)`）压出来是一片均匀的色块，看不出底图。
//!
//! 于是配置里的两个颜色就是「乘上去的系数」（`mopemope_sp_color_a` / `mopemope_sp_color_b`）：
//! 通道值越小压得越狠，白色 `#FFFFFF` 等于没有蒙板。
//!
//! 两种红在原演出里分工不同：红 A 是**闪**（每段 3~36 帧，出现在每段红色区间的开头 / 结尾，
//! 以及 62~65.5 秒那一串频闪里），红 B 是**持续**（只有 3 段长镜头，其中 65.5~98.9 秒一段
//! 就占 33 秒）。A 比 B 亮一点。
//!
//! 时间表按谱面时钟（`Resource::time`，也就是音乐播放位置）写死，与「痉挛SP」的 `SEGMENTS`
//! 同一个做法。这些秒数是从参考录像逐帧量出来的（录像时长与音乐时长一致，两者一一对应）。

use macroquad::prelude::*;
use miniquad::{BlendFactor, BlendState, BlendValue, Equation};
use once_cell::sync::Lazy;

use crate::core::Resource;

/// 红 A（亮一点的那一种）的区间：原演出里它是「闪」，每段只有 3~36 帧。
///
/// 区间之间那几帧原色不照搬，由 `mask_at` 的补缝规则并进同一次开关（见那里的说明）。
///
/// 开头那三下（`RED_A` 前三项）是照着「闪红那几帧谱面恰好没有按键」校准的：录像里的按键比本程序
/// 早 2 帧出现，所以这里比录像量到的秒数整体晚 1/30 秒，三下都落在空屏的那几帧里。
const RED_A: [(f64, f64); 15] = [
    (45.600, 45.650),
    (45.750, 45.800),
    (46.067, 46.133),
    (50.517, 50.650),
    (56.367, 56.467),
    (60.283, 60.550),
    (62.067, 62.100),
    (63.017, 63.050),
    (63.317, 63.400),
    (63.717, 63.850),
    (64.167, 64.450),
    (64.883, 65.200),
    (65.367, 65.500),
    (98.883, 98.950),
    (109.467, 110.050),
];

/// 红 B（更暗更纯的那一种）的区间：只有 3 段，但占了 41.5 秒。
const RED_B: [(f64, f64); 3] = [(46.167, 50.500), (56.483, 60.267), (65.517, 98.867)];

/// 蒙板要用的着色器：顶点部分与 macroquad 默认的一致，片元把顶点色乘上纹理
/// （画矩形时绑的是那张 1×1 白纹理，乘出来就是顶点色本身）。
mod shader {
    pub const VERTEX: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform mat4 Model;
uniform mat4 Projection;

void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    color = color0 / 255.0;
    uv = texcoord;
}"#;

    pub const FRAGMENT: &str = r#"#version 100
varying lowp vec4 color;
varying lowp vec2 uv;

uniform sampler2D Texture;

void main() {
    gl_FragColor = color * texture2D(Texture, uv);
}"#;
}

/// 蒙板材质：混合方式换成 `src * dst + dst * 0`，也就是把源色乘到已经画好的背景上。
///
/// 普通 `draw_rectangle` 走的是 alpha 混合，做不出「按背景亮度缩放」，所以这里单独建一条管线。
fn load_mask_material() -> Option<Material> {
    load_material(
        shader::VERTEX,
        shader::FRAGMENT,
        MaterialParams {
            pipeline_params: PipelineParams {
                color_blend: Some(BlendState::new(
                    Equation::Add,
                    BlendFactor::Value(BlendValue::DestinationColor),
                    BlendFactor::Zero,
                )),
                ..Default::default()
            },
            uniforms: Vec::new(),
            textures: Vec::new(),
        },
    )
    .ok()
}

/// 只在第一次真的要画蒙板时才编译着色器（那时 GL 上下文已经就绪）。
static MASK_MATERIAL: Lazy<Option<Material>> = Lazy::new(load_mask_material);

/// `time` 落在哪一段区间里（左闭右开）。
fn in_ranges(time: f64, ranges: &[(f64, f64)]) -> bool {
    ranges.iter().any(|(start, end)| time >= *start && time < *end)
}

/// 两张区间表摊成一张按时间排好的列表：`(开始, 结束, 是否红 B)`。
fn intervals() -> impl Iterator<Item = (f64, f64, bool)> {
    RED_A
        .iter()
        .map(|&(start, end)| (start, end, false))
        .chain(RED_B.iter().map(|&(start, end)| (start, end, true)))
}

/// 两段红色之间只隔这么短的空白时，算同一次开关（补缝规则见 `mask_at`）。
const BRIDGE_GAP: f64 = 0.3;

/// 这些窗口里的空隙是原演出本来的「灭灯」，不补：
/// 开头连闪三下（45.567 / 45.717 / 46.033，每下 4~5 帧、中间夹几帧原色）与 62~65.4 的频闪。
const KEEP_GAPS: [(f64, f64); 2] = [(45.5, 46.167), (62.0, 65.4)];

/// 某一刻该乘上去的颜色；不用画时返回 `None`。
///
/// 原演出在切换强度时偶尔会留一两帧原色（例如长段红 B 结束、红 A 接上之前），逐帧照搬会看成
/// 画面在抖；所以两段红色之间的短暂空隙按前一段的颜色补上。`KEEP_GAPS` 里的空隙是演出本身
/// 让蒙板灭了再亮，保留原样。两张表各自不重叠；万一某帧两边都命中，按长段（红 B）算。
fn mask_at(time: f64, config: &crate::config::Config) -> Option<Color> {
    let color_of = |base: [f32; 3]| {
        Color::new(
            base[0].clamp(0., 1.),
            base[1].clamp(0., 1.),
            base[2].clamp(0., 1.),
            1.,
        )
    };

    if in_ranges(time, &RED_B) {
        return Some(color_of(config.mopemope_sp_color_b));
    }
    if in_ranges(time, &RED_A) {
        return Some(color_of(config.mopemope_sp_color_a));
    }
    if KEEP_GAPS.iter().any(|(start, end)| time >= *start && time <= *end) {
        return None;
    }

    // 找前后最近的两段红色：空隙够短就沿用前一段的颜色。
    let mut prev: Option<(f64, bool)> = None;
    let mut next: Option<f64> = None;
    for (start, end, is_b) in intervals() {
        if end <= time {
            let closer = match prev {
                Some((prev_end, _)) => end > prev_end,
                None => true,
            };
            if closer {
                prev = Some((end, is_b));
            }
        } else if start > time {
            let closer = match next {
                Some(next_start) => start < next_start,
                None => true,
            };
            if closer {
                next = Some(start);
            }
        }
    }
    match (prev, next) {
        (Some((end, is_b)), Some(start)) if start - end < BRIDGE_GAP => {
            let base = if is_b {
                config.mopemope_sp_color_b
            } else {
                config.mopemope_sp_color_a
            };
            Some(color_of(base))
        }
        _ => None,
    }
}

/// 在谱面空间里铺一层乘算蒙板。基础空间是 `x ∈ [-1, 1]`、`y ∈ [-1/aspect, 1/aspect]`，
/// 这里按整屏矩形画（比 `2 × 2/aspect` 再放大一点，浮点误差下不会漏边）。
pub fn render(res: &mut Resource) {
    if !res.config.mopemope_sp_mode {
        return;
    }
    let aspect = res.aspect_ratio;
    if !aspect.is_finite() || aspect <= 0. {
        return;
    }
    let Some(color) = mask_at(res.time, &res.config) else {
        return;
    };
    let Some(material) = *MASK_MATERIAL else {
        return;
    };

    gl_use_material(material);
    let height = 2. / aspect;
    draw_rectangle(-1., -height, 2., height * 2., color);
    // 后面的判定线、按键都走默认材质，这里必须换回去。
    gl_use_default_material();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn segments_match_the_chart() {
        let config = Config::default();
        // 没进入红色区间的时刻（含补缝之后仍然空着的那些）
        for t in [0., 40., 45.55, 50.66, 60.60, 62.50, 63.20, 99.50, 105., 110.06] {
            assert!(mask_at(t, &config).is_none(), "{} 不该有蒙板", t);
        }
        // 开头那三下碎片闪之间的空隙是原效果本身，保留灭灯
        for t in [45.66, 45.82, 46.15] {
            assert!(mask_at(t, &config).is_none(), "{} 该保持灭灯", t);
        }
        // 长段与紧接着的碎片闪之间只隔一两帧时补上，免得出现单帧全亮
        for t in [50.51, 56.475, 60.275, 98.875] {
            assert!(mask_at(t, &config).is_some(), "{} 该补上蒙板", t);
        }
        // 红 B 的三段长镜头
        for t in [46.5, 50.0, 58.0, 60.0, 70.0, 98.0] {
            assert!(mask_at(t, &config).is_some(), "{} 该有蒙板", t);
        }
        // 红 A 的闪：开头三下 + 结尾几次 + 62~65.5 秒的频闪 + 片尾
        for t in [45.62, 45.77, 46.10, 50.55, 56.40, 60.40, 62.08, 63.02, 64.20, 65.45, 98.90, 109.5]
        {
            assert!(mask_at(t, &config).is_some(), "{} 该有蒙板", t);
        }
    }

    #[test]
    fn flashes_use_the_brighter_red() {
        let config = Config::default();
        let flash = mask_at(46.10, &config).unwrap();
        let long = mask_at(47.0, &config).unwrap();
        // 闪（红 A）比长镜头（红 B）亮一点。
        let lum = |c: Color| 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
        assert!(lum(flash) > lum(long));
    }

    #[test]
    fn default_factors_are_dark_red() {
        let config = Config::default();
        let a = config.mopemope_sp_color_a;
        let b = config.mopemope_sp_color_b;
        // 两个系数都是「红留得多、绿其次、蓝最少」，且都明显小于 1（也就是确实压暗了）。
        for c in [a, b] {
            assert!(c[0] < 1. && c[0] > c[1] && c[1] > c[2]);
        }
        // 第一种红（闪）三个通道都压得更轻，所以比第二种亮。
        for i in 0..3 {
            assert!(a[i] > b[i], "通道 {} 上第一种红应该更亮", i);
        }
    }
}
