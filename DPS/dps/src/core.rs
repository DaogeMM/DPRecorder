pub use macroquad::color::Color;

pub const NOTE_WIDTH_RATIO_BASE: f64 = 0.13175016;
pub const HEIGHT_RATIO: f64 = 0.83175;

pub const EPS: f64 = 1e-5;

pub type Point = nalgebra::Point2<f32>;
pub type Vector = nalgebra::Vector2<f32>;
pub type Matrix = nalgebra::Matrix3<f32>;

mod anim;
pub use anim::{Anim, AnimFloat, AnimFloatF64, AnimVector, Keyframe};

mod block_area;
pub use block_area::simple as simple_block_area;
pub use block_area::{
    assign_layers as assign_next_layers, block_touch_blocked, collect_zones, ease as block_area_ease, group_block_areas, is_native_block_area,
    is_next_block_area, parse_block_areas, parse_next as parse_next_block_areas, render_native_block_areas, touch_inset_world, BlockArea,
    BlockAreaRenderer, BlockAreaStyle, BlockGroup, BlockPhase, NativeBlocks, NextArea, NextTempo, NoiseLayer, NoiseLayers, Zone, EASE_LINEAR, EASE_ONE,
    EASE_ZERO,
};

pub mod appendant;

mod chart;
pub use chart::{Chart, ChartExtra, ChartSettings, HitSoundMap};

pub mod clock_paradox_sp;

mod effect;
pub use effect::{Effect, Uniform};

mod line;
pub use line::{GifFrames, JudgeLine, JudgeLineCache, JudgeLineKind, TextData, UIElement};

mod note;
use macroquad::prelude::set_pc_assets_folder;
pub use note::{BadNote, HitSound, Note, NoteKind, RenderConfig};

mod object;
pub use object::{CtrlObject, Object};

pub mod chart_mod;

pub mod fixed_lane;

pub mod jingluan_sp;

pub mod message_sp;

pub mod mopemope_sp;

pub mod pure_config;

pub mod qzkago_sp;

mod render;
pub use render::{copy_fbo, internal_id, MSRenderTarget};

mod resource;
pub use resource::{DigitStyle, NoteStyle, ParticleEmitter, ResPackInfo, Resource, ResourcePack, SfxMap, BUFFER_SIZE, DPI_VALUE};

mod smooth;
pub use smooth::Smooth;

mod tween;
pub use tween::{
    easing_from, BezierTween, ClampedTween, CurveTween, GeneralIntegralTween, IntegralClampedTween, IntegralStaticTween, SpeedIntegralTween, StaticTween,
    TweenFunction, TweenId, TweenMajor, TweenMinor, Tweenable, TWEEN_FUNCTIONS,
};

#[cfg(feature = "video")]
mod video;
pub mod water;
#[cfg(feature = "video")]
pub use prpr_avc::demux_audio;
#[cfg(feature = "video")]
pub use video::{Video, VideoAttach};

pub fn init_assets() {
    if let Ok(mut exe) = std::env::current_exe() {
        while exe.pop() {
            if exe.join("assets").exists() {
                std::env::set_current_dir(exe).unwrap();
                break;
            }
        }
    }
    set_pc_assets_folder("assets");
}

#[derive(serde::Deserialize, serde::Serialize, Clone)]
pub struct Triple(i32, i32, i32);
impl Default for Triple {
    fn default() -> Self {
        Self(0, 0, 1)
    }
}

impl Triple {
    pub fn beats(&self) -> f64 {
        self.0 as f64 + self.1 as f64 / self.2 as f64
    }

    pub fn display(&self) -> String {
        format!("{}:{}/{}", self.0, self.1, self.2)
    }
}

#[derive(Default, Clone)] // the default is a dummy
pub struct BpmList {
    elements: Vec<(f64, f64, f64)>, // (beats, time, bpm)
    cursor: usize,
    // compatible pgr formatVersion
    // false: use global bpm list storage.
    // true: use per-line bpm list storage. For compatibility, f32 is still used as index here, but don't worry, treat it as int
    per_line_bpm_storage: bool,
}

impl BpmList {
    pub fn new(ranges: Vec<(f64, f64)> /*(beat, bpm)*/) -> Self {
        let mut elements = Vec::new();
        let mut time = 0.0;
        let mut last_beats = 0.0;
        let mut last_bpm: Option<f64> = None;
        for (now_beats, bpm) in ranges {
            if let Some(bpm) = last_bpm {
                time += (now_beats - last_beats) * (60. / bpm);
            }
            last_beats = now_beats;
            last_bpm = Some(bpm);
            elements.push((now_beats, time, bpm));
        }
        BpmList {
            elements,
            cursor: 0,
            per_line_bpm_storage: false,
        }
    }

    // compatible pgr formatVersion
    pub fn from_time(ranges: Vec<(f64, f64)> /*(time/index, bpm)*/) -> Self {
        let mut elements = Vec::new();
        for (time, bpm) in ranges {
            elements.push((0.0, time, bpm));
        }
        BpmList {
            elements,
            cursor: 0,
            per_line_bpm_storage: true,
        }
    }

    pub fn time_beats(&mut self, beats: f64) -> f64 {
        while let Some(kf) = self.elements.get(self.cursor + 1) {
            if kf.0 > beats {
                break;
            }
            self.cursor += 1;
        }
        while self.cursor != 0 && self.elements[self.cursor].0 > beats {
            self.cursor -= 1;
        }
        let (start_beats, time, bpm) = &self.elements[self.cursor];
        time + (beats - start_beats) * (60. / bpm)
    }

    pub fn time(&mut self, triple: &Triple) -> f64 {
        self.time_beats(triple.beats())
    }

    pub fn beat(&mut self, time: f64) -> f64 {
        while let Some(kf) = self.elements.get(self.cursor + 1) {
            if kf.1 > time {
                break;
            }
            self.cursor += 1;
        }
        while self.cursor != 0 && self.elements[self.cursor].1 > time {
            self.cursor -= 1;
        }
        let (beats, start_time, bpm) = &self.elements[self.cursor];
        beats + (time - start_time) / (60. / bpm)
    }

    pub fn now_bpm(&mut self, time: f64) -> f64 {
        while let Some(kf) = self.elements.get(self.cursor + 1) {
            if kf.1 > time {
                break;
            }
            self.cursor += 1;
        }
        while self.cursor != 0 && self.elements[self.cursor].1 > time {
            self.cursor -= 1;
        }
        self.elements[self.cursor].2
    }

    /// 取「某个判定线在某个时刻」的 BPM。
    ///
    /// 官方 PGR 格式没有全局 BPM 表：`from_time` 建的 `elements` 里，第二项存的是判定线下标
    /// 而不是秒，所以这时必须按线下标查；其余格式的 BPM 表按秒排，直接查时间轴。
    pub fn bpm_at(&mut self, time: f64, line: usize) -> f64 {
        if !self.per_line_bpm_storage {
            return self.now_bpm(time);
        }
        let line = line as f64;
        while let Some(kf) = self.elements.get(self.cursor + 1) {
            if kf.1 > line {
                break;
            }
            self.cursor += 1;
        }
        while self.cursor != 0 && self.elements[self.cursor].1 > line {
            self.cursor -= 1;
        }
        self.elements[self.cursor].2
    }

    /// 整张谱面 BPM 表的最小、最大值；没有 BPM 事件时返回 `None`。
    pub fn bpm_range(&self) -> Option<(f64, f64)> {
        let mut iter = self.elements.iter().map(|(_, _, bpm)| *bpm);
        let first = iter.next()?;
        Some(iter.fold((first, first), |(lo, hi), bpm| (lo.min(bpm), hi.max(bpm))))
    }
}

/// 把 BPM 值写成人读的文本：谱面里写了几位小数就保留几位，末尾多余的 0 去掉。
///
/// `f64` 只存二进制近似值，`format!` 默认输出「最短且能原样读回」的十进制写法，
/// 因此 `186.0` 得到 `186`、`139.50` 得到 `139.5`、`560 / 3` 得到 `186.66666666666666`。
/// 这样不会把谱面里本来就写的高精度 BPM 四舍五入掉。
pub fn format_bpm(bpm: f64) -> String {
    let mut text = format!("{bpm}");
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::format_bpm;

    #[test]
    fn format_bpm_keeps_source_precision() {
        assert_eq!(format_bpm(186.), "186");
        assert_eq!(format_bpm(139.5), "139.5");
        assert_eq!(format_bpm(232.50), "232.5");
        assert_eq!(format_bpm(0.125), "0.125");
        assert_eq!(format_bpm(186.666_666_666_666_66), "186.66666666666666");
        assert_eq!(format_bpm(0.), "0");
    }
}
