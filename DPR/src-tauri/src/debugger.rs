//! DPDebugger：谱面演出动画的调试窗口。
//!
//! 界面在 `src/debugger/` 下（与主窗口、编辑窗口一样是同一份前端里的另一个窗口：
//! `index.html?debug=1`），这里只提供它需要的三条后端命令：
//!
//! - 打开 / 聚焦窗口，并把「这次调试哪张谱面、用哪份配置起步」记下来；
//! - 告诉窗口它该调试哪张谱面（窗口可能是被重新激活的，不是新开的）；
//! - 谱面包里那三个文件名（谱面、音乐、曲绘），左侧预览要把它们按字节喂给网页版引擎。
//!
//! 演出参数与谱面字节都走已有的命令：参数用 `render::debug_preview_config`，
//! 字节用 `read_chart_asset`。

use anyhow::{Context, Result};
use serde::Serialize;
use std::{ops::DerefMut, path::Path, sync::Mutex};
use tauri::{ipc::InvokeError, Emitter, Manager};

use crate::render::RenderConfig;
use dps::{
    fs::{self, FileSystem},
    info::ChartInfo,
};
use tracing::info;

/// 调试窗口的窗口标签。关闭它**不能**退出程序，见 `lib.rs` 的 `on_window_event`。
pub const DEBUG_WINDOW_LABEL: &str = "debugger";

/// 主窗口换了一张要调试的谱面时，发给已经开着的调试窗口。
pub const DEBUG_CONTEXT_EVENT: &str = "debug-context";

/// 窗口起来时要知道的两件事。
#[derive(Serialize, Clone)]
pub struct DebugContext {
    /// 要调试的谱面（目录或 `.pez` 包）
    pub path: String,
    /// 主窗口当前的渲染配置（`RenderConfig` 转成的 JSON）。
    /// 调试窗口从它起步 —— 窗口里改的就是这一份，改完实时回传给主窗口。
    pub config: serde_json::Value,
}

/// 最近一次打开的上下文。窗口自己起来时读它（也可能是空的）。
static DEBUG_CONTEXT: Mutex<Option<DebugContext>> = Mutex::new(None);

/// 谱面包里的三个文件名，左侧预览按它们把字节喂给网页版引擎。
///
/// 另外几个字段是谱面信息里那几行字：预览页的界面（曲名、难度、分数栏）读的是它自己那份
/// `info.txt`，第 4 段起由前端按这几项现拼一份，跟真渲染时看到的一样。
#[derive(Serialize)]
pub struct DebugChartFiles {
    /// 谱面文件（`.json` / `.pec` …）在包内的相对路径
    pub chart: String,
    /// 音乐文件在包内的相对路径
    pub music: String,
    /// 曲绘在包内的相对路径，没有就是空串
    pub illustration: String,
    /// 曲名
    pub name: String,
    /// 难度文本（`info.difficulty`，界面上 `Lv.` 后面那一串）
    pub level: String,
    /// 谱师
    pub charter: String,
    /// 曲师
    pub composer: String,
}

/// 打开 DPDebugger；已经开着就聚焦过去，并把新的上下文发过去。
///
/// **必须是 `async fn`**：同步命令跑在主线程上，而建窗口要等主线程的事件循环，
/// 同步写法会把两边互相等死 —— 现象就是新窗口一片空白、按钮一直转圈。
#[tauri::command]
pub async fn open_debug_window(
    app: tauri::AppHandle,
    path: String,
    config: RenderConfig,
) -> Result<(), InvokeError> {
    let context = DebugContext {
        path: path.clone(),
        config: serde_json::to_value(&config).map_err(|err| InvokeError::from_anyhow(err.into()))?,
    };
    *DEBUG_CONTEXT.lock().unwrap() = Some(context.clone());

    (|| -> Result<()> {
        if let Some(window) = app.get_webview_window(DEBUG_WINDOW_LABEL) {
            window.set_focus()?;
            window.emit(DEBUG_CONTEXT_EVENT, context)?;
            return Ok(());
        }

        let window = tauri::WebviewWindowBuilder::new(
            &app,
            DEBUG_WINDOW_LABEL,
            tauri::WebviewUrl::App("index.html?debug=1".into()),
        )
        // 无系统标题栏：标题栏由页面自己画（与 DPEditor、主窗口一致），
        // 否则会多出一条重复的标题栏。
        .title("DPDebugger")
        .decorations(false)
        .inner_size(1500.0, 900.0)
        .min_inner_size(1000.0, 640.0)
        .build()?;

        // 后建的窗口不一定拿到前台激活，显式抬到前面，
        // 否则点「调试」看着像什么都没发生（窗口开在主窗口背后）。
        window.unminimize()?;
        window.show()?;
        window.set_focus()?;

        info!("[调试窗口] 打开：{path}");
        Ok(())
    })()
    .map_err(InvokeError::from_anyhow)
}

/// 调试窗口启动时问「我该调试哪张谱面、从哪份配置起步」。
#[tauri::command]
pub fn debug_context() -> Option<DebugContext> {
    DEBUG_CONTEXT.lock().unwrap().clone()
}

/// 读附带资源包里的一条资源，给调试窗口灌进网页版引擎。
///
/// 网页版引擎没有磁盘（`appendant::entry` 在 wasm 上查的是内存文件表），特殊演出的贴图与
/// 皮肤只能由这里按字节读出来、前端再按 `<prefix>/<包名>/<包内路径>` 挂到 `__dprw_files`。
/// 返回 `tauri::ipc::Response` ⇒ 走 IPC 的裸字节通道，前端 `invoke` 直接拿到 `ArrayBuffer`。
#[tauri::command]
pub async fn debug_appendant_file(name: String, file: String) -> Result<tauri::ipc::Response, InvokeError> {
    crate::wrap_async(async move {
        let dir = crate::common::appendant_dir()?;
        let pack = dps::core::appendant::pack_in(&dir, &name)
            .with_context(|| format!("找不到附带资源包：{name}"))?;
        let bytes = dps::core::appendant::entry_file(&pack, &file)
            .with_context(|| format!("包里没有 {file}：{name}"))?;
        Ok(tauri::ipc::Response::new(bytes))
    })
    .await
}

/// DPEditor 的预览要摊进网页版引擎的默认皮肤包。
///
/// 桌面端成片 / 预览的默认皮肤本来就是 `appendant.official.zip`（见
/// `common::default_respack_path`），但网页版读不到 exe 旁边的 `appendant/`，所以编辑器
/// 装配播放页之前问一句：包在且齐全就返回它（`files` 是包内路径），由前端读成字节挂到
/// `respack/<包名>/…`；返回 `None` 就用编进 `assets/respack` 的内置皮肤。
#[tauri::command]
pub fn default_skin_pack() -> Option<crate::render::DebugPreviewPack> {
    let pack = crate::render::debug_preview_pack(
        dps::core::appendant::OFFICIAL,
        "respack",
        dps::core::appendant::SKIN_FILES,
    );
    pack.present.then_some(pack)
}

/// 读谱面包里的三个文件名（不读字节，字节走 `read_chart_asset`）。
#[tauri::command]
pub async fn debug_chart_files(path: String) -> Result<DebugChartFiles, InvokeError> {
    crate::wrap_async(async move {
        let mut fs: Box<dyn FileSystem + Send + Sync + 'static> = fs::fs_from_file(Path::new(&path))
            .with_context(|| format!("无法打开谱面：{path}"))?;
        let info: ChartInfo = fs::load_info(fs.deref_mut())
            .await
            .with_context(|| format!("无法读取谱面信息：{path}"))?;
        Ok(DebugChartFiles {
            chart: info.chart,
            music: info.music,
            illustration: info.illustration,
            name: info.name,
            level: info.difficulty,
            charter: info.charter,
            composer: info.composer,
        })
    })
    .await
}
