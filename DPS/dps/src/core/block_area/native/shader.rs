//! 官方噪域渲染：整份照搬 `Phira-Pro-main/prpr/src/core/block_shader.rs`。
//!
//! 一趟全屏着色器（`assets/blockarea/block_shader_full.{vert,frag}`，原版 Unity
//! 反编译产物）+ 一张 CPU 算好的掩码纹理，**不需要任何离屏 FBO**。场景颜色直接从当前
//! 渲染 pass 用 `glCopyTexSubImage2D` 抓一份（必要时先把 MSAA target 解析掉）。
//!
//! 移植改动只有三类：
//! * 素材路径与缺失素材的兜底（`FD_Noise_00000.png` 不在仓库里 → 退化成 `EMPTY_TEX`）；
//! * `Vector` / `Vec2` 的类型名对齐；
//! * hover 材质按 [`super::HOVER_ASSET`] 决定建不建。

use crate::core::block_area::native::mask;
use crate::core::block_area::native::touch;
use crate::core::block_area::native::{NativeBlocks, HOVER_ASSET};
use crate::core::{BlockArea, BlockPhase, Matrix, Resource, Vector};
use macroquad::prelude::*;
use miniquad::{TextureWrap, UniformType};
use once_cell::sync::Lazy;
use std::cell::RefCell;

/// 官方噪域的原生红（`84/255` 是绿蓝）：没有配色的噪域就是这个颜色。
///
/// 片元里 `uColored == 0` 那条路径、`blockPalette()` 的恒等分支、以及 `nativeColor` 的
/// 早退判据用的都是它。所以「颜色恰好等于这个值」与「没有颜色」在画面上是同一件事，
/// 颜色平面不必为这类噪域做任何特化。
pub const DEFAULT_BLOCK_COLOR: [f32; 3] = [1., 84. / 255., 84. / 255.];

/// 解算好的矩形（在它自己的空间里是轴对齐的）。
#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub center: Vector,
    pub half: Vector,
    pub angle: f32,
    pub invert: bool,
    pub active: bool,
    /// Ready 层在 `enableTime` 之前的最后 0.5 秒内选中。
    pub ready: bool,
    /// 这一帧处在 `appearTime..enableTime`（「还没生效」那一段）。
    ///
    /// 「岩浆噪域」用它决定这块该印 netherrock 还是 lava：`Disabled` 与 `DisabledOut`
    /// 两段喂的是**同一个** disabled 掩码通道，只能在 Rust 侧按 zone 拆成两趟画。
    pub before_enable: bool,
    /// 初始 `DisabledBlockShow` 会把精灵掩码在 0.5 秒内渐显出来。
    pub opacity: f32,
    /// 在谱面 `blockAreaList` 里的序号（从 0 起，由 `collect` 填）。只给「简化噪域文字」用。
    pub index: usize,
    /// 「判定范围让位」的覆盖度（0 = 普通方块）。`> 0` 的块是**合成块**：它只往
    /// `Masks::judge_cover` 里打覆盖度、把噪域按 `alpha` 擦掉，本身不参与绘制。
    pub judge_cover: u8,
    /// 这块噪域的调色板颜色（线性 0~1 的 RGB，`None` = 原生红 = 没有颜色）。
    ///
    /// 它与 [`DEFAULT_BLOCK_COLOR`] 相等的效果就是「没有颜色」。有颜色的噪域会额外烘
    /// 一张颜色平面（`Masks::active_colors` / `disabled_colors`），并改用彩色材质：
    /// 片元把原生红材质的饱和度/明度搬到这个色相上（`blockPalette()`）。
    pub color: Option<[f32; 3]>,
}

impl Zone {
    /// 本帧这块噪域要不要走彩色材质。
    ///
    /// 三个条件缺一不可：配了色、颜色不等于原生红（后者可以整条走老路径）、
    /// 以及这块真的会画出像素（`opacity > 0`，初始渐显进度 0 时它还没出现）。
    pub fn is_colored(&self) -> bool {
        self.opacity > 0. && self.color.is_some_and(|color| color != DEFAULT_BLOCK_COLOR)
    }

    /// 设置调色板颜色；`None` 退回原生红。颜色按 0~1 夹取，越界会让片元的色相搬运算出负值。
    pub fn set_color(&mut self, color: Option<[f32; 3]>) {
        self.color = color.map(|c| c.map(|v| v.clamp(0., 1.)));
    }
}

impl Zone {
    pub fn from_area(area: &BlockArea, time: f64, aspect: f32) -> Option<Self> {
        let phase = area.phase(time);
        if phase == BlockPhase::HiddenBefore || phase == BlockPhase::HiddenAfter {
            return None;
        }
        let tr = area.transform(time, aspect);
        let half = Vector::new(tr.size.x.abs() * 0.5, tr.size.y.abs() * 0.5);
        if half.x == 0. || half.y == 0. {
            return None;
        }
        let active = phase == BlockPhase::Active;
        let fades_in = area.phase(area.appear_time) != BlockPhase::Active;
        Some(Self {
            center: tr.center,
            half,
            angle: tr.rotation.to_radians(),
            invert: area.is_subtract,
            active,
            ready: !active && time < area.enable_time && time >= area.enable_time - 0.5,
            before_enable: !active && time < area.enable_time,
            opacity: if !active && fades_in {
                ((time - area.appear_time) / 0.5).clamp(0., 1.) as f32
            } else {
                1.
            },
            index: 0,
            judge_cover: 0,
            color: area.color,
        })
    }
}

fn tex(bytes: &[u8], wrap: TextureWrap) -> Texture2D {
    let source = image::load_from_memory(bytes).unwrap().to_rgba8();
    tex_from_rgba(&source, wrap)
}

/// 把一张已经解码好的 RGBA 图变成点采样纹理（GIF 的逐帧解码也走这里）。
///
/// 导出的 PNG / GIF 行序自顶向下；Unity GLES 采样这些纹理素材时以 (0,0) 为左下。
/// macroquad 原样上传图像字节，所以这里翻一次。
fn tex_from_rgba(source: &image::RgbaImage, wrap: TextureWrap) -> Texture2D {
    let source = image::imageops::flip_vertical(source);
    let texture = Texture2D::from_rgba8(source.width() as u16, source.height() as u16, source.as_raw());
    texture.set_filter(FilterMode::Nearest);
    texture
        .raw_miniquad_texture_handle()
        .set_wrap(unsafe { get_internal_gl() }.quad_context, wrap);
    texture
}

static DISPLACE_TEX: Lazy<Texture2D> = Lazy::new(|| tex(include_bytes!("../../../../../../assets/blockarea/BlockNoise1.png"), TextureWrap::Mirror));
static SPARK_TEX: Lazy<Texture2D> = Lazy::new(|| tex(include_bytes!("../../../../../../assets/blockarea/PointNoise.png"), TextureWrap::Repeat));
static EMPTY_TEX: Lazy<Texture2D> = Lazy::new(|| Texture2D::from_rgba8(4, 4, &[0; 64]));

/// 「岩浆噪域」用的两张 Minecraft 贴图（在附带资源包 `appendant/appendant.lava.zip` 里，不随程序分发）。
///
/// `netherrock.png` 16×16 单帧；`lava.gif` 32×32、16 帧、每帧 160ms —— 这里把 16 帧各解成
/// 一张纹理，按 [`LAVA_FRAME_SECONDS`] 在 Rust 侧选帧（采样器一次只认一张纹理，换帧就是换绑定，
/// 也不需要在绘制中途上传纹理）。两张都按「屏幕高 1/16 一格」平铺、点采样 ⇒ MC 的颗粒感。
///
/// 采样是**屏幕空间**的（`sceneUV`），所以噪域移动/缩放时贴图从它下面滑过，不会跟着变形。
///
/// 缺包时两张都是 `None` / 空表，[`lava_mode`] 会把岩浆模式退回 0（噪域保持官方原样）。
const LAVA_CELLS_PER_SCREEN_HEIGHT: f32 = 16.;
const LAVA_FRAME_SECONDS: f32 = 0.16;
static NETHERROCK_TEX: Lazy<Option<Texture2D>> =
    Lazy::new(|| crate::core::appendant::netherrock_image().map(|bytes| tex(&bytes, TextureWrap::Repeat)));
static LAVA_FRAMES: Lazy<Vec<Texture2D>> = Lazy::new(|| {
    use image::AnimationDecoder;
    let Some(bytes) = crate::core::appendant::lava_image() else {
        return Vec::new();
    };
    let Ok(decoder) = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)) else {
        // 素材坏了也只是「岩浆噪域」没有贴图可贴，不该让整个渲染崩掉。
        return Vec::new();
    };
    match decoder.into_frames().collect_frames() {
        Ok(frames) => frames.iter().map(|frame| tex_from_rgba(frame.buffer(), TextureWrap::Repeat)).collect(),
        Err(_) => Vec::new(),
    }
});

/// 「岩浆噪域」实际能用的模式：素材缺失（或读坏了）就退回 0，让噪域保持官方原样。
fn lava_mode(lava: u8) -> u8 {
    match lava {
        1 if NETHERROCK_TEX.is_some() => 1,
        2 if !LAVA_FRAMES.is_empty() => 2,
        _ => 0,
    }
}

/// 当前该绑哪一帧 lava。时间用 `res.time`（`FRAME` 里的帧时钟），所以全场岩浆同步跳动。
fn lava_frame(time: f32) -> Texture2D {
    let frames = &*LAVA_FRAMES;
    if frames.is_empty() {
        return *EMPTY_TEX;
    }
    let index = (time / LAVA_FRAME_SECONDS).floor() as i64;
    frames[index.rem_euclid(frames.len() as i64) as usize]
}

/// hover 溶解用的噪声底。
///
/// 官方 `FD_Noise_00000.png` 不在 `Phira-Pro-main` 的备份里（`prpr/assets/` 是空的），
/// 仓库里也找不到。原版直接 `include_bytes!` 它，缺文件连编译都过不去，所以
/// `assets/blockarea/FD_Noise_00000.png` 里放了一份**我们自己生成的近似替身**
///（64x64 值噪声，见 `assets/blockarea/README.md`）。拿到官方素材直接覆盖该文件即可。
///
/// 它只被 `block_shader_full.frag:429` 的 `texture2D(uNoiseTex, u_xlat1.xy).xy` 用到一次，
/// 且只在 `uLayer == 3` 的 hover 溶解分支里 —— AUTOPLAY 不参与。
static NOISE_TEX: Lazy<Texture2D> = Lazy::new(|| tex(include_bytes!("../../../../../../assets/blockarea/FD_Noise_00000.png"), TextureWrap::Mirror));

// 这几个 f32 必须保持 f32：本仓库的 macroquad fork 会拒绝把 f64 塞进 Float1。
// 这些 uniform 也不能被折进 GLSL 字面量，否则原生 mediump 运算与 GPU 优化就不再等同。
const FLOATS: &[(&str, f32)] = &[
    ("_EdgeOpacity", 0.8),
    ("_FillStrength", 0.667),
    ("_FillOpacity", 0.667),
    ("_GlowIntensity", 0.8),
    ("_SparkMapOpacity", 5.69),
    ("_SparkHueShiftAmount", 0.2),
    ("_SparkDisplaceIntensity", 2.39),
    ("_DisplaceBlendIntensity", 0.411),
    ("_DisplaceSpeed", 1.5),
    ("_DisplaceStrength", 0.15),
    ("_TouchPosShine", 0.),
    ("_TouchPosRadius", 0.5),
    ("_TouchPosSDFSmoothness", 0.47),
    ("_TouchPosSDFFalloff", 0.41),
    ("_BackgroundPixelScale", 6.),
    // 本仓库新增（`block_shader_full.frag` 里官方原本硬编码 3.0 的那处场景快照降采样）：
    // 默认 3.0 ⇒ 与官方逐位一致；「噪域扭曲」关闭时置 1.0 ⇒ 快照 1:1 采样，后面不再发虚。
    // 官方把 ActiveBlock 的 Ready 分支按 `uReadyExtra` 再叠一层亮度（默认 0 = 官方 0.12）。
    ("uReadyExtra", 0.),
    ("_SnapshotPixelScale", 3.),
    // 本仓库新增（「岩浆噪域」）：`_LavaMode` 0 = 关闭 ⇒ 逐位保持官方原样；
    // `_LavaScale` 是每屏高铺多少格，16 ⇒ 720p 下 45px 一格。
    ("_LavaMode", 0.),
    ("_LavaScale", LAVA_CELLS_PER_SCREEN_HEIGHT),
    ("uDisabledFillOpacity", 0.4),
    ("uDisabledSparkOpacity", 3.5),
    ("uDisabledSparkIntensity", 2.29),
    ("uDisabledSpeed", 0.3),
    ("_ShineSpeed", 37.9),
    ("_ShineBrightness", 0.12),
    ("_TouchDisplaceSpeed", 2.9),
    ("_TouchDisplaceStrength", 0.08),
    ("_NoiseEvoSpeed", 0.03),
    ("_NoiseDirChangeSpeed", 60.),
    ("_NoiseDisplaceStrength", 1.),
    ("_NoiseRadius", 0.48),
    ("_NoiseSmoothness", 1.),
    ("_SDFCellSize", 0.11),
    ("_SDFSmoothness", 0.63),
    ("_SDFFalloff", 0.34),
    ("_SDFMoveSpeed", 9.3),
    ("_TouchBackgroundPixelScale", 8.),
];
const COLORS: &[(&str, [f32; 4])] = &[
    ("_EdgeColor", [1., 0.33018857, 0.33018857, 1.]),
    ("_FillColor", [0.7132075, 0.23549296, 0.23549296, 1.]),
    ("_GlowColor", [1., 0.17924517, 0.17924517, 1.]),
    ("_DisplaceDirection", [1., 1., 0., 0.]),
    ("uDisabledFillColor", [0.497, 0.13766898, 0.13766898, 1.]),
    ("_ShineColor", [1., 1., 1., 1.]),
    ("_TouchDisplaceDirection", [1., 1., 0., 0.]),
    ("_NoiseTint", [1., 0., 0., 1.]),
    ("_TouchGlowColor", [1., 0., 0., 1.]),
];

/// 取 `FLOATS` 里的上游默认值。
///
/// 「简化噪域」会把其中几个 uniform 覆写成纯色模式需要的值，而 uniform 挂在材质上、
/// 跨帧跨 pass 存活，所以**不简化时也必须逐帧把默认值写回去**，否则从简化切回完整
/// 噪域会残留上一次的覆写。用查表而不是再抄一份常数，避免以后上游改数值时两边走散。
fn float_default(name: &str) -> f32 {
    FLOATS.iter().find(|(n, _)| *n == name).map(|(_, v)| *v).unwrap_or(0.)
}

/// 取 `COLORS` 里的上游默认值，见 [`float_default`]。
fn color_default(name: &str) -> [f32; 4] {
    COLORS.iter().find(|(n, _)| *n == name).map(|(_, v)| *v).unwrap_or([0.; 4])
}

/// rgb → hsv 的**色相**（0~360 度）。灰阶（max == min）没有色相，返回 0。
fn hue_of(rgb: [f32; 3]) -> f32 {
    let (r, g, b) = (rgb[0], rgb[1], rgb[2]);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if d.abs() <= f32::EPSILON {
        return 0.;
    }
    let h = if max == r {
        60. * (((g - b) / d) % 6.)
    } else if max == g {
        60. * ((b - r) / d + 2.)
    } else {
        60. * ((r - g) / d + 4.)
    };
    if h < 0. { h + 360. } else { h }
}

/// 把颜色整体旋转 `shift` 度色相，**饱和度与明度原样保留**。
///
/// 上游那套配色（`_FillColor` / `_EdgeColor` / `_GlowColor` / `uDisabledFillColor`）
/// 色相全是 0（纯红），明度与饱和度各不相同，所以只转色相就能得到「同一个噪域、换了个颜色」
/// —— 层次、边缘与辉光的相对强弱都不会变。`shift == 0.` 时逐位返回原值（保证不回归）。
fn rotate_hue(c: [f32; 4], shift: f32) -> [f32; 4] {
    if shift.abs() <= f32::EPSILON {
        return c;
    }
    let (r, g, b) = (c[0], c[1], c[2]);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if d.abs() <= f32::EPSILON {
        return c;
    }
    let mut h = if max == r {
        60. * (((g - b) / d) % 6.)
    } else if max == g {
        60. * ((b - r) / d + 2.)
    } else {
        60. * ((r - g) / d + 4.)
    };
    if h < 0. {
        h += 360.;
    }
    h = (h + shift).rem_euclid(360.);
    let s = d / max;
    let v = max;
    let cc = v * s;
    let x = cc * (1. - ((h / 60.) % 2. - 1.).abs());
    let m = v - cc;
    let (r2, g2, b2) = match (h / 60.) as u32 {
        0 => (cc, x, 0.),
        1 => (x, cc, 0.),
        2 => (0., cc, x),
        3 => (0., x, cc),
        4 => (x, 0., cc),
        _ => (cc, 0., x),
    };
    [r2 + m, g2 + m, b2 + m, c[3]]
}

#[derive(Default)]
struct FrameTextures {
    uploaded_masks: Option<u64>,
    masks: mask::Masks,
    effect: Option<Texture2D>,
    aux: Option<Texture2D>,
    /// 彩色噪域的两张颜色平面（`uActiveColors` / `uDisabledColors`），尺寸与 `effect` 相同。
    ///
    /// 只有本帧至少一块噪域带颜色（`masks.colored`）时才会建出来并上传；其余帧保持
    /// `None`，绘制时绑空纹理 —— 加颜色之前那条路径因此一个字节都不变。
    color_textures: [Option<Texture2D>; 2],
    /// 颜色平面当前对应的掩码 `revision`（与 `uploaded_masks` 同一套判据）。
    uploaded_colors: Option<u64>,
    scene: Option<Texture2D>,
    /// `copy_scene` 的写入端 framebuffer：把 `scene` 那张纹理挂上去，
    /// 这样场景副本走 GPU 端 `glBlitFramebuffer`，不必读回 CPU。
    scene_pass: Option<miniquad::RenderPass>,
    touch: touch::TouchMask,
    clock: Option<f32>,
    /// 本帧是否走「简化噪域」（由 `native::render_all` 写入，见 `set_fill_style`）。
    simplified: bool,
    /// 掩码纹理对应的**视口矩形**（GL 帧缓冲坐标：x, y, w, h，原点在左下）。
    /// `judge_range.glsl` 用它把 `gl_FragCoord` 折算成掩码 uv —— 掩码覆盖的就是这个视口。
    view: [f32; 4],
}

thread_local! {
    static FRAME: RefCell<FrameTextures> = RefCell::new(FrameTextures::default());
}

/// 告诉本帧的材质要不要退化成纯色。`native::render_all` 在画之前调一次。
pub(super) fn set_simplified(value: bool) {
    FRAME.with(|frame| frame.borrow_mut().simplified = value);
}

/// `MATERIAL` 的下标。
///
/// 布局是 `[disabled, active, hover]`，彩色变体是同一套再往后挪 3 张。抽成函数是为了让
/// 「带颜色的噪域会选彩色材质」这件事能单测 —— 材质本身要 GL 上下文才建得起来。
fn material_index(disabled: bool, hover: bool, colored: bool) -> usize {
    3 * usize::from(colored) + if disabled { 0 } else if hover { 2 } else { 1 }
}

/// `colored` 为真时把整条彩色路径编译进去。
///
/// `uColored` 做成**编译期常量**（而不是每帧设的 uniform）与参考实现一致：GLES 驱动能
/// 折掉 `uColored == 0` 那些分支，`blockPalette()` 的恒等路径也就整段消失；同时按参考
/// 实现把 `mediump` 提到 `highp` —— 把原生红的饱和度搬到别的色相时要够精度，否则
/// 深色噪域的调色板会在 mediump 上量化出可见的色阶。
fn load_block_material(disabled: bool, hover: bool, colored: bool) -> Result<Material, miniquad::ShaderError> {
    let mut uniforms = vec![
        ("uView".to_owned(), UniformType::Float3),
        ("uUnityTime".to_owned(), UniformType::Float4),
        ("_ScreenParams".to_owned(), UniformType::Float4),
        ("_EffectRT_TexelSize".to_owned(), UniformType::Float4),
        ("_SparkTint".to_owned(), UniformType::Float3),
        ("uDisabledSparkTint".to_owned(), UniformType::Float3),
        ("_TouchPosCount".to_owned(), UniformType::Int1),
        ("uLayer".to_owned(), UniformType::Int1),
    ];
    if !colored {
        // 彩色噪域：1 时片元按 `fieldUV` 去采 `uActiveColors` / `uDisabledColors`。
        // 彩色变体里它是编译期常量（见下面的特化），没有 uniform 可设。
        uniforms.push(("uColored".to_owned(), UniformType::Int1));
    }
    uniforms.extend(FLOATS.iter().map(|(name, _)| (name.to_string(), UniformType::Float1)));
    uniforms.extend(COLORS.iter().map(|(name, _)| (name.to_string(), UniformType::Float4)));
    for i in 0..10 {
        uniforms.push((format!("_TouchPos[{i}]"), UniformType::Float2));
    }
    let params = MaterialParams {
        pipeline_params: PipelineParams {
            color_blend: Some(miniquad::BlendState::new(
                miniquad::Equation::Add,
                miniquad::BlendFactor::One,
                if disabled {
                    miniquad::BlendFactor::One
                } else {
                    miniquad::BlendFactor::OneMinusValue(miniquad::BlendValue::SourceAlpha)
                },
            )),
            ..Default::default()
        },
        uniforms,
        textures: vec![
            "uDisplaceTex".to_owned(),
            "uSparkTex".to_owned(),
            "uMasks".to_owned(),
            "uScene".to_owned(),
            "uAuxMasks".to_owned(),
            "uNoiseTex".to_owned(),
            "_LavaTex".to_owned(),
            // 彩色噪域的两张颜色图。没有彩色噪域时绑 1x1 空纹理、`uColored` 归零，
            // 片元里的 `uColored == 0` 分支不会去采它们。
            "uActiveColors".to_owned(),
            "uDisabledColors".to_owned(),
        ],
    };
    // 把「只跟 uniform 有关」的分支特化掉。移动端 GPU 上，原生 hover SDF 即使零手指
    // 也会吃掉寄存器和指令。
    let mut fragment = FRAGMENT.replace("uniform int uLayer;", if disabled { "const int uLayer = 0;" } else { "const int uLayer = 3;" });
    if colored {
        fragment = fragment
            .replace("uniform int uColored;", "const int uColored = 1;")
            .replace("mediump", "highp");
    }
    if !hover {
        fragment = fragment
            .replace("uniform \tint _TouchPosCount;", "const int _TouchPosCount = 0;")
            .replace(
                "return texture2D(uAuxMasks, (floor(uv * size) + 0.5) / size).a;",
                "return 0.0;",
            );
    }
    let material = load_material(VERTEX, &fragment, params)?;
    for (name, value) in FLOATS {
        material.set_uniform(name, *value);
    }
    for (name, value) in COLORS {
        material.set_uniform(name, *value);
    }
    material.set_uniform("_SparkTint", vec3(1., 0.28490567, 0.28490567));
    material.set_uniform("uDisabledSparkTint", vec3(0.31132078, 0.077830195, 0.077830195));
    material.set_uniform("_TouchPosCount", 0_i32);
    material.set_uniform("uLayer", if disabled { 0_i32 } else { 3_i32 });
    if !colored {
        material.set_uniform("uColored", 0_i32);
    }
    for i in 0..10 {
        material.set_uniform(&format!("_TouchPos[{i}]"), vec2(0., 0.));
    }
    Ok(material)
}

static MATERIAL: Lazy<Option<[Material; 6]>> = Lazy::new(|| {
    // 六张一起编：`MATERIAL` 本来就是在第一次真正要画噪域时才 Lazy 一次的事（离线录制
    // 卡一帧无所谓），而按需再编彩色那三张会把「哪一帧第一次出现彩色噪域」变成一次额外的
    // 着色器编译停顿，还会让 `MATERIAL` 的类型变成可变的 Option 内层。无颜色的谱面只是
    // 多编了 3 份变体，不选就永远不画。
    //
    // 第 2 张是「噪域打击效果」专用的 hover 材质：同一份片元着色器，但不把 `hoverSample`
    // 特化成 0，于是它能真的去采 aux 纹理的 alpha（零手指时那里本来就是 0，画面零回归）。
    // 只有指到手的帧才会切到它，没开「噪域打击效果」时永远走第 1 张，开销与改动前一致。
    let loaded = (|| {
        Ok([
            load_block_material(true, false, false)?,
            load_block_material(false, false, false)?,
            if HOVER_ASSET { load_block_material(false, true, false)? } else { load_block_material(false, false, false)? },
            load_block_material(true, false, true)?,
            load_block_material(false, false, true)?,
            if HOVER_ASSET { load_block_material(false, true, true)? } else { load_block_material(false, false, true)? },
        ])
    })();
    loaded
        .map_err(|e: miniquad::ShaderError| {
            tracing::warn!("block-area shader failed: {e}");
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::fs::write(exe.with_file_name("block_shader_error.txt"), format!("{e}"));
            }
        })
        .ok()
});

const JUDGE_RANGE_VERT: &str = include_str!("../../shaders/judge_range_vert.glsl");
const JUDGE_RANGE_FRAG: &str = include_str!("../../shaders/judge_range.glsl");

/// 判定范围那份材质：`judge_range.glsl` + macroquad 默认顶点着色器（吃 Model / Projection）。
///
/// 上游 `block_area/gl.rs` 里它是 `build_material_verts(JR_VERT, JR_FRAG, &[("u_screen", F2)],
/// &["u_paint"], alpha_blend())`；这里把 `u_screen` 换成 `u_view`（vec4 = 视口矩形），
/// 因为原生掩码的尺寸与渲染目标并不一致，`(gl_FragCoord - 视口原点) / 视口尺寸` 才是掩码 uv。
static JUDGE_MATERIAL: Lazy<Option<Material>> = Lazy::new(|| {
    load_material(
        JUDGE_RANGE_VERT,
        JUDGE_RANGE_FRAG,
        MaterialParams {
            pipeline_params: PipelineParams {
                color_blend: Some(miniquad::BlendState::new(
                    miniquad::Equation::Add,
                    miniquad::BlendFactor::Value(miniquad::BlendValue::SourceAlpha),
                    miniquad::BlendFactor::OneMinusValue(miniquad::BlendValue::SourceAlpha),
                )),
                ..Default::default()
            },
            uniforms: vec![("u_view".to_owned(), UniformType::Float4)],
            textures: vec!["u_paint".to_owned()],
        },
    )
    .map_err(|e: miniquad::ShaderError| tracing::warn!("judge-range shader failed: {e}"))
    .ok()
});

/// 判定范围那份材质（懒加载）。`None` = 着色器没编出来，调用方回落到 NoteBuffer 批量绘制。
fn judge_material() -> Option<Material> {
    Lazy::force(&JUDGE_MATERIAL);
    *JUDGE_MATERIAL
}

/// 判定范围的画布视口矩形（GL 帧缓冲坐标）—— 每次烘掩码时由 `draw_layer_at` 写入。
pub fn judge_paint_view() -> [f32; 4] {
    FRAME.with(|frame| frame.borrow().view)
}

/// 首次真正要画噪域时的一次性预热。
///
/// 原版在谱面加载阶段（`prepare_block_effects`）就链好着色器、解码好纹理，理由是
/// 「某些 GLES 驱动会拖到第一次 draw 才编译着色器」，怕第一块噪域出现时卡住一个判定帧。
/// 我们这里是**离线录制**，卡一帧无所谓，但把预热放进 `draw_layer_at`（而不是加载阶段）
/// 更安全：那里**一定**在 GL 上下文与渲染帧之内，而加载阶段不一定。
///
/// 预热用全零的 `EMPTY_TEX` 画一个亚像素矩形：空掩码会让每个片元都被丢弃，所以不会在
/// 画面上留下任何东西，也就不需要切 framebuffer / 相机。
fn warm_up() {
    static WARM: std::sync::Once = std::sync::Once::new();
    WARM.call_once(|| {
        Lazy::force(&MATERIAL);
        Lazy::force(&JUDGE_MATERIAL);
        Lazy::force(&DISPLACE_TEX);
        Lazy::force(&SPARK_TEX);
        Lazy::force(&NOISE_TEX);
        if let Some(materials) = MATERIAL.as_ref() {
            let empty = *EMPTY_TEX;
            for material in materials {
                material.set_texture("uDisplaceTex", *DISPLACE_TEX);
                material.set_texture("uSparkTex", *SPARK_TEX);
                material.set_texture("uNoiseTex", *NOISE_TEX);
                for sampler in ["uMasks", "uAuxMasks", "uScene", "_LavaTex", "uActiveColors", "uDisabledColors"] {
                    material.set_texture(sampler, empty);
                }
                material.set_uniform("uView", vec3(1., 1., 1.));
                material.set_uniform("uUnityTime", vec4(0., 0., 0., 0.));
                material.set_uniform("_ScreenParams", vec4(1., 1., 2., 2.));
                material.set_uniform("_EffectRT_TexelSize", vec4(1., 1., 1., 1.));
                material.set_uniform("_LavaMode", 0.0_f32);
                material.set_uniform("_LavaScale", LAVA_CELLS_PER_SCREEN_HEIGHT);
                material.set_uniform("_TouchPosCount", 0_i32);
                gl_use_material(*material);
                draw_rectangle(0., 0., 0.001, 0.001, WHITE);
            }
            gl_use_default_material();
            unsafe { get_internal_gl() }.flush();
        }
    });
}

/// 回到开局 / 跳转时清掉触摸 slot 与一帧内的 `_Time` 缓存。
pub fn reset_block_effects() {
    FRAME.with(|frame| {
        let mut frame = frame.borrow_mut();
        frame.touch.reset();
        frame.clock = None;
    });
}

pub fn draw_disabled_zones(res: &mut Resource, aspect: f32, zones: &[Zone]) {
    // Unity 的 `_Time` 在一帧内对所有 pass 都是常量。谱面即使在 disabled 层为空时，
    // 也会在音符之前调用这里。
    let time = res.time as f32;
    FRAME.with(|frame| frame.borrow_mut().clock = Some(time));
    draw_phases(res, aspect, zones, time, true, &[]);
}

/// 这一趟该给 `zone` 贴哪张贴图：0 = 不贴（纯色）、1 = netherrock、2 = lava。
///
/// 「还没生效」（`appearTime..enableTime`）印石头，「生效中」与「生效结束后」印岩浆。
/// `index == usize::MAX` 是强制定位/反向强制定位合成的方块（不印号的那些），
/// 它们与 `is_subtract` 一样保持纯色。
/// 只在**完整噪域**（`!simplified`）那一支里被调用：「岩浆噪域」只在非简化噪域下生效。
fn lava_fill(zone: &Zone, lava: bool, disabled: bool) -> u8 {
    if !lava || zone.index == usize::MAX {
        return 0;
    }
    if !disabled {
        2
    } else if zone.before_enable {
        1
    } else {
        2
    }
}

/// 把一帧里该画的方块按相位画出去。
///
/// 简化噪域先按「是否减算」拆成两趟：简化模式的填充色是**材质级 uniform**，一趟绘制里
/// 只可能有一个颜色；而 `isSubtract` 的块要用反色，所以只能把两类方块各画一遍
///（掩码会因此重算一次，简化模式本来就不跑描边/辉光/位移，代价可以接受）。
/// 完整噪域只有一趟（减算块靠 `enabled_compose` 的异或语义挖洞），只有「岩浆噪域」
/// 需要分贴图时才会再拆组。
fn draw_phases(res: &mut Resource, aspect: f32, zones: &[Zone], time: f32, disabled: bool, touches: &[(u64, Vec2)]) {
    let simplified = res.config.simplified_block_area || FRAME.with(|frame| frame.borrow().simplified);
    // 「判定范围让位」/「强制定位」的合成块（`judge_cover > 0`）只参与掩码
    //（`Masks::judge_cover`），**不参与绘制**：它们若混进 `zones`，材质缺失时的
    // 逐方块调试路径会把整屏那块也画成红矩形。但掩码**必须**看得见它们，所以拆出来的
    // `covers` 要跟着每一趟一起交给 `draw_layer_at`（那里只拿它算覆盖度位图）。
    let (covers, zones): (Vec<Zone>, Vec<Zone>) = zones.iter().cloned().partition(|z| z.judge_cover > 0);
    let zones = zones.as_slice();
    let covers = covers.as_slice();
    if simplified {
        let (normal, subtract): (Vec<Zone>, Vec<Zone>) = zones.iter().cloned().partition(|z| !z.invert);
        if !normal.is_empty() {
            draw_layer_at(res, aspect, &normal, covers, time, disabled, touches, false, 0);
        }
        if !subtract.is_empty() {
            draw_layer_at(res, aspect, &subtract, covers, time, disabled, touches, true, 0);
        }
    } else {
        // 「岩浆噪域」只在**完整噪域**下生效：按「这一趟该贴哪张贴图」再把本趟分成最多三组
        //（关闭时所有方块都落进第 0 组，等价于改动前的一趟，逐位不回归）。
        // 必须分趟的原因是填充贴图是**材质级 uniform**，而 `Disabled`（`appearTime..enableTime`）
        // 与 `DisabledOut`（`disableTime..disappearTime`）两段喂的是同一个 disabled 掩码通道，
        // 一趟里画不出「石头 + 岩浆」两种贴图。
        // 完整噪域本来就只有一趟（`isSubtract` 的块靠 `enabled_compose` 的异或语义挖洞，
        // 不分正负两趟），所以这里的分组不动任何几何/掩码语义。
        let lava = res.config.block_area_lava;
        let mut groups: [Vec<Zone>; 3] = Default::default();
        for zone in zones.iter().cloned() {
            groups[lava_fill(&zone, lava, disabled) as usize].push(zone);
        }
        for (fill, group) in groups.iter().enumerate() {
            if !group.is_empty() {
                draw_layer_at(res, aspect, group, covers, time, disabled, touches, false, fill as u8);
            }
        }
    }
}

/// 触摸中心是 chart 坐标；ID 用来保持原生 slot 的生命周期。
pub fn draw_zones_with_touches(res: &mut Resource, aspect: f32, zones: &[Zone], touches: &[(u64, Vector)], flip_x: bool) {
    // chart 坐标 -> 屏幕 UV，与 `draw_layer_at` 里那台 `zoom = (1, 1/aspect)` 相机一致：
    // 该相机把 chart 的 x∈[-1,1]、y∈[-1/aspect,1/aspect]（y 向上）直接映到 NDC。
    let mut touches: Vec<_> = touches
        .iter()
        .map(|&(id, p)| {
            let uv = vec2(0.5 + 0.5 * if flip_x { -p.x } else { p.x }, 0.5 + 0.5 * aspect * p.y);
            (id, uv)
        })
        .collect();
    // 判定从 `HashMap` 收集触摸。新增/移除手指时要让 SDF 累加顺序稳定。
    touches.sort_by_key(|(id, _)| *id);
    let time = FRAME.with(|frame| frame.borrow().clock).unwrap_or(res.time as f32);
    draw_phases(res, aspect, zones, time, false, &touches);
}

fn draw_layer_at(
    res: &mut Resource,
    aspect: f32,
    zones: &[Zone],
    covers: &[Zone],
    time: f32,
    disabled: bool,
    touches: &[(u64, Vec2)],
    invert: bool,
    lava: u8,
) {
    let needed = if disabled {
        zones.iter().any(|z| !z.active && z.opacity > 0.)
    } else {
        zones.iter().any(|z| z.active || z.ready)
            || !touches.is_empty()
            || FRAME.with(|frame| frame.borrow().touch.visible())
    };
    if !needed {
        return;
    }
    warm_up();
    let mut gl = unsafe { get_internal_gl() };
    let pass = gl.quad_gl.get_active_render_pass();
    let (width, height) = if let Some(pass) = pass {
        let texture = pass.texture(gl.quad_context);
        (texture.width as f32, texture.height as f32)
    } else {
        gl.quad_context.screen_size()
    };
    let viewport = gl.quad_gl.get_viewport().unwrap_or((0, 0, width as i32, height as i32));
    // 官方顶点着色器假定**顶点坐标本身就是 chart 归一化坐标**（x∈[-1,1]、
    // y∈[-1/aspect,1/aspect]、y 向上），`fieldUV = position.xy * vec2(0.5, uView.z*0.5) + 0.5`
    // 与 `sceneUV` 都建立在这一点上。但 DPS 的基础空间由**当前相机**决定，默认是
    // x∈[-aspect,aspect]、y∈[-1,1]、y 向下。这里直接从投影矩阵反解「chart -> 当前基础空间」
    // 的缩放，于是换分辨率、换 `chart_ratio` 都不用改代码。
    //
    // 前提：调用点是 `Game::render` 里 `set_camera` 之后（模型矩阵为单位阵）的位置。
    let (proj_x, proj_y) = {
        let proj = gl.quad_gl.get_projection_matrix();
        (proj.x_axis.x, proj.y_axis.y)
    };
    debug_assert!(proj_x.abs() > f32::EPSILON && proj_y.abs() > f32::EPSILON, "block area needs an invertible 2D camera");
    // 「谱面缩放」：上面这步把 chart 空间归一化到**当前相机**，等于把相机缩放整个抵消掉，
    // 于是谱面跟着 `chartRatio` 放大/缩小时噪域不动。`Game::render` 会把谱面相机相对本
    // 相机的缩放比记在 `res.block_area_chart_scale` 里，这里乘回去（`chartRatio == 1.`
    // 时它是 `[1., 1.]`，逐像素与原行为一致）。
    let k = res.block_area_chart_scale;
    // 单位阵：`Game::render` 给噪域这趟单独设的相机是 `zoom = vec2(1., -aspect)`，基础空间
    // 的 `(-1, -1/aspect)~(1, 1/aspect)` 正好等于 chart 空间的那块矩形，于是全屏四边形的
    // clip 就是 `±1`、铺满整屏，而 `fieldUV = position.xy * vec2(0.5, uView.z * 0.5) + 0.5`
    // 也从整屏取到 `[0, 1]`。谱面缩放（`chartRatio`）整条改由掩码承担 —— 见下面
    // `render_displaced` 的调用点，那里按 `k` 缩放方块几何。
    let chart_to_base = Matrix::identity();
    let Some(m) = MATERIAL.as_ref() else {
        for z in zones.iter().filter(|z| !z.invert && z.active != disabled) {
            let m = Matrix::new_translation(&z.center)
                * nalgebra::Rotation2::new(z.angle).to_homogeneous()
                * Matrix::identity().append_nonuniform_scaling(&(z.half * 2.));
            res.apply_model_of(&(chart_to_base * m), |_| {
                draw_rectangle(-0.5, -0.5, 1., 1., Color::new(1., 0., 0., if z.active { 0.4 } else { 0.12 }));
            });
        }
        return;
    };
    // ActiveBlock 是精灵 / 音符 / HUD 之后的后处理；Disabled 是判定线之前的背景层。
    // 两者都直接沿用当前 pass / 视口，不新建 framebuffer。
    gl.flush();
    // 注意：这里**绝不能** `clone()`。`MSRenderTarget` 是 `#[derive(Clone)]` 且实现了 `Drop`
    //（`core/render.rs:113` 会删掉 renderbuffer / framebuffer / 输出纹理），克隆体一析构
    // 整帧的渲染目标就没了，画面直接全黑。只有 `sample_count > 1` 时 `res.chart_target`
    // 才是 `Some`，所以这个坑只在开 MSAA 时显形。
    // 下面的闭包里还要 `res.apply_model_of`（需要 `&mut res`），与借用冲突，
    // 因此用裸指针取一下：`apply_model_of` 不碰 `chart_target`，指针全程有效。
    let chart_target: Option<*const crate::core::MSRenderTarget> = res.chart_target.as_ref().map(|it| it as *const _);
    FRAME.with(|frame| {
        let mut frame = frame.borrow_mut();
        // SAFETY: 见上，`res.chart_target` 在这个闭包期间不会被改动（`apply_model_of` 只推/弹模型矩阵）。
        let chart_target = chart_target.map(|it| unsafe { &*it });
        let (width, height) = (viewport.2.max(1) as usize, viewport.3.max(1) as usize);
        // 「噪域密度」：它就是掩码格子边长（上游固定 8 像素 = 1/8 降采样）。
        // 密度越大 ⇒ 格子越小 ⇒ 边缘颗粒越细越密、描边与辉光也越细。
        // 光栅化开销按 1/cell² 增长，所以把用户输入 clamp 到一个安全区间。
        let density = res.config.block_area_density;
        // 「谱面缩放」也要作用在掩码粒度上：`chartRatio < 1.` 时整张谱面被缩小，噪域的
        // 边缘颗粒、描边、辉光都必须跟着变小，否则缩得越小越显得噪点粗糙。做法是让掩码
        // **提分辨率**（`cell_scale`）而不是把 `cell` 乘小 —— `cell` 是整数屏幕像素，
        // `3 × 0.53 = 1.59` 会被 `round` 量化成 `2`（相当于只缩到 0.67，观感上仍是「边缘太
        // 强」），比例更小干脆缩不动。`k == 1.` 时严格等于原值、零回归。
        let scale = res.block_area_chart_scale[0].abs().clamp(0.05, 1.);
        let density_cell = if density.is_finite() && density > 0. {
            (8. / density.clamp(0.25, 4.)).round().clamp(2., 32.)
        } else {
            8.
        };
        frame.masks.cell = density_cell as usize;
        frame.masks.cell_scale = scale;
        // 边缘破碎（每纹素的噪声位移）只由 `block_area_edge_noise` 决定 —— 「噪域扭曲」管的是
        // 填充内部的位移噪声与快照采样（见 `set_fill_style`），**不碰边缘**：那是两个独立开关，
        // 关掉扭曲不该把边缘也一起抹平。
        frame.masks.edge_noise = res.config.block_area_edge_noise;
        // 「谱面缩放」：`chartRatio < 1.` 时屏幕会显示出「chart 空间 ±1」以外的区域（那些越界
        // 的长条就长在那儿），所以掩码必须也覆盖到那儿 —— 把方块几何按 `k` 缩小，掩码就从
        // 「覆盖 chart ±1」变成「覆盖 chart ±1/k」，正好等于此时屏幕显示的范围。`k == [1., 1.]`
        // 时走原样，逐像素零回归。
        // 「强制定位」的洞口完全由掩码决定：`Masks::render_displaced_covers` 会把 `covers`
        // 打成覆盖度位图，`enabled_compose` 再按 `1 - cover/255` 衰减 —— 那一圈矩形里不留噪域。
        // （判定范围**不**走这条路：它反过来是「自己**被**噪域擦掉」，见 `judge_range.glsl`。）
        //
        // **必须把 `zones` 与 `covers` 分开送进去**：`covers`（强制定位的矩形）每帧都在动，一旦和
        // `zones` 拼成同一个切片，掩码那半边「方块没变就跳过逐格栅格化」的缓存就永远命不中，
        // 每帧都要把全部方块重栅格化一遍。这是实测的 5 倍性能差异（80.3 → 15.5 FPS）。
        let scale_zone = |z: &Zone| Zone {
            center: z.center.component_mul(&Vector::new(k[0], k[1])),
            half: z.half.component_mul(&Vector::new(k[0].abs(), k[1].abs())),
            ..z.clone()
        };
        let scale_needed = (k[0] - 1.).abs() > f32::EPSILON || (k[1] - 1.).abs() > f32::EPSILON;
        let scaled_zones: Vec<Zone> = if scale_needed { zones.iter().map(scale_zone).collect() } else { Vec::new() };
        let scaled_covers: Vec<Zone> = if scale_needed { covers.iter().map(scale_zone).collect() } else { Vec::new() };
        let mask_zones: &[Zone] = if scaled_zones.is_empty() { zones } else { scaled_zones.as_slice() };
        let mask_covers: &[Zone] = if scaled_covers.is_empty() { covers } else { scaled_covers.as_slice() };
        frame.masks.render_displaced_covers(width, height, aspect, mask_zones, mask_covers, time);
        if !disabled {
            frame.touch.update_fingers(touches, time);
        }
        let hover = !disabled && HOVER_ASSET && frame.touch.visible();
        // 颜色是整帧的性质（`uColored` 在材质里是编译期常量），由掩码那一步写好。
        let colored = frame.masks.colored;
        let m = m[material_index(disabled, hover, colored)];
        if hover {
            let bw = frame.masks.width / 2;
            let bh = frame.masks.height / 2;
            for y in 0..bh {
                for x in 0..bw {
                    let value = frame
                        .touch
                        .sample(vec2((x as f32 + 0.5) / bw as f32, (y as f32 + 0.5) / bh as f32), width as f32 / height as f32);
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let i = ((y * 2 + dy) * frame.masks.width + x * 2 + dx) * 4 + 3;
                            frame.masks.aux_rgba[i] = value;
                        }
                    }
                }
            }
        }
        let dim = (width as u32, height as u32);
        let effect_dim = (frame.masks.width as u32, frame.masks.height as u32);
        let resized = frame
            .effect
            .is_none_or(|texture| (texture.width() as u32, texture.height() as u32) != effect_dim);
        if resized {
            // 普通被采样纹理，绝不挂到新的 framebuffer 上。
            // 先分配再删除：GL 可能在 miniquad 的 sampler 缓存还记得旧绑定时就复用这个名字。
            let texture = Texture2D::from_rgba8(effect_dim.0 as u16, effect_dim.1 as u16, &frame.masks.rgba);
            texture.set_filter(FilterMode::Linear);
            if let Some(old) = frame.effect.replace(texture) {
                old.delete();
            }
        } else if frame.uploaded_masks != Some(frame.masks.revision) {
            frame
                .effect
                .unwrap()
                .raw_miniquad_texture_handle()
                .update(unsafe { get_internal_gl() }.quad_context, &frame.masks.rgba);
        }
        let aux = frame.aux;
        let resized_aux = aux.is_none_or(|t| (t.width() as u32, t.height() as u32) != effect_dim);
        if resized_aux {
            let texture = Texture2D::from_rgba8(effect_dim.0 as u16, effect_dim.1 as u16, &frame.masks.aux_rgba);
            texture.set_filter(FilterMode::Linear);
            if let Some(old) = frame.aux.replace(texture) {
                old.delete();
            }
        } else if hover || frame.uploaded_masks != Some(frame.masks.revision) {
            frame
                .aux
                .unwrap()
                .raw_miniquad_texture_handle()
                .update(unsafe { get_internal_gl() }.quad_context, &frame.masks.aux_rgba);
        }
        frame.uploaded_masks = Some(frame.masks.revision);
        // 彩色噪域的两张颜色平面：尺寸与掩码一致、点采样（颜色归属是逐掩码纹素定的，
        // 线性插值会把颜色糊出噪域边缘）。没有颜色的帧连一次 `update` 都不做。
        if frame.masks.colored {
            for layer in 0..2 {
                let colors = if layer == 0 { &frame.masks.active_colors } else { &frame.masks.disabled_colors };
                let resized = frame.color_textures[layer].is_none_or(|texture| (texture.width() as u32, texture.height() as u32) != effect_dim);
                if resized {
                    let texture = Texture2D::from_rgba8(effect_dim.0 as u16, effect_dim.1 as u16, colors);
                    texture.set_filter(FilterMode::Nearest);
                    if let Some(old) = frame.color_textures[layer].replace(texture) {
                        old.delete();
                    }
                } else if frame.uploaded_colors != Some(frame.masks.revision) {
                    frame.color_textures[layer]
                        .unwrap()
                        .raw_miniquad_texture_handle()
                        .update(unsafe { get_internal_gl() }.quad_context, colors);
                }
            }
            frame.uploaded_colors = Some(frame.masks.revision);
        }
        // 判定范围「让位给噪域」：把这份掩码纹理连同它的视口交给判定范围 —— 掩码 R 通道就是
        // 噪域方块的覆盖度，`judge_range.glsl` 按 `color.a * (1 - R)` 画，于是重合处被噪域擦掉。
        // 判定范围画在 `Chart::render` 里（早于本函数），读到的是**上一帧**留下的这份；
        // `Chart::render_block_area` 每帧开头先清成 None，所以噪域整个关掉时它一直是 None。
        frame.view = [viewport.0 as f32, viewport.1 as f32, viewport.2 as f32, viewport.3 as f32];
        res.judge_range_mat = judge_material();
        res.block_area_paint = frame.effect;
        if frame.scene.is_none_or(|texture| (texture.width() as u32, texture.height() as u32) != dim) {
            let texture = miniquad::Texture::new(
                unsafe { get_internal_gl() }.quad_context,
                miniquad::TextureAccess::Static,
                None,
                miniquad::TextureParams {
                    width: dim.0,
                    height: dim.1,
                    // 原生 SceneColor 用线性 blit 把相机降采样，再对那张更小的 RT 做点采样。
                    // 从这张全尺寸副本上去采它量化后的纹素中心。
                    filter: miniquad::FilterMode::Linear,
                    ..Default::default()
                },
            );
            // 同一张纹理既做采样器又做 blit 目标：`miniquad::Texture` 是 `Copy`，
            // `MSRenderTarget` 里也是这么用的（`core/render.rs:54-68`）。
            let pass = miniquad::RenderPass::new(unsafe { get_internal_gl() }.quad_context, texture, None);
            if let Some(old) = frame.scene.replace(Texture2D::from_miniquad_texture(texture)) {
                old.delete();
            }
            frame.scene_pass = Some(pass);
        }
        let scene = frame.scene.unwrap();
        let scene_pass = frame.scene_pass.unwrap();
        if !disabled {
            copy_scene(chart_target, pass, viewport, scene_pass);
        }
        m.set_texture("uDisplaceTex", *DISPLACE_TEX);
        m.set_texture("uSparkTex", *SPARK_TEX);
        m.set_texture("uMasks", frame.effect.unwrap());
        m.set_texture("uScene", scene);
        m.set_texture("uAuxMasks", frame.aux.unwrap());
        m.set_texture("uNoiseTex", *NOISE_TEX);
        // 「岩浆噪域」：`lava` 0 = 关闭（绑空纹理并把 `_LavaMode` 归零 ⇒ 官方原样），
        // 1 = netherrock（还没生效那一段），2 = lava 的当前动画帧。只有简化噪域会传非 0；
        // 附带资源包缺素材时 `lava_mode` 已经把它折回 0。
        let lava = lava_mode(lava);
        match lava {
            1 => m.set_texture("_LavaTex", NETHERROCK_TEX.unwrap_or(*EMPTY_TEX)),
            2 => m.set_texture("_LavaTex", lava_frame(time)),
            _ => m.set_texture("_LavaTex", *EMPTY_TEX),
        }
        m.set_uniform("_LavaMode", if lava == 0 { 0.0_f32 } else { 1.0_f32 });
        m.set_uniform("_LavaScale", LAVA_CELLS_PER_SCREEN_HEIGHT);
        // 彩色噪域：本帧至少一块带颜色才把 `uColored` 置 1 并绑上颜色平面。没有颜色的帧
        // 把 `uColored` 归零、两张颜色绑空纹理 —— 片元走的是 `blockPalette` 恒等分支，
        // 逐像素与加颜色之前相同。
        match (colored, frame.color_textures[0], frame.color_textures[1]) {
            (true, Some(active), Some(disabled_colors)) => {
                m.set_texture("uActiveColors", active);
                m.set_texture("uDisabledColors", disabled_colors);
            }
            _ => {
                m.set_uniform("uColored", 0_i32);
                m.set_texture("uActiveColors", *EMPTY_TEX);
                m.set_texture("uDisabledColors", *EMPTY_TEX);
            }
        }
        m.set_uniform("uUnityTime", vec4(time / 20., time, time * 2., time * 3.));
        m.set_uniform("uView", vec3(width as f32, height as f32, aspect));
        m.set_uniform("_ScreenParams", vec4(width as f32, height as f32, 1. + 1. / width as f32, 1. + 1. / height as f32));
        m.set_uniform("_EffectRT_TexelSize", vec4(1. / effect_dim.0 as f32, 1. / effect_dim.1 as f32, effect_dim.0 as f32, effect_dim.1 as f32));
        m.set_uniform("_TouchPosCount", touches.len().min(10) as i32);
        for (i, (_, uv)) in touches.iter().take(10).enumerate() {
            // `uv` 是 [0,1] 的屏幕 UV（y 向上）；片元里拿它比较的是
            // `screenPos.xy / screenPos.w * _ScreenParams.xy / _ScreenParams.y`，也就是
            // x∈[-w/h, w/h]、y∈[-1, 1] 的 NDC 空间，所以这里要先乘 2 减 1。
            m.set_uniform(&format!("_TouchPos[{i}]"), vec2((uv.x * 2. - 1.) * width as f32 / height as f32, uv.y * 2. - 1.));
        }
        m.set_uniform("_TouchPosShine", (0.63 + 0.37 * ((time * 43.).sin() * 0.5 + 0.5)) * 2.);
        // 「噪域打击光晕半径」：官方默认 0.5，这里允许按配置放大 / 缩小。
        m.set_uniform("_TouchPosRadius", res.config.block_area_hover_radius.max(0.));
        let simplified = frame.simplified || res.config.simplified_block_area;
        set_fill_style(m, disabled, simplified, invert, &res.config);
        // `chart_to_base` 把这台四边形从 chart 归一化空间送到当前基础空间：全屏四边形
        // 就是 chart 空间的 (-1,-1/aspect)-(1,1/aspect)，于是官方顶点着色器逐字成立。
        res.apply_model_of(&chart_to_base, |_| {
            gl_use_material(m);
            draw_rectangle(-1., -1. / aspect, 2., 2. / aspect, WHITE);
            gl_use_default_material();
            // 马上冲刷。绘制是延迟到帧末才真正执行的，而 `uMasks` 是材质上的**同一张**
            // 纹理：简化模式把「普通块」和「减算块」分成两趟（`draw_phases`），两趟的
            // `zones` 不同 ⇒ `render_displaced` 会重算掩码覆盖那张纹理。不在这里冲刷的话，
            // 两趟的全屏四边形都会采样到最后那一趟的掩码，前一趟的方块就整块消失。
            unsafe { get_internal_gl() }.flush();
        });
    });
}

/// 「简化噪域」（`simplified_block_area`）与「完整噪域」共用同一份官方材质，
/// 区别只是每帧覆写哪一组 uniform —— 所以这里也叫「降级成纯色」而不是另一条管线。
///
/// 简化时把 `_FillStrength` 归零是**唯一**能同时掐掉场景快照与火花的开关
///（`_SparkMapOpacity = 0` 只压掉火花那一项，`frag:329` 后色调化的场景那一路照旧输出）；
/// `_EdgeOpacity` / `_GlowIntensity` 掐掉描边与辉光，`_ShineBrightness` 掐掉 ready 相位的白闪，
/// `_DisplaceBlendIntensity` 掐掉填充上的噪声斑驳。`frag:197` 的 `discard` 判据用的是
/// **未缩放**的 `u_xlat16_50`，所以归零这几个不会缩小绘制范围，只是那些像素写出
/// `(0,0,0,0)`；active 材质是 `(One, OneMinusValue(SourceAlpha))`，全零即恒等元。
///
/// 另外注意 active 材质是**预乘**混合，而 `_FillOpacity` 只进 `result.w`（`frag:339`），
/// 所以 rgb 必须先乘 alpha，否则 `block_area_alpha < 1` 时颜色会偏亮。
fn set_fill_style(m: Material, disabled: bool, simplified: bool, invert: bool, cfg: &crate::config::Config) {
    if !simplified {
        // 「岩浆噪域」只在完整噪域下生效，且开启它时「噪声斑点 / 噪域扭曲 /
        // 噪域火花」三项**强制关闭**：岩浆 / 下界岩必须是一整张干净的贴图 —— 噪声斑点会糊在
        // 贴图上、填充位移会把矩形边缘撕出锯齿、火花点阵也是多余的一层。
        // 这里只改「有效值」，不动用户存下来的配置本身：关掉岩浆噪域后三项照旧生效。
        // 三项的强制关闭**只发生在这里**：前端不再因岩浆噪域而禁用 / 清零这三项，界面里照旧可勾。
        let lava = cfg.block_area_lava;
        // `_SnapshotPixelScale` 是本仓库新增的 uniform（场景快照的降采样倍率，官方在
        // `frag` 里写死 3.0）：两趟都先复位成官方默认值，避免「噪域扭曲」从关到开时
        // 上一帧写下的 1.0 留在材质上。
        m.set_uniform("_SnapshotPixelScale", float_default("_SnapshotPixelScale"));
        // 先把官方值复位（uniform 挂在材质上、跨帧跨 pass 存活，从简化切回来会残留覆写）。
        if disabled {
            let c = color_default("uDisabledFillColor");
            m.set_uniform("uDisabledFillColor", vec4(c[0], c[1], c[2], c[3]));
            m.set_uniform("uDisabledFillOpacity", float_default("uDisabledFillOpacity"));
            m.set_uniform("uDisabledSparkOpacity", float_default("uDisabledSparkOpacity"));
        } else {
            let c = color_default("_FillColor");
            m.set_uniform("_FillColor", vec4(c[0], c[1], c[2], c[3]));
            m.set_uniform("_FillOpacity", float_default("_FillOpacity"));
            // 「岩浆噪域」下填充必须是**纯贴图**：`frag:350-351` 会用 `_FillStrength` 把场景快照
            // 混进填充色（官方噪域正是靠它显示「被色调化 + 像素化的场景」，也正是它把岩浆混成
            // 一团发暗的褐色）。归零后填充就是贴图本身 × `_FillOpacity`。
            m.set_uniform("_FillStrength", if lava { 0.0_f32 } else { float_default("_FillStrength") });
            m.set_uniform("_EdgeOpacity", float_default("_EdgeOpacity"));
            m.set_uniform("_GlowIntensity", float_default("_GlowIntensity"));
            m.set_uniform("_ShineBrightness", float_default("_ShineBrightness"));
            m.set_uniform("_DisplaceBlendIntensity", float_default("_DisplaceBlendIntensity"));
            m.set_uniform("_SparkMapOpacity", float_default("_SparkMapOpacity"));
            m.set_uniform("_BackgroundPixelScale", float_default("_BackgroundPixelScale"));
            // `_DisplaceStrength` 也要复位：下面「偏离默认才写」的写法在「噪域扭曲」从关到开、
            // 而扭曲强度又恰好等于官方默认值时会漏写，导致上一帧的 0 残留。
            m.set_uniform("_DisplaceStrength", float_default("_DisplaceStrength"));
        }
        // 「噪域颜色」：完整噪域那套官方配色的**色相全是 0（纯红）**，所以只把色相整体
        // 旋转「用户颜色色相 - 0」就能换色，而饱和度 / 明度全部保持官方值 —— 填充、描边、
        // 辉光、未启用填充的相对层次原样保留，不会像直接改 rgb 那样把配色配歪。
        // 旋转量为 0（默认红，或用户选了灰阶）时**一个 uniform 都不写**，保证默认配置逐位不回归。
        let shift = hue_of(cfg.block_area_normal_color);
        if shift.abs() > f32::EPSILON {
            let names: &[&str] = if disabled { &["uDisabledFillColor"] } else { &["_FillColor", "_EdgeColor", "_GlowColor"] };
            for name in names {
                let c = rotate_hue(color_default(name), shift);
                m.set_uniform(*name, vec4(c[0], c[1], c[2], c[3]));
            }
        }
        if !cfg.block_area_spark || lava {
            // 「噪域火花」关闭（或「岩浆噪域」开着时的强制关闭）。火花是独立于场景快照的一项：
            // active 那一趟由 `_SparkMapOpacity` 控制（`u_xlat1` 里那层被色调化的场景照旧输出，
            // 只是不再叠火花点阵），未启用那一趟由 `uDisabledSparkOpacity` 控制。
            if disabled {
                m.set_uniform("uDisabledSparkOpacity", 0.0_f32);
            } else {
                m.set_uniform("_SparkMapOpacity", 0.0_f32);
            }
        }
        if !cfg.block_area_noise_blotch || lava {
            // 「噪声斑点」关闭（或「岩浆噪域」开着时的强制关闭）：`frag:331` 里
            // `(-噪声值) * _DisplaceBlendIntensity + _FillColor` 那一项就是往填充色里混噪声斑点，
            // 归零后填充变成纯净色（贴图路径下这一项本来也不参与填充，但留着会混进别的分支）。
            m.set_uniform("_DisplaceBlendIntensity", 0.0_f32);
        }
        // 「扭曲强度」/「颗粒大小」/「快照降采样」：只在偏离官方默认值时才写，默认配置逐位不回归。
        // 「噪域扭曲」关闭时填充位移强行归零、像素化也一起关掉（快照 1:1 采样 + 不做像素块
        // 量化）：噪域不再扭曲，后面的字 / 按键保持清晰，只剩「场景快照 / 纯色 × 透明度」。
        let displace_strength = if cfg.block_area_distort && !lava {
            cfg.block_area_displace_strength
        } else {
            0.
        };
        if (displace_strength - float_default("_DisplaceStrength")).abs() > f32::EPSILON {
            m.set_uniform("_DisplaceStrength", displace_strength);
        }
        let pixel_scale = if cfg.block_area_distort && !lava {
            cfg.block_area_pixel_scale.max(1.0_f32)
        } else {
            1.0_f32
        };
        if (pixel_scale - float_default("_BackgroundPixelScale")).abs() > f32::EPSILON {
            // `frag:210` 是 `max(_BackgroundPixelScale, 1.0)`，所以这里也夹一下下限。
            m.set_uniform("_BackgroundPixelScale", pixel_scale);
        }
        // 场景快照的降采样倍率：「噪域扭曲」关掉时按 1:1（画面清晰）；开着时保持官方
        // 写死的 3.0 —— 但如果用户把「颗粒大小」调离官方默认值（说明他本来就在手动控制
        // 场景的像素化程度），那就不再叠加官方的 1/3 降采样，避免「调到 1 还是发虚」。
        let snapshot_scale = if cfg.block_area_distort
            && !lava
            && (pixel_scale - float_default("_BackgroundPixelScale")).abs() <= f32::EPSILON
        {
            float_default("_SnapshotPixelScale")
        } else {
            1.0_f32
        };
        if (snapshot_scale - float_default("_SnapshotPixelScale")).abs() > f32::EPSILON {
            m.set_uniform("_SnapshotPixelScale", snapshot_scale);
        }
        // 「流动速度」/「噪声半径」/「噪声平滑度」：同样只在偏离官方默认值时才写 uniform。
        if (cfg.block_area_displace_speed - float_default("_DisplaceSpeed")).abs() > f32::EPSILON {
            m.set_uniform("_DisplaceSpeed", cfg.block_area_displace_speed);
        }
        if (cfg.block_area_noise_radius - float_default("_NoiseRadius")).abs() > f32::EPSILON {
            m.set_uniform("_NoiseRadius", cfg.block_area_noise_radius);
        }
        if (cfg.block_area_noise_smoothness - float_default("_NoiseSmoothness")).abs() > f32::EPSILON {
            m.set_uniform("_NoiseSmoothness", cfg.block_area_noise_smoothness);
        }
        return;
    }
    // 减算（`isSubtract`）的方块用反色：enable 是 `1 - rgb`，disable 是它的淡色。
    let color = cfg.block_area_color;
    let color = if invert { [1. - color[0], 1. - color[1], 1. - color[2]] } else { color };
    // 简化模式的淡色公式：`pale = rgb + (1 - rgb) * 0.6`。
    let pale = [color[0] + (1. - color[0]) * 0.6, color[1] + (1. - color[1]) * 0.6, color[2] + (1. - color[2]) * 0.6];
    if disabled {
        m.set_uniform("uDisabledFillColor", vec4(pale[0], pale[1], pale[2], 1.));
        m.set_uniform("uDisabledFillOpacity", cfg.block_area_alpha);
        m.set_uniform("uDisabledSparkOpacity", 0.0_f32);
    } else {
        m.set_uniform("_FillColor", vec4(color[0] * cfg.block_area_alpha, color[1] * cfg.block_area_alpha, color[2] * cfg.block_area_alpha, 1.));
        m.set_uniform("_FillOpacity", cfg.block_area_alpha);
        // 这些字面量必须显式写成 `f32`：`set_uniform` 是泛型，`0.` 会被推断成 `f64`（8 字节），
        // 而 uniform 注册的是 `Float1`（4 字节），miniquad 只会打印
        // `Trying to set uniform … sized 4 bytes value of 8 bytes` 然后**整个丢弃**这次写入。
        m.set_uniform("_FillStrength", 0.0_f32);
        m.set_uniform("_EdgeOpacity", 0.0_f32);
        m.set_uniform("_GlowIntensity", 0.0_f32);
        m.set_uniform("_ShineBrightness", 0.0_f32);
        m.set_uniform("_DisplaceBlendIntensity", 0.0_f32);
        m.set_uniform("_SparkMapOpacity", 0.0_f32);
        m.set_uniform("_BackgroundPixelScale", 1.0_f32);
    }
}

/// 把当前渲染目标的画面拷进 `scene_pass` 挂着的纹理，供方块着色器的 `uScene` 采样。
///
/// 原版 Phira-Pro 用的是 `glCopyTexSubImage2D`，但本仓库的 miniquad fork 没有静态导出它。
/// 这里原先写成「`glReadPixels` 读回 CPU、再 `glTexSubImage2D` 传上去」，但在桌面 GL 4.6
///（NVIDIA）上 `glReadPixels` **恒返回 `GL_INVALID_OPERATION` 且读回全 0**：framebuffer 状态是
/// `GL_FRAMEBUFFER_COMPLETE`、`GL_READ_BUFFER` 是 `GL_COLOR_ATTACHMENT0`、`GL_RGB` 与
/// `GL_RGBA` 两种 format 都失败。结果是 `uScene` 一直是黑的，方块整体偏暗、缺少场景采样与位移。
/// 现在改成 GPU 端 `glBlitFramebuffer`（与 `core/render.rs:16` 的 `copy_fbo` 同一条路）：
/// 没有 CPU 往返，也不挑像素格式。
///
/// MSAA 时当前 pass 是多重采样 FBO，必须先解析到单采样的 `output()` 才能 blit。
fn copy_scene(
    chart_target: Option<&crate::core::MSRenderTarget>,
    pass: Option<miniquad::RenderPass>,
    viewport: (i32, i32, i32, i32),
    scene_pass: miniquad::RenderPass,
) {
    use miniquad::gl::*;
    // 这个 GLES3 常量在本仓库 miniquad fork 的桌面表里没有。
    const READ_FRAMEBUFFER_BINDING: u32 = 0x8CAA;
    unsafe {
        let ctx = get_internal_gl().quad_context;
        let (mut read, mut draw) = (0, 0);
        glGetIntegerv(READ_FRAMEBUFFER_BINDING, &mut read);
        glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &mut draw);
        let src = if let Some(target) = chart_target.filter(|target| Some(target.input().render_pass) == pass) {
            target.blit();
            target.output().render_pass.gl_internal_id(get_internal_gl().quad_context)
        } else {
            // 否则源就是当前逻辑 pass 自己；`None` 表示默认 framebuffer（id 0）。
            pass.map(|pass| pass.gl_internal_id(ctx)).unwrap_or(0)
        };
        let (w, h) = (viewport.2.max(1), viewport.3.max(1));
        glBindFramebuffer(GL_READ_FRAMEBUFFER, src);
        glBindFramebuffer(GL_DRAW_FRAMEBUFFER, scene_pass.gl_internal_id(ctx));
        glBlitFramebuffer(viewport.0, viewport.1, viewport.0 + w, viewport.1 + h, 0, 0, w, h, GL_COLOR_BUFFER_BIT, GL_NEAREST);
        // `glBlitFramebuffer` 不恢复绑定，这里自己还原成进来时的样子。
        glBindFramebuffer(GL_READ_FRAMEBUFFER, read as u32);
        glBindFramebuffer(GL_DRAW_FRAMEBUFFER, draw as u32);
    }
}

const FRAGMENT: &str = include_str!("../../../../../../assets/blockarea/block_shader_full.frag");
const VERTEX: &str = include_str!("../../../../../../assets/blockarea/block_shader_full.vert");

#[cfg(test)]
mod tests {
    use super::*;

    /// 布局必须是 `[disabled, active, hover]`，彩色变体在同一套上再加 3。选错下标就是
    /// 「带颜色的噪域仍然画成原生红」或者「无颜色的噪域去采空颜色图」。
    #[test]
    fn material_index_matches_the_layout() {
        assert_eq!(material_index(true, false, false), 0);
        assert_eq!(material_index(false, false, false), 1);
        assert_eq!(material_index(false, true, false), 2);
        assert_eq!(material_index(true, false, true), 3);
        assert_eq!(material_index(false, false, true), 4);
        assert_eq!(material_index(false, true, true), 5);
        // disabled 永远不会是 hover（`draw_layer_at` 里 `hover = !disabled && ...`），
        // 但真传进来也得落在自己那一格。
        assert_eq!(material_index(true, true, false), 0);
        assert_eq!(material_index(true, true, true), 3);
    }

    /// 彩色材质靠**逐字替换**片元里的这几行来特化。片元是从官方反编译产物搬过来的，
    /// 再同步一次就可能改掉空白/写法；那时替换会静默失效（`uColored` 退回 uniform 的 0，
    /// 于是带颜色的噪域仍然画成原生红），所以这里把字面量钉住。
    #[test]
    fn fragment_keeps_the_literals_the_colored_specialization_replaces() {
        assert_eq!(FRAGMENT.matches("uniform int uColored;").count(), 1, "彩色特化靠这条精确替换");
        assert!(FRAGMENT.contains("texture2D(uActiveColors, fieldUV)"));
        assert!(FRAGMENT.contains("texture2D(uDisabledColors, fieldUV)"));
        assert!(FRAGMENT.contains("mediump"), "彩色变体会把 mediump 提到 highp");
    }

    /// 「有没有颜色」的判据：原生红与 `None` 等价，`opacity == 0` 的块不算。
    #[test]
    fn is_colored_treats_native_red_as_no_color() {
        let mut zone = Zone {
            center: Vector::new(0., 0.),
            half: Vector::new(0.5, 0.5),
            angle: 0.,
            invert: false,
            active: true,
            ready: false,
            before_enable: false,
            opacity: 1.,
            index: 0,
            judge_cover: 0,
            color: None,
        };
        assert!(!zone.is_colored());
        zone.set_color(Some(DEFAULT_BLOCK_COLOR));
        assert!(!zone.is_colored(), "原生红不是「有颜色」");
        zone.set_color(Some([0., 1., 0.]));
        assert!(zone.is_colored());
        zone.opacity = 0.;
        assert!(!zone.is_colored(), "还没出现的块不该把整帧拖进彩色材质");
        zone.opacity = 1.;
        zone.set_color(None);
        assert!(!zone.is_colored());
    }

    /// `set_color` 按 0~1 夹取：越界会让片元的色相搬运算出负的饱和度。
    #[test]
    fn set_color_clamps_to_unit_range() {
        let mut zone = Zone {
            center: Vector::new(0., 0.),
            half: Vector::new(0.5, 0.5),
            angle: 0.,
            invert: false,
            active: true,
            ready: false,
            before_enable: false,
            opacity: 1.,
            index: 0,
            judge_cover: 0,
            color: None,
        };
        zone.set_color(Some([-1., 2., 0.5]));
        assert_eq!(zone.color, Some([0., 1., 0.5]));
    }
}
