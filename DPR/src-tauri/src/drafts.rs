//! 编辑窗口的「草稿」：剪映那样的一个个项目。
//!
//! 一个草稿 = `<程序目录>/dpe/editor/drafts/<id>/draft.json`。草稿里**只记素材的原文件路径**
//! （谱面 / 音频 / 曲绘各一项，另外还有一份导入过的素材清单），不复制、不改动那些文件：
//! 素材属于用户自己的那份，草稿本身只有这一个几 KB 的 JSON。
//!
//! 原文件被挪走或删掉时草稿本身不会坏：`path` 照样留着，界面按「文件不在」显示，
//! 用户把文件放回去就还能用。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::common;
use crate::lyrics;

/// 谱面：落到「谱面」轨道上。目录（摊开的谱面）与 `.pez` / `.zip` 都算。
pub const KIND_CHART: &str = "chart";

/// 音频：落到「音乐」轨道上。
pub const KIND_MUSIC: &str = "music";

/// 曲绘：落到「曲绘」轨道上。
pub const KIND_COVER: &str = "cover";

/// 歌词：不占某条固定轨道 —— 拖到时间轴上时按文件里的时间摊成一条特效轨，一句一个片段。
pub const KIND_LYRIC: &str = "lyric";

/// 「导入素材」往下翻的层数上限：素材通常就摊在谱面包顶层，但允许三十一层嵌套。
const MAX_MEDIA_DEPTH: usize = 31;

/// 草稿里的一项素材（就是磁盘上的某个文件或谱面目录）。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct DraftMedia {
    /// 原文件的绝对路径；目录型谱面就是目录本身。
    pub path: String,
    /// 显示名（文件名或目录名）。
    pub name: String,
    /// [`KIND_CHART`] / [`KIND_MUSIC`] / [`KIND_COVER`] / [`KIND_LYRIC`]。
    pub kind: String,
    /// 素材库里归到哪个文件夹（[`DraftFolder::id`]）；空着就是摊在根上。
    pub folder: Option<String>,
}

impl Default for DraftMedia {
    fn default() -> Self {
        Self { path: String::new(), name: String::new(), kind: String::new(), folder: None }
    }
}

/// 素材库里的一个文件夹：导入一个谱面包（目录 / 压缩包）时按它里面的真实层级建，也可以自己新建来归类。
///
/// 文件夹可以套文件夹，`parent` 指向上级文件夹；最多三十一层，更深的目录摊在第三十一级里。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct DraftFolder {
    pub id: String,
    pub name: String,
    /// 上级文件夹（[`DraftFolder::id`]）；空着就是「全部素材」底下那一层。
    pub parent: Option<String>,
}

/// 草稿自己填的谱面信息（曲名 / 难度 / 谱师 / 曲师）。
///
/// 谱面里自带的那份信息（`info.txt`、谱面 json 的 META）只当兜底：草稿填了就以草稿为准，
/// 预览时按这份内容拼一份 `info.txt` 塞给引擎。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct DraftInfo {
    pub name: String,
    /// 形如 `IN Lv.15`（引擎会从中拆出难度标签与定数）。
    pub difficulty: String,
    pub charter: String,
    pub composer: String,
}

/// 默认字体：仓库根 `assets/font.ttf`，界面里注册成 `Phigros`，与引擎画面同一份。
pub const NATIVE_FONT: &str = "Phigros";

/// 引擎原生噪域红（`block_area_color = [0.9, 0.1, 0.1]`）；等于它的颜色走原生材质。
pub const NATIVE_NOISE_COLOR: &str = "#E61A1A";

/// 文字噪域的参数：一段文字按字号、字体排出来，再按格子取成噪域方块。
///
/// 字号、颗粒、位置都按**屏幕高度的比例**记（0~1），换个分辨率放出来还是同一个构图；
/// 这样谱面里也不依赖具体像素。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct TextNoiseEffect {
    /// 要铺成噪域的文字；可以多行（`\n` 分行）。
    pub text: String,
    /// 字体名，CSS 与引擎两侧都按这个名字找（默认是内置那份 `assets/font.ttf`）。
    pub font_family: String,
    /// 噪域颜色（`#rrggbb`）。等于引擎原生红时按原生色走，不写进谱面。
    pub color: String,
    /// 字号：占屏高的比例。
    pub font_size: f64,
    /// 颗粒：一个格子的边长占屏高的比例，越小格子越细。
    pub cell: f64,
    /// 文字中心点的位置（屏幕比例，0~1，`y` 从上往下）。
    pub x: f64,
    pub y: f64,
    /// 行距：字号的倍数。
    pub line_height: f64,
    /// 字间距：字号的倍数（0 是字体自己的间距，负数收紧）。
    pub letter_spacing: f64,
    /// 整段文字绕自己的中心转多少度（正数顺时针）。
    pub rotation: f64,
    /// 激活：`false` 时这一段不生成噪域（谱面里留着，引擎跳过）。
    ///
    /// 缺省就是激活 —— 导出只在没激活时才写 `enable: false`，老草稿里没这个字段也照旧画。
    pub enable: bool,
    /// 反转：铺「除文字之外」的整片噪域，把文字挖成空白（用补集矩形拼）。
    pub invert: bool,
    /// 反相：生成方块时把 `isSubtract` 标成 true（引擎的减算通道），几何不变，可与反转同开。
    pub subtract: bool,
    /// 融入原谱：不写颜色与材质覆盖，跟随谱面自己的噪域渲染设置。
    pub blend: bool,
    /// 这条特效轨自己的噪域材质；老草稿留空，读的时候由界面按草稿上那份（`EditorDraft.noise`）兜底。
    pub material: Option<NoiseMaterial>,
}

impl Default for TextNoiseEffect {
    fn default() -> Self {
        Self {
            text: "Text".to_string(),
            font_family: NATIVE_FONT.to_string(),
            color: NATIVE_NOISE_COLOR.to_string(),
            font_size: 0.5,
            cell: 0.012,
            x: 0.5,
            y: 0.5,
            line_height: 1.25,
            letter_spacing: 0.0,
            rotation: 0.0,
            enable: true,
            invert: false,
            subtract: false,
            blend: false,
            material: None,
        }
    }
}

/// 特效轨上的一个片段：一段起止时间 + 这一段自己的效果参数。
///
/// 一条特效轨可以放好几个片段（同一行在不同时间各出现一次），片段之间互不影响；
/// 拖动片段可以换到别的特效轨上（换层级）。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct DraftClip {
    pub id: String,
    /// 片段起点（秒）。
    pub start: f64,
    /// 片段长度（秒）。
    pub length: f64,
    /// 这一段的文字噪域参数（`effect` 轨专用）。
    pub effect: Option<TextNoiseEffect>,
}

/// 自己加上去的一条轨道上的片段：放的一样是素材库里的某一项。
///
/// 主轨道（音乐 / 谱面 / 曲绘）的长度跟着音乐走；这些轨道的长度由 `length` 自己定，
/// 所以能比音乐还长（比如把曲绘铺到曲子结束之后）。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct DraftLane {
    pub id: String,
    /// 轨道名，空着时界面显示「轨道 n」。
    pub name: String,
    /// 轨道上的素材；空着就是还没放东西。
    pub media: Option<DraftMedia>,
    /// 片段起点（秒）。
    pub start: f64,
    /// 片段长度（秒）。
    pub length: f64,
    /// 这条轨道放什么：`image` 图片 / `text` 文字 / `audio` 音频 / `effect` 特效。
    /// 往上加的轨道只能放图片或文字，往下加的只能放音频。
    #[serde(default = "default_lane_kind")]
    pub kind: String,
    /// 文字轨上的文字（只有 `text` 用）。
    #[serde(default)]
    pub text: String,
    /// 特效轨的老字段：只有一段时（老草稿）参数写在这儿，载入时迁进 `clips`。
    #[serde(default)]
    pub effect: Option<TextNoiseEffect>,
    /// 特效轨的片段列表（一行可以放好几段）。
    #[serde(default)]
    pub clips: Vec<DraftClip>,
}

fn default_lane_kind() -> String {
    "image".to_string()
}

/// 手工写草稿 JSON 时不该被旧字段卡住，所以除了 id 之外都给默认值。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct EditorDraft {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// 创建 / 最后修改时间，Unix 毫秒（前端自己按本地时区显示）。
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub updated: u64,
    /// 谱面信息（曲名 / 难度 / 谱师 / 曲师），由用户自己填。
    #[serde(default)]
    pub info: DraftInfo,
    /// 草稿目录的绝对路径，只给界面显示用。写盘时会被清空，所以不进 draft.json。
    #[serde(default)]
    pub dir: String,
    /// 三条主轨道上的素材；空着就是那条轨道上还没放东西。
    #[serde(default)]
    pub chart: Option<DraftMedia>,
    #[serde(default)]
    pub music: Option<DraftMedia>,
    #[serde(default)]
    pub cover: Option<DraftMedia>,
    /// 自己加的轨道（主轨道之外），长度不受音乐限制。
    #[serde(default)]
    pub lanes: Vec<DraftLane>,
    /// 导入过的全部素材（素材库）。轨道上那三项是这里面的引用。
    #[serde(default)]
    pub media: Vec<DraftMedia>,
    /// 素材库里的文件夹（只有分组作用，素材还在原来的位置）。
    #[serde(default)]
    pub folders: Vec<DraftFolder>,
    /// 老草稿的噪域材质：新草稿的材质存在每条特效轨的 `effect.material` 上，
    /// 这一份只在读老草稿时给轨道兜底、并当新轨道的默认值。
    #[serde(default)]
    pub noise: Option<NoiseMaterial>,
}

impl EditorDraft {
    /// 把某一项素材放到它对应的轨道上（按 `kind` 分派）。
    ///
    /// 同时把它挂进素材库：面板上拖进来的文件不一定先经过「导入」这一步。
    pub fn assign(&mut self, media: DraftMedia) {
        match media.kind.as_str() {
            KIND_CHART => self.chart = Some(media.clone()),
            KIND_MUSIC => self.music = Some(media.clone()),
            KIND_COVER => self.cover = Some(media.clone()),
            _ => return,
        }
        if !self.media.iter().any(|it| it.path == media.path && it.kind == media.kind) {
            self.media.push(media);
        }
    }
}

/// 草稿根目录：`<dpe>/editor/drafts`（没有就建）。
pub fn drafts_dir() -> Result<PathBuf> {
    let dir = common::editor_dir()?.join("drafts");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建草稿目录失败：{}", dir.display()))?;
    Ok(dir)
}

/// 草稿 id 就是目录名，只允许这几类字符 —— 不然 `id` 里掺进 `..` 就能读写到别处去。
fn check_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 64 {
        bail!("非法的草稿 id：{id}");
    }
    if !id
        .chars()
        .all(|it| it.is_ascii_alphanumeric() || it == '-' || it == '_')
    {
        bail!("非法的草稿 id：{id}");
    }
    Ok(())
}

fn draft_file(id: &str) -> Result<PathBuf> {
    check_id(id)?;
    Ok(drafts_dir()?.join(id).join("draft.json"))
}

/// 当前时间（Unix 毫秒）。
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|it| it.as_millis() as u64)
        .unwrap_or(0)
}

/// 读一个草稿。
pub fn load(id: &str) -> Result<EditorDraft> {
    let file = draft_file(id)?;
    let text = std::fs::read_to_string(&file)
        .with_context(|| format!("读取草稿失败：{}", file.display()))?;
    let mut draft: EditorDraft = serde_json::from_str(&text)
        .with_context(|| format!("草稿文件不是合法的 JSON：{}", file.display()))?;
    draft.dir = drafts_dir()?.join(id).to_string_lossy().to_string();
    normalize_folders(&mut draft);
    normalize_dirs(&mut draft);
    normalize_lanes(&mut draft);
    normalize_noise(&mut draft);
    Ok(draft)
}

/// 自己加的轨道：时间参数只认有限的正数（手改过草稿时别让 NaN 一路传到界面上算宽度）。
/// 指向目录的项也摘掉，理由和音乐 / 曲绘轨道一样 —— 目录不知道该放哪份文件。
/// 种类只认四个值，别的（老草稿、手写错的）当图片轨；文字轨与特效轨不挂素材文件。
fn normalize_lanes(draft: &mut EditorDraft) {
    for lane in &mut draft.lanes {
        if !lane.start.is_finite() || lane.start < 0.0 {
            lane.start = 0.0;
        }
        if !lane.length.is_finite() || lane.length < 0.0 {
            lane.length = 0.0;
        }
        if !matches!(lane.kind.as_str(), "image" | "text" | "audio" | "effect") {
            lane.kind = default_lane_kind();
        }
        if lane.kind == "text"
            || lane.kind == "effect"
            || lane.media.as_ref().is_some_and(|it| Path::new(&it.path).is_dir())
        {
            lane.media = None;
        }
        if lane.kind == "effect" {
            if lane.clips.is_empty() {
                // 老草稿的轨道上只有一段：起止与参数都在轨道自己身上，迁成一个片段
                if let Some(effect) = lane.effect.take() {
                    lane.clips.push(DraftClip {
                        id: format!("{}-clip", lane.id),
                        start: lane.start,
                        length: lane.length,
                        effect: Some(effect),
                    });
                }
            } else {
                lane.effect = None;
            }
            normalize_clips(lane);
        } else {
            // 换过种类的轨道（比如手改 JSON）别把上一个种类的参数带在身上
            lane.effect = None;
            lane.clips.clear();
        }
    }
    draft.lanes.retain(|lane| !lane.id.is_empty());
}

/// 特效轨的片段：时间收成有限正数、参数逐段收范围，id 重复或空着就补一个。
fn normalize_clips(lane: &mut DraftLane) {
    let mut seen: Vec<String> = Vec::new();
    for (index, clip) in lane.clips.iter_mut().enumerate() {
        if !clip.start.is_finite() || clip.start < 0.0 {
            clip.start = 0.0;
        }
        if !clip.length.is_finite() || clip.length < 0.01 {
            clip.length = 0.01;
        }
        if clip.id.is_empty() || seen.contains(&clip.id) {
            clip.id = format!("{}-c{}", lane.id, index + 1);
        }
        seen.push(clip.id.clone());
        normalize_effect(clip.effect.get_or_insert_with(TextNoiseEffect::default));
    }
}

/// 噪域材质：噪域方块里那层噪声长什么样，一条特效轨一份。
///
/// 引擎按方块带材质（每个噪域条目可以带上自己那份），所以同一张谱面里两条特效轨
/// 能用不同的材质；没写进条目的键按渲染配置走。字段名与 `RenderConfig` 的 camelCase 键一致，
/// 前后端不用再做一层映射。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct NoiseMaterial {
    pub block_area_noise_blotch: bool,
    pub block_area_spark: bool,
    pub block_area_distort: bool,
    pub block_area_displace_strength: f32,
    pub block_area_noise_radius: f32,
    pub block_area_noise_smoothness: f32,
    pub block_area_displace_speed: f32,
    pub block_area_edge_noise: f32,
    pub block_area_density: f32,
    pub block_area_pixel_scale: f32,
    pub simplified_block_area: bool,
    pub block_area_hover: bool,
    pub block_area_hover_radius: f32,
    pub block_area_alpha: f32,
}

impl Default for NoiseMaterial {
    fn default() -> Self {
        Self {
            block_area_noise_blotch: true,
            block_area_spark: true,
            block_area_distort: true,
            block_area_displace_strength: 0.15,
            block_area_noise_radius: 0.48,
            block_area_noise_smoothness: 1.0,
            block_area_displace_speed: 1.5,
            block_area_edge_noise: 1.0,
            block_area_density: 2.5,
            block_area_pixel_scale: 6.0,
            simplified_block_area: false,
            block_area_hover: true,
            block_area_hover_radius: 0.5,
            block_area_alpha: 0.25,
        }
    }
}

/// 特效参数：越界或非有限的值收进可用范围，别让噪域一格都画不出来（或一次画几十万个）。
fn normalize_effect(effect: &mut TextNoiseEffect) {
    let fallback = TextNoiseEffect::default();
    effect.font_size = clamp_or(effect.font_size, 0.005, 2.0, fallback.font_size);
    effect.cell = clamp_or(effect.cell, 0.002, 0.5, fallback.cell);
    effect.x = clamp_or(effect.x, 0.0, 1.0, fallback.x);
    effect.y = clamp_or(effect.y, 0.0, 1.0, fallback.y);
    effect.line_height = clamp_or(effect.line_height, 0.5, 4.0, fallback.line_height);
    effect.letter_spacing = clamp_or(effect.letter_spacing, -1.0, 3.0, fallback.letter_spacing);
    effect.rotation = clamp_or(effect.rotation, -360.0, 360.0, fallback.rotation);
    effect.color = normalize_color(&effect.color);
    if let Some(material) = effect.material.as_mut() {
        normalize_material(material);
    }
}

/// 颜色收成大写 `#RRGGBB`：只认 3 / 6 位十六进制，认不出来就退回原生红。
///
/// 与前端 `text-noise.ts::normalizeNoiseColor` 同一套规则；两边各收一道，
/// 免得手改过的草稿 JSON 把一个引擎解析不了的颜色写进谱面。
fn normalize_color(color: &str) -> String {
    let digits = color.trim().trim_start_matches('#');
    let hex: Vec<u8> = digits.bytes().collect();
    let upper = digits.to_ascii_uppercase();
    if hex.len() == 6 && hex.iter().all(|it| it.is_ascii_hexdigit()) {
        return format!("#{upper}");
    }
    if hex.len() == 3 && hex.iter().all(|it| it.is_ascii_hexdigit()) {
        let doubled: String = upper.chars().flat_map(|it| [it, it]).collect();
        return format!("#{doubled}");
    }
    NATIVE_NOISE_COLOR.to_string()
}

/// 噪域材质：数值收进渲染页允许的范围。
fn normalize_material(noise: &mut NoiseMaterial) {
    let fallback = NoiseMaterial::default();
    noise.block_area_displace_strength = clamp_or32(noise.block_area_displace_strength, 0.0, 2.0, fallback.block_area_displace_strength);
    noise.block_area_noise_radius = clamp_or32(noise.block_area_noise_radius, 0.0, 2.0, fallback.block_area_noise_radius);
    noise.block_area_noise_smoothness = clamp_or32(noise.block_area_noise_smoothness, 0.0, 8.0, fallback.block_area_noise_smoothness);
    noise.block_area_displace_speed = clamp_or32(noise.block_area_displace_speed, 0.0, 10.0, fallback.block_area_displace_speed);
    noise.block_area_edge_noise = clamp_or32(noise.block_area_edge_noise, 0.0, 4.0, fallback.block_area_edge_noise);
    noise.block_area_density = clamp_or32(noise.block_area_density, 0.1, 20.0, fallback.block_area_density);
    noise.block_area_pixel_scale = clamp_or32(noise.block_area_pixel_scale, 0.2, 64.0, fallback.block_area_pixel_scale);
    noise.block_area_hover_radius = clamp_or32(noise.block_area_hover_radius, 0.0, 3.0, fallback.block_area_hover_radius);
    noise.block_area_alpha = clamp_or32(noise.block_area_alpha, 0.0, 1.0, fallback.block_area_alpha);
    // 「简化噪域」与「打击光晕」不在编辑器的可调项里（简化分支会关掉火花 / 斑点 / 扭曲，
    // 而打击光晕要手指按在画面上才亮）。老草稿里勾过的值在这里归位，免得存着的 true
    // 让预览看起来「没开火花」。
    noise.simplified_block_area = false;
    noise.block_area_hover = false;
}

/// 老草稿上那份「整张谱面共用」的材质：没有特效轨就丢掉（没有噪域可画，留着只会
/// 在导出包里多一份用不上的配置）；有的话仍然收一遍范围，界面上会拿它给新轨道当默认值。
fn normalize_noise(draft: &mut EditorDraft) {
    if !draft.lanes.iter().any(|lane| lane.kind == "effect") {
        draft.noise = None;
        return;
    }
    if let Some(noise) = draft.noise.as_mut() {
        normalize_material(noise);
    }
}

fn clamp_or32(value: f32, low: f32, high: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(low, high)
    } else {
        fallback
    }
}

fn clamp_or(value: f64, low: f64, high: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value.clamp(low, high)
    } else {
        fallback
    }
}

/// 把素材库的文件夹关系修一遍：上级已经不在了（手改过草稿 JSON、或者旧草稿）就退回根上，
/// 指着不存在的文件夹的素材也摊回根上 —— 否则那一层钻不进去，素材看着像丢了。
fn normalize_folders(draft: &mut EditorDraft) {
    let ids: std::collections::HashSet<String> =
        draft.folders.iter().map(|it| it.id.clone()).collect();
    for folder in &mut draft.folders {
        let dangling = folder
            .parent
            .as_ref()
            .is_some_and(|parent| parent == &folder.id || !ids.contains(parent));
        if dangling {
            folder.parent = None;
        }
    }
    for item in &mut draft.media {
        if item.folder.as_ref().is_some_and(|folder| !ids.contains(folder)) {
            item.folder = None;
        }
    }
}

/// 把「拿目录当谱面」的旧草稿就地摊平：谱面轨道换成目录里真正的那份谱面文件，
/// 素材库里的目录项摊成包里的各个文件。
///
/// 草稿只记路径，所以这里认不出来的（包已经被挪走等）直接丢掉，不打断读取。
/// 只动轨道与素材库的内容，**不替用户决定别的轨道放什么**。
fn normalize_dirs(draft: &mut EditorDraft) {
    let has_slot_dir = [&draft.chart, &draft.music, &draft.cover]
        .into_iter()
        .flatten()
        .any(|it| Path::new(&it.path).is_dir());
    let has_media_dir = draft.media.iter().any(|it| Path::new(&it.path).is_dir());
    if !has_slot_dir && !has_media_dir {
        return;
    }
    let sources = std::mem::take(&mut draft.media);
    for item in sources {
        if !Path::new(&item.path).is_dir() {
            push_media(&mut draft.media, item);
            continue;
        }
        // 旧草稿把目录直接挂进了素材库：摊成这一层里的各个文件（包里再深的部分也归到同一层）
        let found = media_import_in_dir(Path::new(&item.path)).unwrap_or_default();
        for mut at in found {
            at.media.folder = item.folder.clone();
            push_media(&mut draft.media, at.media);
        }
    }
    if let Some(old) = draft.chart.take_if(|it| Path::new(&it.path).is_dir()) {
        draft.chart = chart_file_in(Path::new(&old.path))
            .and_then(|file| media_from_path(&file).ok());
    }
    // 音乐 / 曲绘轨道上挂的是目录时没有靠谱的对应文件，只能摘掉让用户自己放。
    if draft.music.as_ref().is_some_and(|it| Path::new(&it.path).is_dir()) {
        draft.music = None;
    }
    if draft.cover.as_ref().is_some_and(|it| Path::new(&it.path).is_dir()) {
        draft.cover = None;
    }
}

fn push_media(media: &mut Vec<DraftMedia>, item: DraftMedia) {
    if !media
        .iter()
        .any(|it| it.kind == item.kind && it.path == item.path)
    {
        media.push(item);
    }
}

/// 写回一个草稿（顺手把 `updated` 刷成现在）。
pub fn save(mut draft: EditorDraft) -> Result<EditorDraft> {
    check_id(&draft.id)?;
    draft.updated = now_ms();
    if draft.name.trim().is_empty() {
        draft.name = "未命名草稿".to_string();
    }
    let dir = drafts_dir()?.join(&draft.id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建草稿目录失败：{}", dir.display()))?;
    // `dir` 只是算出来的显示值：存盘前清掉，免得草稿 JSON 绑死在这台机器的路径上。
    let mut stored = draft.clone();
    stored.dir = String::new();
    let text = serde_json::to_string_pretty(&stored)?;
    let file = dir.join("draft.json");
    // 覆盖前把上一版留一份：改坏了还能从 `draft.bak.json` 翻回来。
    if file.exists() {
        if let Ok(previous) = std::fs::read_to_string(&file) {
            if previous != text {
                std::fs::write(dir.join("draft.bak.json"), previous).ok();
            }
        }
    }
    tracing::info!(
        "保存草稿 {}：谱面={:?} 音乐={:?} 曲绘={:?}，素材 {} 项",
        stored.id,
        stored.chart.as_ref().map(|it| it.name.as_str()),
        stored.music.as_ref().map(|it| it.name.as_str()),
        stored.cover.as_ref().map(|it| it.name.as_str()),
        stored.media.len(),
    );
    std::fs::write(&file, text)
        .with_context(|| format!("写入草稿失败：{}", dir.display()))?;
    draft.dir = dir.to_string_lossy().to_string();
    Ok(draft)
}

/// 全部草稿，最近改过的排在前面。单个草稿读不出来只跳过它，不让整张列表失败。
pub fn list() -> Result<Vec<EditorDraft>> {
    let dir = drafts_dir()?;
    let mut drafts = Vec::new();
    for entry in
        std::fs::read_dir(&dir).with_context(|| format!("读取草稿目录失败：{}", dir.display()))?
    {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        let Some(id) = path.file_name().map(|it| it.to_string_lossy().to_string()) else {
            continue;
        };
        match load(&id) {
            Ok(draft) => drafts.push(draft),
            Err(err) => warn!(
                "[编辑窗口] 跳过读不出来的草稿 {}：{}",
                path.display(),
                common::error_chain(&err)
            ),
        }
    }
    drafts.sort_by(|a, b| b.updated.cmp(&a.updated));
    Ok(drafts)
}

/// 新建一个空草稿。名字缺省时按「草稿 N」自动编号。
pub fn create(name: Option<String>) -> Result<EditorDraft> {
    let now = now_ms();
    let id = format!("{now:x}");
    // 同一毫秒里连点两次「新建」（或者两个窗口同时建）时给 id 加个尾巴。
    // 用 `create_dir` 抢目录：它要么原子地建出来、要么报「已存在」，比先 exists 再建更稳。
    let mut unique = id.clone();
    let mut suffix = 1;
    loop {
        let dir = drafts_dir()?.join(&unique);
        match std::fs::create_dir(&dir) {
            Ok(()) => break,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                suffix += 1;
                unique = format!("{id}-{suffix}");
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("创建草稿目录失败：{}", dir.display()))
            }
        }
    }

    let name = name.filter(|it| !it.trim().is_empty()).unwrap_or_else(|| {
        let n = list().map(|it| it.len()).unwrap_or(0) + 1;
        format!("草稿 {n}")
    });

    save(EditorDraft {
        id: unique,
        name,
        created: now,
        updated: now,
        info: DraftInfo::default(),
        dir: String::new(),
        chart: None,
        music: None,
        cover: None,
        lanes: Vec::new(),
        media: Vec::new(),
        folders: Vec::new(),
        noise: None,
    })
}

/// 删掉一个草稿（连同它的目录）。
pub fn remove(id: &str) -> Result<()> {
    let dir = drafts_dir()?.join(id);
    check_id(id)?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("删除草稿失败：{}", dir.display()))?;
    }
    Ok(())
}

/// 找「已经引用了这份谱面」的草稿：主窗口再点同一张谱面时用它，而不是又建一个新草稿。
pub fn find_by_chart(path: &str) -> Option<EditorDraft> {
    list()
        .ok()?
        .into_iter()
        .find(|it| it.chart.as_ref().is_some_and(|chart| chart.path == path))
}

/// 按扩展名判断一份文件能落到哪条轨道上；判断不出来的返回 `None`。
///
/// 谱面除了 `.pez` / `.zip` 之外还认 `.mc` / `.pec` / `.json` / `.yaml`（单文件谱面）。
/// 目录**永远不算素材**：一个谱面包摊开后是「谱面 json + 音频 + 曲绘」三份文件，
/// 而不是一个文件夹在轨道上代表整张谱面。
///
/// 歌词（[`KIND_LYRIC`]）只进素材库，铺到时间轴上时才按里面的时间拆成一条特效轨。
pub fn classify(path: &Path) -> Option<&'static str> {
    if path.is_dir() {
        return None;
    }
    let ext = path
        .extension()
        .map(|it| it.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    // 歌词的扩展名只在 `lyrics` 里列一次（那边还要按扩展名挑解析器），这里不重复
    if lyrics::is_lyric(path) {
        return Some(KIND_LYRIC);
    }
    match ext.as_str() {
        "pez" | "zip" | "mc" | "pec" | "json" | "yaml" | "yml" => Some(KIND_CHART),
        "mp3" | "ogg" | "wav" | "flac" | "m4a" | "aac" | "opus" => Some(KIND_MUSIC),
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" => Some(KIND_COVER),
        _ => None,
    }
}

/// 把一条磁盘路径包成素材项；不是文件、或者认不出种类时报错（由前端提示）。
pub fn media_from_path(path: &Path) -> Result<DraftMedia> {
    if !path.exists() {
        bail!("文件不存在：{}", path.display());
    }
    if path.is_dir() {
        bail!("文件夹本身不能当素材：{}（往里挑文件，或用「导入谱面文件夹」）", path.display());
    }
    let Some(kind) = classify(path) else {
        bail!(
            "认不出这个文件能放到哪条轨道上：{}",
            path.display()
        );
    };
    let name = path
        .file_name()
        .map(|it| it.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    Ok(DraftMedia {
        path: path.to_string_lossy().to_string(),
        name,
        kind: kind.to_string(),
        folder: None,
    })
}

/// 是不是压缩包：`.pez` 与 `.zip` 都是 zip，素材库里不按文件出现，解开成一层文件夹。
fn is_archive(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|it| it.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    matches!(ext.as_str(), "pez" | "zip")
}

/// 把一个压缩包里的素材摊出来：`dir` 的第一段是包名（去掉扩展名），往里照包里的层级。
///
/// 包里的东西先解到解包缓存（[`common::unpack_editor_archive`]），素材记的是缓存里的真实文件，
/// 这样素材行点「定位」、预览读字节都跟普通文件一样。
fn archive_media(archive: &Path) -> Result<Vec<MediaInDir>> {
    let unpacked = common::unpack_editor_archive(archive)?;
    let stem = archive
        .file_stem()
        .map(|it| it.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(media_tree_in_dir(&unpacked)?
        .into_iter()
        .map(|at| MediaInDir { media: at.media, dir: join_dir(&stem, &at.dir) })
        .collect())
}

/// 把两段 `/` 分隔的相对目录接起来（有一段是空串就取另一段）。
fn join_dir(outer: &str, inner: &str) -> String {
    match (outer.is_empty(), inner.is_empty()) {
        (true, _) => inner.to_string(),
        (_, true) => outer.to_string(),
        _ => format!("{outer}/{inner}"),
    }
}

/// 按相对目录、种类、名字排一遍：同一层里谱面在前，前端建树时顺序稳定。
fn sorted_media(mut out: Vec<MediaInDir>) -> Vec<MediaInDir> {
    out.sort_by(|a, b| {
        a.dir
            .cmp(&b.dir)
            .then_with(|| a.media.kind.cmp(&b.media.kind))
            .then_with(|| a.media.name.cmp(&b.media.name))
    });
    out
}

/// 「导入素材 → 选文件夹」收进来的素材：相对这个目录的层级，压缩包在里面会解开成一层文件夹。
///
/// 跳过以 `.` 开头的项，往后最多翻 [`MAX_MEDIA_DEPTH`] 层。目录本身不往上列（谱面轨道只收文件）。
pub fn media_import_in_dir(dir: &Path) -> Result<Vec<MediaInDir>> {
    if !dir.is_dir() {
        bail!("不是文件夹：{}", dir.display());
    }
    let mut out = Vec::new();
    collect_import(dir, dir, 0, &mut out);
    Ok(sorted_media(out))
}

fn collect_import(root: &Path, dir: &Path, depth: usize, out: &mut Vec<MediaInDir>) {
    if depth > MAX_MEDIA_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let rel = rel_dir(root, dir);
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_import(root, &path, depth + 1, out);
            continue;
        }
        if is_archive(&path) {
            match archive_media(&path) {
                Ok(mut inner) => {
                    for at in &mut inner {
                        at.dir = join_dir(&rel, &at.dir);
                    }
                    out.extend(inner);
                }
                // 扩展名是 .pez / .zip 但内容不是 zip：当一份谱面文件收着，至少还能拖上轨道
                Err(err) => {
                    warn!("[编辑窗口] 解不开 {}：{err}", path.display());
                    if let Ok(media) = media_from_path(&path) {
                        out.push(MediaInDir { media, dir: rel.clone() });
                    }
                }
            }
            continue;
        }
        if let Ok(media) = media_from_path(&path) {
            out.push(MediaInDir { media, dir: rel.clone() });
        }
    }
}

/// 相对 `root` 的目录（用 `/` 拼，空串就是 `root` 自己）。
fn rel_dir(root: &Path, dir: &Path) -> String {
    dir.strip_prefix(root)
        .unwrap_or_else(|_| Path::new(""))
        .components()
        .map(|it| it.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// 「导入素材」挑中的路径（文件 / 文件夹 / 压缩包）都包成素材项，只记路径、不复制文件。
///
/// 文件夹与压缩包都带层：文件夹按自己的目录名当第一级，压缩包按包名当第一级，往里照原样嵌套；
/// 单个文件摊在根上（`dir` 是空串）。
pub fn media_import(paths: &[String]) -> Result<Vec<MediaInDir>> {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        let root = Path::new(path);
        if root.is_dir() {
            let name = root
                .file_name()
                .map(|it| it.to_string_lossy().into_owned())
                .unwrap_or_default();
            match media_import_in_dir(root) {
                Ok(found) if !found.is_empty() => out.extend(found.into_iter().map(|at| {
                    MediaInDir { media: at.media, dir: join_dir(&name, &at.dir) }
                })),
                Ok(_) => errors.push(format!("{} 里没有能放上轨道的素材", root.display())),
                Err(err) => errors.push(common::error_chain(&err)),
            }
            continue;
        }
        if is_archive(root) {
            match archive_media(root) {
                Ok(found) if !found.is_empty() => out.extend(found),
                Ok(_) => errors.push(format!("{} 里没有能放上轨道的素材", root.display())),
                Err(err) => errors.push(common::error_chain(&err)),
            }
            continue;
        }
        match media_from_path(root) {
            Ok(media) => out.push(MediaInDir { media, dir: String::new() }),
            Err(err) => errors.push(common::error_chain(&err)),
        }
    }
    if out.is_empty() {
        bail!(
            "没认出可以导入的文件：{}",
            errors.first().cloned().unwrap_or_default()
        );
    }
    for err in &errors {
        warn!("[编辑窗口] 导入时跳过：{err}");
    }
    Ok(sorted_media(out))
}

/// 「导入素材 → 选文件夹」拿回来的一项：素材本身，加上它在选中目录下的相对目录。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct MediaInDir {
    pub media: DraftMedia,
    /// 相对目录（`/` 分隔）：空串是选中目录本身，其余形如 `专辑/曲目`。
    pub dir: String,
}

/// 一个目录里认得出的素材，连同它们各自所在的相对目录（压缩包不再往里解，当谱面文件收）。
///
/// 解包缓存里的那份谱面包（见 [`archive_media`]）用它摊开：素材库照这份相对目录建真实层级，
/// 往后最多翻 [`MAX_MEDIA_DEPTH`] 层，跳过以 `.` 开头的项。
pub fn media_tree_in_dir(dir: &Path) -> Result<Vec<MediaInDir>> {
    if !dir.is_dir() {
        bail!("不是文件夹：{}", dir.display());
    }
    let mut out = Vec::new();
    collect_media_tree(dir, dir, 0, &mut out);
    Ok(sorted_media(out))
}

fn collect_media_tree(root: &Path, dir: &Path, depth: usize, out: &mut Vec<MediaInDir>) {
    if depth > MAX_MEDIA_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let rel = rel_dir(root, dir);
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_media_tree(root, &path, depth + 1, out);
        } else if let Ok(media) = media_from_path(&path) {
            out.push(MediaInDir { media, dir: rel.clone() });
        }
    }
}

/// 在谱面包（目录）里找出真正的那份谱面文件：优先 `chart.json` / `chart.pec`，
/// 否则取目录里第一个谱面文件（`json` / `pec` / `mc` 等，按名字排序取第一个）。
///
/// 谱面轨道上放的永远是**文件**，不是目录 —— 目录只是「从哪里找」。
pub fn chart_file_in(dir: &Path) -> Option<PathBuf> {
    let mut found = Vec::new();
    collect_chart_files(dir, 0, &mut found);
    found.sort();
    let preferred = ["chart.json", "chart.pec", "chart.yaml", "chart.yml"];
    for want in preferred {
        if let Some(hit) = found
            .iter()
            .find(|it| it.file_name().is_some_and(|name| name.eq_ignore_ascii_case(want)))
        {
            return Some(hit.clone());
        }
    }
    found.into_iter().next()
}

fn collect_chart_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_MEDIA_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_chart_files(&path, depth + 1, out);
        } else if classify(&path) == Some(KIND_CHART) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    //! 只测纯逻辑与一次落盘往返。草稿目录跟着 exe 走（`<exe 目录>/dpe/editor/drafts`），
    //! 所以测试里建的那份也在 `target/…/deps` 下的临时位置，用完自己删掉。

    use super::*;

    fn media(kind: &str, path: &str) -> DraftMedia {
        DraftMedia {
            path: path.to_string(),
            name: path.rsplit(['/', '\\']).next().unwrap_or(path).to_string(),
            kind: kind.to_string(),
            folder: None,
        }
    }

    #[test]
    fn normalize_color_writes_uppercase_hex() {
        assert_eq!(normalize_color("#e61a1a"), "#E61A1A");
        assert_eq!(normalize_color("e61a1a"), "#E61A1A");
        assert_eq!(normalize_color(" #E61A1A "), "#E61A1A");
        // 三位缩写补成六位，同样大写
        assert_eq!(normalize_color("#f0a"), "#FF00AA");
        // 认不出来退回原生红
        assert_eq!(normalize_color("red"), NATIVE_NOISE_COLOR);
        assert_eq!(NATIVE_NOISE_COLOR, "#E61A1A");
    }

    #[test]
    fn classify_by_extension_never_accepts_dir() {
        let dir = std::env::temp_dir();
        assert_eq!(classify(&dir), None);
        assert!(media_from_path(&dir).is_err());
        assert_eq!(classify(Path::new("a/b.pez")), Some(KIND_CHART));
        assert_eq!(classify(Path::new("a/b.ZIP")), Some(KIND_CHART));
        assert_eq!(classify(Path::new("a/b.mp3")), Some(KIND_MUSIC));
        assert_eq!(classify(Path::new("a/b.PNG")), Some(KIND_COVER));
        assert_eq!(classify(Path::new("a/b.lrc")), Some(KIND_LYRIC));
        assert_eq!(classify(Path::new("a/b.SRT")), Some(KIND_LYRIC));
        assert_eq!(classify(Path::new("a/b.txt")), None);
    }

    #[test]
    fn check_id_blocks_path_traversal() {
        assert!(check_id("1700000000000").is_ok());
        assert!(check_id("abc-1_2").is_ok());
        assert!(check_id("").is_err());
        assert!(check_id("../x").is_err());
        assert!(check_id("a/b").is_err());
        assert!(check_id(&"a".repeat(65)).is_err());
    }

    #[test]
    fn assign_puts_media_on_its_track_once() {
        let mut draft = EditorDraft {
            id: "t".to_string(),
            name: String::new(),
            created: 0,
            updated: 0,
            info: DraftInfo::default(),
            dir: String::new(),
            chart: None,
            music: None,
            cover: None,
            lanes: Vec::new(),
            media: Vec::new(),
            folders: Vec::new(),
            noise: None,
        };
        draft.assign(media(KIND_MUSIC, "C:/a/song.ogg"));
        draft.assign(media(KIND_CHART, "C:/a/chart.pez"));
        draft.assign(media(KIND_MUSIC, "C:/a/song.ogg"));
        assert_eq!(draft.music.as_ref().map(|it| it.path.as_str()), Some("C:/a/song.ogg"));
        assert_eq!(draft.chart.as_ref().map(|it| it.path.as_str()), Some("C:/a/chart.pez"));
        // 同一份素材重复放上去不该在素材库里出现两次
        assert_eq!(draft.media.len(), 2);
        // 认不出种类的直接忽略
        draft.assign(media("nope", "C:/a/x.txt"));
        assert_eq!(draft.media.len(), 2);
    }

    #[test]
    fn draft_json_tolerates_old_shape() {
        let draft: EditorDraft = serde_json::from_str(r#"{"id":"abc"}"#).unwrap();
        assert_eq!(draft.id, "abc");
        assert!(draft.name.is_empty());
        assert!(draft.chart.is_none());
        assert!(draft.media.is_empty());
    }

    #[test]
    fn save_load_list_remove_roundtrip() {
        let mut draft = create(Some("测试草稿".to_string())).unwrap();
        draft.assign(media(KIND_COVER, "C:/a/cover.png"));
        let saved = save(draft).unwrap();
        assert_eq!(saved.name, "测试草稿");
        assert!(saved.updated >= saved.created);

        let loaded = load(&saved.id).unwrap();
        assert_eq!(loaded.name, "测试草稿");
        assert_eq!(loaded.cover.map(|it| it.path), Some("C:/a/cover.png".to_string()));

        assert!(list().unwrap().iter().any(|it| it.id == saved.id));
        assert!(find_by_chart("C:/a/chart.pez").is_none());

        remove(&saved.id).unwrap();
        assert!(load(&saved.id).is_err());
    }

    #[test]
    fn old_dir_chart_is_flattened_on_load() {
        let root = std::env::temp_dir().join(format!("dpe-normalize-{}", now_ms()));
        let pkg = root.join("pkg");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("chart.json"), b"{}").unwrap();
        std::fs::write(pkg.join("song.ogg"), b"x").unwrap();
        std::fs::write(pkg.join("cover.png"), b"x").unwrap();

        // 旧草稿：谱面轨道上挂的是目录本身
        let mut draft = create(Some("旧草稿".to_string())).unwrap();
        draft.chart = Some(DraftMedia {
            path: pkg.to_string_lossy().to_string(),
            name: "pkg".to_string(),
            kind: KIND_CHART.to_string(),
            folder: None,
        });
        draft.media.push(draft.chart.clone().unwrap());
        let saved = save(draft).unwrap();

        let loaded = load(&saved.id).unwrap();
        let chart = loaded.chart.expect("谱面轨道应该还在");
        assert_eq!(chart.path, pkg.join("chart.json").to_string_lossy());
        assert!(loaded.media.iter().all(|it| !Path::new(&it.path).is_dir()));
        assert!(loaded.media.iter().any(|it| it.kind == KIND_MUSIC));
        assert!(loaded.media.iter().any(|it| it.kind == KIND_COVER));
        // 空着的音乐 / 曲绘轨道不会被包里那份填上：放哪首、哪张由用户自己挑
        assert!(loaded.music.is_none());
        assert!(loaded.cover.is_none());

        remove(&saved.id).unwrap();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn empty_name_falls_back() {
        let mut draft = create(None).unwrap();
        draft.name = "   ".to_string();
        let saved = save(draft).unwrap();
        assert_eq!(saved.name, "未命名草稿");
        remove(&saved.id).unwrap();
    }

    #[test]
    fn media_tree_in_dir_keeps_nesting() {
        // 三层嵌套：根上一份音频、`专辑/` 里一张曲绘、`专辑/曲目/` 里一份谱面
        let root = std::env::temp_dir().join(format!("dpe-media-tree-{}", now_ms()));
        let nested = root.join("专辑").join("曲目");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("song.mp3"), b"x").unwrap();
        std::fs::write(root.join("专辑").join("cover.png"), b"x").unwrap();
        std::fs::write(nested.join("chart.json"), b"{}").unwrap();
        std::fs::write(nested.join("note.txt"), b"x").unwrap();

        let found = media_tree_in_dir(&root).unwrap();
        std::fs::remove_dir_all(&root).ok();

        // 相对目录用 `/` 分隔，根上是空串；认不出的扩展名（note.txt）不收
        let pairs: Vec<(&str, &str)> =
            found.iter().map(|it| (it.media.name.as_str(), it.dir.as_str())).collect();
        assert_eq!(pairs, vec![("song.mp3", ""), ("cover.png", "专辑"), ("chart.json", "专辑/曲目")]);
        assert_eq!(found[2].media.kind, KIND_CHART);
        assert_eq!(found[2].media.folder, None);
    }

    #[test]
    fn archives_are_unpacked_into_a_folder() {
        // 一个谱面包：谱面在根上，曲绘在包里的子目录里
        let root = std::env::temp_dir().join(format!("dpe-media-zip-{}", now_ms()));
        std::fs::create_dir_all(&root).unwrap();
        let archive = root.join("曲目.pez");
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        {
            let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
            writer.start_file("chart.json", options).unwrap();
            std::io::Write::write_all(&mut writer, b"{}").unwrap();
            writer.start_file("img/cover.png", options).unwrap();
            std::io::Write::write_all(&mut writer, b"x").unwrap();
            writer.finish().unwrap();
        }

        let found = media_import(&[archive.to_string_lossy().into_owned()]).unwrap();
        // 包本身不出现在素材库里：包名是文件夹的第一段，再往里照包里的层级
        let pairs: Vec<(&str, &str)> =
            found.iter().map(|it| (it.media.name.as_str(), it.dir.as_str())).collect();
        assert_eq!(pairs, vec![("chart.json", "曲目"), ("cover.png", "曲目/img")]);
        assert_eq!(found[0].media.kind, KIND_CHART);
        assert!(found.iter().all(|it| !it.media.path.ends_with(".pez")));

        // 选文件夹时混在里面的压缩包也一样处理，位置接在所在目录后面
        let picked = media_import_in_dir(&root).unwrap();
        assert_eq!(picked.len(), found.len());
        assert!(picked.iter().all(|it| it.dir == "曲目" || it.dir.starts_with("曲目/")));

        std::fs::remove_dir_all(&root).ok();
        if let Ok(cache) = common::editor_unpacked_dir() {
            std::fs::remove_dir_all(cache).ok();
        }
    }

    #[test]
    fn load_puts_folders_with_missing_parent_back_on_root() {
        let mut draft = create(Some("文件夹".to_string())).unwrap();
        // 上级不存在的文件夹、以及指着这个文件夹的素材都退回根上
        draft.folders.push(DraftFolder {
            id: "f-child".to_string(),
            name: "子文件夹".to_string(),
            parent: Some("f-gone".to_string()),
        });
        draft.media.push(DraftMedia {
            path: "C:/a/song.ogg".to_string(),
            name: "song.ogg".to_string(),
            kind: KIND_MUSIC.to_string(),
            folder: Some("f-child".to_string()),
        });
        let saved = save(draft).unwrap();

        let loaded = load(&saved.id).unwrap();
        remove(&saved.id).unwrap();
        assert_eq!(loaded.folders.len(), 1);
        assert_eq!(loaded.folders[0].parent, None);
        assert_eq!(loaded.media[0].folder.as_deref(), Some("f-child"));
    }
}
