//! Malody 转谱的两条 tauri 命令。
//!
//! 转换本身在 `src/dpe/malody.rs`（与编辑器那一侧的预览进程放在同一个目录），
//! 这里只补上 tauri 这一侧必须存在的东西：把路径交给它、把生成好的 `.pez` 写进编辑器的输出目录。
//! 模块按路径引用而不是把预览进程那几个模块也拉进来 —— 这里只需要 `malody` 这一段纯逻辑。

#[path = "dpe/malody.rs"]
mod malody;

use anyhow::{Context, Result};
use serde::Serialize;
use std::{path::{Path, PathBuf}};
use tauri::ipc::InvokeError;

/// 转换产物的目录：`<程序目录>/dpe/Mal2Phi`（Malody → Phigros）。
const MAL2PHI_DIR: &str = "Mal2Phi";

/// 转换结果：写到哪里、转进来多少键。
#[derive(Serialize)]
pub struct Converted {
    /// 产物 `.pez` 的绝对路径
    pub path: String,
    /// 转换出来的按键数
    pub notes: usize,
    /// 产物的显示名（曲名 + 模式 + 难度）
    pub name: String,
}

/// 探明一个输入里能转什么：几张谱面、包里有哪些音频与曲绘。
#[tauri::command]
pub async fn dpe_malody_probe(path: String) -> Result<malody::Probe, InvokeError> {
    crate::wrap_async(async move {
        let mut source = open(&path)?;
        source.probe().await
    })
    .await
}

/// 转换第 `chart` 张谱面，落到 `<程序目录>/dpe/Mal2Phi/<曲名>.<模式>.<难度>.pez`，返回它的路径。
///
/// `audio` / `illustration` 是页面上手动挑的文件（单独的 `.mc` 谱面只能这么给音频）；不传就从
/// 输入里找 BGM 提示指向的那份。
#[tauri::command]
pub async fn dpe_malody_convert(
    path: String,
    chart: usize,
    audio: Option<String>,
    illustration: Option<String>,
    options: malody::Options,
) -> Result<Converted, InvokeError> {
    crate::wrap_async(async move {
        let mut source = open(&path)?;
        let output = source
            .convert(
                chart,
                audio.as_deref().map(Path::new),
                illustration.as_deref().map(Path::new),
                options,
            )
            .await?;
        let dir = crate::common::dpe_output_dir(MAL2PHI_DIR)?;
        let target = malody::output_path(&dir, &output.name);
        let mut pack = crate::common::ZipPack::create(&target, 9)?;
        for (name, bytes) in &output.files {
            pack.bytes(name, bytes)?;
        }
        // 收尾才把 `.part` 改名成目标文件：半路出错不会留下一个看着能打开、其实不完整的包
        pack.finish()?;
        Ok(Converted {
            path: target.to_string_lossy().to_string(),
            notes: output.notes,
            name: output.name,
        })
    })
    .await
}

/// Malody 转 Phigros 的输出目录：`<程序目录>/dpe/Mal2Phi`。页面用它列已经生成过的谱面包。
#[tauri::command]
pub fn get_dpe_malody_dir_path() -> Result<String, InvokeError> {
    crate::common::dpe_output_dir(MAL2PHI_DIR)
        .map(|path| path.to_string_lossy().to_string())
        .map_err(InvokeError::from_anyhow)
}

fn open(path: &str) -> Result<malody::Source> {
    let path = PathBuf::from(path);
    malody::Source::open(&path).with_context(|| format!("打不开谱面：{}", path.display()))
}
