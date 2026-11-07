//! Malody 谱面（`.mc` / `.mcz` / 谱面文件夹）→ Phigros 谱面（装进 `.pez`）。
//!
//! 转两种模式：**Key**（4K / 5K / 7K…，「固定轨道 + 按键落在轨道上」）写 PGR v3；**Slide**
//! （触摸屏上一排可长可短的横条）写 PEC。判定线都钉在一个固定高度（换算与 DPR 的「定轨谱面」同一套，
//! 见 `DPS/dps/src/core/fixed_lane.rs`）。
//!
//! Slide 为什么写 PEC：Slide 的横条自带宽度（`x` 是中心、`w` 是宽度），PGR 的按键结构里没有「单个
//! 按键多宽」这个字段，而 PEC 的 `& <size>` 正好只缩放按键的横向（解析器写的是 `object.scale.0`，
//! 见 `DPS/dps/src/parse/pec.rs`）。
//!
//! Slide 的长条还可能分成多段（`seg` 里好多个顶点，横条会横着滑过去）：PEC 的长按是竖直不动的，
//! 表达不了斜的，所以横着走的那些沿折线撒成一串同宽的滑动键（间距取按键宽度的九成，串起来就是
//! 一条连贯的斜长条），不动的直条仍然写 `n2` 长按。
//!
//! Key 为什么不写 PEC：PEC 里按键的 `x` 按 `0~1024 → 0~1` 折算、判定线的 `cp x` 却按
//! `0~2048 → -1~1` 折算（见 `DPS/dps/src/parse/pec.rs`），两者口径不一致，照 PEC 的语义写出来的
//! 按键会整体右移半屏；PGR v3 的换算在解析器里是自洽的。Slide 写 PEC 时按引擎的实际口径
//! （按键 `x` 除以 1024）给值，落点仍然是准的。
//!
//! 时间轴：Malody 的 `beat` 是 `[拍, 分子, 分母]`，即 `拍 + 分子 / 分母` 个四分音符，按 `time[]`
//! 里的 BPM 折成秒。PGR v3 每条判定线自己带 BPM、文件里的时间单位是「1/32 拍」，所以这里先求出
//! 秒数、再按判定线的 BPM 折算回去 —— Malody 谱面中途换过 BPM 也照样对得上。
//!
//! 这个模块只管解析与生成，路径、压缩包读写都由调用方（DPR 的命令层）负责，所以它既能被
//! 预览入口编进去，也能被 `DPR/src-tauri` 直接按路径引用。

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use dps::fs::{self, FileSystem};
use serde::{Deserialize, Serialize};

/// 判定线高度、轨道间距、流速、按键宽度 —— 转换页上的参数。
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Options {
    /// 判定线钉在屏幕纵向的哪个高度：`0` 是屏幕底边、`1` 是屏幕顶边、`0.5` 是正中。
    pub line_y: f32,
    /// 相邻轨道的间隔，屏宽的几成：`0.18` 就是相邻两条相距屏幕宽度的 18%（只有 Key 用得上）。
    pub gap: f32,
    /// 按键流速：判定线高度积分与长按尾端都按它算，与 DPR「定轨谱面」的流速同义。
    pub speed: f64,
    /// 按键宽度的额外倍率（只有 Slide 用得上）：Malody 横条的宽度先按「一个轨道宽 = 1」折算，
    /// 再乘这个倍率，方便整体调胖调瘦。
    pub width: f64,
}

impl Default for Options {
    fn default() -> Self {
        // 前三个参数与 DPR「定轨谱面」的 `fixed_lane_line_y` / `fixed_lane_gap` / `fixed_lane_speed`
        // 同义（同一个画面含义），数值取一组常用的定轨排布
        Self { line_y: 0.12, gap: 0.18, speed: 2.7, width: 1.0 }
    }
}

/// 包内的一张谱面（转换页列表里的一行）。
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    /// 在 [`Probe::charts`] 里的下标，转换时原样传回。
    pub index: usize,
    /// 谱面文件名（`.mcz` 里是包内路径，谱面文件夹里是文件名）。
    pub file: String,
    pub title: String,
    pub artist: String,
    pub charter: String,
    /// Malody 的难度名（`meta.version`）
    pub version: String,
    /// `meta.mode`：0 是 Key，其余见 [`mode_label`]
    pub mode: i32,
    /// [`mode`](Self::mode) 的模式名，列表里直接显示
    pub mode_name: String,
    /// 轨道数（`mode_ext.column`，缺失时按最大列号 + 1 猜）；只有 Key 模式有这个数字
    pub column: i32,
    /// 可演奏按键数（不含 BGM 提示）
    pub notes: usize,
    /// 其中长按的条数
    pub holds: usize,
    /// 谱面时长，秒
    pub duration: f64,
    /// 基准 BPM（`time[]` 里第一个正数 BPM）
    pub bpm: f64,
    /// BGM 提示指向的音频文件名
    pub audio: Option<String>,
    /// BGM 提示的偏移，毫秒
    pub audio_offset: f64,
    /// `meta.background` 指向的曲绘文件名
    pub illustration: Option<String>,
}

/// 转换页要显示的东西：能转几张谱面，以及包里有哪些音频 / 曲绘候选。
#[derive(Clone, Debug, Serialize)]
pub struct Probe {
    pub charts: Vec<Entry>,
    /// 包内的音频候选（按扩展名挑出来）
    pub audio: Vec<String>,
    /// 包内的图片候选
    pub illustration: Vec<String>,
    /// 真正打开的那一层（页面读曲绘用；拖的是曲包外层时这里是里面的 `0`）
    pub root: String,
}

/// 转换结果：`.pez` 里该有哪些条目，按写入顺序排好。
pub struct Output {
    /// 产物文件名（不含扩展名），`<曲名>.<模式>.<难度>`，由曲名、模式与难度整理而来。
    pub name: String,
    /// 转换出来的按键数（页面回显用）
    pub notes: usize,
    /// 谱面包里的各个条目：`info.yml`、`chart.json`、音频、曲绘。
    pub files: Vec<(String, Vec<u8>)>,
}

/// 转换的输入：谱面文件夹、单个 `.mc`、或者 `.mcz`（zip）。
pub struct Source {
    fs: Box<dyn FileSystem + Send + Sync>,
    /// 输入是单个 `.mc` 时只转它（同一个文件夹里可能还躺着别的难度）
    single: Option<String>,
    /// 真正当作包根的那一层：页面读曲绘要用它（拖进来的可能还是曲包的上一层）
    root: PathBuf,
}

impl Source {
    /// 打开一个输入。`.mc` 按它所在的文件夹打开，这样也能顺手在同一层里找到音频与曲绘。
    pub fn open(path: &Path) -> Result<Self> {
        if path.is_dir() {
            let Some(root) = resolve_dir_root(path) else {
                // Malody 的曲包是 `<曲名>_<键数>K/<难度序号>/<时间戳>.mc` 两层结构：往下找不到
                // 唯一一层时，直接说清谱面在哪几层里，别让用户对着「没有谱面」自己翻
                match nested_charts(path) {
                    Some(inner) => bail!("这个文件夹里没有 `.mc` 谱面；谱面在下层文件夹里：{inner}，选其中一层再打开"),
                    None => bail!("这个文件夹里没有 Malody 谱面（`.mc`）"),
                }
            };
            return Ok(Self { fs: open_fs(&root)?, single: None, root });
        }
        if !path.is_file() {
            bail!("找不到谱面：{}", path.display());
        }
        if extension_of(&path.to_string_lossy()) == "mc" {
            let parent = path
                .parent()
                .filter(|it| !it.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .to_path_buf();
            let name = path
                .file_name()
                .map(|it| it.to_string_lossy().to_string())
                .with_context(|| format!("谱面文件名读不出来：{}", path.display()))?;
            return Ok(Self { fs: open_fs(&parent)?, single: Some(name), root: parent });
        }
        Ok(Self { fs: open_fs(path)?, single: None, root: path.to_path_buf() })
    }

    /// 输入里的谱面文件名（按名字排好，顺序稳定）。
    pub fn charts(&self) -> Result<Vec<String>> {
        let names = self.fs.list_root().context("读不出输入里的文件列表")?;
        let mut charts: Vec<String> = match &self.single {
            Some(name) => names.into_iter().filter(|it| it == name).collect(),
            None => names
                .into_iter()
                .filter(|it| extension_of(it) == "mc")
                .collect(),
        };
        charts.sort();
        if charts.is_empty() {
            bail!("这个位置没有 Malody 谱面（`.mc`）");
        }
        Ok(charts)
    }

    /// 包里的音频 / 图片候选。
    fn candidates(&self) -> Result<(Vec<String>, Vec<String>)> {
        let names = self.fs.list_root().context("读不出输入里的文件列表")?;
        let audio = names.iter().filter(|it| is_audio(it)).cloned().collect();
        let illustration = names.iter().filter(|it| is_image(it)).cloned().collect();
        Ok((audio, illustration))
    }

    /// 读出并解析一张谱面。
    async fn load(&mut self, name: &str) -> Result<RawChart> {
        let bytes = self
            .fs
            .load_file(name)
            .await
            .with_context(|| format!("读不出谱面 {name}"))?;
        parse_raw(&bytes).with_context(|| format!("解析谱面 {name} 失败"))
    }

    /// 探明输入里能转什么。
    pub async fn probe(&mut self) -> Result<Probe> {
        let (audio, illustration) = self.candidates()?;
        let names = self.charts()?;
        let mut charts = Vec::with_capacity(names.len());
        for (index, name) in names.iter().enumerate() {
            let raw = self.load(name).await?;
            let parsed = parse(raw)?;
            charts.push(entry_of(index, name, &parsed));
        }
        Ok(Probe {
            charts,
            audio,
            illustration,
            root: self.root.to_string_lossy().to_string(),
        })
    }

    /// 转换第 `chart` 张谱面，返回 `.pez` 的内容。
    ///
    /// `audio` / `illustration` 是用户手动挑的文件：给了就用它（`.mc` 单独一张谱面时音频只能这么给），
    /// 没给才从输入里找 BGM 提示指向的那份。
    pub async fn convert(
        &mut self,
        chart: usize,
        audio: Option<&Path>,
        illustration: Option<&Path>,
        options: Options,
    ) -> Result<Output> {
        let names = self.charts()?;
        let name = names
            .get(chart)
            .with_context(|| format!("输入里没有第 {} 张谱面", chart + 1))?
            .clone();
        let raw = self.load(&name).await?;
        // 目前只认 Key 与 Slide：其余模式（Catch / Pad / Taiko / Ring）的按键字段与几何各不相同，
        // 按这两套硬转会产出一张错谱，所以只做展示、不转。
        // 模式先看再解析：别的模式按 Key 解析出来往往一个按键都不剩，报「没有按键」会误导。
        if raw.meta.mode != 0 && raw.meta.mode != SLIDE_MODE {
            bail!(
                "这张是 {} 模式的谱面，现在只能转 Key 与 Slide 模式",
                mode_label(raw.meta.mode)
            );
        }
        let parsed = parse(raw)?;
        if parsed.notes.is_empty() && parsed.slides.is_empty() {
            bail!("这张谱面里没有可演奏的按键（Key 要带 `column`，Slide 要带 `x`）");
        }

        let (audio_entry, audio_bytes) = match audio {
            Some(path) => (
                format!("music{}", dotted_extension(path)),
                std::fs::read(path).with_context(|| format!("读不出音频：{}", path.display()))?,
            ),
            None => {
                let name = match &parsed.bgm {
                    Some(bgm) if self.exists(&bgm.sound).await => bgm.sound.clone(),
                    // BGM 提示指向的文件不在包里：退到包里唯一那份音频
                    Some(bgm) => {
                        let candidates: Vec<String> = self
                            .fs
                            .list_root()
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|it| is_audio(it))
                            .collect();
                        match candidates.len() {
                            1 => candidates[0].clone(),
                            0 => bail!(
                                "包里没有音频（BGM 指向 {}），请在页面上挑一个音频文件",
                                bgm.sound
                            ),
                            _ => bail!(
                                "包里有多份音频、BGM 指向的 {} 又不在，请在页面上挑一个音频文件",
                                bgm.sound
                            ),
                        }
                    }
                    None => {
                        let candidates: Vec<String> = self
                            .fs
                            .list_root()
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|it| is_audio(it))
                            .collect();
                        match candidates.len() {
                            1 => candidates[0].clone(),
                            0 => bail!("谱面里没有 BGM 提示、包里也没有音频，请在页面上挑一个音频文件"),
                            _ => bail!("谱面里没有 BGM 提示、包里又有好几份音频，请在页面上挑一个"),
                        }
                    }
                };
                let bytes = self
                    .fs
                    .load_file(&name)
                    .await
                    .with_context(|| format!("读不出音频 {name}"))?;
                (format!("music{}", dotted_extension(Path::new(&name))), bytes)
            }
        };

        let (illustration_entry, illustration_bytes) = match illustration {
            Some(path) => (
                Some(format!("illustration{}", dotted_extension(path))),
                Some(
                    std::fs::read(path)
                        .with_context(|| format!("读不出曲绘：{}", path.display()))?,
                ),
            ),
            None => {
                let found = match parsed.meta.background.trim() {
                    "" => None,
                    name if self.exists(name).await => Some(name.to_owned()),
                    _ => None,
                };
                let found = match found {
                    Some(name) => Some(name),
                    None => {
                        let candidates: Vec<String> = self
                            .fs
                            .list_root()
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|it| is_image(it))
                            .collect();
                        (candidates.len() == 1).then(|| candidates[0].clone())
                    }
                };
                match found {
                    Some(name) => {
                        let bytes = self
                            .fs
                            .load_file(&name)
                            .await
                            .with_context(|| format!("读不出曲绘 {name}"))?;
                        (
                            Some(format!(
                                "illustration{}",
                                dotted_extension(Path::new(&name))
                            )),
                            Some(bytes),
                        )
                    }
                    None => (None, None),
                }
            }
        };

        let (chart_entry, chart_bytes) = if parsed.meta.mode == SLIDE_MODE {
            ("chart.pec", build_pec(&parsed, &options).into_bytes())
        } else {
            (
                "chart.json",
                serde_json::to_vec(&build_chart(&parsed, &options))?,
            )
        };
        let format = if parsed.meta.mode == SLIDE_MODE {
            "pec"
        } else {
            "pgr"
        };
        let info = build_info(
            &parsed,
            &audio_entry,
            illustration_entry.as_deref(),
            chart_entry,
            format,
        );

        let mut files = Vec::with_capacity(4);
        files.push(("info.yml".to_owned(), info.into_bytes()));
        files.push((chart_entry.to_owned(), chart_bytes));
        files.push((audio_entry, audio_bytes));
        if let (Some(name), Some(bytes)) = (illustration_entry, illustration_bytes) {
            files.push((name, bytes));
        }

        Ok(Output {
            // 产物名用 `<曲名>.<模式>.<难度>`：同一首曲子的几个难度、几个模式转出来不会互相覆盖
            name: output_name(&parsed.meta),
            notes: parsed.notes.len() + parsed.slides.len(),
            files,
        })
    }

    async fn exists(&mut self, name: &str) -> bool {
        self.fs.exists(name).await.unwrap_or(false)
    }
}

fn open_fs(path: &Path) -> Result<Box<dyn FileSystem + Send + Sync>> {
    fs::fs_from_file(path).with_context(|| format!("打不开 {}", path.display()))
}

/// 一层文件夹里的条目名（排好序，报错信息与提示里用）。
fn dir_names(path: &Path) -> Result<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(path)
        .with_context(|| format!("读不出文件夹：{}", path.display()))?
        .filter_map(|entry| Some(entry.ok()?.file_name().into_string().ok()?))
        .collect();
    names.sort();
    Ok(names)
}

/// 谱面真正所在的那一层。判断只看「这层里有没有 `.mc`」，不看文件夹叫什么 ——
/// 自己这层有 `.mc` 就是自己；否则往下看一层，取第一个装着 `.mc` 的子文件夹
/// （曲包的难度文件夹常叫 `0`，但也可以叫别的名字）。
/// 一层都不合适时返回 `None`，由调用方去报错。
fn resolve_dir_root(path: &Path) -> Option<PathBuf> {
    if has_chart(path) {
        return Some(path.to_path_buf());
    }
    let dirs: Vec<String> = dir_names(path)
        .ok()?
        .into_iter()
        .filter(|name| has_chart(&path.join(name)))
        .collect();
    dirs.first().map(|name| path.join(name))
}

/// 这一层里有没有 `.mc` 谱面。
fn has_chart(path: &Path) -> bool {
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .filter_map(|entry| Some(entry.ok()?.file_name().into_string().ok()?))
                .any(|name| extension_of(&name) == "mc")
        })
        .unwrap_or(false)
}

/// 下一层里「装着 `.mc` 谱面」的子文件夹名（用「、」连起来）；一层都没有时返回 `None`。
fn nested_charts(path: &Path) -> Option<String> {
    let mut dirs: Vec<String> = std::fs::read_dir(path)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if !entry.file_type().ok()?.is_dir() {
                return None;
            }
            let name = entry.file_name().into_string().ok()?;
            let has = std::fs::read_dir(entry.path())
                .ok()?
                .filter_map(|it| it.ok()?.file_name().into_string().ok())
                .any(|it| extension_of(&it) == "mc");
            has.then_some(name)
        })
        .collect();
    dirs.sort();
    (!dirs.is_empty()).then(|| dirs.join("、"))
}

// ---------------------------------------------------------------- Malody 文件

/// `.mc` 里用得上的那几个字段。Malody 的谱面是一行压紧的 JSON，还可能带 BOM，别的字段一概不管。
#[derive(Deserialize)]
struct RawChart {
    #[serde(default)]
    meta: Meta,
    #[serde(default)]
    time: Vec<Tempo>,
    #[serde(default)]
    note: Vec<RawNote>,
}

#[derive(Deserialize, Default)]
struct Meta {
    #[serde(default)]
    creator: String,
    #[serde(default)]
    background: String,
    /// 难度名（`4K HD Lv.22` 这种）
    #[serde(default)]
    version: String,
    /// 模式：0 = Key，3 = Catch，4 = Pad，5 = Taiko，6 = Ring，7 = Slide
    #[serde(default)]
    mode: i32,
    #[serde(default)]
    song: Song,
    #[serde(default, rename = "mode_ext")]
    mode_ext: ModeExt,
}

#[derive(Deserialize, Default)]
struct Song {
    #[serde(default)]
    title: String,
    #[serde(default)]
    artist: String,
}

#[derive(Deserialize, Default)]
struct ModeExt {
    /// 键数：Malody 的键数只写在这里
    #[serde(default)]
    column: i32,
}

/// BPM 变更点。
#[derive(Deserialize)]
struct Tempo {
    beat: Beat,
    /// 非正数是老谱面拿它当 SV 用的写法，转换时跳过
    bpm: f64,
}

#[derive(Deserialize)]
struct RawNote {
    beat: Beat,
    /// 有它就说明这是长按
    #[serde(default)]
    endbeat: Option<Beat>,
    /// BGM 提示没有这一列
    #[serde(default)]
    column: Option<i32>,
    #[serde(default)]
    sound: Option<String>,
    /// BGM 提示的偏移，毫秒
    #[serde(default)]
    offset: Option<f64>,
    /// 键型：1 是 BGM 提示（Key / Slide 都一样），Slide 里 4 是「滑动」、没有就是「点按」
    #[serde(default, rename = "type")]
    kind: Option<i32>,
    /// Slide 横条的中心与宽度，都在一条 256 宽的行里（实测中心落在 21~234、宽度落在 25~204）
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    w: Option<f64>,
    /// Catch / Slide 里标「这是 BGM」的另一种写法
    #[serde(default, rename = "isBgm")]
    is_bgm: Option<bool>,
    /// 横条的移动段：每个元素是一个「离起点多少拍」的时间点，最后那个就是横条的尾端
    #[serde(default)]
    seg: Option<Vec<Seg>>,
}

/// Slide 横条的一个移动段：`beat` 是离起点多少拍，`x` 是横条挪到起点的哪个相对位置。
#[derive(Deserialize)]
struct Seg {
    beat: Beat,
    #[serde(default)]
    x: f64,
}

/// Malody 的拍号：`[拍, 分子, 分母]` 就是 `拍 + 分子 / 分母` 个四分音符；也兼容直接写一个数。
#[derive(Clone, Copy, Deserialize)]
#[serde(untagged)]
enum Beat {
    Triple([f64; 3]),
    Single(f64),
}

impl Beat {
    fn beats(self) -> f64 {
        match self {
            Self::Triple([beat, numerator, denominator]) => {
                if denominator == 0. {
                    beat
                } else {
                    beat + numerator / denominator
                }
            }
            Self::Single(beat) => beat,
        }
    }
}

/// 解析出来的一张谱面。
struct Parsed {
    meta: Meta,
    /// `(起始拍, BPM)`，第一段一定从 0 拍开始、BPM 一定为正
    timeline: Vec<(f64, f64)>,
    notes: Vec<PlayNote>,
    /// Slide 的横条（Key 谱面里是空的）
    slides: Vec<SlideNote>,
    bgm: Option<Bgm>,
    /// 键数（`mode_ext.column` 缺失或小于实际列号时按后者补）
    column_count: i32,
}

/// 一个可演奏的按键。
struct PlayNote {
    /// 判定时刻，秒
    start: f64,
    /// 长按的尾端，秒
    end: Option<f64>,
    column: i32,
}

/// Slide 的一根横条。
struct SlideNote {
    /// 判定时刻，秒
    start: f64,
    /// 横条中心的谱面横坐标（`-1` 左边缘、`0` 正中、`1` 右边缘）
    x: f64,
    /// 横条宽度，屏宽的几成（`0.25` 就是一个轨道宽）
    width: f64,
    /// `true` 是滑动键（Malody 的 `type: 4`），`false` 是点按键
    drag: bool,
    /// 长条的顶点 `(时间, 横坐标)`：`seg` 里每个移动段一个，空表示单点按键。
    /// 横条会沿这些顶点滑过去，所以一根长条可能有好几段、好几个拐点。
    path: Vec<(f64, f64)>,
}

impl SlideNote {
    /// 尾端时刻：单点按键就是起点，长条取最后一个顶点。
    fn end(&self) -> f64 {
        self.path.last().map_or(self.start, |it| it.0)
    }

    /// 这根横条是否真的会横着走（`seg` 的位移全是 0 时它就是一根直条）。
    fn moving(&self) -> bool {
        self.path.iter().any(|(_, x)| (x - self.x).abs() > 1e-4)
    }
}

/// Malody `meta.mode` 里的 Slide。
const SLIDE_MODE: i32 = 7;
/// Slide 横条所在的那条行的宽度（`x` / `w` 都以它为基准，实测中心最大 234、宽度最大 204）
const SLIDE_ROW_WIDTH: f64 = 256.;
/// 一个轨道宽的横条算多少个屏宽：256 宽的行按 4 轨排，一格就是屏宽的 1/4
const SLIDE_LANE_WIDTH: f64 = 0.25;

/// BGM 提示：哪一份音频、偏移多少毫秒。
struct Bgm {
    sound: String,
    offset: f64,
}

fn parse_raw(bytes: &[u8]) -> Result<RawChart> {
    let text = std::str::from_utf8(bytes).context("谱面不是 UTF-8 文本")?;
    let text = text.trim_start_matches('\u{feff}');
    Ok(serde_json::from_str(text)?)
}

fn parse(raw: RawChart) -> Result<Parsed> {
    let timeline = timeline(&raw.time);
    let slide = raw.meta.mode == SLIDE_MODE;
    let mut bgm = None;
    let mut notes = Vec::new();
    let mut slides = Vec::new();
    let mut max_column = -1;
    for note in &raw.note {
        let beat = note.beat.beats();
        // 负拍是 Malody 的 `EmptyBeat` 占位，跳过
        if beat < 0. {
            continue;
        }
        // BGM 提示（`type: 1` 带声音文件名，或 Catch / Slide 的 `isBgm`）不是按键，两种模式下都这么认
        if note.kind == Some(1) || note.is_bgm == Some(true) {
            if bgm.is_none() {
                if let Some(sound) = note.sound.as_deref().filter(|it| !it.trim().is_empty()) {
                    bgm = Some(Bgm {
                        sound: sound.to_owned(),
                        offset: note.offset.unwrap_or(0.),
                    });
                }
            }
            continue;
        }
        if slide {
            // Slide 的横条：`x` 是中心、`w` 是宽度，都在一条 256 宽的行里；
            // 带 `seg` 的是长条（要按住到最后一个移动段的时间点），不带的就是单点按键
            let Some(x) = note.x else {
                continue;
            };
            let start = seconds_at(&timeline, beat);
            let width = note.w.unwrap_or(SLIDE_ROW_WIDTH * SLIDE_LANE_WIDTH) / SLIDE_ROW_WIDTH;
            // `seg` 的每个元素是「离起点多少拍、横条挪到起点的哪个相对位置」，横条会按顺序滑过去；
            // 顶点的时间从谱面起点算起，`x` 是相对起点的位移（不是累加），所以直接加在起点上
            let mut path: Vec<(f64, f64)> = note
                .seg
                .iter()
                .flatten()
                .map(|seg| {
                    let time = seconds_at(&timeline, beat + seg.beat.beats());
                    let x = ((x + seg.x) / SLIDE_ROW_WIDTH * 2. - 1.).clamp(-1., 1.);
                    (time, x)
                })
                .filter(|(time, _)| *time > start)
                .collect();
            path.sort_by(|a, b| a.0.total_cmp(&b.0));
            slides.push(SlideNote {
                start,
                x: (x / SLIDE_ROW_WIDTH * 2. - 1.).clamp(-1., 1.),
                width: width.clamp(0.02, 1.),
                drag: note.kind == Some(4),
                path,
            });
            continue;
        }
        // Key：负列（或不写列）的是 BGM 提示，可演奏的按键一定有列号
        if let Some(column) = note.column.filter(|it| *it >= 0) {
            let start = seconds_at(&timeline, beat);
            let end = note
                .endbeat
                .map(|it| seconds_at(&timeline, it.beats()))
                .filter(|it| *it > start);
            max_column = max_column.max(column);
            notes.push(PlayNote { start, end, column });
        }
    }
    notes.sort_by(|a, b| a.start.total_cmp(&b.start));
    slides.sort_by(|a, b| a.start.total_cmp(&b.start));
    let column_count = raw.meta.mode_ext.column.max(max_column + 1).max(1);
    Ok(Parsed { meta: raw.meta, timeline, notes, slides, bgm, column_count })
}

/// BPM 变更点整理成 `(起始拍, BPM)`：按拍排好、丢掉非正数、补上 0 拍那一段。
fn timeline(raw: &[Tempo]) -> Vec<(f64, f64)> {
    let mut points: Vec<(f64, f64)> = raw
        .iter()
        .map(|it| (it.beat.beats(), it.bpm))
        .filter(|(beat, bpm)| *bpm > 0. && beat.is_finite() && bpm.is_finite())
        .collect();
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    // 同一个拍上有两条就以最后一条为准
    points.dedup_by(|later, earlier| later.0 == earlier.0);
    // 第一段必须从 0 拍开始，`seconds_at` 才不用为「0 拍之前是什么 BPM」再分一次支
    match points.first().map(|it| it.0) {
        Some(beat) if beat <= 0. => {
            if let Some(first) = points.first_mut() {
                first.0 = 0.;
            }
        }
        Some(_) => {
            let bpm = points[0].1;
            points.insert(0, (0., bpm));
        }
        None => points.push((0., 120.)),
    }
    // 补完第一段之后，与上一段 BPM 相同的点没有意义，去掉（`seconds_at` 不必多看一段）
    points.dedup_by(|later, earlier| later.1 == earlier.1);
    points
}

/// 第 `beat` 拍的时刻，秒。
fn seconds_at(timeline: &[(f64, f64)], beat: f64) -> f64 {
    let Some(&(mut start, mut bpm)) = timeline.first() else {
        return 0.;
    };
    if beat <= start {
        return (beat - start) * 60. / bpm;
    }
    let mut seconds = 0.;
    for &(next_start, next_bpm) in timeline.iter().skip(1) {
        if beat <= next_start {
            return seconds + (beat - start) * 60. / bpm;
        }
        seconds += (next_start - start) * 60. / bpm;
        start = next_start;
        bpm = next_bpm;
    }
    seconds + (beat - start) * 60. / bpm
}

/// 第 `seconds` 秒落在第几拍 —— `seconds_at` 的反函数，PEC 的时间轴是拍所以要折回去。
fn beats_at(timeline: &[(f64, f64)], seconds: f64) -> f64 {
    let Some(&(mut start, mut bpm)) = timeline.first() else {
        return 0.;
    };
    let mut elapsed = 0.;
    for &(next_start, next_bpm) in timeline.iter().skip(1) {
        let span = (next_start - start) * 60. / bpm;
        if seconds <= elapsed + span {
            return start + (seconds - elapsed) * bpm / 60.;
        }
        elapsed += span;
        start = next_start;
        bpm = next_bpm;
    }
    start + (seconds - elapsed) * bpm / 60.
}

// ---------------------------------------------------------------- PGR v3

/// Phigros 官方谱面（PGR v3）。字段名与解析器 `DPS/dps/src/parse/pgr.rs` 里的结构一一对应。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChartFile {
    format_version: u32,
    /// 全局音频偏移，秒：音频时刻 = 谱面时刻 + `offset`
    offset: f64,
    judge_line_list: Vec<Line>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Line {
    bpm: f64,
    #[serde(rename = "judgeLineDisappearEvents")]
    alpha_events: Vec<Event>,
    #[serde(rename = "judgeLineRotateEvents")]
    rotate_events: Vec<Event>,
    #[serde(rename = "judgeLineMoveEvents")]
    move_events: Vec<Event>,
    speed_events: Vec<SpeedEvent>,
    notes_above: Vec<PgrNote>,
    notes_below: Vec<PgrNote>,
}

/// 一个关键帧区间。`start` / `start2` 是 x / y 归一化到 `0~1` 的值（解析器会换算成 `-1~1`），
/// 透明度与转角直接用原值。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Event {
    start_time: f64,
    end_time: f64,
    start: f32,
    end: f32,
    start2: f32,
    end2: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SpeedEvent {
    start_time: f64,
    end_time: f64,
    value: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PgrNote {
    /// 1 点按、2 拖动、3 长按、4 滑动
    #[serde(rename = "type")]
    kind: u8,
    /// 判定时刻，单位是 1/32 拍
    time: f64,
    position_x: f32,
    /// 长按长度，单位是 1/32 拍
    hold_time: f64,
    speed: f64,
    /// 引擎只要求它存在（解析时不用），给 0 即可
    floor_position: f32,
}

/// 折成 PGR v3：一条横判定线 + 吸附到轨道上的按键。
fn build_chart(parsed: &Parsed, options: &Options) -> ChartFile {
    let line_bpm = parsed.timeline[0].1;
    // PGR 文件里的时间单位是「1/32 拍」：秒数 × BPM × 32 / 60 就折回去了
    let ratio = 60. / 32. / line_bpm;
    let line_y = options.line_y.clamp(0., 1.);
    // 世界坐标里横向 1 单位 = 半个屏幕宽，所以「屏宽的几成」换成世界坐标要乘 2
    let step = options.gap.clamp(0., 1.) as f64 * 2.;
    let center = (parsed.column_count - 1) as f64 / 2.;
    let speed = options.speed.max(0.01);

    let mut notes = Vec::with_capacity(parsed.notes.len());
    for note in &parsed.notes {
        let x = (note.column as f64 - center) * step;
        // PGR 的横向坐标要先除 `2 * 9 / 160` 才是世界坐标（解析器乘的就是这个系数）
        let position_x = (x / (2. * 9. / 160.)) as f32;
        let time = note.start / ratio;
        notes.push(match note.end {
            Some(end) if end > note.start => PgrNote {
                kind: 3,
                time,
                position_x,
                hold_time: (end - note.start) / ratio,
                // 长条的 `speed` 只用来算身体长度（`end_height = height + hold_time × speed`）：
                // 解析器把长条**头**的倍率钉成 1、尾端的流速另记在 `end_speed` 里，所以这里的
                // 流速不会让长条比按键落得更快
                speed,
                floor_position: 0.,
            },
            _ => PgrNote {
                kind: 1,
                time,
                position_x,
                hold_time: 0.,
                // 按键的 `speed` 是**自己**的下落倍率（绘制时偏移就是「高度差 × speed」），
                // 判定线的高度差已经按流速积过一遍了。这里再写流速就等于乘了两次，
                // 按键会比长条快 `speed` 倍 —— 长条的头被解析器钉成 1（它的流速烘焙在
                // 尾端高度里，见 `pgr.rs` 的 `parse_notes`）。写 1 让两种键型都按判定线的
                // 流速落下来，滑块控制的就只剩判定线那一份。
                speed: 1.,
                floor_position: 0.,
            },
        });
    }
    notes.sort_by(|a, b| a.time.total_cmp(&b.time));

    // 判定线整个谱面不动：位移的 x 归一化后是 0.5（世界坐标 0），y 归一化后就是「屏高几成」
    let move_event = Event {
        start_time: 0.,
        end_time: 0.,
        start: 0.5,
        end: 0.5,
        start2: line_y,
        end2: line_y,
    };
    let line = Line {
        bpm: line_bpm,
        alpha_events: vec![Event {
            start_time: 0.,
            end_time: 0.,
            start: 1.,
            end: 1.,
            start2: 0.,
            end2: 0.,
        }],
        rotate_events: vec![Event {
            start_time: 0.,
            end_time: 0.,
            start: 0.,
            end: 0.,
            start2: 0.,
            end2: 0.,
        }],
        move_events: vec![move_event],
        // 空的速度事件会让按键高度恒为 0（永远贴在判定线上），所以至少写一条零长事件：
        // 解析器会把它铺成一条到谱面结束的匀速积分
        speed_events: vec![SpeedEvent { start_time: 0., end_time: 0., value: speed }],
        notes_above: notes,
        notes_below: Vec::new(),
    };

    ChartFile {
        format_version: 3,
        offset: -parsed.bgm.as_ref().map_or(0., |it| it.offset) / 1000.,
        judge_line_list: vec![line],
    }
}

// ---------------------------------------------------------------- PEC

/// PGR 的按键高度是「秒 × 流速 ÷ 它」，PEC 的判定线高度却是直接积分流速，两边要下落得一样快，
/// 就得把这份换算折进 `cv` 的数值里（值见 `DPS/dps/src/core.rs`）。
const HEIGHT_RATIO: f64 = 0.83175;

/// PEC 的 `cv` 数值与流速之间还隔着这一份：解析器把它除以 5.85 才是流速（见 `pec.rs` 的 `b"v"`）。
const PEC_SPEED_SCALE: f64 = 5.85;

/// 折成 PEC：一条横判定线 + 一根根带自己宽度的横条。
fn build_pec(parsed: &Parsed, options: &Options) -> String {
    let line_y = options.line_y.clamp(0., 1.) as f64;
    let speed = options.speed.max(0.01);
    let width_scale = options.width.max(0.01);
    let offset = -parsed.bgm.as_ref().map_or(0., |it| it.offset) / 1000.;

    // 第一行是音频偏移（毫秒）：解析器按 `值 / 1000 - 0.15` 折成谱面偏移
    let mut text = format!("{}\n", num((offset + 0.15) * 1000.));
    // `bp` 必须全部写在按键与事件之前
    for (beat, bpm) in &parsed.timeline {
        text.push_str(&format!("bp {} {}\n", num(*beat), num(*bpm)));
    }
    // 判定线：`cp` 的 x 归一化后是 0（1024 是正中），y 按 `值 / 1400 × 2 - 1` 折算
    text.push_str(&format!("cp 0 0 1024 {}\n", num(line_y * 1400.)));
    // 流速：PEC 积的是 `cv 值 / 5.85`，而 PGR 的高度还要再除以 `HEIGHT_RATIO`
    text.push_str(&format!(
        "cv 0 0 {}\n",
        num(speed * PEC_SPEED_SCALE / HEIGHT_RATIO)
    ));

    for note in &parsed.slides {
        let size = (note.width / SLIDE_LANE_WIDTH * width_scale).clamp(0.2, 8.);
        let mut push = |kind: &str, time: f64, end: Option<f64>, x: f64| {
            // 按键的横向写 `谱面坐标 × 1024`：引擎读 PEC 的按键是 `x / 1024`（见 `parse/pec.rs`），
            // 照这个口径写落点才准；`above` = 1（在判定线上方）、`fake` = 0
            let beat = num(beats_at(&parsed.timeline, time));
            match end {
                Some(end) => text.push_str(&format!(
                    "{kind} 0 {beat} {} {} 1 0\n",
                    num(beats_at(&parsed.timeline, end)),
                    num(x * 1024.)
                )),
                None => text.push_str(&format!("{kind} 0 {beat} {} 1 0\n", num(x * 1024.))),
            }
            // 宽度单独写一行 `&`：它缩放的是按键的横向（一个轨道宽 = 1），正是 Malody 的 `w`
            if (size - 1.).abs() > 1e-4 {
                text.push_str(&format!("& {}\n", num(size)));
            }
        };
        if !note.moving() {
            // 直条：没有横向位移，一根长按就够（单点按键走 `n1` / `n4`）
            if note.path.is_empty() {
                push(if note.drag { "n4" } else { "n1" }, note.start, None, note.x);
            } else {
                push("n2", note.start, Some(note.end()), note.x);
            }
            continue;
        }
        // 斜条：Phigros 没有「能倾斜的长条」，只好沿折线撒一串同一宽度的滑动键 —— 间距取按键宽度的
        // 九成，串起来就是一条连贯的斜条；拐点各自落在折线的顶点上，多段折线也照样跟着走。
        // 横坐标 1 个单位 = 半个屏宽，所以屏宽几成的 `width` 折成横坐标要乘 2
        let gap = (2. * size * SLIDE_LANE_WIDTH * 0.9).max(0.01);
        push(if note.drag { "n4" } else { "n1" }, note.start, None, note.x);
        let mut from = (note.start, note.x);
        for &(time, x) in &note.path {
            let dx = x - from.1;
            let steps = ((dx.abs() / gap).ceil() as usize).clamp(1, 64);
            for step in 1..=steps {
                let ratio = step as f64 / steps as f64;
                push(
                    "n4",
                    from.0 + (time - from.0) * ratio,
                    None,
                    from.1 + dx * ratio,
                );
            }
            from = (time, x);
        }
    }
    text
}

/// 去掉多余的小数位：整数不带小数点，其余最多三位。
fn num(value: f64) -> String {
    let rounded = (value * 1000.).round() / 1000.;
    if rounded == rounded.trunc() && rounded.abs() < 1e9 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}

/// `info.yml`：只写单字键（`ChartInfo` 按 camelCase 反序列化，单字键两边同名）。
///
/// 难度名前面带上模式（`Key 4K HD Lv.22`），同一首曲子的几种模式摆在列表里才分得清；
/// `difficulty` 写定数、`level` 写难度文本，与引擎「`difficulty` 是定数、`level` 是难度名」的约定一致。
fn build_info(
    parsed: &Parsed,
    music: &str,
    illustration: Option<&str>,
    chart: &str,
    format: &str,
) -> String {
    let meta = &parsed.meta;
    let version = meta.version.trim();
    let (_, lvl) = split_version(version);
    let label = mode_label(meta.mode);
    let level = if version.is_empty() {
        label.clone()
    } else {
        format!("{label} {version}")
    };
    let lines = [
        format!("name: {}", yaml_str(meta.song.title.trim())),
        format!("composer: {}", yaml_str(meta.song.artist.trim())),
        format!("charter: {}", yaml_str(meta.creator.trim())),
        format!("illustrator: {}", yaml_str("")),
        format!("level: {}", yaml_str(&level)),
        format!("difficulty: {lvl:.1}"),
        format!("diff: {}", yaml_str(&label)),
        format!("chart: {chart}"),
        format!("music: {music}"),
        format!("illustration: {}", illustration.unwrap_or("")),
        format!("format: {format}"),
    ];
    lines.join("\n") + "\n"
}

/// 从难度名里拆出「Lv.」后面的数字：`4K HD Lv.22` → (`4K HD`, 22.0)。
fn split_version(version: &str) -> (String, f32) {
    let lower = version.to_ascii_lowercase();
    if let Some(at) = lower.rfind("lv.") {
        let rest = version[at + 3..].trim();
        // 只取开头的数字，`Lv.22+` 这类后缀忽略
        let number: String = rest.chars().take_while(|it| it.is_ascii_digit() || *it == '.').collect();
        if let Ok(lvl) = number.parse::<f32>() {
            return (version[..at].trim().to_owned(), lvl);
        }
    }
    (version.trim().to_owned(), 1.0)
}

/// YAML 里的一段文本：一律写成双引号字符串，换行压成空格。
fn yaml_str(text: &str) -> String {
    let text = text.replace(['\r', '\n'], " ").replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{text}\"")
}

fn entry_of(index: usize, file: &str, parsed: &Parsed) -> Entry {
    let bgm = parsed.bgm.as_ref();
    let duration = parsed
        .notes
        .iter()
        .map(|it| it.end.unwrap_or(it.start))
        .chain(parsed.slides.iter().map(SlideNote::end))
        .fold(0.0_f64, f64::max);
    Entry {
        index,
        file: file.to_owned(),
        title: parsed.meta.song.title.trim().to_owned(),
        artist: parsed.meta.song.artist.trim().to_owned(),
        charter: parsed.meta.creator.trim().to_owned(),
        version: parsed.meta.version.trim().to_owned(),
        mode: parsed.meta.mode,
        mode_name: mode_label(parsed.meta.mode),
        column: parsed.column_count,
        notes: parsed.notes.len() + parsed.slides.len(),
        holds: parsed
            .notes
            .iter()
            .filter(|it| it.end.is_some())
            .count()
            + parsed
                .slides
                .iter()
                .filter(|it| !it.path.is_empty() && !it.moving())
                .count(),
        duration,
        bpm: parsed.timeline[0].1,
        audio: bgm.map(|it| it.sound.clone()),
        audio_offset: bgm.map_or(0., |it| it.offset),
        illustration: Some(parsed.meta.background.trim())
            .filter(|it| !it.is_empty())
            .map(str::to_owned),
    }
}

/// 产物的文件名整理：只留字母数字与 `-_. `，与 DPR 合并谱面那份规则一致。
pub fn file_stem(name: &str) -> String {
    let stem: String = name
        .chars()
        .filter(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.' | ' '))
        .collect();
    let stem = stem.trim().to_owned();
    if stem.is_empty() {
        "malody".to_owned()
    } else {
        stem
    }
}

fn extension_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|it| it.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// `.png` 这种带点的后缀；没有后缀就是空串。
fn dotted_extension(path: &Path) -> String {
    match path.extension() {
        Some(extension) => format!(".{}", extension.to_string_lossy().to_ascii_lowercase()),
        None => String::new(),
    }
}

fn is_audio(name: &str) -> bool {
    matches!(
        extension_of(name).as_str(),
        "ogg" | "mp3" | "wav" | "m4a" | "aac" | "flac" | "opus" | "wma"
    )
}

fn is_image(name: &str) -> bool {
    matches!(
        extension_of(name).as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp"
    )
}

/// 产物文件名（不含扩展名）：`<曲名>.<模式>.<难度>`。难度取 Malody 的 `meta.version`
/// （`4K HD Lv.22` 这种整串）；多 K 谱面的模式一律写 `Key`，轨道数只体现在谱面上。
fn output_name(meta: &Meta) -> String {
    let title = meta.song.title.trim();
    let title = if title.is_empty() { "Untitled" } else { title };
    let mode = mode_label(meta.mode);
    let version = meta.version.trim();
    if version.is_empty() {
        format!("{title}.{mode}")
    } else {
        format!("{title}.{mode}.{version}")
    }
}

/// Malody 的 `meta.mode`：0 = Key、3 = Catch、4 = Pad、5 = Taiko、6 = Ring、7 = Slide。
///
/// 只有 Key 与 Slide 能转；其余模式的按键字段各不相同，界面上只做展示。
pub fn mode_label(mode: i32) -> String {
    match mode {
        0 => "Key",
        3 => "Catch",
        4 => "Pad",
        5 => "Taiko",
        6 => "Ring",
        7 => "Slide",
        other => return format!("模式{other}"),
    }
    .to_owned()
}

/// 便于命令层拼路径：`<目录>/<曲名>.<模式>.<难度>.pez`。
pub fn output_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}.pez", file_stem(name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timeline_of(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
        let temps: Vec<Tempo> = points
            .iter()
            .map(|&(beat, bpm)| Tempo { beat: Beat::Single(beat), bpm })
            .collect();
        timeline(&temps)
    }

    /// 恒定 120 BPM 时一拍 0.5 秒。
    #[test]
    fn seconds_follow_bpm() {
        let timeline = timeline_of(&[(0., 120.)]);
        assert!((seconds_at(&timeline, 4.) - 2.).abs() < 1e-9);
    }

    /// 中途换 BPM：前 4 拍按 120、之后按 60。
    #[test]
    fn seconds_follow_bpm_changes() {
        let timeline = timeline_of(&[(0., 120.), (4., 60.)]);
        assert!((seconds_at(&timeline, 4.) - 2.).abs() < 1e-9);
        assert!((seconds_at(&timeline, 6.) - 4.).abs() < 1e-9);
    }

    /// 非正 BPM 是老的 SV 写法，跳过；缺 0 拍那一段就补上。
    #[test]
    fn timeline_drops_non_positive_bpm() {
        let timeline = timeline_of(&[(-1., 0.), (8., 150.)]);
        assert_eq!(timeline, vec![(0., 150.)]);
    }

    /// `[拍, 分子, 分母]` 是 `拍 + 分子 / 分母` 个四分音符。
    #[test]
    fn beat_triple_is_fraction() {
        assert!((Beat::Triple([309., 1., 8.]).beats() - 309.125).abs() < 1e-9);
        assert!((Beat::Single(3.5).beats() - 3.5).abs() < 1e-9);
    }

    /// 难度名里拆出定数；没有 `Lv.` 就退回默认值。
    #[test]
    fn version_splits_rating() {
        assert_eq!(split_version("4K HD Lv.22"), ("4K HD".to_owned(), 22.0));
        assert_eq!(split_version("Hard"), ("Hard".to_owned(), 1.0));
    }

    /// 轨道横坐标：4 键、间距 0.14 时落在 ±0.42 / ±0.14（世界坐标）。
    #[test]
    fn lanes_center_on_screen() {
        let step = 0.14 * 2.;
        let center = 1.5;
        let xs: Vec<f64> = (0..4).map(|it| (it as f64 - center) * step).collect();
        assert!((xs[0] + 0.42).abs() < 1e-9);
        assert!((xs[3] - 0.42).abs() < 1e-9);
    }

    /// 选到曲包外层文件夹时自动认下里面那一层（`0`），而不是让用户再往里点一次。
    #[test]
    fn outer_folder_finds_inner_difficulty() {
        let dir = Path::new("E:/DPRecorder/example/malody/_song_15494");
        if !dir.is_dir() {
            eprintln!("跳过：没有示例曲包 {}", dir.display());
            return;
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("建 tokio 运行时失败");
        let probe = runtime
            .block_on(async {
                let mut source = Source::open(dir)?;
                source.probe().await
            })
            .expect("曲包外层文件夹应该能直接打开");
        assert_eq!(probe.charts.len(), 1);
        assert_eq!(probe.charts[0].column, 4);
        assert!(probe.root.ends_with('0'), "打开的那一层应该是 `0`：{}", probe.root);
        assert_eq!(probe.audio.len(), 1, "音频在同一层里，应该一起找得到");
    }

    /// 曲包的难度文件夹不一定叫 `0`：只看那层里有没有 `.mc`，名字叫什么都要能打开。
    #[test]
    fn difficulty_folder_name_does_not_matter() {
        let example = Path::new("E:/DPRecorder/example/malody/_song_15494/0");
        let Some(mc) = std::fs::read_dir(example).ok().and_then(|entries| {
            entries.filter_map(|it| it.ok()).find(|it| {
                it.path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext == "mc")
            })
        }) else {
            eprintln!("跳过：没有示例谱面 {}", example.display());
            return;
        };

        let root = std::env::temp_dir().join(format!("dpe-malody-name-{}", std::process::id()));
        let inner = root.join("4K");
        std::fs::create_dir_all(&inner).expect("建临时文件夹失败");
        std::fs::copy(mc.path(), inner.join(mc.file_name())).expect("复制示例谱面失败");

        let resolved = resolve_dir_root(&root);
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(
            resolved,
            Some(inner),
            "应该按「这一层有没有 .mc」选到 `4K`，而不是去找名为 `0` 的文件夹"
        );
    }

    /// 端到端：把示例 4K 谱面转出来的 `chart.json` 再过一遍引擎自己的 PGR 解析器。
    /// 转出来的包最终是交给 DPS 读的，光看 JSON 长得对不够 —— 这里让真正的解析器读一遍，
    /// 张数、偏移、长条、最后一个按键的时刻都对得上才算转成。没有示例谱面就跳过。
    #[test]
    fn example_chart_parses_with_dps() {
        let dir = Path::new("E:/DPRecorder/example/malody/_song_15494/0");
        if !dir.is_dir() {
            eprintln!("跳过：没有示例谱面 {}", dir.display());
            return;
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("建 tokio 运行时失败");
        let (probe, output) = runtime
            .block_on(async {
                let mut source = Source::open(dir)?;
                let probe = source.probe().await?;
                let output = source.convert(0, None, None, Options::default()).await?;
                Ok::<_, anyhow::Error>((probe, output))
            })
            .expect("转换示例谱面失败");

        assert_eq!(probe.charts.len(), 1);
        let entry = &probe.charts[0];
        assert_eq!(entry.column, 4);
        assert_eq!(entry.notes, 1498);
        assert_eq!(entry.holds, 47);
        assert_eq!(entry.audio.as_deref(), Some("Letters.ogg"));

        // 产物名是 `<曲名>.<模式>.<难度>`，同一首曲子的几种模式、几个难度都不会互相覆盖
        assert_eq!(entry.mode, 0);
        assert_eq!(entry.mode_name, "Key");
        assert_eq!(
            output.name,
            format!("{}.Key.{}", entry.title.trim(), entry.version.trim())
        );
        assert_eq!(
            output_path(Path::new("out"), &output.name),
            Path::new("out").join(format!("{}.Key.{}.pez", entry.title.trim(), entry.version.trim()))
        );

        let file = |wanted: &str| {
            output
                .files
                .iter()
                .find(|(name, _)| name == wanted)
                .map(|(_, bytes)| bytes)
                .unwrap_or_else(|| panic!("产物里没有 {wanted}"))
        };
        let info = String::from_utf8_lossy(file("info.yml")).into_owned();
        assert!(info.contains("format: pgr"), "{info}");
        assert!(info.contains("chart: chart.json"), "{info}");
        assert!(info.contains("music: music.ogg"), "{info}");
        // 难度名前面标模式；定数单独写进 `difficulty`
        assert!(
            info.contains(&format!("level: \"Key {}\"", entry.version.trim())),
            "{info}"
        );
        assert!(info.contains("diff: \"Key\""), "{info}");
        assert!(info.contains("difficulty: 22.0"), "{info}");

        let text = String::from_utf8_lossy(file("chart.json")).into_owned();
        let chart = dps::parse::parse_phigros(&text, dps::core::ChartExtra::default())
            .expect("引擎读不懂生成的 PGR");
        assert!(
            (chart.offset + 0.47).abs() < 1e-6,
            "音频偏移应是 -0.470 秒（BGM 提示的 470ms），实际 {}",
            chart.offset
        );
        assert_eq!(chart.lines.len(), 1);
        let notes = &chart.lines[0].notes;
        assert_eq!(notes.len(), 1498);
        let last = notes.iter().map(|it| it.time).fold(0., f64::max);
        // 最后一个按键在 309.125 拍 = 154.5625 秒；再往后只有一条长按的尾端（311.625 拍）
        assert!((last - 154.5625).abs() < 0.05, "最后一个按键应在 154.56 秒，实际 {last}");
        let holds = notes
            .iter()
            .filter(|it| matches!(it.kind, dps::core::NoteKind::Hold { .. }))
            .count();
        assert_eq!(holds, 47);
        // 点按与长条的下落倍率必须是同一个值：绘制时按键离判定线的偏移就是
        // 「高度差 × 按键自己的 speed」，两者不同就会一种键型比另一种落得快。
        // 长条的头由解析器钉成 1，所以点按也得是 1（流速只由判定线的速度事件给）。
        assert!(
            notes.iter().all(|it| it.speed == 1.),
            "点按与长条的下落倍率不一致：{:?}",
            notes.iter().map(|it| it.speed).fold(Vec::new(), |mut acc, speed| {
                if !acc.contains(&speed) {
                    acc.push(speed);
                }
                acc
            })
        );
    }

    /// 端到端：产物打成压缩包之后，用引擎自己的文件系统读回来。
    /// 谱面包最终是 DPS 直接读 zip 的，条目名字或位置错了这里就会露馅 —— `info.yml` 不在根上、
    /// `chart.json` 名字对不上时，读回来的就不是一张 PGR 谱面。
    #[test]
    fn package_loads_through_engine_file_system() {
        use std::io::Write as _;

        let dir = Path::new("E:/DPRecorder/example/malody/_song_15494/0");
        if !dir.is_dir() {
            eprintln!("跳过：没有示例谱面 {}", dir.display());
            return;
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("建 tokio 运行时失败");
        runtime
            .block_on(async {
                let mut source = Source::open(dir)?;
                let output = source.convert(0, None, None, Options::default()).await?;

                // 谱面、信息、音频、曲绘都摊在压缩包的根上，与 DPR 侧写盘时一致
                let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
                let options = zip::write::SimpleFileOptions::default();
                for (name, bytes) in &output.files {
                    writer.start_file(name.as_str(), options)?;
                    writer.write_all(bytes)?;
                }
                let bytes = writer.finish()?.into_inner();

                let mut fs = dps::fs::ZipFileSystem::new(bytes)?;
                let info = dps::fs::load_info(&mut fs).await?;
                assert!(
                    matches!(info.format, Some(dps::info::ChartFormat::Pgr)),
                    "包里的 info.yml 没让引擎认出是 PGR 谱面"
                );
                assert_eq!(info.chart, "chart.json");
                assert_eq!(info.music, "music.ogg");
                // 难度名带模式前缀；定数按 `difficulty` 读回来，`diff` 是模式
                assert_eq!(info.level, "Key 4K HD Lv.22");
                assert_eq!(info.diff, "Key");
                assert!((info.lvl - 22.0).abs() < 1e-6, "定数应读成 22.0，实际 {}", info.lvl);
                let text = String::from_utf8_lossy(&fs.load_file(&info.chart).await?).into_owned();
                let chart = dps::parse::parse_phigros(&text, dps::core::ChartExtra::default())?;
                assert_eq!(chart.lines.len(), 1);
                assert_eq!(chart.lines[0].notes.len(), 1498);
                Ok::<_, anyhow::Error>(())
            })
            .expect("读回谱面包失败");
    }

    /// 模式名：Malody 的 `meta.mode` 只有 Key（0）与 Slide（7）能转，其余是展示用的名字。
    #[test]
    fn mode_names_match_malody() {
        assert_eq!(mode_label(0), "Key");
        assert_eq!(mode_label(3), "Catch");
        assert_eq!(mode_label(4), "Pad");
        assert_eq!(mode_label(5), "Taiko");
        assert_eq!(mode_label(6), "Ring");
        assert_eq!(mode_label(7), "Slide");
        assert_eq!(mode_label(9), "模式9");
    }

    /// 非 Key 模式的谱面挡住不转（拿 Malody 自带的 Catch 谱面试一下）。
    #[test]
    fn non_key_mode_is_refused() {
        let chart = Path::new("J:/Malody-4.3.7/beatmap/_song_978/4168/mamsc.mc");
        if !chart.is_file() {
            eprintln!("跳过：没有参考谱面 {}", chart.display());
            return;
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("建 tokio 运行时失败");
        let error = runtime
            .block_on(async {
                let mut source = Source::open(chart)?;
                source.convert(0, None, None, Options::default()).await
            })
            .err()
            .expect("Catch 模式的谱面不该转换成功")
            .to_string();
        assert!(error.contains("Catch"), "报错里应该点出模式：{error}");
    }

    /// 一个 Malody 曲包的文件夹里常常混着各模式、各难度的谱面（`_song_978/4168` 就是这样）：
    /// 列表要逐张列出模式名，Key 那张照常能转，别的模式只在列表里占位。
    #[test]
    fn mixed_mode_folder_probes_every_chart() {
        let dir = Path::new("J:/Malody-4.3.7/beatmap/_song_978/4168");
        if !dir.is_dir() {
            eprintln!("跳过：没有参考谱面包 {}", dir.display());
            return;
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("建 tokio 运行时失败");
        let probe = runtime
            .block_on(async {
                let mut source = Source::open(dir)?;
                source.probe().await
            })
            .expect("读取参考谱面包失败");

        assert!(probe.charts.len() > 1, "这个包里应该有好几张谱面");
        let modes: Vec<&str> = probe.charts.iter().map(|it| it.mode_name.as_str()).collect();
        assert!(modes.contains(&"Catch"), "模式名要按 `meta.mode` 译出来：{modes:?}");
        let key = probe
            .charts
            .iter()
            .find(|it| it.mode == 0)
            .expect("这个包里应该有一张 Key 谱面");
        assert_eq!(key.mode_name, "Key");
        assert!(key.notes > 0);

        // 真谱面也能一路转到 `.pez` 的内容（示例谱面之外再拿一张 Malody 自己导出的 4K 谱面验一遍）
        let output = runtime
            .block_on(async {
                let mut source = Source::open(dir)?;
                source.convert(key.index, None, None, Options::default()).await
            })
            .expect("转换参考包里的 Key 谱面失败");
        assert_eq!(output.notes, key.notes);
        assert!(output.name.contains(".Key."), "产物名要带模式：{}", output.name);
        assert!(output.files.iter().any(|(name, _)| name == "chart.json"));
        assert!(output.files.iter().any(|(name, _)| name.starts_with("music.")));
    }

    /// Slide：横条的 `x` / `w` / `type` 分别落到谱面横坐标、按键宽度、键型上，产物是 PEC
    /// （PGR 的按键结构里没有「单个按键多宽」，宽度只能靠 PEC 的 `& <size>` 表达）。
    #[test]
    fn slide_chart_becomes_pec_with_widths() {
        use dps::core::NoteKind;

        let chart = Path::new("J:/Malody-4.3.7/beatmap/_song_978/4168/1549989004.mc");
        if !chart.is_file() {
            eprintln!("跳过：没有参考谱面 {}", chart.display());
            return;
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("建 tokio 运行时失败");
        runtime
            .block_on(async {
                let mut source = Source::open(chart)?;
                let output = source.convert(0, None, None, Options::default()).await?;
                assert!(output.name.contains(".Slide."), "产物名要带模式：{}", output.name);
                // 这张谱面 62 条：1 条 BGM 提示 + 61 个可演奏的横条；
                // 其中 4 根是直条（写 `n2`），4 根斜条要撒成 6 个滑动键，所以 PEC 里一共 67 个按键
                assert_eq!(output.notes, 61);
                let file = |name: &str| {
                    output
                        .files
                        .iter()
                        .find(|(entry, _)| entry == name)
                        .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
                        .unwrap_or_else(|| panic!("产物里没有 {name}"))
                };
                let info = file("info.yml");
                assert!(info.contains("format: pec"), "info.yml 要写 pec：{info}");
                assert!(info.contains("chart: chart.pec"), "info.yml 的谱面名不对：{info}");
                let text = file("chart.pec");
                assert!(text.lines().any(|it| it.starts_with("bp 0 ")), "缺 `bp`：{text}");
                assert!(text.lines().any(|it| it.starts_with("cv 0 0 ")), "缺 `cv`：{text}");
                let count = |prefix: &str| text.lines().filter(|it| it.starts_with(prefix)).count();
                assert_eq!(count("n1 "), 32, "点按数量不对");
                assert_eq!(count("n4 "), 31, "滑动数量不对");
                assert_eq!(count("n2 "), 4, "直条数量不对");
                // 横条的宽度各不相同，所以一定有按键带横向缩放
                assert!(count("& ") > 0, "没有任何按键写宽度：{text}");

                // 引擎自己读一遍：键型、数量、落点都要对得上
                let chart = dps::parse::parse_pec(&text, dps::core::ChartExtra::default())?;
                assert_eq!(chart.lines.len(), 1);
                let notes = &chart.lines[0].notes;
                assert_eq!(notes.len(), 67);
                assert_eq!(
                    notes.iter().filter(|it| matches!(it.kind, NoteKind::Drag)).count(),
                    31
                );
                for note in notes {
                    let x = note.object.translation.0.now();
                    assert!((-1. ..=1.).contains(&x), "横坐标越界：{x}");
                }

                // 打成谱面包再用引擎的文件系统读回来：`format: pec` + `chart.pec` 这个组合要能认
                use std::io::Write as _;
                let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
                let zip_options = zip::write::SimpleFileOptions::default();
                for (name, bytes) in &output.files {
                    writer.start_file(name.as_str(), zip_options)?;
                    writer.write_all(bytes)?;
                }
                let bytes = writer.finish()?.into_inner();
                let mut fs = dps::fs::ZipFileSystem::new(bytes)?;
                let info = dps::fs::load_info(&mut fs).await?;
                assert!(
                    matches!(info.format, Some(dps::info::ChartFormat::Pec)),
                    "包里的 info.yml 没让引擎认出是 PEC 谱面"
                );
                assert_eq!(info.chart, "chart.pec");
                let text = String::from_utf8_lossy(&fs.load_file(&info.chart).await?).into_owned();
                let chart = dps::parse::parse_pec(&text, dps::core::ChartExtra::default())?;
                assert_eq!(chart.lines[0].notes.len(), 67);
                Ok::<_, anyhow::Error>(())
            })
            .expect("转换参考 Slide 谱面失败");
    }

    /// Slide 的长条：直条写 `n2` 长按，斜条沿折线撒成一串滑动键。
    #[test]
    fn slide_long_notes_follow_their_segments() {
        use dps::core::NoteKind;

        let chart = Path::new("J:/Malody-4.3.7/beatmap/_song_4900/0/1553093571.mc");
        if !chart.is_file() {
            eprintln!("跳过：没有参考谱面 {}", chart.display());
            return;
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("建 tokio 运行时失败");
        runtime
            .block_on(async {
                let mut source = Source::open(chart)?;
                let output = source.convert(0, None, None, Options::default()).await?;
                assert!(output.name.contains(".Slide."), "产物名要带模式：{}", output.name);
                // 这张谱面 796 条：1 条 BGM 提示 + 795 个可演奏的横条，
                // 其中 58 根是不动的直条、其余都带位移；斜条撒点之后 PEC 里一共 982 个按键
                assert_eq!(output.notes, 795);
                let text = output
                    .files
                    .iter()
                    .find(|(name, _)| name == "chart.pec")
                    .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
                    .expect("产物里没有 chart.pec");
                let count = |prefix: &str| text.lines().filter(|it| it.starts_with(prefix)).count();
                assert_eq!(count("n1 "), 633, "点按数量不对");
                assert_eq!(count("n4 "), 291, "滑动数量不对");
                assert_eq!(count("n2 "), 58, "直条数量不对");

                // 引擎读回来：长按的尾端在起点之后，落点都在屏幕内
                let chart = dps::parse::parse_pec(&text, dps::core::ChartExtra::default())?;
                let notes = &chart.lines[0].notes;
                assert_eq!(notes.len(), 982);
                let holds: Vec<_> = notes
                    .iter()
                    .filter(|it| matches!(it.kind, NoteKind::Hold { .. }))
                    .collect();
                assert_eq!(holds.len(), 58, "长按数量不对");
                for note in holds {
                    let NoteKind::Hold { end_time, .. } = &note.kind else {
                        unreachable!()
                    };
                    assert!(*end_time > note.time, "长按尾端没排在起点之后");
                }
                for note in notes {
                    let x = note.object.translation.0.now();
                    assert!((-1. ..=1.).contains(&x), "横坐标越界：{x}");
                }
                Ok::<_, anyhow::Error>(())
            })
            .expect("转换长条 Slide 谱面失败");
    }

    /// 斜条展开：顶点要落准、相邻按键的间距不超过按键宽度、整条宽度一致。
    ///
    /// 合成一张两段折线的谱面：120 BPM、第 4 拍起、`x = 128`（正中）、`w = 64`（一个轨道宽），
    /// 两个移动段把横条移到 `+64`（正中偏右半格）再移回原位。
    #[test]
    fn slanted_slide_lands_on_its_vertices() {
        let json = r#"{
            "meta": {"$ver": 0, "creator": "t", "version": "Slide TEST Lv.1", "mode": 7, "mode_ext": {}},
            "time": [{"beat": [0, 0, 1], "bpm": 120.0}],
            "note": [
                {"beat": [0, 0, 1], "type": 1, "sound": "a.ogg", "offset": 0},
                {"beat": [4, 0, 1], "x": 128, "w": 64,
                 "seg": [{"beat": [2, 0, 1], "x": 64}, {"beat": [4, 0, 1], "x": 0}]},
                {"beat": [16, 0, 1], "x": 64, "w": 32}
            ]
        }"#;
        let parsed = parse(parse_raw(json.as_bytes()).unwrap()).unwrap();
        let text = build_pec(&parsed, &Options::default());

        // 只留按键行：`[键型, 拍, 横坐标, (长按尾端)]`
        let notes: Vec<(String, Vec<f64>)> = text
            .lines()
            .filter(|it| it.starts_with("n1 ") || it.starts_with("n2 ") || it.starts_with("n4 "))
            .map(|it| {
                let mut parts = it.split_whitespace();
                let kind = parts.next().unwrap().to_owned();
                let numbers: Vec<f64> = parts
                    .filter_map(|it| it.parse::<f64>().ok())
                    .collect();
                (kind, numbers)
            })
            .collect();
        // 第 4 拍（2 秒）起两段各 2 拍：段 1 横坐标 0 → 0.25 → 0.5（×1024 = 0 / 256 / 512），
        // 段 2 再原路退回；间距 0.5 一半是按键宽度 0.5 的九成以上，所以每段刚好两点
        assert_eq!(
            notes[0],
            ("n1".to_owned(), vec![0., 4., 0., 1., 0.]),
            "长条的头要按原按键的键型"
        );
        let chain: Vec<(f64, f64)> = notes[1..5]
            .iter()
            .map(|(kind, numbers)| {
                assert_eq!(kind, "n4", "斜条要撒成滑动键");
                (numbers[1], numbers[2])
            })
            .collect();
        assert_eq!(
            chain,
            vec![(5., 256.), (6., 512.), (7., 256.), (8., 0.)],
            "折线顶点没落准"
        );
        // 一个轨道宽 = `& 1`，正好是 1 的时候不写 `&`；半格的横条写成 `& 0.5`
        assert!(
            !text.lines().any(|it| it == "& 1"),
            "一个轨道宽的横条不该再缩放：{text}"
        );
        assert!(
            notes[5].1[1] == 16. && (notes[5].1[2] - -512.).abs() < 1e-6,
            "单点横条没落准：{:?}",
            notes[5]
        );
        assert!(text.contains("& 0.5\n"), "半格宽的横条要写 `& 0.5`：{text}");
    }
}
