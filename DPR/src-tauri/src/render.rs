// Prevents additional console window on Windows in release, DO NOT REMOVE!!
dps::tl_file!("render");

use crate::{
    common::{get_output_dir, parse_args, read_config, test_output_dir},
    ipc::IPCEvent,
    task::generate_filename,
    ASSET_PATH,
};
use anyhow::{bail, Context, Result};
use macroquad::{miniquad::gl::*, prelude::*};
use ndarray::{s, Array1};
use dps::{
    config::{ChallengeModeColor, Config, Mods},
    core::{internal_id, HitSound, MSRenderTarget, Note, ResourcePack},
    ext::{NotNanExt, SafeTexture, BLACK_TEXTURE},
    fs::{self, FileSystem},
    info::ChartInfo,
    scene::{game::WAIT_TIME, BasicPlayer, EndingScene, GameMode, GameScene, LoadingScene},
    time::TimeManager,
    ui::{FontArc, TextPainter},
    Main,
};
use rustc_hash::FxHashMap;
use sasa::AudioClip;
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    cmp::Ordering,
    io::{BufRead, BufWriter, Write},
    ops::DerefMut,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering as AtomicOrdering},
    sync::Arc,
    time::{Duration, Instant},
};
use std::{ffi::OsStr, fmt::Write as _};
use tempfile::NamedTempFile;

/// 渲染配置：GUI、`config.toml`(CLI)、预设文件共用这一个结构。
///
/// ---------- 新增 / 修改一个渲染参数时按这个顺序改（漏一处就会出现「界面有、渲染不认」或反之） ----------
/// 1. 本结构体加字段（snake_case）+ 一句 `///` 中文注释写清含义与默认值。
///    序列化看下面两个属性：`rename_all = "camelCase"` ⇒ JSON / TOML 里一律用 camelCase；
///    `default` ⇒ 旧预设 / 旧 config.toml 缺这个键时自动取 `impl Default`，不会反序列化失败。
/// 2. `to_config()` 把它透传给 DPS 的 `dps::config::Config`（只有渲染真的要用它时才需要动这里，
///    此时 DPS 侧还要另加字段并在 `DPS/dps/src/core/` 里读取）。
/// 3. `impl Default for RenderConfig` 加默认值 —— **必须和前端 `src/model.ts` 的 `DEFAULT_RENDER_CONFIG` 一致**，
///    否则「界面显示的默认值」和「命令行直接渲染的默认值」会对不上。
/// 4. `config.toml` 加一行 `camelCaseKey = 值   # 中文说明`。注意只有 CLI 不带 `--config` 时才读 toml，
///    且 dev 时实际被读的是 exe 同目录那份 `src-tauri/target/debug/config.toml`
///    （见本文件下面 `current_exe()?.parent().unwrap().join("config.toml")` 那段），两份都要同步。
///    GUI 渲染不走 toml：前端 `buildConfig()` 把 JSON 直接发给 `post_render`，所以 GUI 的默认值由 model.ts 决定。
/// 5. 前端三处 + i18n + 上下级 `:disabled` 的写法，见 `src/model.ts` 里 `RenderConfig` 接口上方的注释
///    （`ConfigView.vue` 的 ref / `buildConfig()` / `applyConfig()` / 模板控件 / 两个语言的 config.json /
///    「高级页: 功能之间的先后关系」那段注释里登记层级）。
/// 6. 改完跑 `& .\node_modules\.bin\vue-tsc.CMD --noEmit -p tsconfig.app.json --composite false`
///    （本仓库没有测试套件）。
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct RenderConfig {
    pub resolution: (u32, u32),
    pub ending_length: f64,
    pub render_loading: bool,
    pub hires: bool,
    pub chart_debug_line: f32,
    /// 判定线调试文本里显示哪些项：逗号分隔的名单，由 dps 侧拼文本时读取。
    pub chart_debug_line_fields: String,
    pub chart_debug_note: f32,
    /// 按键调试文本显示哪些项（逗号分隔，见 dps 的 `core::note::debug_note_text`）。
    pub chart_debug_note_fields: String,
    pub chart_ratio: f32,
    pub all_good: bool,
    pub all_bad: bool,
    pub fps: u32,
    pub hardware_accel: bool,
    /// 高性能模式（默认关闭）：让渲染进程尽量把帧数据留在 GPU 侧。
    /// 只影响「怎么跑渲染」，不改画面 —— 所以不进 `to_config()`、引擎侧也不需要对应字段。
    /// 目前做两件事：① Windows 下给进程设高能显卡偏好（`NvOptimusEnablement` /
    /// `AmdPowerXpressRequestHighPerformance`，双显卡机器上让 GL 上下文落在独显而不是核显）；
    /// ② 帧读回时直接在 `glReadPixels` 里上下翻转，省掉 ffmpeg 那一整趟 CPU 的 `-vf vflip`
    ///    （只在硬件编码器下启用，见 `mark_readback_flip`）。关掉时一切与从前逐字节一致。
    ///
    /// **勾选框不在渲染页**：它住在设置页，值存进 app 配置（`common::AppConfig`），
    /// 提交渲染时由 `lib.rs` 的 `post_render` / `preview_play` 现读现注入这个字段。
    /// 这里留着字段是为了 CLI（`dpr --render` + JSON 配置）仍能直接开关它。
    pub high_performance: bool,
    pub hevc: bool,
    pub mpeg4: bool,
    pub custom_encoder: Option<String>,
    pub dynamic_bitrate_control: bool,
    pub bitrate: String,

    pub aggressive_chart: bool,
    pub aggressive_note: bool,
    pub aggressive_particle: bool,
    pub challenge_color: ChallengeModeColor,
    pub challenge_rank: u32,
    pub note_scale: f32,
    //pub offset: f32,
    pub particle: bool,
    pub particle_square: bool,
    pub player_avatar: Option<String>,
    pub player_name: String,
    pub player_rks: f32,
    pub sample_count: u32,
    pub fxaa: bool,
    pub res_pack_path: Option<String>,
    pub speed: f32,
    pub volume_music: f32,
    pub volume_sfx: f32,
    pub force_limit: bool,
    pub limit_threshold: f32,
    pub loudness_equalization: bool,
    pub audio_mix_optimization: bool,
    pub watermark: String,
    pub notecolor: String,
    pub hitcolor: String,
    pub linecolor: String,
    pub roman: bool,
    pub chinese: bool,
    pub combo: String,
    pub lvl: f32,
    pub diff: String,
    pub difficulty: String,
    pub judge_offset: f64,
    pub file_name_format: String,

    pub render_line: bool,
    pub render_line_extra: bool,
    pub render_note: bool,
    pub render_double_hint: bool,
    /// 待判按键：把最近一个将要判定的按键染成 `pending_note_color` 再画一遍，画在噪域之上。
    pub render_pending_note: bool,
    /// 待判按键染色用的 rgb（0~1），默认红色
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
    /// 「背景暗淡」：黑色遮罩的不透明度（0~1）。作为渲染参数时会**覆盖**谱面信息里的同名值。
    pub background_dim: f32,

    // ---------------------- 谱面侧渲染参数 ----------------------
    // 这些键与 `dps::info::ChartInfo` 同名；谱面信息自身的值存不下来（改一次就随谱面走），
    // 所以在这里单独存一份。现在它们是配置项：`ConfigView` 的 `buildConfig()` 会把它们发出去，`RenderView` 的
    // `buildParams()` 再把它们并进发给后端的 `info`（**配置值优先** ⇒ 导入谱面不再覆盖它们）。
    // 因此这里只负责**存下来**（预设 / config.toml / 渲染页工作配置），Rust 侧不再做覆盖。
    /// 「强制宽高比」：按 `aspect_ratio` 强制画面比例；关闭时只保证画面不超过该比例
    pub force_aspect_ratio: bool,
    /// 「折叠式入场」：按键的入场动画改成折叠式
    pub fold_animation: bool,
    /// 「画面宽高比」：宽 ÷ 高（默认 16:9 ≈ 1.7777778）
    pub aspect_ratio: f32,
    /// 「音符统一缩放」
    pub note_uniform_scale: bool,
    /// 「长按部分遮挡」
    pub hold_partial_cover: bool,
    /// 「长按反向遮挡」（负长度长按）
    pub negative_length_hold: bool,
    /// 「长按打击特效间隔倍率」（默认 1）
    pub hold_particle_interval_ratio: f32,
    /// 「判定线长度」（默认 6）
    pub line_length: f32,
    /// 「总分」（默认 1000000）
    pub score_total: u32,
    /// 「预览开始时间」（秒，默认 0）
    pub preview_start: f64,
    /// 「预览结束时间」（秒，默认 0 = 到谱面结尾）
    pub preview_end: f64,
    /// 「谱面偏移」（**秒**，界面上按毫秒编辑，默认 0）；渲染时与谱面文件自身的偏移相加
    pub offset: f64,

    pub render_block_area: bool,
    pub simplified_block_area: bool,
    /// 「简化噪域文字」：简化噪域时把噪域编号印在每个方块正中心
    pub simplified_block_area_text: bool,
    /// 「噪域优化」：把几何与动画形状相同的噪域条目认成**同一个噪域的多次出现**，编号共享
    pub block_area_reuse: bool,
    /// 「强制定位」：忽略谱面自带的噪域，整屏铺一块噪域，并在每个按键周围挖掉一个矩形
    pub block_area_force_position: bool,
    /// 「强制定位」：按键矩形的高度 = 该值 × 屏幕高度（默认 0.2 = 20% 屏高）
    pub block_area_force_height: f32,
    /// 「强制定位」：按键矩形的半宽 = 该值 × (屏幕高度 / 10)，基准与判定范围一致
    pub block_area_force_width: f32,
    /// 「强制定位」：把矩形本身也着色画出来（默认只挖洞）
    pub block_area_force_show: bool,
    /// 「强制定位」：挖洞的时间窗（秒，默认 3）。只有判定时刻进入本值以内的按键才挖洞
    pub block_area_force_time: f32,
    /// 「强制定位」：判定之后的**延后视窗**（秒，默认 0.3）——判定完洞再停留这么久才被噪域盖回
    pub block_area_force_delay: f32,
    /// 「反向强制定位」：把强制定位的矩形本身当噪域画出来，其余地方不铺噪域（默认关闭）
    pub block_area_force_invert: bool,
    /// 「岩浆噪域」：把完整噪域的填充换成按屏幕平铺的 MC 贴图 —— 还没生效（appear..enable）
    /// 印 netherrock，生效中与生效结束后印 lava（16 帧、每帧 160ms 循环）。
    /// 只在**没打开**「简化噪域」时生效，且只贴谱面里真实的普通噪域；打开时会把
    /// 「噪声斑点 / 噪域扭曲 / 噪域火花」当成关闭（默认关闭）
    pub block_area_lava: bool,
    /// 「屏幕进水」总开关（高级 → 更多）：整屏盖一层 Minecraft 水的贴图（在噪域图层之后、压在判定线/音符/UI 之上），并启用「按键血量」系统。默认关闭。
    pub water_screen: bool,
    /// 水层的不透明度（0~1）。默认 `0.5`。
    pub water_opacity: f32,
    /// 水贴图向下流动的速度，单位「屏幕高 / 秒」；`0` 表示静止。默认 `0.15`。
    pub water_flow_speed: f32,
    /// 水贴图每格的尺寸：一格高 = 屏幕高 / 这个值（贴图是方形，格宽同格高）。默认 `16.`（1/16 屏高）。
    pub water_cell_scale: f32,
    /// 每个按键的血量（点）。按键只要被画在屏幕里，就会按下面的速度持续掉血，归零即「死亡」。默认 `1.`。
    pub water_hp: f32,
    pub water_drown_delay: f32,
    /// 每次扣血的血量（点）。默认 `1.`。
    pub water_damage: f32,
    /// 两次扣血之间的间隔（秒）。默认 `1.`。
    pub water_damage_interval: f32,
    /// 按键死亡后「常红 + 倾斜 + 淡出」持续多久（秒），之后彻底消失。默认 `0.5`。
    pub water_death_time: f32,
    /// 按键死亡瞬间喷出的白色粒子数量。默认 `8.`。
    pub water_death_particles: f32,
    /// 按键受伤 / 死亡音效的音量（0~1）。默认 `0.5`。
    pub water_sfx_volume: f32,
    /// 受伤 / 死亡音效的并发上限（1~512）。默认 `500.`。
    pub water_sfx_max: f32,
    pub block_area_color: [f32; 3],
    /// 「噪域颜色」：普通（完整）噪域的颜色，只取它的**色相**去整体旋转官方配色
    ///（饱和度与明度保持官方值）；默认纯红 ⇒ 旋转 0 ⇒ 与改动前逐位一致
    pub block_area_normal_color: [f32; 3],
    pub simplified_block_area_text_color: [f32; 3],
    pub simplified_block_area_text_pale_color: [f32; 3],
    pub block_area_alpha: f32,
    pub block_area_density: f32,
    /// 「噪域火花」：完整噪域里那层火花点阵（默认开）
    pub block_area_spark: bool,
    pub block_area_noise_blotch: bool,
    pub block_area_displace_strength: f32,
    pub block_area_pixel_scale: f32,
    pub block_area_displace_speed: f32,
    pub block_area_noise_radius: f32,
    pub block_area_noise_smoothness: f32,
    pub block_area_edge_noise: f32,
    pub direct_block_hollow: bool,
    /// 「噪域扭曲」：关掉后噪域不扭曲（填充位移与边缘破碎归零），只剩纯色/场景快照 + 透明度
    pub block_area_distort: bool,
    pub render_judge_range: bool,
    /// Tap / Hold 判定范围半宽 = 该值 × (屏幕高度 / 10)
    pub judge_range_width: f32,
    /// Flick / Drag 判定范围半宽 = 该值 × (屏幕高度 / 10)，与 Tap / Hold 的
    /// `judge_range_width` 同一单位（默认 2.1，略宽于 1.9）
    pub judge_range_width_flick: f32,
    pub judge_range_length: f32,
    pub judge_range_time: f64,
    pub judge_range_fade_time: f64,
    pub judge_range_alpha: f32,
    pub judge_range_color: [f32; 3],
    /// 判定范围与噪域重合的部分是否让给噪域（官方行为 = 开）
    pub judge_range_noise_occlusion: bool,
    pub preserve_framebuffer: bool,
    pub render_extra: bool,
    /// 「纯配置谱面」：解析完谱面后把判定线拍平，只留下按键在屏幕上的横向位置
    /// （见 dps 的 `core::pure_config`）。谱面文件不受影响，默认关闭。
    pub pure_config_chart: bool,
    /// 「纯配置谱面」的判定线高度：`0` 是屏幕底边、`1` 是屏幕顶边、`0.5` 是正中
    pub pure_config_chart_line_y: f32,
    /// 「纯配置谱面」的按键流速：判定线高度与长按尾端都按它算
    pub pure_config_chart_speed: f64,
    /// 「纯配置谱面 · 竖判定线」：判定时刻的角度接近竖直的按键分到屏幕左右两条竖判定线上
    pub pure_config_chart_side_lines: bool,
    /// 「纯配置谱面」竖判定线距最近那条屏幕竖边的距离：`0.5` 是屏幕正中、`0.1` 是离边一成
    pub pure_config_chart_side_x: f32,
    /// 「纯配置谱面」竖直判定的角度阈值：与水平方向相差多少度以内仍算横向按键，默认 45
    pub pure_config_chart_angle: f32,
    /// 「纯配置谱面 · 按键噪域」：判定时刻落在噪域里的按键，渲染时在它周围长出一块噪域矩形
    pub pure_config_chart_noise: bool,
    /// 「纯配置谱面 · 按键噪域」矩形高度：全高 = 该值 × 屏幕高度（默认 0.2 ⇒ 20% 屏高）
    pub pure_config_chart_noise_height: f32,
    /// 「纯配置谱面 · 按键噪域」矩形宽度：半宽 = 该值 × 屏幕高度 / 10（与判定范围同一套基准）
    pub pure_config_chart_noise_width: f32,
    /// 「纯配置谱面 · 按键噪域」时间窗（秒）：判定时刻进入本值以内的按键才长矩形（默认 3）
    pub pure_config_chart_noise_time: f32,
    /// 「纯配置谱面 · 按键噪域」延后视窗（秒）：判定完成后矩形再停留这么久才消失（默认 0.3）
    pub pure_config_chart_noise_delay: f32,
    /// 「定轨谱面」：把所有判定线拍平成一条横向基准线，按键按判定时刻吸附到固定轨道
    /// （见 dps 的 `core::fixed_lane`）。与「纯配置谱面」互不影响，谱面文件不受影响，默认关闭。
    pub fixed_lane_chart: bool,
    /// 「定轨谱面」相邻轨道间隔：屏宽的几成，`0.14` 就是屏宽的 14%
    pub fixed_lane_gap: f32,
    /// 「定轨谱面」按键流速：越大落得越快
    pub fixed_lane_speed: f64,
    /// 「定轨谱面」判定线高度：`0` 是屏幕底边、`1` 是屏幕顶边、`0.5` 是正中
    pub fixed_lane_line_y: f32,
    /// 「定轨谱面」同一轨道上两个按键之间至少留几秒（长按按尾巴算）
    pub fixed_lane_interval: f64,
    /// 「定轨谱面」是否按原谱面画长条（Hold）的长度：关掉只画头部，判定时长不受影响
    pub fixed_lane_hold_length: bool,
    /// 「定轨谱面」是否启用单侧第 3 条轨道：关掉时排不下的按键会叠在最近的轨道上
    pub fixed_lane_third: bool,
    /// 「游玩分屏」：试玩时开两个窗口，主窗口放原谱面、副窗口放改过的谱面（仅对
    /// 「纯配置谱面」「定轨谱面」生效）。只影响试玩入口，不影响渲染成片。
    pub play_split_view: bool,
    pub bg_blurriness: f32,

    pub max_particles: usize,
    pub play_start_time: f64,
    pub play_end_time: Option<f64>,

    pub fade: f32,
    pub alpha_tint: bool,
}

impl RenderConfig {
    pub fn to_config(&self) -> Config {
        Config {
            aggressive_chart: self.aggressive_chart,
            aggressive_note: self.aggressive_note,
            aggressive_particle: self.aggressive_particle,
            challenge_color: self.challenge_color.clone(),
            challenge_rank: self.challenge_rank,
            enter_animation: self.render_loading,
            fxaa: self.fxaa,
            note_scale: self.note_scale,
            //offset: self.offset,
            particle: self.particle,
            particle_square: self.particle_square,
            player_name: self.player_name.clone(),
            player_rks: self.player_rks,
            sample_count: self.sample_count,
            res_pack_path: self.res_pack_path.clone(),
            speed: self.speed,
            volume_music: self.volume_music,
            volume_sfx: self.volume_sfx,
            chart_debug_line: self.chart_debug_line,
            chart_debug_line_fields: self.chart_debug_line_fields.clone(),
            chart_debug_note: self.chart_debug_note,
            chart_debug_note_fields: self.chart_debug_note_fields.clone(),
            chart_ratio: self.chart_ratio,
            all_good: self.all_good,
            all_bad: self.all_bad,
            watermark: self.watermark.clone(),
            notecolor: self.notecolor.clone(),
            hitcolor: self.hitcolor.clone(),
            linecolor: self.linecolor.clone(),
            roman: self.roman,
            chinese: self.chinese,
            combo: self.combo.clone(),
            lvl: self.lvl,
            diff: self.diff.clone(),
            difficulty: self.difficulty.clone(),
            judge_offset: self.judge_offset,

            render_line: self.render_line,
            render_line_extra: self.render_line_extra,
            render_note: self.render_note,
            render_double_hint: self.render_double_hint,
            render_pending_note: self.render_pending_note,
            pending_note_color: self.pending_note_color,
            render_ui_pause: self.render_ui_pause,
            render_ui_name: self.render_ui_name,
            render_ui_level: self.render_ui_level,
            render_ui_difficulty: self.render_ui_difficulty,
            render_ui_score: self.render_ui_score,
            render_ui_combo: self.render_ui_combo,
            render_ui_bar: self.render_ui_bar,
            render_bg: self.render_bg,
            render_bg_dim: self.render_bg_dim,
            render_block_area: self.render_block_area,
            simplified_block_area: self.simplified_block_area,
            simplified_block_area_text: self.simplified_block_area_text,
            block_area_reuse: self.block_area_reuse,
            block_area_force_position: self.block_area_force_position,
            block_area_force_height: self.block_area_force_height,
            block_area_force_width: self.block_area_force_width,
            block_area_force_show: self.block_area_force_show,
            block_area_force_time: self.block_area_force_time,
            block_area_force_delay: self.block_area_force_delay,
            block_area_force_invert: self.block_area_force_invert,
            block_area_lava: self.block_area_lava,
            water_screen: self.water_screen,
            water_opacity: self.water_opacity,
            water_flow_speed: self.water_flow_speed,
            water_cell_scale: self.water_cell_scale,
            water_hp: self.water_hp,
            water_drown_delay: self.water_drown_delay,
            water_damage: self.water_damage,
            water_damage_interval: self.water_damage_interval,
            water_death_time: self.water_death_time,
            water_death_particles: self.water_death_particles,
            water_sfx_volume: self.water_sfx_volume,
            water_sfx_max: self.water_sfx_max,
            block_area_color: self.block_area_color,
            block_area_normal_color: self.block_area_normal_color,
            simplified_block_area_text_color: self.simplified_block_area_text_color,
            simplified_block_area_text_pale_color: self.simplified_block_area_text_pale_color,
            block_area_alpha: self.block_area_alpha,
            block_area_density: self.block_area_density,
            block_area_spark: self.block_area_spark,
            block_area_noise_blotch: self.block_area_noise_blotch,
            block_area_displace_strength: self.block_area_displace_strength,
            block_area_pixel_scale: self.block_area_pixel_scale,
        block_area_displace_speed: self.block_area_displace_speed,
        block_area_noise_radius: self.block_area_noise_radius,
        block_area_noise_smoothness: self.block_area_noise_smoothness,
        block_area_edge_noise: self.block_area_edge_noise,
            direct_block_hollow: self.direct_block_hollow,
            block_area_distort: self.block_area_distort,
            render_judge_range: self.render_judge_range,
            judge_range_width: self.judge_range_width,
            judge_range_width_flick: self.judge_range_width_flick,
            judge_range_length: self.judge_range_length,
            judge_range_time: self.judge_range_time,
            judge_range_fade_time: self.judge_range_fade_time,
            judge_range_alpha: self.judge_range_alpha,
            judge_range_color: self.judge_range_color,
            judge_range_noise_occlusion: self.judge_range_noise_occlusion,
            preserve_framebuffer: self.preserve_framebuffer,
            render_extra: self.render_extra,
            pure_config_chart: self.pure_config_chart,
            pure_config_chart_line_y: self.pure_config_chart_line_y,
            pure_config_chart_speed: self.pure_config_chart_speed,
            pure_config_chart_side_lines: self.pure_config_chart_side_lines,
            pure_config_chart_side_x: self.pure_config_chart_side_x,
            pure_config_chart_angle: self.pure_config_chart_angle,
            pure_config_chart_noise: self.pure_config_chart_noise,
            pure_config_chart_noise_height: self.pure_config_chart_noise_height,
            pure_config_chart_noise_width: self.pure_config_chart_noise_width,
            pure_config_chart_noise_time: self.pure_config_chart_noise_time,
            pure_config_chart_noise_delay: self.pure_config_chart_noise_delay,
            fixed_lane_chart: self.fixed_lane_chart,
            fixed_lane_gap: self.fixed_lane_gap,
            fixed_lane_speed: self.fixed_lane_speed,
            fixed_lane_line_y: self.fixed_lane_line_y,
            fixed_lane_interval: self.fixed_lane_interval,
            fixed_lane_hold_length: self.fixed_lane_hold_length,
            fixed_lane_third: self.fixed_lane_third,
            bg_blurriness: self.bg_blurriness,

            max_particles: self.max_particles,
            play_start_time: self.play_start_time,
            play_end_time: self.play_end_time,

            fade: self.fade,
            alpha_tint: self.alpha_tint,
            ..Default::default()
        }
    }
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            resolution: (1920, 1080),
            ending_length: 0.0,
            render_loading: false,
            hires: false,
            fps: 60,
            hardware_accel: true,
            // 高性能模式默认关闭：默认渲染流程必须与从前逐字节一致（见字段上的注释）
            high_performance: false,
            hevc: false,
            mpeg4: false,
            custom_encoder: None,
            dynamic_bitrate_control: true,
            bitrate: "28".to_string(),
            // 谱面激进优化：默认关闭（与前端 DEFAULT_RENDER_CONFIG 一致）
            aggressive_chart: false,
            aggressive_note: false,
            aggressive_particle: false,
            challenge_color: ChallengeModeColor::Rainbow,
            challenge_rank: 52,
            fxaa: false,
            note_scale: 1.0,
            particle: true,
            particle_square: true,
            player_name: "DaogeMM".to_string(),
            player_rks: 17.0,
            sample_count: 8,
            res_pack_path: None,
            speed: 1.0,
            volume_music: 0.5,
            volume_sfx: 0.4,
            force_limit: true,
            limit_threshold: 0.5,
            loudness_equalization: false,
            audio_mix_optimization: true,
            chart_debug_line: 0.0,
            chart_debug_line_fields: "id,parent,height,ulp,z,aui,anchor,kind".to_owned(),
            chart_debug_note: 0.0,
            chart_debug_note_fields: "line,time,height,base,speed,above,fake,angle".to_owned(),
            chart_ratio: 1.0,
            all_good: false,
            all_bad: false,
            watermark: "".to_string(),
            notecolor: "".to_string(),
            hitcolor: "".to_string(),
            linecolor: "".to_string(),
            roman: false,
            chinese: false,
            combo: "AUTOPLAY".to_string(),
            lvl: 1.,
            diff: "".to_string(),
            difficulty: "".to_string(),
            player_avatar: None,
            judge_offset: 0.,
            file_name_format: "%date% %time% %info.name%_%level_prefix%".to_string(),

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
            background_dim: 0.1,
            // 谱面侧渲染参数：默认值与前端 DEFAULT_RENDER_CONFIG 一致。注意 dps
            // `ChartInfo::default()` 的值与这里不同
            // （force_aspect_ratio / fold_animation 默认 false、negative_length_hold 默认 true），
            // 但配置值优先，所以界面与出片以这里的值为准。
            force_aspect_ratio: true,
            fold_animation: true,
            aspect_ratio: 16. / 9.,
            note_uniform_scale: false,
            hold_partial_cover: false,
            negative_length_hold: false,
            hold_particle_interval_ratio: 1.,
            line_length: 6.,
            score_total: 1_000_000,
            preview_start: 0.,
            preview_end: 0.,
            offset: 0.,
            render_block_area: true,
            simplified_block_area: false,
            simplified_block_area_text: false,
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
            simplified_block_area_text_color: [1.0, 0.3765, 0.5804],
            simplified_block_area_text_pale_color: [1.0, 1.0, 1.0],
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
            // 判定范围：默认关闭（与前端 DEFAULT_RENDER_CONFIG、config.toml 一致）
            render_judge_range: false,
            judge_range_width: 1.9,
            // Flick / Drag 的半宽 = 该值 × (屏幕高度 / 10)，与 Tap / Hold 同一单位
            judge_range_width_flick: 2.1,
            judge_range_length: 5.0,
            // 判定范围：显现时间 1.0s、淡入结束时间 0.3s（界面上的「过渡时间」= 1.0 − 0.3 = 0.7）
            judge_range_time: 1.0,
            judge_range_fade_time: 0.3,
            judge_range_alpha: 0.1,
            judge_range_color: [1., 1., 1.],
            // 判定范围被噪域遮蔽：默认开（官方行为，与 config.toml 一致）
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
            // 定轨谱面：默认关闭（与前端 DEFAULT_RENDER_CONFIG、config.toml 一致）
            fixed_lane_chart: false,
            fixed_lane_gap: 0.14,
            fixed_lane_speed: 2.8,
            fixed_lane_line_y: 0.1,
            fixed_lane_interval: 0.2,
            fixed_lane_hold_length: true,
            fixed_lane_third: false,
            play_split_view: false,
            bg_blurriness: 80.,

            max_particles: 5000,
            play_start_time: 0.0,
            play_end_time: None,

            fade: 0.0,
            alpha_tint: false,
        }
    }
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RenderParams {
    pub path: PathBuf,
    pub info: ChartInfo,
    pub config: RenderConfig,
}

/// 高性能模式：让进程优先用高能显卡。
///
/// 这两个环境变量是 NVIDIA / AMD 的官方私有约定（Optimus / PowerXpress 的「应用级显卡偏好」），
/// 驱动在**创建 GL 上下文时**读它们 —— 所以必须在第一次碰 GL 之前调用：本函数在 `render::main()`
/// 里紧跟着读出配置就调，而渲染进程的上下文要到后面 `get_internal_gl()` 才建。
/// 值必须是字面量 `"1"`；只设 `NvOptimusEnablement` 不够，A 卡要另一个名字。
/// 没法验证是否真的切到独显（驱动不告诉我们），所以这里只设变量、不报错，
/// 真正的选卡结果由显卡驱动决定；万一没生效也只是回到核显，不会坏画面。
#[cfg(target_os = "windows")]
fn prefer_high_performance_gpu() {
    use std::os::windows::ffi::OsStrExt;

    // 直接用 kernel32 的 FFI：本 crate 的 winapi 没开 `winbase` feature 里的
    // SetEnvironmentVariableW，为一个函数去动依赖不划算（见 `Cargo.toml` 的 winapi features）。
    #[link(name = "kernel32")]
    extern "system" {
        fn SetEnvironmentVariableW(name: *const u16, value: *const u16) -> i32;
    }

    // 逐字写成 UTF-16 加 NUL；不能用 OsStr 的字节宽度假设（这里全是 ASCII 字面量）
    let wide = |s: &str| -> Vec<u16> { std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect() };
    unsafe {
        SetEnvironmentVariableW(
            wide("NvOptimusEnablement").as_ptr(),
            wide("1").as_ptr(),
        );
        SetEnvironmentVariableW(
            wide("AmdPowerXpressRequestHighPerformance").as_ptr(),
            wide("1").as_ptr(),
        );
    }
}

/// 非 Windows 平台没有这套「应用级显卡偏好」约定，什么都不做（保持同一个调用点）。
#[cfg(not(target_os = "windows"))]
fn prefer_high_performance_gpu() {}

/// 帧读回时是否已经在 GL 里做过上下翻转（＝ ffmpeg 那一趟不用再 `-vf vflip`）。
///
/// 全局只写一次、在起 ffmpeg 之前（`mark_readback_flip`），读的地方是每帧的读回；
/// `Relaxed` 足够 —— 值在一个线程里写，读回循环只关心它已经写完了。
static READBACK_FLIP: AtomicBool = AtomicBool::new(false);

/// 高性能模式下，若这次真的让 GPU 参与编码，就把上下翻转挪进 `glReadPixels`
/// （负高度读回），ffmpeg 侧去掉 `-vf vflip` —— 省掉每帧一整趟 CPU 的整帧翻转。
///
/// 只在硬件编码器下这么做：软件编码器（libx264 / libx265）的整条滤镜链都在 CPU 上，
/// 若此时改读回方向而不改 `-vf`，画面会上下颠倒。跨进程（GUI / 我们的子进程）也可能被设过，
/// 所以不用环境变量传状态。
fn mark_readback_flip(enabled: bool) {
    READBACK_FLIP.store(enabled, AtomicOrdering::Relaxed);
    if enabled {
        eprintln!("[高性能模式] 帧读回交由 GPU 翻转，省掉 ffmpeg 的 -vf vflip");
    }
}

pub async fn build_player(config: &RenderConfig) -> Result<BasicPlayer> {
    Ok(BasicPlayer {
        avatar: if let Some(path) = &config.player_avatar {
            Some(
                Texture2D::from_file_with_format(
                    &tokio::fs::read(path)
                        .await
                        .with_context(|| tl!("load-avatar-failed"))?,
                    None,
                )
                .into(),
            )
        } else {
            None
        },
        id: 0,
        rks: config.player_rks,
    })
}

fn cmd_hidden(program: impl AsRef<OsStr>) -> Command {
    let cmd = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = cmd;
        cmd.creation_flags(0x08000000);
        cmd
    }
    #[cfg(not(target_os = "windows"))]
    cmd
}

/// 把「FFmpeg 路径」那格的值换算成**可执行文件**。
///
/// 那格填的是 **ffmpeg 所在的目录**（不是 exe 本身），
/// 而渲染管线（`cmd_hidden`）与探测要的都是可执行文件本身，所以读这份配置的地方都先过这一道：
/// **给目录就补上 `ffmpeg.exe`（非 Windows 是 `ffmpeg`）**，给文件就原样用 ——
/// 老配置里存的是 exe 完整路径，照旧能跑。
pub fn resolve_ffmpeg_exe(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if path.is_dir() {
        let exe = if cfg!(target_os = "windows") {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        };
        return path.join(exe);
    }
    path.to_path_buf()
}

/// `path` 能不能当 ffmpeg 用（跑一下 `-version`）—— 目录会自动换成里面的 `ffmpeg.exe`。
pub fn test_ffmpeg(path: impl AsRef<Path>) -> bool {
    matches!(cmd_hidden(resolve_ffmpeg_exe(path)).arg("-version").output(), Ok(_))
}

pub fn find_ffmpeg() -> Result<Option<String>> {
    if let Some(ffmpeg_path) = read_config()?.ffmpeg_path {
        // 那格是「exe 所在目录」，先换算成可执行文件再验（老配置的 exe 路径原样可用）
        let ffmpeg_path = resolve_ffmpeg_exe(&ffmpeg_path);
        if test_ffmpeg(&ffmpeg_path) {
            return Ok(Some(ffmpeg_path.display().to_string()));
        }
    }
    if test_ffmpeg("ffmpeg") {
        return Ok(Some("ffmpeg".to_owned()));
    }
    let exe_dir = std::env::current_exe()?.parent().unwrap().to_owned();
    let ffmpeg = if cfg!(target_os = "windows") {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let ffmpeg = exe_dir.join(ffmpeg);
    Ok(if test_ffmpeg(&ffmpeg) {
        Some(ffmpeg.display().to_string())
    } else {
        None
    })
}

/// 设置页「FFmpeg 路径」那格显示的**目录**（那格写的是 exe 所在的文件夹，不是 exe 本身）。
/// `find_ffmpeg()` 在 PATH 上找到时只会给出裸名字 `ffmpeg`（可执行文件交给系统按 PATH 解析），
/// 裸名字再在各 PATH 目录里查一遍拿到绝对路径，最后取父目录；连绝对路径都查不到就原样返回
/// 裸名字，绝不因为没有目录就报错。
pub fn find_ffmpeg_dir() -> Result<Option<String>> {
    let Some(found) = find_ffmpeg()? else {
        return Ok(None);
    };
    // 已经带目录（用户填的目录算出来的 exe、或 exe 旁边那个）就直接用
    let full = if Path::new(&found).parent().is_some_and(|dir| !dir.as_os_str().is_empty()) {
        found
    } else {
        let file_name = if cfg!(target_os = "windows") {
            format!("{found}.exe")
        } else {
            found.clone()
        };
        let mut absolute = None;
        if let Some(path_var) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path_var) {
                let candidate = dir.join(&file_name);
                if candidate.is_file() {
                    absolute = Some(candidate);
                    break;
                }
            }
        }
        absolute
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or(found)
    };
    // `C:\…\bin\ffmpeg.exe` → `C:\…\bin`；裸名字没有父目录，就原样给它
    Ok(Some(match Path::new(&full).parent().filter(|dir| !dir.as_os_str().is_empty()) {
        Some(dir) => dir.to_string_lossy().to_string(),
        None => full,
    }))
}

pub const ENCODER_LIST_HEVC: [&str; 4] = ["hevc_nvenc", "hevc_qsv", "hevc_amf", "hevc_vaapi"];
pub const ENCODER_LIST_AVC: [&str; 4] = ["h264_nvenc", "h264_qsv", "h264_amf", "h264_vaapi"];

/// 这个编码器名字是不是「把整条编码都放在显卡上」的预置硬件编码器。
///
/// 只看名字 —— `use_global_config` 分支可能返回全局配置里手填的编码器，
/// 那种情况下**不能**假设它是硬件编码器。
fn is_hardware_encoder_name(encoder: &str) -> bool {
    ENCODER_LIST_HEVC.contains(&encoder) || ENCODER_LIST_AVC.contains(&encoder)
}

/// 这个编码器名字是不是「原生就吃 RGBA」的 NVENC。
///
/// `ffmpeg -h encoder=h264_nvenc` 的 `Supported pixel formats` 里有 `rgba` / `bgr0` /
/// `rgb0` —— 也就是说 RGB→YUV 的转换可以由 **显卡**（NVENC）自己做，不必在 CPU 上过一遍
/// swscale。1080p60 那一趟 swscale 是整条渲染里 CPU 侧最大的一块，所以高性能模式会换成
/// `-pix_fmt rgba` 把它整段省掉。qsv / amf / vaapi / libx* 都没有这个保证，一律不碰。
fn nvenc_native_rgba(encoder: &str) -> bool {
    matches!(encoder, "h264_nvenc" | "hevc_nvenc")
}

/// 真的跑一遍才敢用：拿 lavfi 的小图让这个编码器吃一次 `-pix_fmt rgba`，成功才返回 true。
///
/// 用户可以在设置里手填任意编码器（`customEncoder`），而不同 ffmpeg 构建的 NVENC 支持的
/// 输入格式并不一致；这里失败只是**退回** `-pix_fmt yuv420p`（也就是老行为），不会让渲染挂掉。
fn encoder_accepts_rgba_input(ffmpeg: &String, encoder: &str) -> bool {
    eprintln!("[高性能模式] 探测 {} 是否直接吃 RGBA…", encoder);
    let output = Command::new(ffmpeg)
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=1920x1080:rate=5:duration=1",
            "-pix_fmt",
            "rgba",
            "-c:v",
            encoder,
            "-f",
            "null",
            "-",
        ])
        .args(["-loglevel", "warning"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output();
    matches!(output, Ok(output) if output.status.success())
}

pub fn get_encoder(
    ffmpeg: &String,
    config: &RenderConfig,
    encoder_list: [&str; 4],
    use_global_config: bool,
) -> Option<String> {
    if let Some(custom_encoder) = &config.custom_encoder {
        return Some(custom_encoder.to_string());
    };

    if config.mpeg4 {
        return Some("mpeg4".to_string());
    };

    if !config.hardware_accel {
        if config.hevc {
            return Some("libx265".to_string());
        } else {
            return Some("libx264".to_string());
        }
    }

    if use_global_config {
        let global_config = read_config().unwrap_or_default();
        if let Some(encoder_avc) = global_config.encoder_avc {
            if !config.hevc && !config.mpeg4 {
                return Some(encoder_avc);
            }
        }
        if let Some(encoder_hevc) = global_config.encoder_hevc {
            if config.hevc && !config.mpeg4 {
                return Some(encoder_hevc);
            }
        }
    }

    for encoder in encoder_list {
        if test_encoder(ffmpeg, encoder) {
            return Some(encoder.to_string());
        } else {
            warn!("Encoder {} not supported", encoder);
        }
    }

    None
}

pub fn test_encoder(ffmpeg: &String, encoder: &str) -> bool {
    eprintln!("Testing encoder: {}", encoder);
    let output = Command::new(ffmpeg)
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=1920x1080:rate=5:duration=1",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            encoder,
            "-f",
            "null",
            "-",
        ])
        .args(["-loglevel", "warning"])
        // .arg("-hide_banner")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| tl!("run-ffmpeg-failed"))
        .expect("failed test encoder");
    output.status.success()
}

fn round_to_step(v: f64, step: f64) -> f64 {
    (v / step).round() * step
}

pub async fn generate_resource(
    is_cli: bool,
    generate_output: bool,
) -> Result<(
    Box<dyn FileSystem + Send + Sync>,
    PathBuf,
    RenderConfig,
    ChartInfo,
)> {
    if is_cli {
        let (args_input, args_output, args_config, args_info) =
            parse_args(std::env::args().collect());

        let config: RenderConfig = if let Some(config) = &args_config {
            match serde_json::from_str(config) {
                Ok(config_json) => {
                    eprintln!("Using config from json");
                    config_json
                }
                Err(error) => {
                    eprintln!("{}", error);
                    eprintln!("Failed to parse json. Using config from toml file");
                    toml::from_str(&std::fs::read_to_string(config)?)?
                }
            }
        } else {
            eprintln!("Using config from config.toml");
            toml::from_str(&std::fs::read_to_string(
                std::env::current_exe()?
                    .parent()
                    .unwrap()
                    .join("config.toml"),
            )?)?
        };

        let path = args_input.unwrap();

        let mut fs = fs::fs_from_file(path.as_ref())?;

        let mut info: ChartInfo = if let Some(info) = args_info {
            serde_json::from_str(&info)?
        } else {
            fs::load_info(fs.deref_mut()).await?
        };
        // 「背景暗淡」现在是渲染参数里的全局设置，覆盖谱面信息里的同名值。
        info.background_dim = config.background_dim;

        let output_path = if generate_output {
            let file_name = generate_filename(&info, &config);

            let output_path = if let Some(output_string) = args_output {
                let output_dir = PathBuf::from(output_string);
                if output_dir.extension().is_some() {
                    output_dir
                } else {
                    test_output_dir(output_dir.clone())?;
                    output_dir.join(file_name)
                }
            } else {
                get_output_dir()?.join(file_name)
            };
            eprintln!("output file: {:?}", output_path);
            output_path
        } else {
            PathBuf::new()
        };

        Ok((fs, output_path, config, info))
    } else {
        let mut stdin = std::io::stdin().lock();

        let mut line = String::new();
        stdin.read_line(&mut line)?;
        let params: RenderParams = serde_json::from_str(line.trim())?;
        let path = params.path;

        let output_path = if generate_output {
            line.clear();
            stdin.read_line(&mut line)?;
            serde_json::from_str::<PathBuf>(line.trim())?
        } else {
            PathBuf::new()
        };

        let fs = fs::fs_from_file(&path)?;

        let config = params.config;
        let info = params.info;

        Ok((fs, output_path, config, info))
    }
}

/// 把音频线性重采样到 `target_rate`（保持时长与音高；`Frame` 自带 `interpolate`）。
///
/// 资源包与曲子里的音频不一定是 48kHz（自制材质包常见 44.1kHz），而渲染管线固定按 48000
/// 合成 ⇒ 这里统一就地重采样，采样率对不上时只留一条 `warn`，不让整个渲染
/// 因为这一条音频失败。
fn resample_to(clip: &AudioClip, target_rate: u32, name: &str) -> AudioClip {
    let rate = clip.sample_rate();
    if rate == target_rate || rate == 0 {
        return clip.clone();
    }
    let frames = clip.frames();
    if frames.is_empty() {
        return AudioClip::from_raw(Vec::new(), target_rate);
    }
    warn!(
        "`{}` 的采样率为 {}Hz，已重采样到 {}Hz",
        name, rate, target_rate
    );
    let out_frames =
        ((frames.len() as f64 * target_rate as f64 / rate as f64).ceil() as usize).max(1);
    let mut out = Vec::with_capacity(out_frames);
    for i in 0..out_frames {
        let pos = i as f64 * rate as f64 / target_rate as f64;
        let i0 = (pos.floor() as usize).min(frames.len() - 1);
        let i1 = (i0 + 1).min(frames.len() - 1);
        let frac = (pos - i0 as f64) as f32;
        out.push(frames[i0].interpolate(&frames[i1], frac));
    }
    AudioClip::from_raw(out, target_rate)
}

pub async fn main(cmd: bool) -> Result<()> {
    // 渲染全程静音：引擎里的实时播放（AUTOPLAY 按键音、进水受击 / 死亡音效、BGM、结尾曲）
    // 只是在跑帧时被听见，成片的音轨全部由这个进程离线合成 / 混流 ⇒ 直接关掉实时播放。
    dps::ext::set_audio_silent(true);
    let loading_time = Instant::now();
    let (mut fs, output_path, mut config, info) = generate_resource(cmd, true).await?;
    // 高性能模式的第一件事：在碰 GL / 建窗口**之前**把高能显卡偏好在进程上设好
    // （后面 `get_internal_gl()` 才会创建 GL 上下文）。见 `prefer_high_performance_gpu`。
    if config.high_performance {
        prefer_high_performance_gpu();
    }

    set_pc_assets_folder(ASSET_PATH.get().unwrap().to_str().unwrap());
    use crate::ipc::client::*;
    let ipc = !cmd;
    let font = FontArc::try_from_vec(load_file("font.ttf").await?)?;
    let mut painter = TextPainter::new(font);
    let volume_music = std::mem::take(&mut config.volume_music);
    let volume_sfx = std::mem::take(&mut config.volume_sfx);
    let mut prpr_config = config.to_config();
    prpr_config.mods = Mods::AUTOPLAY;
    let respack_root = ASSET_PATH.get().unwrap().join("respack");
    let respack_path = config
        .res_pack_path
        .clone()
        .unwrap_or_else(|| respack_root.join("new").to_string_lossy().into_owned());
    config.res_pack_path = Some(respack_path.clone());
    prpr_config.res_pack_path = Some(respack_path);
    let Some(ffmpeg) = find_ffmpeg()? else {
        bail!("FFmpeg not found")
    };

    let pause_requested = Arc::new(AtomicBool::new(false));
    if !cmd {
        let pause_requested = Arc::clone(&pause_requested);
        let render_thread = std::thread::current();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            let mut line = String::new();
            loop {
                line.clear();
                match stdin.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => match line.trim() {
                        "pause" => pause_requested.store(true, AtomicOrdering::SeqCst),
                        "resume" => {
                            pause_requested.store(false, AtomicOrdering::SeqCst);
                            render_thread.unpark();
                        }
                        _ => {}
                    },
                    Err(_) => break,
                }
            }
        });
    }

    let (chart, format) = GameScene::load_chart(fs.deref_mut(), &info, &prpr_config)
        .await
        .with_context(|| tl!("load-chart-failed"))?;
    let res_pack = ResourcePack::from_path(config.res_pack_path.as_ref())
        .await
        .context("Failed to load resource pack")?;
    let sample_rate = 48000;
    let sample_rate_f64 = sample_rate as f64;
    let music: Result<_> = async { AudioClip::new(fs.load_file(&info.music).await?) }.await;
    let music = music.with_context(|| tl!("load-music-failed"))?;
    // 采样率不是 48kHz 的音频统一重采样到渲染管线用的 48kHz（见 `resample_to`）。
    let music = resample_to(&music, sample_rate, "music");
    let music_length = music.length();
    let music_sample_rate = music.sample_rate();
    let ending_music = resample_to(&res_pack.endings[0], sample_rate, "ending_music");
    let ending_music_sample_rate = ending_music.sample_rate();
    let sfx_click = resample_to(&res_pack.sfx_click, sample_rate, "sfx_click");
    let sfx_drag = resample_to(&res_pack.sfx_drag, sample_rate, "sfx_drag");
    let sfx_flick = resample_to(&res_pack.sfx_flick, sample_rate, "sfx_flick");
    let sfx_protect_time = if let Some(sfx_longest) = chart
        .hitsounds
        .values()
        .max_by_key(|v| v.length().not_nan())
    {
        sfx_longest.length()
    } else {
        sfx_drag.length()
    };

    let music = Array1::from_vec(music.to_vec());
    let ending_music = Array1::from_vec(ending_music.to_vec());
    let sfx_click = Array1::from_vec(sfx_click.to_vec());
    let sfx_drag = Array1::from_vec(sfx_drag.to_vec());
    let sfx_flick = Array1::from_vec(sfx_flick.to_vec());

    let mut gl = unsafe { get_internal_gl() };

    let fps = config.fps;
    let offset = chart.offset + info.offset;
    let speed = config.speed as f64;
    let speed_time_ratio = 1.0 / speed;

    let before_time: f64 = if config.render_loading {
        LoadingScene::TOTAL_TIME + GameScene::BEFORE_DURATION * speed_time_ratio
    } else {
        0.0
    };
    let before_time_music: f64 = if config.render_loading {
        LoadingScene::TOTAL_TIME * speed + GameScene::BEFORE_DURATION
    } else {
        0.0
    };

    let chart_length = before_time
        + config
            .play_end_time
            .unwrap_or(music_length)
            .min(music_length)
            * speed_time_ratio
        - config.play_start_time * speed_time_ratio
        - offset
        + WAIT_TIME * speed_time_ratio;
    let chart_length_music = before_time_music
        + config
            .play_end_time
            .unwrap_or(music_length)
            .min(music_length)
        - config.play_start_time
        - offset
        + WAIT_TIME;
    let chart_length_sfx = config
        .play_end_time
        .unwrap_or(music_length)
        .min(music_length)
        - config.play_start_time
        - offset
        + WAIT_TIME;
    let video_length = chart_length + config.ending_length;
    let video_length_music = chart_length_music + config.ending_length; // chart_length needs to be divided by speed, but music needs to be rendered at the original speed, which is changed by ffmpeg
    let video_frames = (video_length * fps as f64).ceil() as u64;

    let encoder_list = if config.hevc {
        ENCODER_LIST_HEVC
    } else {
        ENCODER_LIST_AVC
    };

    let ffmpeg_encoder =
        if let Some(ffmpeg_encoder) = get_encoder(&ffmpeg, &config, encoder_list, true) {
            ffmpeg_encoder
        } else {
            bail!(tl!("no-hwacc"))
        };

    eprintln!("Encoder: {}", ffmpeg_encoder);

    // 高性能模式：把上下翻转挪进 GPU 的帧读回（见 `mark_readback_flip` 上的注释）。
    // 只有硬件编码器才这么做 —— 软件编码器的滤镜链在 CPU 上，改读回方向不改 `-vf` 会颠倒。
    if config.high_performance && is_hardware_encoder_name(&ffmpeg_encoder) {
        mark_readback_flip(true);
    }

    eprintln!("Loading Time: {:.2?}", loading_time.elapsed());
    eprintln!("video length: {:.2}s frame: {}", video_length, video_frames);

    let render_start_time = Instant::now();

    if ipc {
        send(IPCEvent::Mixing);
    }

    let output_music_len = (video_length_music * music_sample_rate as f64).ceil() as usize * 2;
    let output_sfx_len = ((video_length + sfx_protect_time) * sample_rate_f64).ceil() as usize * 2;
    let output_ending_music_delay =
        chart_length + GameScene::WAIT_AFTER_TIME * speed_time_ratio + EndingScene::BPM_WAIT_TIME;
    let output_ending_music_len =
        ((video_length - output_ending_music_delay).max(0.) * sample_rate_f64).ceil() as usize * 2;
    let output_ending_music_delay_string = output_ending_music_delay * 1000.;
    let output_ending_music_delay_string = format!(
        "{}|{}",
        output_ending_music_delay_string, output_ending_music_delay_string
    );

    let mut output_music = Array1::from_vec(vec![0.0_f32; output_music_len]);
    let mut output_sfx = Array1::from_vec(vec![0.0_f32; output_sfx_len]);
    let mut output_ending_music = Array1::from_vec(vec![0.0_f32; output_ending_music_len]);

    let mut place_sfx = |pos: f64, clip: &Array1<f32>| {
        let position = (pos * sample_rate_f64).ceil() as usize * 2;
        let len = clip.len();
        let end = position + len;
        if end > output_sfx_len {
            return;
        }
        let mut slice = output_sfx.slice_mut(s![position..end]);
        slice += clip;
    };

    if volume_music != 0.0 {
        let music_time = Instant::now();
        let pos = (before_time - offset.min(0.)) * speed;
        let position_wrtie = (pos * music_sample_rate as f64).ceil() as usize * 2;
        let position_read = ((offset.max(0.) + config.play_start_time) * music_sample_rate as f64)
            .ceil() as usize
            * 2;
        let music_len = (chart_length_music * music_sample_rate as f64).ceil() as usize * 2;
        let len = (music.len() - position_read)
            .min(output_music_len - position_wrtie)
            .min(music_len - position_wrtie);
        let clip = music.slice(s![position_read..position_read + len]);
        let mut slice = output_music.slice_mut(s![position_wrtie..position_wrtie + len]);
        slice += &clip;
        eprintln!("Process Music Time: {:.2?}", music_time.elapsed());
    }

    type HitSoundMap = FxHashMap<String, Array1<f32>>;
    let mut extra_sfxs: HitSoundMap = HitSoundMap::with_capacity_and_hasher(16, Default::default());

    chart.hitsounds.iter().for_each(|(name, clip)| {
        extra_sfxs.insert(name.clone(), Array1::from_vec(clip.to_vec()));
    });

    let get_hitsound = |note: &Note| match &note.hitsound {
        HitSound::None => None,
        HitSound::Click => Some(&sfx_click),
        HitSound::Flick => Some(&sfx_flick),
        HitSound::Drag => Some(&sfx_drag),
        HitSound::Custom(s) => extra_sfxs.get(s),
    };

    /// 这一遍真正铺进打击音轨的条目：`(视频内时刻, 素材, 谱面时刻, 类型标签, 横向位置)`。
    /// 帧循环跑完之后用它把「溺水死亡的按键」对应的条目从缓冲里减掉。
    type SfxEntry<'a> = (f64, &'a Array1<f32>, f64, u8, f64);
    let mut sfx_placed: Vec<SfxEntry> = Vec::new();

    if volume_sfx != 0.0 {
        let sfx_time = Instant::now();
        let judge_offset = config.judge_offset;
        let sfx_start_time = config.play_start_time - config.judge_offset;
        let sfx_end_time = sfx_start_time + chart_length_sfx;
        let mut sfx_list: Vec<SfxEntry> = Vec::with_capacity(
            chart
                .lines
                .iter()
                .map(|line| line.notes.len())
                .sum::<usize>(),
        );

        if config.audio_mix_optimization {
            chart
                .lines
                .iter()
                .flat_map(|line| &line.notes)
                .filter(|note| !note.fake && note.time > sfx_start_time && note.time < sfx_end_time)
                .for_each(|note| {
                    if let Some(sfx) = get_hitsound(&note) {
                        sfx_list.push((
                            before_time + note.time * speed_time_ratio + judge_offset
                                - config.play_start_time * speed_time_ratio,
                            sfx,
                            note.time,
                            dps::core::water::note_kind_tag(&note.kind),
                            note.height,
                        ));
                    }
                });
            let len = sfx_list.len();

            sfx_list.sort_by(|a, b| {
                let a1 = round_to_step(a.0, 0.005);
                let a2 = round_to_step(b.0, 0.005);
                match a1.partial_cmp(&a2).unwrap_or(Ordering::Equal) {
                    Ordering::Less => Ordering::Less,
                    Ordering::Greater => Ordering::Greater,
                    Ordering::Equal => {
                        let p1 = a.1.as_ptr() as usize;
                        let p2 = b.1.as_ptr() as usize;
                        p1.cmp(&p2)
                    }
                }
            });

            let mut kept_sfx_list: Vec<SfxEntry> = Vec::with_capacity(len);
            let mut last_arr: Option<&Array1<f32>> = None;
            let mut last_t = 0.0;
            let mut count = 0;

            for &(pos, clip, note_time, kind_tag, note_height) in &sfx_list {
                let pos = round_to_step(pos, 0.005);
                let is_new_group = match last_arr {
                    None => true,
                    Some(prev) => !std::ptr::eq(prev, clip) || pos != last_t,
                };

                if is_new_group {
                    last_arr = Some(clip);
                    last_t = pos;
                    count = 1;
                    kept_sfx_list.push((pos, clip, note_time, kind_tag, note_height));
                } else {
                    if count < 3 {
                        kept_sfx_list.push((pos, clip, note_time, kind_tag, note_height));
                        count += 1;
                    }
                }
            }
            drop(sfx_list);

            let num = kept_sfx_list.len();
            if ipc {
                send(IPCEvent::MixingSfx(num as u64));
            }
            for (pos, sfx, note_time, kind_tag, note_height) in kept_sfx_list {
                place_sfx(pos, sfx);
                sfx_placed.push((pos, sfx, note_time, kind_tag, note_height));
                if ipc {
                    send(IPCEvent::Sfx);
                }
            }

            let elapsed = sfx_time.elapsed();
            eprintln!("Process Hit Effects Time: {:.2?} Equivalent Speed: {:.2} notes/sec Speed: {:.2} notes/sec", elapsed, len as f32 / elapsed.as_secs_f32(), num as f32 / elapsed.as_secs_f32())
        } else {
            chart
                .lines
                .iter()
                .flat_map(|line| &line.notes)
                .filter(|note| !note.fake && note.time > sfx_start_time && note.time < sfx_end_time)
                .for_each(|note| {
                    if let Some(sfx) = get_hitsound(&note) {
                        sfx_list.push((
                            before_time + note.time * speed_time_ratio + judge_offset
                                - config.play_start_time * speed_time_ratio,
                            sfx,
                            note.time,
                            dps::core::water::note_kind_tag(&note.kind),
                            note.height,
                        ));
                    }
                });
            let num = sfx_list.len();
            if ipc {
                send(IPCEvent::MixingSfx(num as u64));
            }
            for (pos, sfx, note_time, kind_tag, note_height) in sfx_list {
                place_sfx(pos, sfx);
                sfx_placed.push((pos, sfx, note_time, kind_tag, note_height));
                if ipc {
                    send(IPCEvent::Sfx);
                }
            }

            let elapsed = sfx_time.elapsed();
            eprintln!(
                "Process Hit Effects Time: {:.2?} Speed: {:.2} notes/sec",
                elapsed,
                num as f32 / elapsed.as_secs_f32()
            )
        }
    }

    if volume_music != 0.0 && output_ending_music_len > 0 {
        let ending_time = Instant::now();
        let mut position_wrtie = 0;
        while position_wrtie < output_ending_music_len {
            let len = (ending_music.len()).min(output_ending_music_len - position_wrtie);
            let clip = ending_music.slice(s![..len]);
            let mut slice = output_ending_music.slice_mut(s![position_wrtie..position_wrtie + len]);
            slice += &clip;
            position_wrtie += len;
        }
        eprintln!("Process Ending Music Time: {:.2?}", ending_time.elapsed());
    }

    if ipc {
        send(IPCEvent::Mixing);
    }
    let output_music_temp = NamedTempFile::new()?;
    let output_sfx_temp = NamedTempFile::new()?;
    let output_ending_temp = NamedTempFile::new()?;

    {
        let output_audio_time = Instant::now();

        let output_audio =
            |output: &Path, sample_rate: u32, samples: ndarray::Array1<f32>| -> Result<()> {
                let mut proc = cmd_hidden(&ffmpeg)
                    .args(
                        format!(
                            "-y -f f32le -ar {} -ac 2 -i pipe:0 -c:a pcm_f32le -f wav",
                            sample_rate
                        )
                        .split_whitespace(),
                    )
                    .arg(output)
                    .args(["-loglevel", "warning"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .with_context(|| tl!("run-ffmpeg-failed"))?;
                let input = proc.stdin.as_mut().unwrap();
                let slice = samples.as_slice().unwrap();
                let byte_slice = unsafe {
                    std::slice::from_raw_parts(
                        slice.as_ptr() as *const u8,
                        std::mem::size_of_val(slice),
                    )
                };
                let mut writer = BufWriter::with_capacity(64 * 1024, input);
                writer.write_all(byte_slice)?;
                writer.flush()?;

                drop(writer);
                proc.wait()?;
                Ok(())
            };

        output_audio(output_music_temp.path(), music_sample_rate, output_music)?;
        // 打击音轨在主 ffmpeg 那一趟只给一条**等长静音**（保持滤镜结构不变）：真正的打击音轨
        // 要等帧循环跑完、知道哪些按键溺水死了之后才写得出来，由最后那趟 remux 混进来。
        write_silent_wav(&ffmpeg, output_sfx_temp.path(), sample_rate, output_sfx_len)?;
        output_audio(
            output_ending_temp.path(),
            ending_music_sample_rate,
            output_ending_music,
        )?;

        eprintln!("Output Audio Time: {:.2?}", output_audio_time.elapsed());
    }

    if ipc {
        send(IPCEvent::Loading);
    }

    let preparing_render_time = Instant::now();
    let (vw, vh) = config.resolution;
    let mst = Rc::new(MSRenderTarget::new((vw, vh), config.sample_count));

    // 高性能模式的「GPU 翻转」离屏目标：帧先 blit 进它（上下镜像），再从它读回。
    // 见下面帧循环里 `READBACK_FLIP` 分支的注释。没开高性能模式时两个 id 都是 0，一个字节都不碰。
    let mut flip_tex: GLuint = 0;
    let mut flip_fbo: GLuint = 0;
    if READBACK_FLIP.load(AtomicOrdering::Relaxed) {
        unsafe {
            glGenTextures(1, &mut flip_tex);
            glBindTexture(GL_TEXTURE_2D, flip_tex);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST as _);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST as _);
            glTexImage2D(
                GL_TEXTURE_2D,
                0,
                GL_RGB8 as _,
                vw as _,
                vh as _,
                0,
                GL_RGB,
                GL_UNSIGNED_BYTE,
                std::ptr::null(),
            );
            glGenFramebuffers(1, &mut flip_fbo);
            glBindFramebuffer(GL_FRAMEBUFFER, flip_fbo);
            glFramebufferTexture2D(
                GL_FRAMEBUFFER,
                GL_COLOR_ATTACHMENT0,
                GL_TEXTURE_2D,
                flip_tex,
                0,
            );
            let status = glCheckFramebufferStatus(GL_FRAMEBUFFER);
            if status != GL_FRAMEBUFFER_COMPLETE {
                eprintln!(
                    "[高性能模式] 翻转用的离屏 FBO 不完整（status=0x{status:x}），这一遍仍走 ffmpeg 的 vflip"
                );
                glDeleteFramebuffers(1, &flip_fbo);
                glDeleteTextures(1, &flip_tex);
                flip_fbo = 0;
                flip_tex = 0;
                mark_readback_flip(false);
            }
            glBindFramebuffer(GL_FRAMEBUFFER, 0);
        }
    }

    let my_time: Rc<RefCell<f64>> = Rc::new(RefCell::new(0.));
    let tm = TimeManager::manual(Box::new({
        let my_time = Rc::clone(&my_time);
        move || *(*my_time).borrow()
    }));
    static MSAA: AtomicBool = AtomicBool::new(false);
    let player = build_player(&config).await?;
    let mut main = if config.render_loading {
        Main::new(
            Box::new(
                LoadingScene::new(
                    Some((chart, format)),
                    GameMode::Normal,
                    info,
                    &prpr_config,
                    fs,
                    Some(player),
                    None,
                    None,
                )
                .await?,
            ),
            tm,
            {
                let mut cnt = 0;
                let mst = Rc::clone(&mst);
                move || {
                    cnt += 1;
                    if cnt == 1 || cnt == 3 {
                        MSAA.store(true, AtomicOrdering::SeqCst);
                        Some(mst.input())
                    } else {
                        MSAA.store(false, AtomicOrdering::SeqCst);
                        Some(mst.output())
                    }
                }
            },
        )
        .await?
    } else {
        let mut fs: Box<dyn FileSystem> = fs;
        let background =
            match LoadingScene::load_background(&mut fs, &prpr_config, &info.illustration).await {
                Ok((ill, bg)) => Some((ill, bg)),
                Err(err) => {
                    warn!("failed to load background: {err:?}");
                    None
                }
            };
        let (illustration, background): (SafeTexture, SafeTexture) = background
            .map(|(ill, back)| (ill.into(), back.into()))
            .unwrap_or_else(|| (BLACK_TEXTURE.clone(), BLACK_TEXTURE.clone()));
        Main::new(
            Box::new(
                GameScene::new(
                    Some((chart, format)),
                    GameMode::Normal,
                    info,
                    prpr_config,
                    fs,
                    Some(player),
                    background,
                    illustration,
                    None,
                    None,
                )
                .await?,
            ),
            tm,
            {
                let mut cnt = 0;
                let mst = Rc::clone(&mst);
                move || {
                    cnt += 1;
                    if cnt == 1 || cnt == 3 {
                        MSAA.store(true, AtomicOrdering::SeqCst);
                        Some(mst.input())
                    } else {
                        MSAA.store(false, AtomicOrdering::SeqCst);
                        Some(mst.output())
                    }
                }
            },
        )
        .await?
    };
    main.top_level = false;
    main.viewport = Some((0, 0, vw as i32, vh as i32));

    let bitrate_control = if config.dynamic_bitrate_control {
        if ffmpeg_encoder == encoder_list[0] && !config.mpeg4 {
            "-cq"
        } else if ffmpeg_encoder == encoder_list[1]
            || config.mpeg4
            || ffmpeg_encoder == encoder_list[3]
        {
            "-q"
        } else if ffmpeg_encoder == encoder_list[2] {
            "-qp_p"
        } else if ffmpeg_encoder == config.custom_encoder.unwrap_or_default() {
            "-q"
        } else {
            "-crf"
        }
    } else {
        "-b:v"
    };

    let mut args = "-probesize 50M -y -f rawvideo -c:v rawvideo".to_owned();
    if ffmpeg_encoder == encoder_list[0] {
        args += " -hwaccel_output_format cuda";
    }
    write!(
        &mut args,
        " -s {vw}x{vh} -r {fps} -pix_fmt rgba -thread_queue_size 1024 -i pipe:0"
    )?;

    let mut ffmpeg_audio_filter_music = if config.loudness_equalization {
        format!(
            "[2:a]loudnorm=I=-16:LRA=24:TP=-1,aresample={}:resampler=swr",
            sample_rate,
        )
    } else {
        format!("[2:a]aresample={}:resampler=swr", sample_rate,)
    };

    let ffmpeg_audio_filter_music_volume = format!(",volume={}", volume_music);
    ffmpeg_audio_filter_music += &ffmpeg_audio_filter_music_volume;

    let ffmpeg_audio_filter_music_speed = if config.speed != 1.0 {
        format!(",rubberband=tempo={}", config.speed)
    } else {
        String::new()
    };
    ffmpeg_audio_filter_music += &ffmpeg_audio_filter_music_speed;
    ffmpeg_audio_filter_music += "[a2];";

    let mut ffmpeg_audio_filter_sfx = format!("[1:a]volume={}", volume_sfx);

    let ffmpeg_audio_filter_sfx_limit = if config.force_limit {
        format!(
            ",alimiter=limit={}:level=false:attack=0.1:release=1",
            config.limit_threshold
        )
    } else {
        String::new()
    };

    ffmpeg_audio_filter_sfx += &ffmpeg_audio_filter_sfx_limit;
    ffmpeg_audio_filter_sfx += "[a1];";

    let ffmpeg_audio_filter_ending = format!(
        "[3:a]volume={},adelay={}[a3];",
        volume_music, output_ending_music_delay_string
    );

    let ffmpeg_audio_filter_mix = if config.hires {
        format!("[a1][a2][a3]amix=inputs=3:duration=first:normalize=0[a]")
    } else {
        format!(
            "[a1][a2][a3]amix=inputs=3:duration=first:normalize=0[aa];[aa]alimiter=limit=1.0:level=false:attack=0.1:release=1[a]"
        )
    };

    let ffmpeg_audio_filter = format!(
        "{}{}{}{}",
        ffmpeg_audio_filter_music,
        ffmpeg_audio_filter_sfx,
        ffmpeg_audio_filter_ending,
        ffmpeg_audio_filter_mix
    );

    // `-vf vflip`：原料是自下而上读回来的，这里整帧翻回来。
    // 高性能模式下这一步已经在 `glReadPixels` 里由 GPU 做完（`READBACK_FLIP`），
    // 再翻一次就会上下颠倒 ⇒ 那种情况下整段 `-vf` 都不写（只写了 vflip，没别的滤镜）。
    let vflip_filter = if READBACK_FLIP.load(AtomicOrdering::Relaxed) {
        ""
    } else {
        " -vf vflip"
    };

    // 高性能模式的第三件事：连 RGB→YUV 也交给显卡。`-pix_fmt rgba` 让 NVENC 直接吃渲染出来的
    // RGBA 帧（驱动在显卡上转 YUV），这样 ffmpeg 那一侧完全没有 swscale。只有探测通过才用。
    let gpu_color_convert = READBACK_FLIP.load(AtomicOrdering::Relaxed)
        && nvenc_native_rgba(&ffmpeg_encoder)
        && encoder_accepts_rgba_input(&ffmpeg, &ffmpeg_encoder);
    if gpu_color_convert {
        eprintln!("[高性能模式] RGBA→YUV 交给 NVENC 在显卡上做，省掉 ffmpeg 的 swscale");
    }
    let pixel_format = if gpu_color_convert { "rgba" } else { "yuv420p" };

    let args2 = format!(
        "-c:a {} -c:v {} -movflags +faststart -pix_fmt {} {} {} -filter_complex {} -map 0:v:0 -map [a]{} -f {}",
        if config.hires {
            "pcm_f32le"
        } else {
            "aac -b:a 320k"
        },
        ffmpeg_encoder,
        pixel_format,
        bitrate_control,
        config.bitrate,
        ffmpeg_audio_filter,
        vflip_filter,
        if config.hires { "mov" } else { "mp4" }
    );

    eprintln!(
        "Preparing Render Time: {:.2?}",
        preparing_render_time.elapsed()
    );

    eprintln!(
        "Command: {} {} {} {} {} {} {} {} {} {}",
        &ffmpeg,
        args,
        "-i",
        output_sfx_temp.path().display(),
        "-i",
        output_music_temp.path().display(),
        "-i",
        output_ending_temp.path().display(),
        args2,
        output_path.display()
    );

    let mut proc = cmd_hidden(&ffmpeg)
        .args(args.split_whitespace())
        .arg("-i")
        .arg(output_sfx_temp.path())
        .arg("-i")
        .arg(output_music_temp.path())
        .arg("-i")
        .arg(output_ending_temp.path())
        .args(args2.split_whitespace())
        .arg(&output_path)
        .args(["-loglevel", "warning"])
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| tl!("run-ffmpeg-failed"))?;
    let mut input = proc.stdin.take().unwrap();

    let byte_size = vw as usize * vh as usize * 4;

    const N: usize = 5; // Buffer Size
    let mut pbos: [GLuint; N] = [0; N];
    unsafe {
        use miniquad::gl::*;
        glGenBuffers(N as _, pbos.as_mut_ptr());
        for pbo in pbos {
            glBindBuffer(GL_PIXEL_PACK_BUFFER, pbo);
            glBufferData(
                GL_PIXEL_PACK_BUFFER,
                (vw as u64 * vh as u64 * 4) as _,
                std::ptr::null(),
                GL_STREAM_READ,
            );
        }
        glBindBuffer(GL_PIXEL_PACK_BUFFER, 0);
    }

    if ipc {
        send(IPCEvent::RenderFrame(video_frames));
    }
    let render_time = Instant::now();

    let fps = fps as f64;
    let frames_per_10 = (video_frames / 10).max(1);
    let frames = video_frames;
    let mut step_time = Instant::now();
    let mut last_print = Instant::now();
    let mut pause_duration = Duration::ZERO;

    // 进水音效（受伤/死亡）是实时播放的，渲染器只能靠引擎记的事件日志事后补进音轨；
    // 这里先清掉实时播放时攒下的事件，这一遍渲染记到的才是「这段视频里该响哪几下」。
    let _ = dps::core::water::take_sfx_events();

    for frame in 0..frames {
        if !cmd && pause_requested.load(AtomicOrdering::SeqCst) {
            eprintln!("Render paused");
            if ipc {
                send(IPCEvent::Paused);
            }
            let pause_begin = Instant::now();
            while pause_requested.load(AtomicOrdering::SeqCst) {
                std::thread::park();
            }
            pause_duration += pause_begin.elapsed();
            step_time = Instant::now();
            eprintln!("Render resumed");
            if ipc {
                send(IPCEvent::Resumed);
            }
        }

        let now = (frame as f64) / fps;
        *my_time.borrow_mut() = now.max(0.);
        gl.quad_gl.render_pass(Some(mst.output().render_pass));
        main.update()?;
        main.render(&mut painter)?;
        if *my_time.borrow() <= LoadingScene::TOTAL_TIME && config.render_loading {
            draw_rectangle(0., 0., 0., 0., Color::default());
        }
        gl.flush();

        if MSAA.load(AtomicOrdering::SeqCst) {
            mst.blit();
        }


        if !cmd && frame % frames_per_10 == 0 {
            let progress = round_to_step((frame as f64 / video_frames as f64 * 100.).ceil(), 10.0);
            eprintln!(
                "Render progress: {:.0}% {}/{} Time elapsed: {:.2}s",
                progress,
                frame,
                video_frames,
                std::mem::replace(&mut step_time, Instant::now())
                    .elapsed()
                    .as_secs_f32()
            );
        }

        if cmd && last_print.elapsed() >= Duration::from_secs(1) {
            let progress = (frame as f64 / video_frames as f64 * 100.).round();
            last_print = Instant::now();
            eprint!(
                "\rprogress={:.0}% frame={}/{}",
                progress, frame, video_frames
            );
        }

        unsafe {
            // 高性能模式（`READBACK_FLIP`）：先用 `glBlitFramebuffer` 在显存里上下翻一遍再读回，
            // 省掉 ffmpeg 那一趟 CPU 的 `-vf vflip`。
            //
            // 这里**不能**用「`glReadPixels` 传负高度」那套（那样整段视频会全黑）：
            // 规格里 ReadPixels 的 width/height 必须非负，负高度只会得到 `GL_INVALID_VALUE`，
            // PBO 里留着全零 ⇒ 黑帧。`glBlitFramebuffer` 的源/目标矩形则**允许** y0 > y1，
            // 所以镜像交给它做（目标矩形 y 从 vh 到 0 就是上下翻转），全程在 GPU 上。
            let flip = READBACK_FLIP.load(AtomicOrdering::Relaxed);
            if flip {
                glBindFramebuffer(GL_READ_FRAMEBUFFER, internal_id(mst.output()));
                glBindFramebuffer(GL_DRAW_FRAMEBUFFER, flip_fbo);
                glBlitFramebuffer(
                    0,
                    0,
                    vw as _,
                    vh as _,
                    0,
                    vh as _,
                    vw as _,
                    0,
                    GL_COLOR_BUFFER_BIT,
                    GL_NEAREST,
                );
                glBindFramebuffer(GL_READ_FRAMEBUFFER, flip_fbo);
            } else {
                glBindFramebuffer(GL_READ_FRAMEBUFFER, internal_id(mst.output()));
            }
            glBindBuffer(GL_PIXEL_PACK_BUFFER, pbos[frame as usize % N]);
            glReadPixels(
                0,
                0,
                vw as _,
                vh as _,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                std::ptr::null_mut(),
            );
            if frame >= N as u64 - 1 {
                glBindBuffer(GL_PIXEL_PACK_BUFFER, pbos[(frame + 1) as usize % N]);
                let src: *const u8 =
                    glMapBuffer(GL_PIXEL_PACK_BUFFER, 0x88B8 /* GL_READ_ONLY */);
                if !src.is_null() {
                    input.write_all(&std::slice::from_raw_parts(src, byte_size))?;
                }
                glUnmapBuffer(GL_PIXEL_PACK_BUFFER);
            }
        }

        if ipc {
            send(IPCEvent::Frame);
        }
    }
    unsafe {
        let start = (frames as usize + 1) % N;
        for i in 0..N - 1 {
            glBindBuffer(GL_PIXEL_PACK_BUFFER, pbos[(start + i) % N]);
            let src: *const u8 = glMapBuffer(GL_PIXEL_PACK_BUFFER, 0x88B8 /* GL_READ_ONLY */);
            if !src.is_null() {
                input.write_all(&std::slice::from_raw_parts(src, byte_size))?;
            }
            glUnmapBuffer(GL_PIXEL_PACK_BUFFER);
        }
        glDeleteBuffers(N as _, pbos.as_ptr());
        if flip_fbo != 0 {
            glDeleteFramebuffers(1, &flip_fbo);
            glDeleteTextures(1, &flip_tex);
        }
    }
    drop(input);
    let water_sfx_events = dps::core::water::take_sfx_events();
    // 「已经溺水死亡的按键不该再响它自己的打击音效」：把它们的条目从内存里的打击音轨里减掉
    // （同一位置、同一素材 ⇒ 精确抵消，而且发生在任何滤镜之前，不受 limiter 影响），
    // 写成第二条 wav 交给下面那趟 remux。
    let drowned_notes = dps::core::water::take_drowned_notes();
    let mut sfx_wav: Option<NamedTempFile> = None;
    // 注意：主 ffmpeg 那一趟的打击音轨是**静音占位**（见上面的 `write_silent_wav`），
    // 真正的打击音一律由这里写出的 `sfx_wav` 在最后的 remux 里混进去。
    // 因此只要铺过音（`sfx_placed` 非空）就必须写出这条 wav，否则普通渲染会丢掉全部打击音。
    if volume_sfx != 0.0 && !sfx_placed.is_empty() {
        let mut removed = 0usize;
        for (pos, sfx, note_time, kind_tag, note_height) in sfx_placed.drain(..) {
            let hit = drowned_notes.iter().any(|(t, k, h)| {
                *k == kind_tag && (t - note_time).abs() < 1e-6 && (h - note_height).abs() < 1e-6
            });
            if !hit {
                continue;
            }
            let position = (pos * sample_rate_f64).ceil() as usize * 2;
            let end = position + sfx.len();
            if end > output_sfx_len {
                continue;
            }
            let mut slice = output_sfx.slice_mut(s![position..end]);
            slice -= sfx;
            removed += 1;
        }
        if removed > 0 {
            eprintln!("[water] 已从打击音轨里去掉 {removed} 个溺水按键的音效");
        }
        let wav = NamedTempFile::new()?;
        write_audio_wav(
            &ffmpeg,
            wav.path(),
            sample_rate,
            output_sfx.as_slice().unwrap(),
        )?;
        sfx_wav = Some(wav);
    }
    if cmd {
        eprintln!("\rprogress=100% frame={}/{}", video_frames, video_frames);
    }
    let actual_render_time = render_time.elapsed().saturating_sub(pause_duration);
    eprintln!("Render Time: {:.2?}", actual_render_time);
    eprintln!(
        "Average FPS: {:.2}",
        frames as f64 / actual_render_time.as_secs_f64()
    );
    proc.wait()?;
    if !water_sfx_events.is_empty() || sfx_wav.is_some() {
        let water_time = Instant::now();
        match mix_water_sfx(
            &ffmpeg,
            &output_path,
            &water_sfx_events,
            sfx_wav.as_ref().map(|wav| wav.path()),
            volume_sfx,
            sample_rate,
            video_length,
            before_time,
            speed_time_ratio,
            config.play_start_time,
            config.hires,
            config.force_limit,
            config.limit_threshold,
        ) {
            Ok(()) => eprintln!(
                "Water Sfx Time: {:.2?}",
                water_time.elapsed()
            ),
            Err(err) => eprintln!("[water] 进水音效混入视频失败（视频本身正常）：{err:?}"),
        }
    }
    eprintln!("Total Time: {:.2?}", loading_time.elapsed());
    if ipc {
        send(IPCEvent::Done(render_start_time.elapsed().as_secs_f64()));
    }
    Ok(())
}

/// 把一段 f32 立体声交错采样写成 wav（交给 ffmpeg 转发）。
///
/// 主 ffmpeg 开跑前就要拿到所有音轨，而「哪些按键溺水死了」只有帧循环跑完才知道，所以打击音轨
/// 分两次写：主 ffmpeg 那一趟先给一条等长静音占位（保持滤镜结构不变），帧循环之后再写真正这条。
fn write_audio_wav(ffmpeg: &str, output: &Path, sample_rate: u32, samples: &[f32]) -> Result<()> {
    let mut proc = cmd_hidden(ffmpeg)
        .args(
            format!(
                "-y -f f32le -ar {} -ac 2 -i pipe:0 -c:a pcm_f32le -f wav",
                sample_rate
            )
            .split_whitespace(),
        )
        .arg(output)
        .args(["-loglevel", "warning"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| tl!("run-ffmpeg-failed"))?;
    {
        let input = proc.stdin.as_mut().unwrap();
        let byte_slice = unsafe {
            std::slice::from_raw_parts(
                samples.as_ptr() as *const u8,
                std::mem::size_of_val(samples),
            )
        };
        let mut writer = BufWriter::with_capacity(64 * 1024, input);
        writer.write_all(byte_slice)?;
        writer.flush()?;
    }
    let status = proc.wait()?;
    if !status.success() {
        bail!("ffmpeg (audio wav) failed: {status}");
    }
    Ok(())
}

/// 写一条指定长度的**静音** wav（分块写，避免再分配一整块静音缓冲）。
fn write_silent_wav(ffmpeg: &str, output: &Path, sample_rate: u32, samples: usize) -> Result<()> {
    let mut proc = cmd_hidden(ffmpeg)
        .args(
            format!(
                "-y -f f32le -ar {} -ac 2 -i pipe:0 -c:a pcm_f32le -f wav",
                sample_rate
            )
            .split_whitespace(),
        )
        .arg(output)
        .args(["-loglevel", "warning"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| tl!("run-ffmpeg-failed"))?;
    {
        let input = proc.stdin.as_mut().unwrap();
        let mut writer = BufWriter::with_capacity(64 * 1024, input);
        let chunk = vec![0u8; 64 * 1024];
        let mut left = samples * std::mem::size_of::<f32>();
        while left > 0 {
            let n = left.min(chunk.len());
            writer.write_all(&chunk[..n])?;
            left -= n;
        }
        writer.flush()?;
    }
    let status = proc.wait()?;
    if !status.success() {
        bail!("ffmpeg (silent wav) failed: {status}");
    }
    Ok(())
}

/// 把这一遍渲染里真正播放过的进水音效（见 [`dps::core::water::take_sfx_events`]）混进渲染好的视频。
///
/// 进水音效由引擎实时播放（`sasa::Sfx`），而主 ffmpeg 进程在开跑前就要拿到三条 wav 轨，
/// 所以只能等视频写完再补一遍：`-c:v copy` 只重混音频、不重编码画面，代价很小。
/// 素材（[`dps::core::water::HIT_SFX_BYTES`]）是 44.1 kHz 的 mp3，这里线性重采样到音轨的
/// 48 kHz；事件时刻是引擎的**谱面时间**，按「音符音效」那套公式换算成视频里的位置
/// （`before_time + (t - play_start_time) * speed_time_ratio`）。
/// 音量已由引擎按 `water_sfx_volume` 施加在事件里，这里不再乘全局 `volume_sfx`（与实时播放一致）。
fn mix_water_sfx(
    ffmpeg: &str,
    output_path: &Path,
    events: &[(f64, usize, f32)],
    sfx_wav: Option<&Path>,
    volume_sfx: f32,
    sample_rate: u32,
    video_length: f64,
    before_time: f64,
    speed_time_ratio: f64,
    play_start_time: f64,
    hires: bool,
    force_limit: bool,
    limit_threshold: f32,
) -> Result<()> {
    let sample_rate_f64 = sample_rate as f64;

    // 1) 解码素材，统一成目标采样率的立体声交错样本
    let mut sources: Vec<Vec<f32>> = Vec::new();
    for bytes in dps::core::water::HIT_SFX_BYTES.iter() {
        let clip = AudioClip::new(bytes.to_vec()).context("decode water sfx")?;
        let rate = clip.sample_rate().max(1) as f64;
        let data = clip.to_vec();
        // `AudioClip` 不给声道字段，用「帧数 = 时长 × 采样率」反推，再夹到 1..=2 声道
        let frames = ((clip.length() as f64 * rate).round() as usize).max(1);
        let channels = (data.len() / frames).clamp(1, 2);
        let out_frames = ((frames as f64 * sample_rate_f64 / rate).ceil() as usize).max(1);
        let mut out = Vec::with_capacity(out_frames * 2);
        for i in 0..out_frames {
            let pos = i as f64 * rate / sample_rate_f64;
            let i0 = pos.floor() as usize;
            let i1 = (i0 + 1).min(frames - 1);
            let frac = (pos - i0 as f64) as f32;
            for c in 0..2 {
                let c = c.min(channels - 1);
                let s0 = data.get(i0 * channels + c).copied().unwrap_or_default();
                let s1 = data.get(i1 * channels + c).copied().unwrap_or_default();
                out.push(s0 + (s1 - s0) * frac);
            }
        }
        sources.push(out);
    }
    if sources.is_empty() {
        return Ok(());
    }

    // 2) 铺进一条与视频等长的缓冲区（末尾多留最长素材的长度）
    let longest = sources.iter().map(|s| s.len()).max().unwrap_or(0);
    let total = ((video_length.max(0.) * sample_rate_f64).ceil() as usize) * 2 + longest;
    let mut water = vec![0f32; total];
    let mut played = 0usize;
    for &(time, idx, volume) in events {
        // 谱面时间 -> 视频里的位置（和 `place_sfx` 给音符音效用的是同一套换算）
        let position = before_time + (time - play_start_time) * speed_time_ratio;
        if position < 0. {
            continue;
        }
        // 引擎的实例池是「素材轮流建」的，下标取模落回素材（`idx` 可能 >= 素材数）
        let Some(src) = sources.get(idx % sources.len()) else {
            continue;
        };
        let start = (position * sample_rate_f64).round() as usize * 2;
        if start >= water.len() {
            continue;
        }
        for (i, sample) in src.iter().enumerate() {
            let pos = start + i;
            if pos >= water.len() {
                break;
            }
            water[pos] += *sample * volume;
        }
        played += 1;
    }
    if played == 0 && sfx_wav.is_none() {
        return Ok(());
    }

    // 3) 写成采样率与音轨一致的 wav
    let water_wav = NamedTempFile::new()?;
    {
        let mut proc = cmd_hidden(ffmpeg)
            .args(
                format!(
                    "-y -f f32le -ar {} -ac 2 -i pipe:0 -c:a pcm_f32le -f wav",
                    sample_rate
                )
                .split_whitespace(),
            )
            .arg(water_wav.path())
            .args(["-loglevel", "warning"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| tl!("run-ffmpeg-failed"))?;
        {
            let input = proc.stdin.as_mut().unwrap();
            let byte_slice = unsafe {
                std::slice::from_raw_parts(
                    water.as_ptr() as *const u8,
                    std::mem::size_of_val(water.as_slice()),
                )
            };
            let mut writer = BufWriter::with_capacity(64 * 1024, input);
            writer.write_all(byte_slice)?;
            writer.flush()?;
        }
        let status = proc.wait()?;
        if !status.success() {
            bail!("ffmpeg (water wav) failed: {status}");
        }
    }
    drop(water);

    // 4) remux：视频流原样 copy，把进水音轨 / 打击音轨 amix 进原音轨
    let mut chain = String::new();
    if sfx_wav.is_some() {
        // 打击音轨原本是主 ffmpeg 用 `[1:a]volume=<volume_sfx>` 加进去的，这里补上同一份增益
        chain += &format!("[2:a]volume={}[s];", volume_sfx);
    }
    chain += "[1:a]";
    if force_limit {
        chain += &format!(
            "alimiter=limit={}:level=false:attack=0.1:release=1",
            limit_threshold
        );
    } else {
        chain += "anull";
    }
    chain += "[w];[0:a][w]";
    if sfx_wav.is_some() {
        chain += "[s]amix=inputs=3:duration=first:normalize=0";
    } else {
        chain += "amix=inputs=2:duration=first:normalize=0";
    }
    if hires {
        chain += "[a]";
    } else {
        chain += "[a0];[a0]alimiter=limit=1.0:level=false:attack=0.1:release=1[a]";
    }

    let mixed_path = output_path.with_extension(if hires {
        "water.mov"
    } else {
        "water.mp4"
    });
    let args = format!(
        "-c:a {} -c:v copy -movflags +faststart -filter_complex {} -map 0:v:0 -map [a] -f {}",
        if hires {
            "pcm_f32le"
        } else {
            "aac -b:a 320k"
        },
        chain,
        if hires { "mov" } else { "mp4" }
    );
    let mut remux = cmd_hidden(ffmpeg);
    remux
        .args(["-y", "-i"])
        .arg(output_path)
        .args(["-i"])
        .arg(water_wav.path());
    if let Some(path) = sfx_wav {
        remux.args(["-i"]).arg(path);
    }
    let status = remux
        .args(args.split_whitespace())
        .arg(&mixed_path)
        .args(["-loglevel", "warning"])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| tl!("run-ffmpeg-failed"))?
        .wait()?;
    if !status.success() {
        let _ = std::fs::remove_file(&mixed_path);
        bail!("ffmpeg (water remux) failed: {status}");
    }
    std::fs::rename(&mixed_path, output_path).with_context(|| {
        format!(
            "replace {} with {}",
            output_path.display(),
            mixed_path.display()
        )
    })?;
    eprintln!(
        "[water] 已把 {} 个进水音效混入视频音轨 {}",
        played,
        output_path.display()
    );
    Ok(())
}
