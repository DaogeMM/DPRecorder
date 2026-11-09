//! 界面配色与圆角：取自 DPRecorder 的 DeepDark 主题（`DPR/src/design/tokens.css`）。
//!
//! 两边的字体是同一份 `assets/font.ttf`，所以这里只管颜色与几何；改主题色时对着
//! DPR 那份令牌改，两边才不会又走散。
//!
//! 长度单位与界面坐标一致：屏幕高 1.0（1280×720 的窗口下 1 个单位 = 1280 px），
//! 于是 DPR 的 8 px 圆角是 `0.00625`、1 px 描边是 `0.00078125`。

use macroquad::prelude::Color;

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::new(r as f32 / 255., g as f32 / 255., b as f32 / 255., 1.)
}

/// 页面底色（DPR `--phi-bg-page`，bluish-875 `#232324`）。
pub const BG_PAGE: Color = rgb(0x15, 0x15, 0x17);
/// 更暗的一层，用于弹窗背后压暗（DPR `--phi-bg-base`，bluish-950 `#151517`）。
pub const BG_BASE: Color = rgb(0x0f, 0x0f, 0x11);
/// 面板 / 卡片（DPR 卡片实测 `#1e1e20`：在 `#151517` 上叠约 4% 白）。
pub const SURFACE_1: Color = rgb(0x1e, 0x1e, 0x20);
/// 抬一层：分组标题条、选中项（DPR 实测 `#272729`）。
pub const SURFACE_2: Color = rgb(0x27, 0x27, 0x29);
/// 面板上的控件底色（DPR `--phi-input-major`）：和卡片同色，靠描边分界。
pub const INPUT: Color = rgb(0x1e, 0x1e, 0x20);
/// 侧栏底色（DPR `--phi-nav-fill`，bluish-900 `#1b1b1c`）。
pub const NAV: Color = rgb(0x1b, 0x1b, 0x1c);
/// 侧栏选中项底色（DPR `--phi-nav-item-active`，bluish-750 `#43454a`）。
pub const NAV_ACTIVE: Color = rgb(0x43, 0x45, 0x4a);

/// 最淡的 1 px 描边（DPR `--phi-border-l1`，`#ffffff0f`）。
pub const BORDER_1: Color = Color::new(1., 1., 1., 0.059);
/// 控件描边（DPR `--phi-border-l2`，`#ffffff1f`）。
pub const BORDER_2: Color = Color::new(1., 1., 1., 0.122);
/// 控件外框描边（DPR `--phi-border-l3`，`#ffffff29`）。
pub const BORDER_3: Color = Color::new(1., 1., 1., 0.161);
/// 滑条已填充段的灰（DPR `--phi-static-neutral-500`）。
pub const TRACK_FILL: Color = rgb(0x7f, 0x82, 0x87);
/// 滑条滑块的深灰（DPR `--phi-static-neutral-600`）。
pub const TRACK_THUMB: Color = rgb(0x54, 0x55, 0x57);
/// 悬停叠层（DPR `--phi-interactive-hover`，`#ffffff14`）。
pub const HOVER: Color = Color::new(1., 1., 1., 0.078);
/// 按下叠层（DPR `--phi-interactive-active`，`#ffffff24`）。
pub const ACTIVE: Color = Color::new(1., 1., 1., 0.141);

/// 正文（DPR `--phi-label-primary`，bluish-50 `#f9fafb`）。
pub const TEXT: Color = rgb(0xf9, 0xfa, 0xfb);
/// 次级文字（DPR `--phi-label-secondary`，bluish-300 `#cfd3d6`）。
pub const TEXT_SECONDARY: Color = rgb(0xcf, 0xd3, 0xd6);
/// 三级文字（DPR `--phi-label-tertiary`，bluish-400 `#adb2b8`）。
pub const TEXT_TERTIARY: Color = rgb(0xad, 0xb2, 0xb8);
/// 说明 / caption（DPR `--phi-label-caption`，bluish-600 `#81858c`）。
pub const TEXT_CAPTION: Color = rgb(0x81, 0x85, 0x8c);
/// 置灰（DPR `--phi-label-dimmed`，bluish-750 `#43454a`）。
pub const TEXT_DIMMED: Color = rgb(0x43, 0x45, 0x4a);

/// 强调色（DPR `--phi-accent`，brand-400 `#7aaaff`）：选中态、链接、开关。
pub const ACCENT: Color = rgb(0x7a, 0xaa, 0xff);
/// 强调色的实心填充（DPR `--phi-button-info-fill`，brand-400）。
pub const ACCENT_FILL: Color = ACCENT;
/// 强调色更实的一档（DPR `--phi-button-info-hover`，brand-500 `#4176e6`）。
pub const ACCENT_STRONG: Color = rgb(0x41, 0x76, 0xe6);
/// 白底按钮上的字（DPR `--phi-brand-text` 的反面，bluish-1000 `#0f1115`）。
pub const ON_PRIMARY: Color = rgb(0x0f, 0x11, 0x15);

/// 成功（DPR 开关勾用的 green-400 `#4ed17e`）。
pub const SUCCESS: Color = rgb(0x4e, 0xd1, 0x7e);
/// 警告（DPR `--phi-warning`，amber-500 `#f59e0b`）。
pub const WARNING: Color = rgb(0xf5, 0x9e, 0x0b);
/// 错误（DPR `--phi-danger`，red-400 `#f25a5a`）。
pub const DANGER: Color = rgb(0xf2, 0x5a, 0x5a);

/// 8 px（DPR `--phi-radius-sm`）。
pub const RADIUS_SM: f32 = 0.00625;
/// 12 px（DPR `--phi-radius-md`）。
pub const RADIUS_MD: f32 = 0.009375;
/// 16 px（DPR `--phi-radius-lg`）。
pub const RADIUS_LG: f32 = 0.0125;
/// 28 px（DPR `--phi-radius-panel`）。
pub const RADIUS_PANEL: f32 = 0.02188;

/// 1 px 描边（界面坐标下的宽度）。
pub const HAIRLINE: f32 = 0.00078;

/// 给颜色换一个透明度：淡入淡出时按 `alpha` 缩放，别直接把颜色调暗。
pub fn with_alpha(c: Color, a: f32) -> Color {
    // 乘起来而不是覆盖：`BORDER_2`、`ACTIVE` 这类颜色自带透明度，
    // 覆盖会把半透明的描边、开关底色变成实心白。
    Color::new(c.r, c.g, c.b, c.a * a)
}
