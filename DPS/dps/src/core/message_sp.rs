//! 「Message SP」演出：七趟整屏后处理 + 一层方块粒子。
//!
//! 这条链路照着 APK 里那张谱面的 `MessageLevelEffects` 复刻，顺序与参考素材一致：
//! 万花筒 → 暗角 → 故障 → 光带 → 镜头畸变 → 曝光 → 泛光。所有随时间变化的量都来自
//! 一条关键帧曲线（见下面 `VIGNETTE_RADIUS` 等常量），曲线之外取端点值。
//!
//! 与 `core::effect` 的区别：那边是通用 shader 预设 + 按时间区间挑一条来跑，这里是写死的
//! 七趟、固定的参数名，只在 `message_sp_mode` 打开时整体跑一遍。每趟都是「拿上一趟的
//! 画面当 `screenTexture` 再画一次全屏四边形」，和 `Effect::render` 一样靠 `swap()` 乒乓。
//!
//! ⚠️ 拖影（`preserve_framebuffer`）下没有 `chart_target`，整段演出直接跳过 —— 乒乓缓冲
//! 被整帧残影占着，在它上面 swap 会把本帧已经画好的内容换成上一帧。
//!
//! 泛光在原作里是「亮部提取 + 多级 DualKawaseBlur + 合成」三步，这里压成一趟：直接在
//! 全屏 shader 里按 3 圈 × 8 方向采亮部再按原作公式合成。观感接近，少两块离屏缓冲。

use macroquad::prelude::*;
use std::cell::RefCell;

use crate::{
    config::Config,
    core::{appendant, Resource},
    ext::get_viewport,
};



// ---------------------------------------------------------------------------
// 时间轴
// ---------------------------------------------------------------------------

/// 关键帧的缓动：参考数据里的 0 是线性、3 是 `InOutQuad`、15 是 `AnimationCurve`。
/// 后两种都按 smoothstep 近似 —— 肉眼看不出一条 60 帧的曲线里这点差别。
#[derive(Clone, Copy, PartialEq)]
enum Ease {
    Linear,
    Smooth,
}

#[derive(Clone, Copy)]
struct Key {
    t: f32,
    v: f32,
    ease: Ease,
}

const fn linear(t: f32, v: f32) -> Key {
    Key { t, v, ease: Ease::Linear }
}

// 缓动挂在「左端关键帧」上，也就是这一段从它出发时怎么走（Unity 的 out-tangent）。
const fn smooth(t: f32, v: f32) -> Key {
    Key { t, v, ease: Ease::Smooth }
}

/// 在关键帧序列上取 `t` 时刻的值；序列之前取首帧、之后取末帧。
fn sample(keys: &[Key], t: f32) -> f32 {
    let Some(first) = keys.first().copied() else { return 0. };
    if t <= first.t {
        return first.v;
    }
    let Some(last) = keys.last().copied() else { return 0. };
    if t >= last.t {
        return last.v;
    }
    for pair in keys.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if t < b.t {
            let span = b.t - a.t;
            if span <= 0. {
                return b.v;
            }
            let k = (t - a.t) / span;
            let k = match a.ease {
                Ease::Linear => k,
                Ease::Smooth => k * k * (3. - 2. * k),
            };
            return a.v + (b.v - a.v) * k;
        }
    }
    last.v
}

const VIGNETTE_RADIUS: &[Key] = &[linear(88.366, 0.3), linear(105., 0.), linear(108.5, -1.), linear(110.5, -1.), linear(110.55, 1.)];
const VIGNETTE_DARKNESS: &[Key] = &[linear(88.36, 1.), linear(110.5, 1.), linear(110.55, 0.)];
const VIGNETTE_NOISE: &[Key] = &[
    linear(0.91, 0.),
    linear(1.37, 0.3),
    linear(75.26, 0.3),
    linear(75.269, 0.5),
    linear(75.479, 0.5),
    linear(75.48, 0.3),
    linear(83.99, 0.3),
    linear(84., 0.8),
    linear(86.178, 0.3),
    linear(108.95, 0.3),
    linear(109., 0.),
];
const VIGNETTE_LINE: &[Key] = &[linear(0.91, 0.), linear(1.37, 0.03), linear(108.95, 0.03), linear(109., 0.)];
const LENS_CONVERGENCE: &[Key] = &[smooth(1.37, -8.), linear(88.36, -8.), linear(106.47, -16.), linear(108.95, -16.), linear(109., 0.)];
const LENS_EDGE: &[Key] = &[smooth(1.37, 0.05), linear(108.95, 0.05), linear(109., 0.)];
const GLITCH_RANGE: &[Key] = &[
    linear(75.25, 0.),
    linear(75.26, 0.02),
    linear(75.475, 0.05),
    linear(75.479, 0.),
    linear(83.99, 0.),
    linear(84., 0.005),
    linear(86.17, 0.),
];
const BAND_WIDTH: &[Key] = &[linear(114.545, 0.), smooth(130.363, 0.28), linear(133.1, 1.)];
const BAND_SMOOTHNESS: &[Key] = &[smooth(115.6, 0.), linear(133.1, 0.1)];
const KALEIDO_MIX: &[Key] = &[linear(0., 0.), linear(133., 0.), linear(134., 0.45), linear(153.15, 0.45), linear(159.5, 0.)];
const EXPOSURE: &[Key] = &[linear(0., 1.)];
const CONTRAST: &[Key] = &[linear(0., 1.)];
const SATURATION: &[Key] = &[
    linear(75.26, 0.75),
    linear(75.47, 0.5),
    linear(75.48, 0.75),
    linear(83.99, 0.75),
    linear(84., 0.5),
    linear(86.16, 0.5),
    linear(86.17, 0.75),
    linear(88.36, 0.75),
    linear(108.95, 0.),
    linear(109., 1.),
];
const BLOOM: &[Key] = &[linear(108.95, 0.), linear(114.538, 1.)];

// 参考数据里固定不变的那几个值。
const VIGNETTE_SMOOTHNESS: f32 = 0.5;
const BAND_BRIGHTNESS: f32 = 1.;
const LENS_SIZE: f32 = 125.;

/// 一帧里用到的全部参数（时间轴模式按曲线求值，固定模式直接用配置值）。
struct Frame {
    time: f32,
    vignette_smoothness: f32,
    vignette_radius: f32,
    vignette_darkness: f32,
    vignette_noise: f32,
    vignette_line: f32,
    lens_size: f32,
    lens_convergence: f32,
    lens_edge: f32,
    lens_aspect: f32,
    glitch_range: f32,
    band_width: f32,
    band_smoothness: f32,
    band_brightness: f32,
    kaleido_mix: f32,
    kaleido_contrast: f32,
    kaleido_saturation: f32,
    kaleido_hue: f32,
    kaleido_center: f32,
    kaleido_smoothness: f32,
    exposure: f32,
    saturation: f32,
    contrast: f32,
    bloom: f32,
    bloom_intensity: f32,
    bloom_threshold: f32,
    bloom_radius: f32,
}

fn resolve(config: &Config, time: f64) -> Frame {
    let t = time as f32;
    if config.message_timeline {
        Frame {
            time: t,
            vignette_smoothness: VIGNETTE_SMOOTHNESS,
            vignette_radius: sample(VIGNETTE_RADIUS, t),
            vignette_darkness: sample(VIGNETTE_DARKNESS, t) * config.message_vignette_strength,
            vignette_noise: sample(VIGNETTE_NOISE, t),
            vignette_line: sample(VIGNETTE_LINE, t),
            lens_size: LENS_SIZE,
            lens_convergence: sample(LENS_CONVERGENCE, t),
            lens_edge: sample(LENS_EDGE, t),
            lens_aspect: config.message_lens_aspect,
            glitch_range: sample(GLITCH_RANGE, t) * config.message_glitch_strength,
            band_width: sample(BAND_WIDTH, t),
            band_smoothness: sample(BAND_SMOOTHNESS, t),
            band_brightness: BAND_BRIGHTNESS,
            kaleido_mix: sample(KALEIDO_MIX, t) * config.message_kaleido_strength,
            kaleido_contrast: config.message_kaleido_contrast,
            kaleido_saturation: config.message_kaleido_saturation,
            kaleido_hue: config.message_kaleido_hue,
            kaleido_center: config.message_kaleido_center,
            kaleido_smoothness: config.message_kaleido_smoothness,
            exposure: sample(EXPOSURE, t) * config.message_exposure,
            // 饱和 / 对比的配置值就是「关掉时间轴时用的固定值」（默认等于曲线的起点值），
            // 时间轴开着时不能再乘一遍，否则 0.75 会变成 0.5625。
            saturation: sample(SATURATION, t),
            contrast: sample(CONTRAST, t),
            bloom: sample(BLOOM, t) * config.message_bloom_strength,
            bloom_intensity: config.message_bloom_intensity,
            bloom_threshold: config.message_bloom_threshold,
            bloom_radius: config.message_bloom_radius,
        }
    } else {
        Frame {
            time: t,
            vignette_smoothness: config.message_vignette_smoothness,
            vignette_radius: config.message_vignette_radius,
            vignette_darkness: config.message_vignette_darkness,
            vignette_noise: config.message_vignette_noise,
            vignette_line: config.message_vignette_line,
            lens_size: config.message_lens_size,
            lens_convergence: config.message_lens_convergence,
            lens_edge: config.message_lens_edge,
            lens_aspect: config.message_lens_aspect,
            glitch_range: config.message_glitch_range,
            band_width: config.message_band_width,
            band_smoothness: config.message_band_smoothness,
            band_brightness: config.message_band_brightness,
            kaleido_mix: config.message_kaleido_mix,
            kaleido_contrast: config.message_kaleido_contrast,
            kaleido_saturation: config.message_kaleido_saturation,
            kaleido_hue: config.message_kaleido_hue,
            kaleido_center: config.message_kaleido_center,
            kaleido_smoothness: config.message_kaleido_smoothness,
            exposure: config.message_exposure,
            saturation: config.message_saturation,
            contrast: config.message_contrast,
            bloom: config.message_bloom_brightness,
            bloom_intensity: config.message_bloom_intensity,
            bloom_threshold: config.message_bloom_threshold,
            bloom_radius: config.message_bloom_radius,
        }
    }
}

// ---------------------------------------------------------------------------
// 材质与贴图
// ---------------------------------------------------------------------------

struct Materials {
    kaleido: Option<Material>,
    vignette: Option<Material>,
    glitch: Option<Material>,
    band: Option<Material>,
    lens: Option<Material>,
    esc: Option<Material>,
    bloom: Option<Material>,
}

struct Textures {
    kaleido: Option<Texture2D>,
    kaleido_normal: Option<Texture2D>,
    glitch: Texture2D,
    cube: Option<Texture2D>,
}

struct State {
    materials: Materials,
    textures: Textures,
}

thread_local! {
    static STATE: RefCell<Option<State>> = RefCell::new(None);
}

fn compile(shader: &str, uniforms: &[(&str, UniformType)], textures: &[&str]) -> Option<Material> {
    let params = MaterialParams {
        uniforms: uniforms.iter().map(|(name, kind)| ((*name).to_owned(), *kind)).collect(),
        textures: textures.iter().map(|name| (*name).to_owned()).collect(),
        ..Default::default()
    };
    match load_material(VERTEX_SHADER, shader, params) {
        Ok(material) => Some(material),
        Err(error) => {
            eprintln!("「Message SP」演出 shader 编译失败：{error}");
            None
        }
    }
}

impl Materials {
    fn load() -> Self {
        use UniformType::Float1;
        let screen = [("screenSize", UniformType::Float2), ("UVScale", UniformType::Float2)];
        let all = |extra: &[(&'static str, UniformType)]| -> Vec<(&'static str, UniformType)> {
            let mut list = screen.to_vec();
            list.extend_from_slice(extra);
            list
        };
        Self {
            kaleido: compile(
                KALEIDO_FRAG,
                &all(&[
                    ("time", Float1),
                    ("mixAmount", Float1),
                    ("contrast", Float1),
                    ("saturation", Float1),
                    ("hue", Float1),
                    ("centerRadius", Float1),
                    ("centerSmoothness", Float1),
                ]),
                &["screenTexture", "Kaleido", "KaleidoNormal"],
            ),
            vignette: compile(
                VIGNETTE_FRAG,
                &all(&[
                    ("time", Float1),
                    ("smoothness", Float1),
                    ("radius", Float1),
                    ("darkness", Float1),
                    ("noiseBrightness", Float1),
                    ("lineOpacity", Float1),
                ]),
                &["screenTexture"],
            ),
            glitch: compile(
                GLITCH_FRAG,
                &all(&[("time", Float1), ("glitchRange", Float1)]),
                &["screenTexture", "GlitchTex"],
            ),
            band: compile(
                BAND_FRAG,
                &all(&[("bandDir", Float1), ("bandWidth", Float1), ("bandSmoothness", Float1), ("bandBrightness", Float1)]),
                &["screenTexture"],
            ),
            lens: compile(
                LENS_FRAG,
                &all(&[
                    ("lensSize", Float1),
                    ("convergence", Float1),
                    ("edgeSmoothness", Float1),
                    ("lensAspect", Float1),
                ]),
                &["screenTexture"],
            ),
            esc: compile(
                ESC_FRAG,
                &all(&[("brightness", Float1), ("saturation", Float1), ("contrast", Float1)]),
                &["screenTexture"],
            ),
            bloom: compile(
                BLOOM_FRAG,
                &all(&[
                    ("bloom", Float1),
                    ("intensity", Float1),
                    ("threshold", Float1),
                    ("radius", Float1),
                ]),
                &["screenTexture"],
            ),
        }
    }
}

/// 从附带资源包里读一张图；缺包或缺文件时返回 `None`。
fn texture(name: &str) -> Option<Texture2D> {
    let bytes = appendant::message_image(name)?;
    let image = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let texture = Texture2D::from_rgba8(image.width() as u16, image.height() as u16, image.as_raw());
    texture.set_filter(FilterMode::Linear);
    Some(texture)
}

/// 方块贴图：灰度图里的形状要当透明度用，所以把灰度复制进 alpha 通道，
/// 之后就能用普通的 `draw_texture_ex` 按 alpha 混合画（省得再写一个只取 `.r` 的材质）。
fn cube_texture(name: &str) -> Option<Texture2D> {
    let bytes = appendant::message_image(name)?;
    let mut image = image::load_from_memory(&bytes).ok()?.to_rgba8();
    for pixel in image.pixels_mut() {
        pixel.0[3] = pixel.0[0];
    }
    let texture = Texture2D::from_rgba8(image.width() as u16, image.height() as u16, image.as_raw());
    texture.set_filter(FilterMode::Linear);
    Some(texture)
}

/// 缺包时的故障贴图：一张 64×64 的哈希噪点。故障那一趟每次只把画面挪几个像素，
/// 拿噪点顶着也能看出「抽帧」的感觉，不至于因为缺一张图整段演出都不动。
fn fallback_noise() -> Texture2D {
    let size = 64u16;
    let mut bytes = vec![0u8; (size as usize) * (size as usize) * 4];
    for i in 0..(size as usize * size as usize) {
        let value = hash_u32(i as u32) * 255.;
        let offset = i * 4;
        bytes[offset] = value as u8;
        bytes[offset + 1] = (value * 0.7) as u8;
        bytes[offset + 2] = (value * 0.3) as u8;
        bytes[offset + 3] = 255;
    }
    let texture = Texture2D::from_rgba8(size, size, &bytes);
    texture.set_filter(FilterMode::Nearest);
    texture
}

/// 32 位整数哈希，值域 0~1（粒子的随机分布与噪声贴图都用它，纯函数、便于跳时间轴后重建）。
fn hash_u32(mut x: u32) -> f32 {
    x = x.wrapping_mul(747796405).wrapping_add(2891336453);
    x = ((x >> ((x >> 28) + 4)) ^ x).wrapping_mul(277803737);
    x = (x >> 22) ^ x;
    x as f32 / u32::MAX as f32
}

impl State {
    fn new() -> Self {
        Self {
            materials: Materials::load(),
            textures: Textures {
                kaleido: texture("KaleidoCol.png"),
                kaleido_normal: texture("KaleidoNormal.png"),
                glitch: texture("Glitch4in1.png").unwrap_or_else(fallback_noise),
                cube: cube_texture("Cube_ch0.png"),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// 渲染
// ---------------------------------------------------------------------------

/// 背景要不要藏起来（参考里的 `rawImageHideTime`：109 秒之后连背景图一起黑掉）。
pub fn background_hidden(res: &Resource) -> bool {
    let config = &res.config;
    config.message_sp_mode
        && config.message_bg_hide_time > 0.
        && res.time - config.message_time_offset >= config.message_bg_hide_time
}

/// 把上一趟的画面当输入再画一遍，结果留在 `chart_target.output()`。
fn pass(material: &Material, res: &mut Resource, top: f32) {
    let Some(target) = res.chart_target.as_mut() else { return };
    let mut gl = unsafe { get_internal_gl() };
    gl.flush();
    target.swap();
    let tex = target.old().texture;
    material.set_texture("screenTexture", tex);
    let dim = vec2(tex.width(), tex.height());
    material.set_uniform("screenSize", dim);
    gl.quad_gl.render_pass(Some(target.output().render_pass));
    let vp = get_viewport();
    material.set_uniform("UVScale", vec2(vp.2 as f32, vp.3 as f32) / dim);
    gl_use_material(material.clone());
    draw_rectangle(-1., -top, 2., top * 2., WHITE);
    gl_use_default_material();
}

/// 跑一遍演出；由 `Game::render` 在谱面自带的 effects 之后、UI 之前调用。
pub fn render(res: &mut Resource) {
    if !res.config.message_sp_mode {
        return;
    }
    let aspect = res.aspect_ratio;
    if !aspect.is_finite() || aspect <= 0. {
        return;
    }
    // 拖影下没有 `chart_target`：这套演出靠乒乓缓冲，跳过。
    if res.chart_target.is_none() {
        return;
    }
    let time = res.time - res.config.message_time_offset;
    let frame = resolve(&res.config, time);
    let top = 1. / aspect;

    // 粒子最早：它们属于「画面内容」，后面的反相 / 白场也要作用在它们身上。
    if res.config.message_pass_particles && frame.time > 0. {
        draw_particles(res, top);
    }

    STATE.with(|cell| {
        let mut guard = cell.borrow_mut();
        let state = guard.get_or_insert_with(State::new);

        if res.config.message_pass_kaleido && frame.kaleido_mix > 0.001 {
            if let (Some(material), Some(color), Some(normal)) =
                (state.materials.kaleido.as_ref(), state.textures.kaleido, state.textures.kaleido_normal)
            {
                material.set_texture("Kaleido", color);
                material.set_texture("KaleidoNormal", normal);
                material.set_uniform("time", frame.time);
                material.set_uniform("mixAmount", frame.kaleido_mix.max(0.).min(1.));
                material.set_uniform("contrast", frame.kaleido_contrast);
                material.set_uniform("saturation", frame.kaleido_saturation);
                material.set_uniform("hue", frame.kaleido_hue);
                material.set_uniform("centerRadius", frame.kaleido_center);
                material.set_uniform("centerSmoothness", frame.kaleido_smoothness);
                pass(material, res, top);
            }
        }

        if res.config.message_pass_vignette {
            if let Some(material) = state.materials.vignette.as_ref() {
                material.set_uniform("time", frame.time);
                material.set_uniform("smoothness", frame.vignette_smoothness.max(0.001));
                material.set_uniform("radius", frame.vignette_radius);
                material.set_uniform("darkness", frame.vignette_darkness);
                material.set_uniform("noiseBrightness", frame.vignette_noise.max(0.));
                material.set_uniform("lineOpacity", frame.vignette_line.max(0.));
                pass(material, res, top);
            }
        }

        if res.config.message_pass_glitch && frame.glitch_range.abs() > 0.0001 {
            if let Some(material) = state.materials.glitch.as_ref() {
                material.set_texture("GlitchTex", state.textures.glitch);
                material.set_uniform("time", frame.time);
                material.set_uniform("glitchRange", frame.glitch_range);
                pass(material, res, top);
            }
        }

        // 带宽为 0 时这一趟会变成屏幕正中的一条反相细线（公式里 `abs(d) - width*0.5` 决定的），
        // 所以只在真的有带宽时才跑：114.545 秒之前那一段没有光带。
        if res.config.message_pass_band && frame.band_width > 0.0001 {
            if let Some(material) = state.materials.band.as_ref() {
                material.set_uniform("bandDir", if res.config.message_band_vertical { 1. } else { 0. });
                material.set_uniform("bandWidth", frame.band_width.max(0.));
                material.set_uniform("bandSmoothness", frame.band_smoothness.max(0.0001));
                material.set_uniform("bandBrightness", frame.band_brightness);
                pass(material, res, top);
            }
        }

        if res.config.message_pass_lens && frame.lens_convergence.abs() > 0.0001 {
            if let Some(material) = state.materials.lens.as_ref() {
                material.set_uniform("lensSize", frame.lens_size.max(1.));
                material.set_uniform("convergence", frame.lens_convergence);
                material.set_uniform("edgeSmoothness", frame.lens_edge.max(0.0001));
                material.set_uniform("lensAspect", frame.lens_aspect.max(0.01));
                pass(material, res, top);
            }
        }

        if res.config.message_pass_esc {
            if let Some(material) = state.materials.esc.as_ref() {
                material.set_uniform("brightness", frame.exposure);
                material.set_uniform("saturation", frame.saturation);
                material.set_uniform("contrast", frame.contrast);
                pass(material, res, top);
            }
        }

        if res.config.message_pass_bloom && frame.bloom > 0.0001 {
            if let Some(material) = state.materials.bloom.as_ref() {
                material.set_uniform("bloom", frame.bloom);
                material.set_uniform("intensity", frame.bloom_intensity);
                material.set_uniform("threshold", frame.bloom_threshold);
                material.set_uniform("radius", frame.bloom_radius);
                pass(material, res, top);
            }
        }
    });
}

/// 方块粒子的分布是「时间 → 位置」的纯函数：跳到任意时刻都能立刻画出那一刻该有的样子，
/// 不用一帧帧攒（调试窗口来回拖时间码时尤其重要）。
fn draw_particles(res: &Resource, top: f32) {
    STATE.with(|cell| {
        let guard = cell.borrow();
        let Some(state) = guard.as_ref() else { return };
        let Some(texture) = state.textures.cube else { return };
        let config = &res.config;
        let time = frame_time(res);
        let start = config.message_particle_start as f32;
        if time < start {
            return;
        }
        let life = config.message_particle_lifetime.max(0.1) as f32;
        let count = config.message_particle_count.min(400);
        if count == 0 {
            return;
        }
        let step = life / count as f32;
        let size = config.message_particle_size.max(0.001);
        let speed = config.message_particle_speed;
        let hide_y = config.message_particle_hide_y;
        let hide_smoothness = config.message_particle_hide_smoothness.max(0.001);
        let glow = config.message_particle_glow;

        for layer in 0..2u32 {
            for index in 0..count {
                let birth = start as f32 + index as f32 * step + layer as f32 * step * 0.5;
                let age = time - birth;
                if !(0. ..=life).contains(&age) {
                    continue;
                }
                let seed = index * 2 + layer;
                let x = -1.05 + 2.1 * hash_u32(seed.wrapping_mul(2654435761).wrapping_add(17));
                let y0 = -top + 2. * top * hash_u32(seed.wrapping_mul(40503).wrapping_add(91));
                // 正的 `speed` 是往上飘：这台相机 y 轴朝下，所以位置里要减去。
                let y = y0 - speed * age;
                let grow = (age / life).sqrt();
                let side = size * (0.45 + 0.55 * grow) * 2.;
                // 参考里用 `HIDE_ABOVE` 在屏幕上方挖掉一条带；这里按粒子的屏幕 y 算同样的淡出。
                let y01 = (-y + top) / (2. * top);
                let hidden = if y01 > hide_y { (y01 - hide_y) / hide_smoothness } else { 0. };
                let mut alpha = (age / (life * 0.15)).min(1.) * (1. - hidden.clamp(0., 1.));
                if alpha <= 0.001 {
                    continue;
                }
                alpha *= config.message_particle_opacity.max(0.);
                if glow {
                    let halo = side * 1.9;
                    draw_texture_ex(
                        texture,
                        x - halo * 0.5,
                        y - halo * 0.5,
                        Color::new(1., 1., 1., alpha * 0.22),
                        DrawTextureParams { dest_size: Some(vec2(halo, halo)), ..Default::default() },
                    );
                }
                draw_texture_ex(
                    texture,
                    x - side * 0.5,
                    y - side * 0.5,
                    Color::new(1., 1., 1., alpha),
                    DrawTextureParams { dest_size: Some(vec2(side, side)), ..Default::default() },
                );
            }
        }
    });
}

fn frame_time(res: &Resource) -> f32 {
    (res.time - res.config.message_time_offset) as f32
}

// ---------------------------------------------------------------------------
// shader
// ---------------------------------------------------------------------------

const VERTEX_SHADER: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;

varying vec2 uv;

uniform mat4 Model;
uniform mat4 Projection;
uniform vec2 UVScale;

void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    uv = (texcoord - vec2(0.5)) * UVScale + vec2(0.5);
}"#;

const KALEIDO_FRAG: &str = r#"#version 100
precision mediump float;

varying vec2 uv;

uniform sampler2D screenTexture;
uniform sampler2D Kaleido;
uniform sampler2D KaleidoNormal;
uniform vec2 screenSize;
uniform vec2 UVScale;
uniform float time;
uniform float mixAmount;
uniform float contrast;
uniform float saturation;
uniform float hue;
uniform float centerRadius;
uniform float centerSmoothness;

vec3 rgb2hsv(vec3 c) {
    vec4 K = vec4(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
    vec4 p = mix(vec4(c.bg, K.wz), vec4(c.gb, K.xy), step(c.b, c.g));
    vec4 q = mix(vec4(p.xyw, c.r), vec4(c.r, p.yzx), step(p.x, c.r));
    float d = q.x - min(q.w, q.y);
    float e = 1.0e-10;
    return vec3(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}

vec3 hsv2rgb(vec3 c) {
    vec4 K = vec4(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    vec3 p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
    return c.z * mix(K.xxx, clamp(p - K.xxx, 0.0, 1.0), c.y);
}

void main() {
    vec2 p = uv.yx - vec2(0.5);
    float r = length(p);
    vec2 n = texture2D(KaleidoNormal, p * 0.5 + vec2(0.5) + vec2(time * 0.02)).xy * 2.0 - 1.0;
    vec2 trans = p + n * 0.06;
    vec3 col = texture2D(Kaleido, trans * 0.9 + vec2(0.5)).rgb;
    col = (col - vec3(0.5)) * contrast + vec3(0.5);
    vec3 hsv = rgb2hsv(clamp(col, 0.0, 1.0));
    hsv.x = fract(hsv.x + hue);
    hsv.y = clamp(hsv.y * saturation, 0.0, 1.0);
    col = hsv2rgb(hsv);
    // 中心蒙板：中心留出原画面，越往外越按 `mixAmount` 换成万花筒。
    float mt = clamp((abs(r) - (centerRadius - centerSmoothness)) / max(centerRadius, 0.0001), 0.0, 1.0);
    float mask = mt * mt * (3.0 - 2.0 * mt);
    vec3 scene = texture2D(screenTexture, uv).rgb;
    gl_FragColor = vec4(mix(scene, col, clamp(mixAmount * mask, 0.0, 1.0)), 1.0);
}"#;

const VIGNETTE_FRAG: &str = r#"#version 100
precision mediump float;

varying vec2 uv;

uniform sampler2D screenTexture;
uniform vec2 screenSize;
uniform vec2 UVScale;
uniform float time;
uniform float smoothness;
uniform float radius;
uniform float darkness;
uniform float noiseBrightness;
uniform float lineOpacity;

float hash12(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

vec3 hash32(vec2 p) {
    return vec3(hash12(p), hash12(p + vec2(17.0, 3.0)), hash12(p + vec2(41.0, 71.0)));
}

void main() {
    vec3 color = texture2D(screenTexture, uv).rgb;
    if (noiseBrightness > 0.001) {
        // 参考里这张噪声是每帧重新生成的一张贴图，值域很小（`_NoiseSaturation` 恒为 0，
        // 也就是灰噪点），叠在画面上只是极淡的颗粒感；所以这里也只抬一点点。
        vec2 cell = floor(uv * max(screenSize, vec2(1.0)) / 3.0);
        float tick = floor(time * 24.0);
        float noise = hash12(cell + vec2(tick * 1.37, tick * 0.71));
        color = max(color, vec3(noise * noiseBrightness * 0.05));
    }
    // 横线：参考的 `sin(uv.y * 850) * 0.5 - 0.25` 再放大 4 倍，只剩很细的一段峰。
    float line = sin(uv.y * 850.0) * 0.5 - 0.25;
    line = max(line * 4.0, 0.0);
    line = line * line * (3.0 - 2.0 * line);
    color += line * lineOpacity;
    // 暗角。`abs()` 是参考里的写法：`darkness > 1` 时反相，`radius < 0` 时整屏都被压黑。
    float r = length(uv - vec2(0.5));
    float t = clamp((r - radius) / max(smoothness, 0.0001), 0.0, 1.0);
    float f = 1.0 - (t * t * (3.0 - 2.0 * t)) * darkness;
    gl_FragColor = vec4(abs(f) * color, 1.0);
}"#;

const GLITCH_FRAG: &str = r#"#version 100
precision mediump float;

varying vec2 uv;

uniform sampler2D screenTexture;
uniform sampler2D GlitchTex;
uniform vec2 screenSize;
uniform vec2 UVScale;
uniform float time;
uniform float glitchRange;

float hash11(float p) {
    p = fract(p * 0.1031);
    p *= p + 33.33;
    p *= p + p;
    return fract(p);
}

vec2 direction(float seed) {
    float angle = hash11(seed) * 6.2831853;
    return vec2(cos(angle), sin(angle));
}

void main() {
    vec3 noise = (texture2D(GlitchTex, uv).rgb - vec3(0.5)) * glitchRange;
    float tick = floor(time * 18.0);
    vec2 offset = direction(tick * 1.7) * noise.x + direction(tick * 3.1 + 11.0) * noise.y;
    vec3 color = texture2D(screenTexture, clamp(uv + offset, vec2(0.0), vec2(1.0))).rgb;
    gl_FragColor = vec4(color, 1.0);
}"#;

const BAND_FRAG: &str = r#"#version 100
precision mediump float;

varying vec2 uv;

uniform sampler2D screenTexture;
uniform vec2 screenSize;
uniform vec2 UVScale;
uniform float bandDir;
uniform float bandWidth;
uniform float bandSmoothness;
uniform float bandBrightness;

const vec3 LUM = vec3(0.2125, 0.7154, 0.0721);

void main() {
    float d = bandDir > 0.5 ? (uv.x - 0.5) : (uv.y - 0.5);
    float t = clamp((abs(d) - bandWidth * 0.5) / max(bandSmoothness, 0.0001), 0.0, 1.0);
    float edge = 1.0 - t * t * (3.0 - 2.0 * t);
    float strength = clamp(edge * bandBrightness, 0.0, 1.0);
    vec3 color = texture2D(screenTexture, uv).rgb;
    vec3 mixed = strength * (vec3(1.0) - 2.0 * color) + color;
    float lum = dot(mixed, LUM);
    // `edge = 1` 时这一趟整体反相（黑变白、白变黑）：133 秒后 `bandWidth` 涨到 1，
    // 于是整屏反相，黑场被翻成纯白，正好接上片尾的白场。
    gl_FragColor = vec4((1.0 - 2.0 * edge) * (mixed - vec3(lum)) + vec3(lum), 1.0);
}"#;

const LENS_FRAG: &str = r#"#version 100
precision mediump float;

varying vec2 uv;

uniform sampler2D screenTexture;
uniform vec2 screenSize;
uniform vec2 UVScale;
uniform float lensSize;
uniform float convergence;
uniform float edgeSmoothness;
uniform float lensAspect;

void main() {
    vec2 center = vec2(0.5);
    vec2 half_ = max(vec2(1.0) - center, center);
    float maxR = length(vec2(half_.y * lensAspect, half_.x));
    vec2 d = uv - center;
    float r = length(vec2(d.x * lensAspect, d.y));
    // 参考的 `_Size` 是「缩放半径」：数值越大畸变越弱，这里换算成一个系数。
    float k = 0.0025 * (125.0 / max(lensSize, 1.0));
    float scale = 1.0 - pow(clamp(r / max(maxR, 0.0001), 0.0, 1.0), 2.0) * convergence * k;
    vec2 warped = d * scale + center;
    float mask = min(min(warped.x, 1.0 - warped.x), min(warped.y, 1.0 - warped.y));
    float at = clamp(mask / max(edgeSmoothness, 0.0001), 0.0, 1.0);
    float alpha = at * at * (3.0 - 2.0 * at);
    gl_FragColor = vec4(alpha * texture2D(screenTexture, clamp(warped, vec2(0.0), vec2(1.0))).rgb, 1.0);
}"#;

const ESC_FRAG: &str = r#"#version 100
precision mediump float;

varying vec2 uv;

uniform sampler2D screenTexture;
uniform vec2 screenSize;
uniform vec2 UVScale;
uniform float brightness;
uniform float saturation;
uniform float contrast;

const vec3 LUM = vec3(0.2125, 0.7154, 0.0721);

void main() {
    vec3 color = texture2D(screenTexture, uv).rgb;
    float lum = dot(color, LUM);
    color = saturation * (color - vec3(lum)) + vec3(lum);
    color = contrast * (color - vec3(0.5)) + vec3(0.5);
    color *= brightness;
    gl_FragColor = vec4(clamp(color, 0.0, 1.0), 1.0);
}"#;

const BLOOM_FRAG: &str = r#"#version 100
precision mediump float;

varying vec2 uv;

uniform sampler2D screenTexture;
uniform vec2 screenSize;
uniform vec2 UVScale;
uniform float bloom;
uniform float intensity;
uniform float threshold;
uniform float radius;

const vec3 LUM = vec3(0.2125, 0.7154, 0.0721);

vec3 bright(vec2 p) {
    vec3 color = texture2D(screenTexture, clamp(p, vec2(0.0), vec2(1.0))).rgb;
    float lum = dot(color, LUM);
    float k = clamp(lum - threshold, 0.0, 1.0) / max(lum, 0.00001);
    return ((color - vec3(lum)) * 0.5 + vec3(lum)) * k;
}

void main() {
    vec3 scene = texture2D(screenTexture, uv).rgb;
    vec2 texel = 1.0 / max(screenSize, vec2(1.0));
    vec3 sum = vec3(0.0);
    float weight = 0.0;
    // 三圈 × 八个方向：近处细、远处糊，代替原作的 DualKawaseBlur 多级模糊。
    for (int ring = 0; ring < 3; ring++) {
        float scale = radius * (1.0 + float(ring) * 2.0);
        float w = 1.0 / (1.0 + float(ring));
        for (int i = 0; i < 8; i++) {
            float angle = float(i) * 0.7853982 + float(ring) * 0.3927;
            vec2 dir = vec2(cos(angle), sin(angle));
            sum += bright(uv + dir * texel * scale * 4.0) * w;
            weight += w;
        }
    }
    vec3 blurred = sum / max(weight, 0.0001);
    float k = clamp(intensity - 1.0, 0.0, 1.0);
    vec3 glow = mix(blurred, blurred * blurred, k) * bloom;
    vec3 mask = vec3(1.0) - glow;
    gl_FragColor = vec4(clamp(vec3(1.0) - mask * (vec3(1.0) - scene), 0.0, 1.0), 1.0);
}"#;
