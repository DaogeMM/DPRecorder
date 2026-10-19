//! 官方触摸相机在 ActiveBlock 的 SDF 之前的 CPU 栅格来源。
//!
//! Source: sharedassets12.assets 里的 `Round10_Blur4` 精灵，44x44 像素，PPU=100，
//! 双线性 / clamp；prefab size=11.5、相机正交尺寸=5。原生 Show 是零 → 尺寸、
//! Hide 是当前 → 零，历时 0.1 秒、线性夹取插值（`Animation.MoveNext` RVA 0x1D708D0）。
//! 目标触摸相机是按点采样的 R8、尺寸等于掩码分辨率。
//!
//! **移植改动**：官方素材 `TouchHover.png` 在本仓库里不存在（`Phira-Pro-main` 的
//! `prpr/assets/` 是空的）。原版在 `Default::default()` 里直接
//! `include_bytes!(...).expect("official touch hover source")`，缺文件会连编译都过不去。
//! 这里改成：编译期探测该文件，存在才 `include_bytes!`，否则退化成 1x1 全零灰度图 ——
//! 于是整套 slot 生命周期与 SDF 采样逻辑都保持原样，只是「没有手指时」永远采到 0。
//! 仓库里放的是一份自生成的近似替身，「噪域打击效果」打开时由它出光晕。

use macroquad::prelude::Vec2;

const MAX_TOUCHES: usize = 10;
const DURATION: f32 = 0.1;
const SPRITE_HEIGHT: f32 = 44.0 / 100.0 * 11.5 / 10.0;

#[derive(Clone, Copy, Default)]
struct Slot {
    finger: Option<u64>,
    seen: bool,
    center: Vec2,
    start_time: f32,
    start_scale: f32,
    end_scale: f32,
    scale: f32,
}

impl Slot {
    fn advance(&mut self, time: f32) {
        let t = ((time - self.start_time) / DURATION).clamp(0.0, 1.0);
        self.scale = self.start_scale + (self.end_scale - self.start_scale) * t;
    }

    fn show(&mut self, finger: u64, center: Vec2, time: f32) {
        // 即便一个正在消失的 slot 在 Hide 协程跑完前被复用，原生 Show 也从
        // Vector3.zero 重新开始。
        self.finger = Some(finger);
        self.center = center;
        self.seen = true;
        self.start_time = time;
        self.start_scale = 0.0;
        self.end_scale = 1.0;
        self.scale = 0.0;
    }

    fn hide(&mut self, time: f32) {
        self.finger = None;
        self.start_time = time;
        self.start_scale = self.scale;
        self.end_scale = 0.0;
    }
}

pub(super) struct TouchMask {
    slots: [Slot; MAX_TOUCHES],
    source: Vec<u8>,
    width: usize,
    height: usize,
    last_time: Option<f32>,
}

/// 官方 hover 源图。
///
/// 官方 `TouchHover.png`（`Round10_Blur4`）不在 `Phira-Pro-main` 的备份里，仓库里也找不到；
/// 而它被 `include_bytes!` 引用，缺文件连编译都过不去，所以
/// `assets/blockarea/TouchHover.png` 里放了一份**我们自己生成的近似替身**
///（44x44 白色径向衰减，见 `assets/blockarea/README.md`）。拿到官方素材直接覆盖该文件即可。
///
/// 注意：「噪域打击效果」由 [`crate::core::chart::Chart::render_block_area`] 侧传入命中点驱动，
/// 不依赖真实触摸输入；这份贴图决定光晕长什么样。
fn hover_source() -> (Vec<u8>, usize, usize) {
    let source = image::load_from_memory(include_bytes!("../../../../../../assets/blockarea/TouchHover.png"))
        .expect("official touch hover source")
        .to_rgba8();
    let (width, height) = (source.width() as usize, source.height() as usize);
    (source.into_raw(), width, height)
}

impl Default for TouchMask {
    fn default() -> Self {
        let (source, width, height) = hover_source();
        Self {
            slots: [Slot::default(); MAX_TOUCHES],
            source,
            width,
            height,
            last_time: None,
        }
    }
}

impl TouchMask {
    pub fn reset(&mut self) {
        self.slots.fill(Slot::default());
        self.last_time = None;
    }

    /// 位置用左下角为原点的屏幕 UV，尚未做 chart 局部翻转。
    /// 保留手指 ID：判定输入来自 `HashMap`，其迭代顺序在其它方面完全相同的帧之间可能变化。
    pub fn update_fingers(&mut self, touches: &[(u64, Vec2)], time: f32) {
        if !time.is_finite() {
            return;
        }
        if self.last_time.is_some_and(|previous| time < previous) {
            // 确定性重放可能回退时间轴。原生实例在初始化时重置，而不是保留「未来」的缩放。
            self.slots.fill(Slot::default());
        }
        self.last_time = Some(time);
        for slot in &mut self.slots {
            slot.advance(time);
            slot.seen = false;
        }
        for &(finger, center) in touches.iter().take(MAX_TOUCHES) {
            if !center.x.is_finite() || !center.y.is_finite() {
                continue;
            }
            if let Some(slot) = self.slots.iter_mut().find(|slot| slot.finger == Some(finger)) {
                slot.center = center;
                slot.seen = true;
            } else if let Some(slot) = self.slots.iter_mut().find(|slot| slot.finger.is_none()) {
                slot.show(finger, center, time);
            }
        }
        for slot in &mut self.slots {
            if slot.finger.is_some() && !slot.seen {
                slot.hide(time);
            }
        }
    }

    /// 给顺序稳定的确定性探针用。
    #[cfg(test)]
    pub fn update_positions(&mut self, touches: &[Vec2], time: f32) {
        let fingers: Vec<_> = touches.iter().copied().enumerate().map(|(index, point)| (index as u64, point)).collect();
        self.update_fingers(&fingers, time);
    }

    pub fn visible(&self) -> bool {
        self.slots.iter().any(|slot| slot.finger.is_some() || slot.scale > 0.0)
    }

    /// 一个原生触摸相机像素中心处的 R8 值。调用方把它按 2x2 复制进 effect 尺寸的 aux 纹理。
    pub fn sample(&self, uv: Vec2, aspect: f32) -> u8 {
        let mut result = 0.0_f32;
        for slot in &self.slots {
            if slot.scale <= 0.0 {
                continue;
            }
            let size = SPRITE_HEIGHT * slot.scale;
            let source_uv = Vec2::new((uv.x - slot.center.x) * aspect / size + 0.5, (uv.y - slot.center.y) / size + 0.5);
            if source_uv.x < 0.0 || source_uv.x > 1.0 || source_uv.y < 0.0 || source_uv.y > 1.0 {
                continue;
            }
            let (red, alpha) = self.source_sample(source_uv);
            // Sprites/Default 是 One, OneMinusSrcAlpha，并把 RGB 乘上纹理 alpha。
            // 每次混合后量化，与 R8 RT 一致。
            result = ((red * alpha + result * (1.0 - alpha)) * 255.0).round().clamp(0.0, 255.0) / 255.0;
        }
        (result * 255.0).round() as u8
    }

    fn source_sample(&self, uv: Vec2) -> (f32, f32) {
        let x = uv.x * self.width as f32 - 0.5;
        // 解码后的 PNG 行序自顶向下，原生精灵纹理 UV 是 Y 向上。
        let y = (1.0 - uv.y) * self.height as f32 - 0.5;
        let x0 = x.floor() as isize;
        let y0 = y.floor() as isize;
        let fx = x - x.floor();
        let fy = y - y.floor();
        let pixel = |x: isize, y: isize, channel: usize| {
            let x = x.clamp(0, self.width as isize - 1) as usize;
            let y = y.clamp(0, self.height as isize - 1) as usize;
            self.source[(y * self.width + x) * 4 + channel] as f32 / 255.0
        };
        let bilinear = |channel| {
            let bottom = pixel(x0, y0, channel) * (1.0 - fx) + pixel(x0 + 1, y0, channel) * fx;
            let top = pixel(x0, y0 + 1, channel) * (1.0 - fx) + pixel(x0 + 1, y0 + 1, channel) * fx;
            bottom * (1.0 - fy) + top * fy
        };
        (bilinear(0), bilinear(3))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 官方素材的具体像素断言（原版 `official_bitmap_show_and_hide_use_native_scale`、
    // `disappearing_slots_restart_from_zero_when_reused`）依赖**官方** `TouchHover.png`
    // 的准确像素值；本仓库放的只是自生成替身（44x44 白色径向衰减），
    // 所以这里断言 slot 生命周期与「show 由 0 涨到 1、Hide 由当前降到 0」的定量行为。
    #[test]
    fn slot_lifecycle_scales_from_zero_and_back() {
        let mut touch = TouchMask::default();
        let center = Vec2::new(0.5, 0.5);
        // t=0：新建 slot，scale 从 0 开始 -> 贴图还没铺开，中心采样为 0。
        touch.update_fingers(&[(42, center)], 0.0);
        assert!(touch.visible(), "手指按着，slot 已存在");
        assert_eq!(touch.slots[0].scale, 0.0);
        assert_eq!(touch.sample(center, 16.0 / 9.0), 0);
        // t=0.1：Show 跑完，scale=1，替身贴图中心接近不透明的白（双线性采样取整，不一定是 255）。
        touch.update_fingers(&[(42, center)], 0.1);
        assert_eq!(touch.slots[0].scale, 1.0);
        assert!(touch.sample(center, 16.0 / 9.0) >= 250, "中心应接近满亮");
        // 松手后 Hide 用 0.1 秒把 scale 降到 0，随后 slot 释放。
        touch.update_fingers(&[], 0.1);
        touch.update_fingers(&[], 0.15);
        assert!(touch.visible(), "还在消失中");
        touch.update_fingers(&[], 0.201);
        assert!(!touch.visible(), "Hide 结束后 slot 释放");
        assert_eq!(touch.sample(center, 16.0 / 9.0), 0);
    }

    #[test]
    fn finger_reordering_does_not_restart_show() {
        let mut touch = TouchMask::default();
        let center = Vec2::new(0.5, 0.5);
        touch.update_fingers(&[(1, center), (2, center)], 1.0);
        touch.update_fingers(&[(2, center), (1, center)], 1.1);
        assert_eq!(touch.slots[0].start_time, 1.0);
        assert_eq!(touch.slots[1].start_time, 1.0);
    }
}
