//! 矢量图标：用路径画，不依赖 `assets` 里的 png。
//!
//! 现在只有返回箭头走这里：它要跟着按钮一起淡入淡出，用贴图得额外处理透明度，
//! 直接描折线更省事。其余图标仍用 `assets` 里的 png。

use super::Ui;
use lyon::math::point;
use macroquad::prelude::{Color, Rect};

/// 用归一化坐标画一条折线，`width` 是相对 `r` 短边的线宽比例。
fn stroke(ui: &mut Ui, r: Rect, pts: &[(f32, f32)], width: f32, color: Color) {
    let at = |(x, y): (f32, f32)| point(r.x + r.w * x, r.y + r.h * y);
    let mut b = lyon::path::Path::builder();
    b.begin(at(pts[0]));
    for p in &pts[1..] {
        b.line_to(at(*p));
    }
    b.end(false);
    ui.stroke_path(&b.build(), r.w.min(r.h) * width, color);
}

/// 返回：左箭头（横杆 + 两条斜边）。
pub fn back(ui: &mut Ui, r: Rect, color: Color) {
    stroke(ui, r, &[(0.78, 0.5), (0.22, 0.5)], 0.09, color);
    stroke(ui, r, &[(0.22, 0.5), (0.47, 0.25)], 0.09, color);
    stroke(ui, r, &[(0.22, 0.5), (0.47, 0.75)], 0.09, color);
}
