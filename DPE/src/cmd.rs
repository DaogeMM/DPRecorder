//! 命令行参数 → 引擎 `Config`。
//!
//! 给了 `--config` 就用宿主传过来的那份（DPR 已经用 `RenderConfig::to_config()` 转好、
//! 序列化成 JSON），所以预览的画面与 DPR 的预览完全一致；没给才从引擎默认值起，
//! 只覆盖预览需要的十几项。

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use dps::{
    config::{Config, Mods},
    core::appendant,
};

pub const USAGE: &str = "\
用法：dpe --input <谱面目录|.pez> [选项]

  --input <路径>     谱面目录或 .pez 文件（必填）
  --speed <倍速>     播放速度，默认 1.0
  --start <秒>       练习段起点，默认 0
  --end <秒>         练习段终点，默认整首
  --respack <路径>   皮肤包（zip 或目录），默认 appendant 里的 official
  --config <路径>    引擎配置 JSON（DPR 传过来的那份，给了就整份采用）
  --no-autoplay      关掉自动播放（音符不会自己打）
  --play             启动后立刻播放（默认暂停在起始位置）
--standalone       不从 stdin 读命令（由别的程序直接起窗口时用，避免 stdin 一关就退出）
  --mute             静音（音频照样推进，只是音量为 0）
  --fullscreen       全屏启动
  --frames <帧数>    只跑这么多帧就退出（冒烟测试用，0 表示一直跑）

协议：stdin 每行一条 JSON 命令，stdout 每帧回一条 JSON 事件，见 protocol.rs。
";

pub struct Args {
    pub input: PathBuf,
    pub config: Config,
    pub fullscreen: bool,
    pub mute: bool,
    /// 启动后立刻播放；不给就暂停在起始位置（编辑器语义）
    pub play: bool,
    pub standalone: bool,
    pub frames: u64,
}

impl Args {
    pub fn parse(mut it: impl Iterator<Item = String>) -> Result<Self> {
        let mut input: Option<PathBuf> = None;
        let mut respack: Option<String> = None;
        let mut config_path: Option<PathBuf> = None;
        let mut speed: Option<f32> = None;
        let mut start: Option<f64> = None;
        let mut end: Option<f64> = None;
        let mut autoplay = true;
        let mut mute = false;
        let mut fullscreen = false;
        let mut play = false;
        let mut standalone = false;
        let mut frames = 0u64;

        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--input" | "-i" => input = Some(PathBuf::from(value(&mut it, &arg)?)),
                "--speed" => speed = Some(parse_num(&value(&mut it, &arg)?, &arg)?),
                "--start" => start = Some(parse_num(&value(&mut it, &arg)?, &arg)?),
                "--end" => end = Some(parse_num(&value(&mut it, &arg)?, &arg)?),
                "--config" => config_path = Some(PathBuf::from(value(&mut it, &arg)?)),
                "--respack" => respack = Some(value(&mut it, &arg)?),
                "--no-autoplay" => autoplay = false,
                "--play" => play = true,
            "--standalone" => standalone = true,
                "--mute" => mute = true,
                "--fullscreen" => fullscreen = true,
                "--frames" => frames = parse_num(&value(&mut it, &arg)?, &arg)?,
                "--help" | "-h" => {
                    println!("{USAGE}");
                    std::process::exit(0);
                }
                other => bail!("认不出的参数 {other}"),
            }
        }

        let input = input.context("缺少 --input（谱面目录或 .pez）")?;
        // `dps::core::init_assets` 之后会把 cwd 切到 exe 旁边，所以先把输入钉成绝对路径。
        let input = std::fs::canonicalize(&input).with_context(|| format!("找不到谱面 {}", input.display()))?;
        if !input.is_dir() && !input.is_file() {
            bail!("{} 既不是目录也不是文件", input.display());
        }

        // 宿主（DPR）给的整份配置优先：它已经用 `RenderConfig::to_config()` 转过，
        // 画面因此与 DPR 自己的预览一致。没给才从引擎默认值起。
        let mut config = match config_path.as_ref() {
            Some(path) => {
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("读不到 {}", path.display()))?;
                serde_json::from_str(&text).with_context(|| format!("{} 不是引擎配置", path.display()))?
            }
            None => {
                let mut config = Config::default();
                // 编辑器拖到哪里就该看到哪里，先不播入场动画
                config.enter_animation = false;
                config
            }
        };
        if let Some(speed) = speed {
            config.speed = speed;
        }
        // 键盘交给宿主（DPE 自己响应空格 / 左右键），别让 GameScene 也去抢
        config.interactive = false;
        if let Some(start) = start {
            config.play_start_time = start;
        }
        if let Some(end) = end {
            config.play_end_time = Some(end);
        }
        if config_path.is_none() {
            config.volume_sfx = 0.;
        }
        config.volume_bgm = config.volume_music;
        config.res_pack_path = resolve_respack(respack.or_else(|| config.res_pack_path.clone()));
        if autoplay {
            config.mods |= Mods::AUTOPLAY;
        }

        Ok(Self {
            input,
            config,
            fullscreen,
            mute,
            play,
            standalone,
            frames,
        })
    }
}

fn value(it: &mut impl Iterator<Item = String>, name: &str) -> Result<String> {
    it.next().with_context(|| format!("{name} 后面缺少取值"))
}

fn parse_num<T: std::str::FromStr>(value: &str, name: &str) -> Result<T> {
    value.parse().map_err(|_| anyhow::anyhow!("{name} 的取值 {value} 不是数字"))
}

/// 皮肤包：给了就用它（必须是齐全的皮肤包），没给或给的不齐全就退回 appendant 里的 official。
/// 与 DPR 的 `common::resolve_respack_path` 同一套判据，DPR 那份是 `dpr` 包私有的，这里重写一遍。
fn resolve_respack(path: Option<String>) -> Option<String> {
    let complete = |path: &Path| appendant::has_files(path, appendant::SKIN_FILES);
    if let Some(path) = &path {
        if complete(Path::new(path)) {
            return Some(path.clone());
        }
        eprintln!("[dpe] 皮肤包 {path} 不完整，改用 appendant 里的默认皮肤");
    }
    appendant::pack(appendant::OFFICIAL)
        .filter(|pack| complete(pack))
        .map(|pack| pack.to_string_lossy().into_owned())
}
