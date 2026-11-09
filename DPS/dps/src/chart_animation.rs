//! 谱面演出动画的默认值。
//!
//! 八套特殊演出（雪降 / 报应 / 痉挛 / QZKAGO / Clock Paradox / もぺもぺ / Message / Rr）的可调参数
//! 默认值放在 `assets/chart-animation.toml`，运行期读一次、之后一直用缓存的那份 ——
//! 所以调默认值只改这个文件、不用重新编译（改完要重启程序）。
//!
//! 查找顺序：环境变量 [`ENV_OVERRIDE`] → 开发构建的仓库路径 → 程序目录里的 `assets/`
//! → 编译时内嵌的那一份。后三级都是同一个文件的不同拷贝，最后一级保证文件丢了也能跑。
//!
//! ⚠️ 内嵌的那一份是 `include_str!` 进来的：改它会让 dps 重新编译，这是内嵌的代价；
//! 正常调值走的是运行期读到的文件，不会触发编译。

use std::path::{Path, PathBuf};

use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};

/// 覆盖默认值文件路径的环境变量
pub const ENV_OVERRIDE: &str = "DPR_CHART_ANIMATION";
/// 相对程序目录 / 仓库根的位置
const RELATIVE_PATH: &str = "assets/chart-animation.toml";
/// 编译时内嵌的一份，作兜底
const EMBEDDED: &str = include_str!("../../../assets/chart-animation.toml");

/// 从 exe 往上最多找几级目录里的 `assets/`
const EXE_WALK_UP: usize = 4;

/// 全部演出动画的默认值
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChartAnimation {
    pub xuejiang: Xuejiang,
    pub baoying: Baoying,
    pub jingluan: Jingluan,
    pub qzkago: Qzkago,
    pub clock_paradox: ClockParadox,
    pub mopemope: Mopemope,
    pub message: Message,
    pub rr: Rr,
}

/// 「雪降SP」：伪装分数、响度进度条与连击表情
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Xuejiang {
    /// 伪装谱面的音符数，分数按 `combo / 这个数 × score_total` 算
    pub fake_notes: f64,
    /// 伪装阶段进度条按音乐时间匀速走满所需的秒数
    pub fake_bar_seconds: f32,
    /// 揭开伪装的 COMBO 数
    pub burst_combo: u32,
    /// 分数倒滚窗口的倍率
    pub roll_scale: f32,
    /// 伪装阶段的难度文本
    pub diff_fake: String,
    /// 揭开伪装后的难度文本
    pub diff_real: String,
    /// 音量进度条的归一化分位（0~1）
    pub volume_percentile: f32,
    /// 音量进度条的响应指数
    pub volume_gamma: f32,
    /// 音量进度条的平滑时间常数（秒）
    pub volume_smooth: f32,
    /// 表情第一次判定的时刻（秒）
    pub face_hit_start: f64,
    /// 表情轮次的间隔（秒）
    pub face_period: f64,
    /// 表情轮数，0 表示不显示
    pub face_rounds: u32,
    /// 每轮从「展示」到判定的提前量（秒）
    pub face_lead_show: f64,
    /// 每轮从「开始滚动」到判定的提前量（秒）
    pub face_lead_roll: f64,
    /// 音符刚出现时的表情
    pub face_text_show: String,
    /// 音符开始滚动时的表情
    pub face_text_roll: String,
    /// 音符被判定时的表情
    pub face_text_hit: String,
}

/// 「报应SP」：手绘贴图界面的排版
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Baoying {
    /// 左上角 WOW! 贴图
    pub wow_height: f32,
    pub wow_offset_x: f32,
    pub wow_offset_y: f32,
    /// 左下角曲名贴图
    pub title_height: f32,
    pub title_offset_x: f32,
    pub title_offset_y: f32,
    /// 右上角分数
    pub score_offset_x: f32,
    pub score_offset_y: f32,
    pub score_height: f32,
    pub score_gap: f32,
    pub score_squeeze: f32,
    /// 中上连击数字
    pub combo_offset_x: f32,
    pub combo_offset_y: f32,
    pub combo_height: f32,
    pub combo_gap: f32,
    pub combo_squeeze: f32,
    /// 连击数字下方的 `COMBO` 贴图
    pub combo_label_height: f32,
    pub combo_label_offset_x: f32,
    pub combo_label_offset: f32,
    /// 右下角 `SP LV.?` 贴图
    pub diff_height: f32,
    pub diff_gap_sp_lv: f32,
    pub diff_gap_lv_question: f32,
    pub diff_squeeze: f32,
    pub diff_offset_x: f32,
    pub diff_offset_y: f32,
    /// 分数与连击数从右往左排
    pub digits_rtl: bool,
    /// 打击特效的贴图缩放覆盖（小于等于 0 沿用资源包）
    pub hit_fx_scale: f32,
    /// 打击特效是否随判定线旋转
    pub hit_fx_rotate: bool,
    /// 打击特效粒子数，0 表示不画
    pub particle_count: u32,
}

/// 「痉挛SP」：三段插画
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Jingluan {
    /// 第一段（`SpasmodicChan`）
    pub chan_height: f32,
    pub chan_offset_y: f32,
    pub chan_fade_in: f32,
    pub chan_fade_out: f32,
    /// 第二段（「光」）
    pub guang_height: f32,
    pub guang_offset_y: f32,
    pub guang_fade_in: f32,
    pub guang_fade_out: f32,
    /// 第三段（「もぺもぺ」）
    pub mopemope_height: f32,
    pub mopemope_offset_y: f32,
    pub mopemope_fade_in: f32,
    pub mopemope_fade_out: f32,
    /// 插画压暗程度（0 不压暗，1 全黑）
    pub dim: f32,
}

/// 「QZKAGO」：整屏静图
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Qzkago {
    /// 整段起点（秒）
    pub start: f64,
    /// 白屏渐入时长（秒），0 就是硬切
    pub white_in: f64,
    /// 白屏渐出时长（秒），整段总长跟着它走
    pub white_out: f64,
}

/// 「Clock Paradox」：正中白线大表盘
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClockParadox {
    /// 表盘大小倍率
    pub scale: f32,
    /// 圆心的水平偏移（像素，按 1920×1080 计）
    pub offset_x: f32,
    /// 圆心的垂直偏移（像素，正值向上）
    pub offset_y: f32,
    /// 第 1~4 次出现的亮起时刻（秒）
    pub flash1_on: f64,
    pub flash2_on: f64,
    pub flash3_on: f64,
    pub flash4_on: f64,
    /// 第 1~4 次出现开始淡出的时刻（秒）
    pub flash1_fade: f64,
    pub flash2_fade: f64,
    pub flash3_fade: f64,
    pub flash4_fade: f64,
}

/// 「もぺもぺSP」：整屏乘一层红
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mopemope {
    /// 用在「闪」上的红（默认 #694C24），文件里写 `#RRGGBB`，旧的三个 0~1 分量也认
    #[serde(with = "color_hex")]
    pub color_a: [f32; 3],
    /// 用在长镜头上的红（默认 #573B0F），写法同 `color_a`
    #[serde(with = "color_hex")]
    pub color_b: [f32; 3],
}

/// 「Message SP」：七趟整屏后处理 + 方块粒子
///
/// 每项都只是「初始值 / 固定模式下的取值」：时间轴模式打开时，随时间变化的那些量由
/// `core::message_sp` 里的关键帧决定，这里只提供不随时间的部分（以及四个整体倍率）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Message {
    /// 随时间变化的量走参考时间轴（关掉就全部用本结构的固定值）
    pub timeline: bool,
    /// 整体平移的秒数（正值 = 演出比音乐晚出现）
    pub time_offset: f64,
    /// 背景图藏掉的时刻（0 表示不藏）
    pub bg_hide_time: f64,
    pub pass_kaleido: bool,
    pub pass_vignette: bool,
    pub pass_glitch: bool,
    pub pass_band: bool,
    pub pass_lens: bool,
    pub pass_esc: bool,
    pub pass_bloom: bool,
    pub pass_particles: bool,
    /// 暗角：中心到边缘的过渡宽度
    pub vignette_smoothness: f32,
    /// 暗角：开始压暗的半径（负值 = 整屏压黑）
    pub vignette_radius: f32,
    /// 暗角：压暗强度
    pub vignette_darkness: f32,
    /// 暗角：噪点亮度
    pub vignette_noise: f32,
    /// 暗角：细横线亮度
    pub vignette_line: f32,
    /// 暗角：时间轴模式的整体倍率
    pub vignette_strength: f32,
    /// 镜头畸变：缩放半径（越大越弱）
    pub lens_size: f32,
    /// 镜头畸变：收敛量（负值向内缩）
    pub lens_convergence: f32,
    /// 镜头畸变：边缘羽化宽度
    pub lens_edge: f32,
    /// 镜头畸变：横向宽高比
    pub lens_aspect: f32,
    /// 故障：位移幅度
    pub glitch_range: f32,
    /// 故障：时间轴模式的整体倍率
    pub glitch_strength: f32,
    /// 光带：带宽（1 = 整屏反相）
    pub band_width: f32,
    /// 光带：带边羽化宽度
    pub band_smoothness: f32,
    /// 光带：亮度
    pub band_brightness: f32,
    /// 光带：true 时带是竖的
    pub band_vertical: bool,
    /// 万花筒：混合比例
    pub kaleido_mix: f32,
    /// 万花筒：时间轴模式的整体倍率
    pub kaleido_strength: f32,
    /// 万花筒：彩色贴图的对比度
    pub kaleido_contrast: f32,
    /// 万花筒：彩色贴图的饱和度
    pub kaleido_saturation: f32,
    /// 万花筒：色相偏移（0~1 是一整圈）
    pub kaleido_hue: f32,
    /// 万花筒：中心蒙板半径
    pub kaleido_center: f32,
    /// 万花筒：中心蒙板羽化宽度
    pub kaleido_smoothness: f32,
    /// 曝光：亮度
    pub exposure: f32,
    /// 曝光：饱和度
    pub saturation: f32,
    /// 曝光：对比度
    pub contrast: f32,
    /// 泛光：亮度（0 = 不跑这一趟）
    pub bloom_brightness: f32,
    /// 泛光：时间轴模式的整体倍率
    pub bloom_strength: f32,
    /// 泛光：合成强度
    pub bloom_intensity: f32,
    /// 泛光：亮部提取阈值
    pub bloom_threshold: f32,
    /// 泛光：采样半径
    pub bloom_radius: f32,
    /// 方块粒子：出场时刻（秒）
    pub particle_start: f64,
    /// 方块粒子：每个方块的存活秒数
    pub particle_lifetime: f64,
    /// 方块粒子：每层同时存在的个数
    pub particle_count: u32,
    /// 方块粒子：边长（按画面高度的一半计）
    pub particle_size: f32,
    /// 方块粒子：上飘速度（同上，每秒）
    pub particle_speed: f32,
    /// 方块粒子：这个高度以上的部分淡出（0 = 底边、1 = 顶边）
    pub particle_hide_y: f32,
    /// 方块粒子：淡出边界的羽化宽度
    pub particle_hide_smoothness: f32,
    /// 方块粒子：是否加一圈低透明光晕
    pub particle_glow: bool,
    /// 方块粒子：整体透明度
    pub particle_opacity: f32,
}

/// 「Rr」：打击特效
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rr {    /// 颜色覆盖（`#RRGGBB`），留空则用全局 `hitcolor`、再没有用资源包 AP 色
    pub hit_fx_color: String,
    /// 大小倍率（乘在资源包的 `hitFxScale` 上）
    pub hit_fx_scale: f32,
    /// 动画速度倍率
    pub hit_fx_speed: f32,
}

/// 颜色字段的读写：文件里写 `#RRGGBB`（与配置页、谱面 config.toml 的写法一致），
/// 读的时候仍然认旧文件那种三个 0~1 的分量。
mod color_hex {
    use serde::{de, Deserialize, Deserializer, Serializer};

    /// 三个 0~1 的分量写成大写色号；取整口径与配置页一致（`0.9` 的 f32 是 0.89999998，
    /// 乘 255 得 229.49999，加一点余量才取成 230）。
    pub(super) fn to_hex(rgb: &[f32; 3]) -> String {
        let mut out = String::from("#");
        for it in rgb {
            let byte = ((*it as f64) * 255. + 1e-3).round().clamp(0., 255.) as u8;
            out.push_str(&format!("{byte:02X}"));
        }
        out
    }

    pub(super) fn from_hex(text: &str) -> Option<[f32; 3]> {
        let body = text.strip_prefix('#').unwrap_or(text);
        if body.len() != 6 || !body.is_ascii() {
            return None;
        }
        let digits = u32::from_str_radix(body, 16).ok()?;
        Some([
            ((digits >> 16) & 0xFF) as f32 / 255.,
            ((digits >> 8) & 0xFF) as f32 / 255.,
            (digits & 0xFF) as f32 / 255.,
        ])
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Hex(String),
        Rgb([f32; 3]),
    }

    impl Repr {
        fn into_rgb<E: de::Error>(self) -> Result<[f32; 3], E> {
            match self {
                Repr::Rgb(rgb) => Ok(rgb),
                Repr::Hex(text) => from_hex(&text).ok_or_else(|| {
                    E::invalid_value(de::Unexpected::Str(&text), &"`#RRGGBB` 或三个 0~1 的数")
                }),
            }
        }
    }

    pub(super) fn serialize<S: Serializer>(rgb: &[f32; 3], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&to_hex(rgb))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[f32; 3], D::Error> {
        Repr::deserialize(deserializer).and_then(Repr::into_rgb)
    }
}

static DEFAULTS: OnceCell<ChartAnimation> = OnceCell::new();

/// 演出动画默认值（进程序后只读一次；改文件要重启程序才生效）
pub fn defaults() -> &'static ChartAnimation {
    DEFAULTS.get_or_init(|| {
        source_path()
            .and_then(|path| load(&path))
            .unwrap_or_else(embedded)
    })
}

/// 实际读的是哪个文件；一个都没找到（用的是内嵌那份）时为 `None`
pub fn source_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(ENV_OVERRIDE).map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }
    // 开发构建直接读仓库里的那份：cargo 从仓库根的子目录里跑，绝对路径是稳的
    if cfg!(debug_assertions) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(RELATIVE_PATH);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(path) = walk_up(&exe) {
            return Some(path);
        }
    }
    let cwd = std::env::current_dir().ok()?;
    let path = cwd.join(RELATIVE_PATH);
    path.is_file().then_some(path)
}

/// 从 `exe` 所在的目录往上找 `assets/chart-animation.toml`
fn walk_up(exe: &Path) -> Option<PathBuf> {
    let mut dir = exe.parent()?;
    for _ in 0..=EXE_WALK_UP {
        let path = dir.join(RELATIVE_PATH);
        if path.is_file() {
            return Some(path);
        }
        dir = dir.parent()?;
    }
    None
}

fn load(path: &Path) -> Option<ChartAnimation> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("读取谱面演出动画默认值 {} 失败：{err}", path.display());
            return None;
        }
    };
    match parse(&text) {
        Ok(value) => Some(value),
        Err(err) => {
            eprintln!(
                "解析谱面演出动画默认值 {} 失败，改用程序内嵌的一份：{err}",
                path.display()
            );
            None
        }
    }
}

fn parse(text: &str) -> Result<ChartAnimation, toml::de::Error> {
    toml::from_str(text)
}

fn embedded() -> ChartAnimation {
    parse(EMBEDDED).expect("内嵌的 assets/chart-animation.toml 无法解析")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 内嵌的那份必须是完整的：少一个键、多一个键都会解析失败
    #[test]
    fn embedded_file_parses() {
        embedded();
    }

    #[test]
    fn values_match_the_shipped_defaults() {
        let v = embedded();
        assert_eq!(v.xuejiang.fake_notes, 305.0);
        assert_eq!(v.xuejiang.burst_combo, 240);
        assert_eq!(v.xuejiang.face_text_hit, "Ｏ(≧▽≦)Ｏ");
        assert_eq!(v.baoying.particle_count, 0);
        assert!(v.baoying.digits_rtl);
        assert_eq!(v.jingluan.dim, 0.18);
        assert_eq!(v.qzkago.white_out, 0.5);
        assert_eq!(v.clock_paradox.flash4_fade, 145.077);
        assert_eq!(color_hex::to_hex(&v.mopemope.color_a), "#694C24");
        assert_eq!(color_hex::to_hex(&v.mopemope.color_b), "#573B0F");
        assert!(v.message.timeline);
        assert_eq!(v.message.bg_hide_time, 109.0);
        assert_eq!(v.message.particle_start, 114.538);
        assert_eq!(v.message.particle_count, 90);
        assert_eq!(v.rr.hit_fx_color, "#FBA373");
    }

    /// 颜色两种写法都认：`#RRGGBB` 与旧文件里的三个 0~1 分量落成同一份颜色。
    #[test]
    fn colors_read_from_hex_and_from_floats() {
        use super::color_hex;

        #[derive(serde::Deserialize)]
        struct Wrap {
            #[serde(with = "color_hex")]
            color: [f32; 3],
        }

        let hex: Wrap = serde_json::from_str(r##"{"color":"#694C24"}"##).unwrap();
        assert_eq!(color_hex::to_hex(&hex.color), "#694C24");
        let floats: Wrap = serde_json::from_str(r#"{"color":[0.4118,0.298,0.1412]}"#).unwrap();
        assert_eq!(color_hex::to_hex(&floats.color), "#694C24");
        assert!(serde_json::from_str::<Wrap>(r#"{"color":"深红"}"#).is_err());
    }
}
