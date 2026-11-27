use super::{Anim, Resource, Tweenable};
use crate::ext::{get_viewport, screen_aspect};
use anyhow::{anyhow, bail, Result};
use macroquad::prelude::*;
use macroquad::texture::RenderTarget;
use miniquad::UniformType;
use once_cell::sync::Lazy;
use phf::phf_map;
use regex::Regex;
use std::{collections::HashSet, ops::Range};

static SHADERS: phf::Map<&'static str, &'static str> = phf_map! {
    "chromatic" => include_str!("shaders/chromatic.glsl"),
    "circleBlur" => include_str!("shaders/circle_blur.glsl"),
    "fisheye" => include_str!("shaders/fisheye.glsl"),
    "glitch" => include_str!("shaders/glitch.glsl"),
    "grayscale" => include_str!("shaders/grayscale.glsl"),
    "noise" => include_str!("shaders/noise.glsl"),
    "pixel" => include_str!("shaders/pixel.glsl"),
    "radialBlur" => include_str!("shaders/radial_blur.glsl"),
    "shockwave" => include_str!("shaders/shockwave.glsl"),
    "vignette" => include_str!("shaders/vignette.glsl"),
};

static RPE_SHADERS: phf::Map<&'static str, &'static str> = phf_map! {
    "bokeh_pr" => include_str!("shaders/rpe/bokeh_pr.glsl"),
    "burn_pr" => include_str!("shaders/rpe/burn_pr.glsl"),
    "camera_pr" => include_str!("shaders/rpe/camera_pr.glsl"),
    "color_shift_pr" => include_str!("shaders/rpe/color_shift_pr.glsl"),
    "distortion_pr" => include_str!("shaders/rpe/distortion_pr.glsl"),
    "emboss_pr" => include_str!("shaders/rpe/emboss_pr.glsl"),
    "flare_pr" => include_str!("shaders/rpe/flare_pr.glsl"),
    "flip_pr" => include_str!("shaders/rpe/flip_pr.glsl"),
    "flowing_pr" => include_str!("shaders/rpe/flowing_pr.glsl"),
    "fog_pr" => include_str!("shaders/rpe/fog_pr.glsl"),
    "glow_effect_pr" => include_str!("shaders/rpe/glow_effect_pr.glsl"),
    "heat_distortion_pr" => include_str!("shaders/rpe/heat_distortion_pr.glsl"),
    "hologram_pr" => include_str!("shaders/rpe/hologram_pr.glsl"),
    "image_noise_pr" => include_str!("shaders/rpe/image_noise_pr.glsl"),
    "kaleidoscope_pr" => include_str!("shaders/rpe/kaleidoscope_pr.glsl"),
    "lightning_pr" => include_str!("shaders/rpe/lightning_pr.glsl"),
    "liquid_pr" => include_str!("shaders/rpe/liquid_pr.glsl"),
    "neon_pr" => include_str!("shaders/rpe/neon_pr.glsl"),
    "night_vision_pr" => include_str!("shaders/rpe/night_vision_pr.glsl"),
    "oil_painting_pr" => include_str!("shaders/rpe/oil_painting_pr.glsl"),
    "old_tv_pr" => include_str!("shaders/rpe/old_tv_pr.glsl"),
    "rain_pr" => include_str!("shaders/rpe/rain_pr.glsl"),
    "rainbow_pr" => include_str!("shaders/rpe/rainbow_pr.glsl"),
    "rays_pr" => include_str!("shaders/rpe/rays_pr.glsl"),
    "scanline_pr" => include_str!("shaders/rpe/scanline_pr.glsl"),
    "snow_pr" => include_str!("shaders/rpe/snow_pr.glsl"),
    "sobel_pr" => include_str!("shaders/rpe/sobel_pr.glsl"),
    "two_tone_pr" => include_str!("shaders/rpe/two_tone_pr.glsl"),
    "underwater_pr" => include_str!("shaders/rpe/underwater_pr.glsl"),
    "wave_pr" => include_str!("shaders/rpe/wave_pr.glsl"),
};

pub trait UniformValue: Clone + Default {
    const UNIFORM_TYPE: UniformType;
}

impl UniformValue for f32 {
    const UNIFORM_TYPE: UniformType = UniformType::Float1;
}

impl UniformValue for Vec2 {
    const UNIFORM_TYPE: UniformType = UniformType::Float2;
}

impl UniformValue for Vec3 {
    const UNIFORM_TYPE: UniformType = UniformType::Float3;
}

impl UniformValue for i32 {
    const UNIFORM_TYPE: UniformType = UniformType::Int1;
}

impl UniformValue for Color {
    const UNIFORM_TYPE: UniformType = UniformType::Float4;
}

pub trait Uniform {
    fn uniform_pair(&self) -> (String, UniformType);
    fn set_time(&mut self, t: f64);
    fn apply(&self, material: &Material);
}

impl<T: UniformValue> Uniform for (String, T) {
    fn uniform_pair(&self) -> (String, UniformType) {
        (self.0.clone(), T::UNIFORM_TYPE)
    }

    fn set_time(&mut self, _t: f64) {}

    fn apply(&self, material: &Material) {
        material.set_uniform(&self.0, self.1.clone());
    }
}

impl<T: UniformValue + Tweenable> Uniform for (String, Anim<T>) {
    fn uniform_pair(&self) -> (String, UniformType) {
        (self.0.clone(), T::UNIFORM_TYPE)
    }

    fn set_time(&mut self, t: f64) {
        self.1.set_time(t);
    }

    fn apply(&self, material: &Material) {
        material.set_uniform(&self.0, self.1.now());
    }
}

/// 布尔参数单独实现：GLSL 的 `bool` uniform 在 GL 里按整数上传，
/// 而 `bool` 本身只有一个字节，走不了按字节大小匹配的通用分支。
impl Uniform for (String, Anim<bool>) {
    fn uniform_pair(&self) -> (String, UniformType) {
        (self.0.clone(), UniformType::Int1)
    }

    fn set_time(&mut self, t: f64) {
        self.1.set_time(t);
    }

    fn apply(&self, material: &Material) {
        material.set_uniform(&self.0, i32::from(self.1.now()));
    }
}

/// 着色器源码里写的布尔默认值（`uniform bool x; // %true%`），同样是按整数上传。
impl Uniform for (String, bool) {
    fn uniform_pair(&self) -> (String, UniformType) {
        (self.0.clone(), UniformType::Int1)
    }

    fn set_time(&mut self, _t: f64) {}

    fn apply(&self, material: &Material) {
        material.set_uniform(&self.0, i32::from(self.1));
    }
}

pub(super) struct EffectInput {
    size: (i32, i32),
    target: RenderTarget,
}

impl Drop for EffectInput {
    fn drop(&mut self) {
        self.target.delete();
    }
}

pub struct Effect {
    time_range: Range<f64>,
    t: f64,
    material: Material,
    defaults: Vec<Box<dyn Uniform>>,
    uniforms: Vec<Box<dyn Uniform>>,
    pub global: bool,
    pub(crate) chart_only: bool,
}

impl Effect {
    pub fn get_preset(name: &str) -> Option<&'static str> {
        SHADERS.get(name).copied()
    }

    pub fn get_rpe_preset(name: &str) -> Option<&'static str> {
        RPE_SHADERS.get(name).copied()
    }

    pub fn new(time_range: Range<f64>, shader: &str, uniforms: Vec<Box<dyn Uniform>>, global: bool) -> Result<Self> {
        // 默认值写在声明行的注释里（`uniform vec2 x; // %0.1, 0.2%`）。rpe-next 的谱面自带
        // 着色器允许类型前加 precision 限定符、`;` 与 `%` 之间还有别的注释内容，这两种写法
        // 都要认；认不出的类型（`mat4` 之类）直接跳过，不能让整张谱面加载失败。
        static DEF_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"uniform\s+(?:(?:lowp|mediump|highp)\s+)?(\w+)\s+(\w+)\s*;[^\r\n]*?%([^%\r\n]+)%").unwrap());
        let defaults = DEF_REGEX
            .captures_iter(shader)
            .filter_map(|caps| -> Option<Result<Box<dyn Uniform>>> {
                let type_name = caps.get(1).unwrap().as_str();
                if !matches!(type_name, "float" | "int" | "bool" | "vec2" | "vec3" | "vec4") {
                    return None;
                }
                let name = caps.get(2).unwrap().as_str().to_owned();
                let value = caps.get(3).unwrap().as_str();
                Some(
                    (|| -> Result<Box<dyn Uniform>> {
                        let numbers = || -> Result<Vec<f32>> { value.split(',').map(|it| Ok(it.trim().parse::<f32>()?)).collect() };
                        Ok(match type_name {
                            "float" => Box::new((name, value.trim().parse::<f32>()?)),
                            "int" => Box::new((name, value.trim().parse::<i32>()?)),
                            "bool" => Box::new((name, matches!(value.trim(), "true" | "1" | "1.0"))),
                            "vec2" => {
                                let (x, y) = value.split_once(',').ok_or_else(|| anyhow!("Expected x,y"))?;
                                Box::new((name, vec2(x.trim().parse()?, y.trim().parse()?)))
                            }
                            "vec3" => {
                                let values = numbers()?;
                                if values.len() != 3 {
                                    bail!("Expected x,y,z");
                                }
                                Box::new((name, vec3(values[0], values[1], values[2])))
                            }
                            "vec4" => {
                                let values = numbers()?;
                                if values.len() != 4 {
                                    bail!("Expected r,g,b,a");
                                }
                                Box::new((name, Color::new(values[0], values[1], values[2], values[3])))
                            }
                            _ => unreachable!(),
                        })
                    })(),
                )
            })
            .collect::<Result<Vec<Box<dyn Uniform>>>>()?;
        let mut ocurred_uniforms = HashSet::new();
        let mut new_uniforms = Vec::new();
        let mut add_uniform = |(name, its_type): (String, UniformType)| {
            if ocurred_uniforms.insert(name.clone()) {
                new_uniforms.push((name, its_type));
            }
        };
        for def in &defaults {
            add_uniform(def.uniform_pair());
        }
        add_uniform(("time".to_owned(), UniformType::Float1));
        add_uniform(("screenSize".to_owned(), UniformType::Float2));
        add_uniform(("UVScale".to_owned(), UniformType::Float2));
        add_uniform(("ChartInputUV".to_owned(), UniformType::Float4));
        for u in &uniforms {
            add_uniform(u.uniform_pair());
        }
        Ok(Self {
            time_range,
            t: f64::NEG_INFINITY,
            defaults,
            material: load_material(
                VERTEX_SHADER,
                shader,
                MaterialParams {
                    uniforms: new_uniforms,
                    textures: vec!["screenTexture".to_owned()],
                    ..Default::default()
                },
            )?,
            uniforms,
            global,
            chart_only: false,
        })
    }

    pub fn update(&mut self, res: &Resource) {
        let t = res.time;
        self.t = t;
        if self.time_range.contains(&t) {
            for uniform in &mut self.uniforms {
                uniform.set_time(t);
            }
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.time_range.contains(&self.t)
    }

    pub fn render(&self, res: &mut Resource, chart_rect: (i32, i32, i32, i32)) {
        if !self.is_active() {
            return;
        }
        let mut gl = unsafe { get_internal_gl() };
        gl.flush();

        for def in &self.defaults {
            def.apply(&self.material);
        }
        for uniform in &self.uniforms {
            uniform.apply(&self.material);
        }
        self.material.set_uniform("time", self.t as f32);
        // 拖影下不建 `chart_target`（谱面走 `keep_target` 那张累积缓冲）。
        // 这套演出 shader 靠 `swap()` 拿「上一帧」的画面当输入，而累积缓冲的乒乓
        // 留给整帧的残影用；在它上面 swap 会把本帧已经画好的内容换成上一帧，
        // 所以拖影下这套演出直接跳过。
        let Some(target) = res.chart_target.as_mut() else {
            return;
        };
        let (width, height) = target.dim();
        let (x, y, w, h) = chart_rect;
        let x0 = x.clamp(0, width as i32);
        let y0 = y.clamp(0, height as i32);
        let x1 = (x + w).clamp(x0, width as i32);
        let y1 = (y + h).clamp(y0, height as i32);
        let clip = (self.chart_only && !cfg!(target_arch = "wasm32")
            && (x0, y0, x1, y1) != (0, 0, width as i32, height as i32))
            .then_some((x0, height as i32 - y1, x1 - x0, y1 - y0));
        if clip.is_some_and(|(_, _, w, h)| w == 0 || h == 0) {
            return;
        }
        let chart_input = clip.filter(|_| res.config.effect_chart_input);
        target.swap();
        let mut tex = target.old().texture;
        if clip.is_some() {
            let source = super::internal_id(target.old());
            let destination = super::internal_id(target.output());
            let input = chart_input.map(|rect| {
                let size = (rect.2, rect.3);
                if res.effect_input.as_ref().is_none_or(|input| input.size != size) {
                    res.effect_input = Some(EffectInput {
                        size,
                        target: render_target(size.0 as u32, size.1 as u32),
                    });
                }
                let input = res.effect_input.as_ref().unwrap();
                tex = input.target.texture;
                (super::internal_id(input.target), (x0, y0, size.0, size.1))
            });
            // The destination may still hold a previous frame. Preserve the
            // current image outside the scissor before drawing this effect.
            let copied = unsafe {
                use miniquad::gl::*;
                glDisable(GL_SCISSOR_TEST);
                let result = super::copy_fbo(source, destination, target.dim())
                    && input.is_none_or(|(dst, rect)| super::render::copy_fbo_region(source, dst, rect));
                glEnable(GL_SCISSOR_TEST);
                result
            };
            if !copied {
                target.swap();
                return;
            }
        }
        self.material.set_texture("screenTexture", tex);
        let screen_dim = vec2(tex.width(), tex.height());
        self.material.set_uniform("screenSize", screen_dim);
        gl.quad_gl.render_pass(Some(target.output().render_pass));

        let vp = get_viewport();
        self.material.set_uniform("UVScale", vec2(vp.2 as _, vp.3 as _) / screen_dim);
        // The viewport and the copied rectangle both use framebuffer coordinates.
        // Map viewport-relative coordinates to the cropped texture's pixel grid.
        let chart_uv = chart_input.map_or(Vec4::ZERO, |(_, _, w, h)| {
            vec4(
                (vp.0 - x0) as f32 / w as f32,
                (vp.1 - y0) as f32 / h as f32,
                vp.2 as f32 / w as f32,
                vp.3 as f32 / h as f32,
            )
        });
        self.material.set_uniform("ChartInputUV", chart_uv);

        gl_use_material(self.material);
        if clip.is_some() {
            gl.quad_gl.scissor(clip);
        }
        let top = 1. / if self.global { screen_aspect() } else { res.aspect_ratio };
        draw_rectangle(-1., -top, 2., top * 2., WHITE);
        if clip.is_some() {
            gl.quad_gl.scissor(None);
        }
        gl_use_default_material();
    }
}

impl Drop for Effect {
    fn drop(&mut self) {
        self.material.delete();
    }
}

const VERTEX_SHADER: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;

varying vec2 uv;

uniform mat4 Model;
uniform mat4 Projection;
uniform vec2 UVScale;
uniform vec4 ChartInputUV;

void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    if (ChartInputUV.z > 0.0) {
        vec2 viewportUV = gl_Position.xy / gl_Position.w * 0.5 + vec2(0.5);
        uv = ChartInputUV.xy + viewportUV * ChartInputUV.zw;
    } else {
        uv = (texcoord - vec2(0.5)) * UVScale + vec2(0.5);
    }
}"#;
