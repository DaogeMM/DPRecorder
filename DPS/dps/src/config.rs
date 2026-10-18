use std::fmt;

use bitflags::bitflags;
use serde::{Deserialize, Serialize};

use crate::health::HealthConfig;

bitflags! {
    #[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq, Debug)]
    #[serde(transparent)]
    pub struct Mods: i32 {
        const AUTOPLAY = 1;
        const FLIP_X = 2;
        const FADE_OUT = 4;
        const FULL_SCREEN_JUDGE = 8;
    }
}

#[derive(Clone, Deserialize, Serialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ChallengeModeColor {
    White,
    Green,
    Blue,
    Red,
    Golden,
    #[default] 
    Rainbow,
}

impl fmt::Display for ChallengeModeColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ChallengeModeColor::White   => "white",
            ChallengeModeColor::Green   => "green",
            ChallengeModeColor::Blue    => "blue",
            ChallengeModeColor::Red     => "red",
            ChallengeModeColor::Golden  => "golden",
            ChallengeModeColor::Rainbow => "rainbow",
        };
        write!(f, "{}", s)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(rename = "adjust_time_new")]
    pub auto_tweak_offset: bool,
    pub aggressive_chart: bool,
    pub aggressive_note: bool,
    pub aggressive_particle: bool,
    pub aspect_ratio: Option<f32>,
    pub audio_buffer_size: Option<u32>,
    #[cfg(target_os = "android")]
    pub audio_compatibility: bool,
    pub challenge_color: ChallengeModeColor,
    pub challenge_rank: u32,
    pub chart_debug_line: f32,
    /// 判定线调试文本里显示哪些项：逗号分隔的名单，取值见 `core::line` 里拼那段文本的地方。
    pub chart_debug_line_fields: String,
    pub chart_debug_note: f32,
    /// 按键调试文本里显示哪些项：逗号分隔的名单，取值见 `core::note::debug_note_text`。
    pub chart_debug_note_fields: String,
    pub chart_ratio: f32,
    pub all_good: bool,
    pub all_bad: bool,
    pub double_click_to_pause: bool,
    pub fxaa: bool,
    pub interactive: bool,
    pub note_scale: f32,
    pub mods: Mods,
    pub mp_enabled: bool,
    pub mp_address: String,
    pub offline_mode: bool,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub fullscreen_mode: bool,
    pub offset: f64,
    pub particle: bool,
    pub particle_square: bool,
    pub player_name: String,
    pub player_rks: f32,
    pub res_pack_path: Option<String>,
    pub sample_count: u32,
    pub low_resolution_mode: bool,
    pub show_acc: bool,
    pub speed: f32,
    pub touch_debug: bool,
    pub volume_music: f32,
    pub volume_sfx: f32,
    pub volume_bgm: f32,
    pub watermark: String,
    pub notecolor: String,
    pub hitcolor: String,
    pub linecolor: String,
    pub roman: bool,
    pub chinese: bool,
    pub combo: String,
    pub lvl: f32,
    pub difficulty: String,
    pub diff: String,
    pub enter_animation: bool,

    // for compatibility
    pub autoplay: Option<bool>,

    pub judge_offset: f64,

    pub render_line: bool,
    pub render_line_extra: bool,
    pub render_note: bool,
    pub render_double_hint: bool,
    /// 「待判按键」：把**最近一个将要判定**的按键染成 `pending_note_color` 再画一遍。
    ///
    /// 判定时刻最靠前的那个按键每帧被记进 `Resource::pending_note_time`，噪域那一趟之后
    /// 由 `Chart::render_pending_tint` 让按键用**自己的**绘制代码再画一遍（只换颜色）——
    /// 所以旋转 / 移动时间 / 长条头尾 / 双押贴图全都和本体一致；染色压在噪域之上、不会被
    /// 噪域糊住。
    pub render_pending_note: bool,
    /// 「待判按键」染色用的 rgb（0~1），默认红色
    pub pending_note_color: [f32; 3],
    pub render_ui_pause: bool,
    pub render_ui_name: bool,
    pub render_ui_level: bool,
    pub render_ui_difficulty: bool,
    pub render_ui_score: bool,
    pub render_ui_combo: bool,
    pub render_ui_bar: bool,
    pub render_bg: bool,
    pub render_bg_dim: bool,
    /// Phigros 4.0 `blockAreaList` 剧情遮挡区域（默认开启）
    pub render_block_area: bool,
    /// 简化噪域：只把方块自己的矩形铺成纯色，不含噪点 / 描边 / 火花
    /// （`enable` 前淡红、`enable` 后红），默认关闭
    pub simplified_block_area: bool,
    /// 「简化噪域文字」：简化噪域时在每个方块正中间印上它的噪域编号（从 1 开始），
    /// `disable` 相位取下面的淡色、`enable` 相位取下面的文字色。只在 `simplified_block_area` 打开时有意义，默认关闭。
    pub simplified_block_area_text: bool,
    /// 「噪域文字颜色」：简化噪域编号在 `enable` **之后**的颜色 rgb（0~1），默认 #FF6094（粉红）
    pub simplified_block_area_text_color: [f32; 3],
    /// 「噪域文字淡色」：简化噪域编号在 `enable` **之前**的颜色 rgb（0~1），默认白
    pub simplified_block_area_text_pale_color: [f32; 3],
    /// 「噪域复用」：让噪域编号可以**复用**。一条噪域 `disappearTime` 一过就再也不会被用到，
    /// 它的编号于是立刻可以交给下一条用；只要「同一时刻存在的两条不同号」，编号总数就只等于
    /// 同时存在的峰值，而不是条目数（`example/pez/ハテ…AT.pez`：14355 条 → 2649 个噪域 → 432 个号）。
    /// 几何与相对动画相同的条目还会优先沿用同一个号，让「同一个噪域又出现一次」看起来仍是它。
    ///
    /// 画面**逐像素不变**：每个窗口照旧按自己的 4 个时间独立显隐、各自算掩码；变的只有
    /// 「简化噪域文字」印出来的号——由「谱面里的第几条」变成「复用号」，默认关闭。
    pub block_area_reuse: bool,
    /// 「强制定位」：**忽略谱面自带的 `blockAreaList`**（谱面一条噪域都没有也照样生效），
    /// 整屏铺一块噪域，并在每个按键周围挖掉一个矩形 —— 效果是「除了按键，哪里都是噪域」。
    ///
    /// 全屏那块噪域是**合成**出来的：几何 `(0,0)-(1,1)`、四个时间取 `-∞ / -∞ / +∞ / +∞`
    ///（于是永远落在 Active 相位、alpha 恒为 1），挖洞复用「判定范围让位」那条覆盖度通路
    ///（`native\mask.rs` 的 `judge_cover`），洞的位置由 `Note::render` 按每个按键当前的
    /// 变换推入 —— 所以洞会跟着判定线与音符一起动。默认关闭。
    pub block_area_force_position: bool,
    /// 「强制定位」：按键矩形的高度 = 该值 × 屏幕高度（默认 0.2 = 20% 屏高）
    pub block_area_force_height: f32,
    /// 「强制定位」：按键矩形的半宽 = 该值 × (屏幕高度 / 10)。
    /// 基准与判定范围完全一致，默认 1.9 即「和默认的判定范围一样宽」
    pub block_area_force_width: f32,
    /// 「强制定位」：把按键矩形本身也**着色画出来**（默认关闭 ⇒ 只挖洞、不画矩形）。
    /// 打开时矩形用「判定范围颜色」`judge_range_color` 与 `judge_range_alpha` 半透明填色，
    /// 用来肉眼确认矩形的位置与大小是否合适。
    pub block_area_force_show: bool,
    /// 「强制定位」：**挖洞的时间窗**（秒，默认 3）。
    ///
    /// 只有「判定时刻已经进入本值以内」的按键才挖洞：`note.time - res.time <= 本值`。
    /// 判定还早得很的按键（差值大于本值）不再挖洞，于是远端的按键会被噪域盖住 ——
    /// 「除了按键，哪里都是噪域」于是变成「除了**快要判定的**按键，哪里都是噪域」。
    /// 已经判定过、还在淡出或正在长按中的音符照旧保留洞（它们本来就在屏幕上，
    /// Hold 的长条也还在被按着）。设成 0 即「只在判定那一瞬间挖」。
    pub block_area_force_time: f32,
    /// 「强制定位」：**判定之后的延后视窗**（秒，默认 0.3）。
    ///
    /// 按键判定完以后，噪域里挖出来的那个洞还会**再停留**本值这么长时间，然后才被噪域重新
    /// 盖住。判据是 `res.time - 判定时刻 <= 本值`；Hold 的判定时刻取长条的**尾端** `end_time`
    ///（整条按住期间都算「还没判完」，洞一直留着），其它音符取 `time`。设成 0 即
    ///「判定完立刻盖回去」。
    pub block_area_force_delay: f32,
    /// 「反向强制定位」：把强制定位的那圈矩形**本身当作噪域画出来**，其余地方不再铺噪域
    ///（默认关闭）。只在 `block_area_force_position` 打开时有意义：此时不再生成那块全屏
    /// 合成噪域，屏幕上只有每个按键周围的长方形是噪域 —— 与「除了按键，哪里都是噪域」
    /// 正好相反。矩形上不印噪域编号。
    pub block_area_force_invert: bool,
    /// 「岩浆噪域」：把**完整噪域**的填充换成一张按屏幕平铺的 Minecraft 贴图 ——
    /// 还没生效（`appearTime..enableTime`）印下界岩 netherrock，生效中以及生效结束之后
    /// 印岩浆 lava（`lava.gif` 的 16 帧、每帧 160ms、全场共用同一个时钟循环播放）。
    ///
    /// 只在**没打开**「简化噪域」时生效；只贴**谱面里真实的普通噪域**（`is_subtract` 的挖洞块、
    /// 强制定位/反向强制定位合成的方块、判定范围让位的覆盖块都保持纯色）。
    /// 打开它时，「噪声斑点 `block_area_noise_blotch`」「噪域扭曲 `block_area_distort`」
    /// 「噪域火花 `block_area_spark`」会被**强制当成关闭**（只改有效值，不写回配置）——
    /// 噪声斑点会糊在贴图上、填充位移会把矩形边缘撕出锯齿、火花点阵也是多余的一层。
    /// 贴图锚定屏幕、每 1/16 屏高一格（720p 下 45px）、点采样，于是噪域移动/缩放时
    /// 贴图从它下面滑过，不会跟着变形；`block_area_alpha` 照旧控制不透明度。
    /// 素材在 `assets/Minecraft/netherrock.png` 与 `assets/Minecraft/lava.gif`（仓库根 assets，`include_bytes!` 进二进制）。
    /// 默认关闭。
    pub block_area_lava: bool,

    // ---- 「屏幕进水」（高级 → 更多）----
    /// 「屏幕进水」总开关（高级 → 更多）：整屏盖一层 Minecraft 水的贴图（在噪域图层之后、压在判定线/音符/UI 之上），并启用「按键血量」系统。默认关闭。
    pub water_screen: bool,
    /// 水层的不透明度（0~1）。默认 0.5。
    pub water_opacity: f32,
    /// 水贴图向下流动的速度，单位「屏幕高 / 秒」；0 表示静止。默认 0.15。
    pub water_flow_speed: f32,
    /// 水贴图每格的尺寸：一格高 = 屏幕高 / 这个值（贴图是方形，格宽同格高）。默认 16.（1/16 屏高）。
    pub water_cell_scale: f32,
    /// 每个按键的血量（点）。按键只要被画在屏幕里，就会按下面的速度持续掉血，归零即「死亡」。默认 1.。
    pub water_hp: f32,
    /// 「入水后多久才溺水」（秒）：按键刚被画进屏幕的这段时间里不扣血，泡够这么久之后才开始按
    /// `water_damage_interval` 的节奏扣血；离开屏幕（不在水里）会清零重新计时。默认 1.。
    pub water_drown_delay: f32,
    /// 每次扣血的血量（点）。默认 1.。
    pub water_damage: f32,
    /// 两次扣血之间的间隔（秒）：泡够 `water_drown_delay` 之后每这么多秒扣一次 `water_damage`，
    /// 同时闪红一次并播一次受伤音效。默认 1.。
    pub water_damage_interval: f32,
    /// 按键死亡后「常红 + 倾斜 + 淡出」持续多久（秒），之后彻底消失。默认 0.5。
    pub water_death_time: f32,
    /// 按键死亡瞬间喷出的白色粒子数量。默认 8.。
    pub water_death_particles: f32,
    /// 按键受伤 / 死亡音效的音量（0~1），音效随机取 assets/Minecraft/hit 里的素材。默认 0.5。
    pub water_sfx_volume: f32,
    /// 受伤 / 死亡音效的并发上限：同一时刻最多同时响几个（夹到 1~512，小数向下取整）。
    /// 默认 500.（实际上等于不限）。
    pub water_sfx_max: f32,

    /// 「噪域颜色」：简化噪域的填充颜色 rgb（0~1），默认红；
    /// `enable` **之前**用它的淡色（向白色混 60%），减算块两种颜色都取反
    pub block_area_color: [f32; 3],
    /// 「噪域颜色」：**完整（非简化）噪域**的颜色 rgb（0~1），默认红。
    ///
    /// 实现是**色相映射**：官方那套配色（填充 / 描边 / 辉光 / 未启用填充）色相全是 0（纯红），
    /// 所以只取这个颜色的色相去整体旋转它们，饱和度与明度保持官方值 ⇒ 换任何颜色都保住原版
    /// 的明暗层次。选灰阶（或保持默认红）时旋转量为 0，与改动前逐位一致
    pub block_area_normal_color: [f32; 3],
    /// 「噪域不透明度」：简化噪域填充的 alpha（0~1，默认 0.25）。< 1 时会另外补一圈合并后的
    /// 最外框，那一圈恒为 alpha = 1，不受这个值影响
    pub block_area_alpha: f32,
    /// 「噪域密度」：噪域边缘那圈像素颗粒的疏密，默认 2.5（= 1 时是上游硬编码的 `width / 8` 栅格）。
    /// 越大 ⇒ 颗粒格子越小、边缘越细越密、越贴近方块轮廓；越小 ⇒ 格子越大越粗。
    /// 落点是掩码栅格化的格边长（`native\mask.rs` 的 `Masks::cell`，`8 / density` 后夹到 2~32），
    /// 不是 shader uniform —— 上游那些 `_SDFCellSize` / `_Noise*` 在本仓库是死代码
    ///（`hoverSample()` 被特化成 0）。它同时决定描边宽度（1 格）与辉光半径（约 5 格），
    /// 完整噪域与简化噪域两种模式都生效；CPU 开销 ∝ `1 / cell²`。
    pub block_area_density: f32,
    /// 「噪域火花」：完整噪域里那层随噪声流动的火花点阵（默认开）。
    /// 关掉只归零火花那一项（active 的 `_SparkMapOpacity` / 未启用的 `uDisabledSparkOpacity`），
    /// 填充、描边、辉光与场景快照都不受影响
    pub block_area_spark: bool,
    /// 「噪声斑点」：往填充色里混的那层噪声斑点（`_DisplaceBlendIntensity`，默认开）。
    /// 关掉填充变成纯净色，填充 / 描边 / 辉光与场景快照的其余部分不受影响
    pub block_area_noise_blotch: bool,
    /// 「扭曲强度」：噪声把采样坐标推开的幅度（`_DisplaceStrength`，官方 0.15）。
    /// 0 = 完全不扭曲，越大噪域里那层插画被揉得越厉害
    pub block_area_displace_strength: f32,
    /// 「颗粒大小」：场景快照的像素颗粒尺度（`_BackgroundPixelScale`，官方 6.0，代码里夹了下限 1.0）。
    /// 越大颗粒越粗，1 = 不像素化
    pub block_area_pixel_scale: f32,
    /// 「流动速度」：噪声在噪域里流动的速度（`_DisplaceSpeed`，官方 1.5）。
    /// 越大画面越躁，0 = 完全静止
    pub block_area_displace_speed: f32,
    /// 「噪声半径」：噪声纹理的采样半径（`_NoiseRadius`，官方 0.48）
    pub block_area_noise_radius: f32,
    /// 「噪声平滑度」：噪声纹理的平滑程度（`_NoiseSmoothness`，官方 1.0），越小颗粒越硬
    pub block_area_noise_smoothness: f32,
    /// 「边缘破碎」：噪域边缘那圈锯齿的强度。合成掩码时每个纹素会按 `BlockNoise1` 噪声
    /// 把采样坐标推开，幅度是 `0.1 × 本值`（0.1 是上游硬编码），这个位移就是边缘锯齿的
    /// 唯一来源 —— 置 0 边缘会变成干净的直边矩形。默认 1.0（= 官方观感）
    pub block_area_edge_noise: f32,
    /// 「直接镂空」：简化噪域里不显示挖孔噪域（减算块），而是直接把它盖住的那片区域
    /// 还原成下面的画面，并照常生成合并最外框（默认关闭）
    pub direct_block_hollow: bool,
    /// 「噪域扭曲」：关掉后噪域不再扭曲 —— 填充位移（`_DisplaceStrength`）与边缘破碎
    /// （合成掩码的 `edge_noise`，每纹素 `0.1 × 值` 的噪声位移）一并归零，噪域只剩
    /// 「纯色 / 场景快照 × 透明度」，形状是干净的直边矩形。默认开启（= 官方观感，
    /// 打开时逐像素与改动前一致）。颗粒（`_BackgroundPixelScale`）、火花、斑点、
    /// 描边/辉光都各有开关，不受本项影响
    pub block_area_distort: bool,
    /// 判定范围：按键下方那条垂直于判定线的长条
    /// （引擎默认开启；记录器 `RenderConfig` 默认关闭，以界面 / TOML 的值为准）
    pub render_judge_range: bool,
    /// 判定范围半宽 = 该值 × (屏幕高度 / 10)（只作用于 Tap / Hold；Flick / Drag 走
    /// `judge_range_width_flick`）
    pub judge_range_width: f32,
    /// Flick / Drag 判定范围半宽 = 该值 × (屏幕高度 / 10) —— **与 `judge_range_width`
    /// 同一单位**（默认 2.1，比 Tap / Hold 的 1.9 略宽），只是两类音符分开配。
    pub judge_range_width_flick: f32,
    /// 判定范围长度 = 该值 × 屏幕宽度（世界坐标里屏幕宽度恒为 2.0，默认 5.0 即 5 倍屏宽）
    pub judge_range_length: f32,
    /// 判定前多少秒内才显示判定范围；其余时刻完全不绘制
    pub judge_range_time: f64,
    /// 判定前多少秒时淡入结束：judge_range_time → 该值 之间透明度 100% → (100 - alpha×100)%
    pub judge_range_fade_time: f64,
    /// 判定范围显示时的不透明度 alpha（0~1，1 = 完全不透明，0.1 = 很淡很透）
    pub judge_range_alpha: f32,
    /// 判定范围颜色 rgb（0~1），默认纯白
    pub judge_range_color: [f32; 3],
    /// 判定范围被噪域遮蔽：开启时判定范围与噪域重合的部分淡出、让给噪域
    /// （官方 `judge_range.glsl` 的语义 `alpha *= 1 - 噪域覆盖度`）；关闭时判定范围照常
    /// 叠在噪域之上。噪域本身没开时本项无效果。
    pub judge_range_noise_occlusion: bool,
    pub preserve_framebuffer: bool,
    pub render_extra: bool,
    /// 「纯配置谱面」：解析完谱面后把判定线拍平 —— 移动 / 旋转 / 消失 / 速度全部变成常量，
    /// 按键则保留自己在屏幕上的横向位置（见 `core::pure_config`）。谱面文件不受影响，默认关闭。
    pub pure_config_chart: bool,
    /// 「纯配置谱面」的判定线高度：`0` 是屏幕底边、`1` 是屏幕顶边、`0.5` 是正中。
    pub pure_config_chart_line_y: f32,
    /// 「纯配置谱面」的按键流速：判定线高度与长按尾端都按它算，点按 / 拖动 / 滑动固定 1。
    pub pure_config_chart_speed: f64,
    /// 「纯配置谱面 · 竖判定线」：判定时刻的角度接近竖直的按键分到屏幕左右两条竖判定线上，
    /// 横判定线只留横向的按键（见 `core::pure_config`）。默认打开。
    pub pure_config_chart_side_lines: bool,
    /// 「纯配置谱面」竖判定线距**最近那条屏幕竖边**的距离：`0.5` 是屏幕正中、`0.1` 是离边一成。
    pub pure_config_chart_side_x: f32,
    /// 「纯配置谱面」竖直判定的角度阈值：判定时刻的角度与水平方向相差多少度以内仍算横向按键。
    /// 默认 45，也就是 -45~45 与 135~225 留在横判定线上。
    pub pure_config_chart_angle: f32,
    /// 「纯配置谱面 · 按键噪域」：判定时刻落在谱面噪域里的按键，渲染时在它周围长出一块噪域矩形
    /// （与「反向强制定位」同一个观感，但只画在被盖住的按键上）。默认打开。
    pub pure_config_chart_noise: bool,
    /// 「纯配置谱面 · 按键噪域」矩形高度：全高 = 该值 × 屏幕高度（默认 0.2 ⇒ 20% 屏高）。
    /// 与「强制定位」那一组分开，是因为这一层只画在被噪域盖住的按键上，大小常要另调。
    pub pure_config_chart_noise_height: f32,
    /// 「纯配置谱面 · 按键噪域」矩形宽度：半宽 = 该值 × 屏幕高度 / 10（与判定范围同一套基准）。
    pub pure_config_chart_noise_width: f32,
    /// 「纯配置谱面 · 按键噪域」时间窗（秒）：只有判定时刻进入本值以内的按键才长矩形。
    pub pure_config_chart_noise_time: f32,
    /// 「纯配置谱面 · 按键噪域」延后视窗（秒）：判定完成之后矩形再停留这么久才消失。
    pub pure_config_chart_noise_delay: f32,
    /// 「定轨谱面」：把所有判定线拍平成一条横向基准线，按键按判定时刻吸附到固定的轨道上
    /// （见 `core::fixed_lane`）。与「纯配置谱面」互不影响，谱面文件不受影响，默认关闭。
    pub fixed_lane_chart: bool,
    /// 「定轨谱面」相邻轨道的间隔，屏宽的几成：`0.14` 就是相邻两条相距屏幕宽度的 14%。
    pub fixed_lane_gap: f32,
    /// 「定轨谱面」按键流速：越大落得越快（判定线高度积分与长按尾端都按它算）。
    pub fixed_lane_speed: f64,
    /// 「定轨谱面」判定线高度：`0` 是屏幕底边、`1` 是屏幕顶边、`0.5` 是正中。
    pub fixed_lane_line_y: f32,
    /// 「定轨谱面」同一轨道上两个按键之间至少留几秒（长按按尾巴算）。
    pub fixed_lane_interval: f64,
    /// 「定轨谱面」是否按原谱面画长条（Hold）的长度：关掉只画头部，判定时长不受影响。
    pub fixed_lane_hold_length: bool,
    /// 「定轨谱面」是否启用单侧第 3 条轨道：关掉时常规 4 条排不下的按键会叠在最近的轨道上。
    pub fixed_lane_third: bool,
    pub bg_blurriness: f32,

    pub max_particles: usize,

    pub fade: f32,
    pub alpha_tint: bool, // note.alpha <=0.5 blue, note.alpha >0.5 red

    pub rotation_mode: bool,

    pub play_start_time: f64,
    pub play_end_time: Option<f64>,
    #[cfg(feature = "play")]
    pub shake_play_mode: bool,
    #[cfg(feature = "play")]
    pub health_mode: Option<HealthConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            #[cfg(not(feature = "play"))]
            auto_tweak_offset: false,
            #[cfg(feature = "play")]
            auto_tweak_offset: true,
            #[cfg(not(feature = "play"))]
            aggressive_chart: false,
            #[cfg(feature = "play")]
            aggressive_chart: true,
            aggressive_note: false,
            aggressive_particle: false,
            aspect_ratio: None,
            audio_buffer_size: None,
            #[cfg(target_os = "android")]
            audio_compatibility: false,
            challenge_color: ChallengeModeColor::Rainbow,
            challenge_rank: 3,
            chart_debug_line: 0.0,
            chart_debug_line_fields: "id,parent,height,ulp,z,aui,anchor,kind".to_owned(),
            chart_debug_note: 0.0,
            chart_debug_note_fields: "line,time,height,base,speed,above,fake,angle".to_owned(),
            chart_ratio: 1.0,
            all_good: false,
            all_bad: false,
            double_click_to_pause: true,
            fxaa: false,
            interactive: true,
            mods: Mods::default(),
            mp_address: "mp2.phira.cn:12345".to_owned(),
            mp_enabled: false,
            note_scale: 1.0,
            offline_mode: false,
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            fullscreen_mode: false,
            offset: 0.0,
            particle: true,
            particle_square: true,
            player_name: "Guest".to_string(),
            player_rks: 15.,
            res_pack_path: None,
            sample_count: 1,
            low_resolution_mode: false,
            show_acc: false,
            speed: 1.0,
            touch_debug: false,
            volume_music: 1.0,
            volume_sfx: 0.0,
            volume_bgm: 1.0,
            watermark: "".to_string(),
            notecolor: "".to_string(),
            hitcolor: "".to_string(),
            linecolor: "".to_string(),
            roman: false,
            chinese: false,
            combo: "RECALL".to_string(),
            lvl: 1.,
            difficulty: "".to_string(),
            diff: "".to_string(),
            enter_animation: true,

            autoplay: None,

            judge_offset: 0.,

            render_line: true,
            render_line_extra: true,
            render_note: true,
            render_double_hint: true,
            render_pending_note: false,
            pending_note_color: [1., 0.18, 0.18],
            render_ui_pause: true,
            render_ui_name: true,
            render_ui_level: true,
            render_ui_difficulty: true,
            render_ui_score: true,
            render_ui_combo: true,
            render_ui_bar: true,
            render_bg: true,
            render_bg_dim: true,
            render_block_area: true,
            simplified_block_area: false,
            simplified_block_area_text: false,
            simplified_block_area_text_color: [1., 0.3765, 0.5804],
            simplified_block_area_text_pale_color: [1., 1., 1.],
            block_area_reuse: false,
            block_area_force_position: false,
            block_area_force_height: 0.2,
            block_area_force_width: 1.9,
            block_area_force_show: false,
            block_area_force_time: 3.0,
            block_area_force_delay: 0.3,
            block_area_force_invert: false,
            block_area_lava: false,
            water_screen: false,
            water_opacity: 0.5,
            water_flow_speed: 0.15,
            water_cell_scale: 16.,
            water_hp: 1.,
            water_drown_delay: 1.,
            water_damage: 1.,
            water_damage_interval: 1.,
            water_death_time: 0.5,
            water_death_particles: 8.,
            water_sfx_volume: 0.5,
            water_sfx_max: 500.,
            block_area_color: [0.9, 0.1, 0.1],
            block_area_normal_color: [0.9, 0.1, 0.1],
            block_area_alpha: 0.25,
            block_area_density: 2.5,
            block_area_spark: true,
            block_area_noise_blotch: true,
            block_area_displace_strength: 0.15,
            block_area_pixel_scale: 6.0,
    block_area_displace_speed: 1.5,
    block_area_noise_radius: 0.48,
    block_area_noise_smoothness: 1.0,
    block_area_edge_noise: 1.0,
            direct_block_hollow: false,
            block_area_distort: true,
            render_judge_range: true,
            judge_range_width: 1.9,
            judge_range_width_flick: 2.1,
            judge_range_length: 5.,
            judge_range_time: 3.,
            judge_range_fade_time: 1.8,
            judge_range_alpha: 0.1,
            judge_range_color: [1., 1., 1.],
            judge_range_noise_occlusion: true,
            preserve_framebuffer: false,
            render_extra: true,
            pure_config_chart: false,
            pure_config_chart_line_y: 0.1,
            pure_config_chart_speed: 2.8,
            pure_config_chart_side_lines: true,
            pure_config_chart_side_x: 0.05,
            pure_config_chart_angle: 45.,
            pure_config_chart_noise: true,
            pure_config_chart_noise_height: 0.1,
            pure_config_chart_noise_width: 1.1,
            pure_config_chart_noise_time: 0.13,
            pure_config_chart_noise_delay: 0.,
            fixed_lane_chart: false,
            fixed_lane_gap: 0.14,
            fixed_lane_speed: 2.8,
            fixed_lane_line_y: 0.1,
            fixed_lane_interval: 0.2,
            fixed_lane_hold_length: true,
            fixed_lane_third: false,
            bg_blurriness: 80.,

            max_particles: 5000,

            fade: 0.,
            alpha_tint: false,

            rotation_mode: false,

            play_start_time: 0.,
            play_end_time: None,
            #[cfg(feature = "play")]
            shake_play_mode: false,
            #[cfg(feature = "play")]
            health_mode: None,
        }
    }
}

impl Config {
    pub fn init(&mut self) {
        if let Some(flag) = self.autoplay {
            self.mods.set(Mods::AUTOPLAY, flag);
        }
    }

    #[inline]
    pub fn has_mod(&self, m: Mods) -> bool {
        self.mods.contains(m)
    }

    #[inline]
    pub fn autoplay(&self) -> bool {
        self.has_mod(Mods::AUTOPLAY)
    }

    #[inline]
    pub fn flip_x(&self) -> bool {
        self.has_mod(Mods::FLIP_X)
    }

    #[inline]
    pub fn full_scrrn_judge(&self) -> bool {
        self.has_mod(Mods::FULL_SCREEN_JUDGE)
    }
}
