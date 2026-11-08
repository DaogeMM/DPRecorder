// Prevents additional console window on Windows in release, DO NOT REMOVE!!
// #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

dps::tl_file!("main" mtl);

mod chart_config;
mod common;
mod dpe;
mod drafts;
mod ffmpeg_download;
mod icon;
mod ipc;
mod lyrics;
mod malody_convert;
mod merge;
mod noise_player;
mod preview;
mod render;
mod split;
mod task;

use anyhow::{bail, Context, Result};
use common::{
    appendant_dir, collect_chart_files, create_zip, ensure_dir, get_output_dir, get_presets_json_file,
    get_presets_toml_file, get_rpe_dir, remove_file_retry, respack_dir, save_presets, AppConfig,
    Extra, CONFIG_DIR, DATA_DIR,
};
use icon::{BIG_ICON, ICON, SMALL_ICON};
use dps::{
    core::appendant,
    fs::{self, FileSystem},
    info::ChartInfo,
    log,
};
use render::{RenderConfig, RenderParams, ENCODER_LIST_AVC, ENCODER_LIST_HEVC};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs::File,
    future::Future,
    io::{BufRead, BufReader},
    ops::DerefMut,
    path::{Path, PathBuf},
    process::Stdio,
    sync::OnceLock,
    time::SystemTime,
};
use task::{TaskQueue, TaskView};
use tauri::{ipc::InvokeError, Emitter, Manager, State, WindowEvent};
use tauri_plugin_prevent_default::Flags;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    process::Command,
};
use tracing::{error, info, warn};

/// 控制台行宏来自 `dps`（与 DPSimulator 共用同一份实现），导出到 crate 根，
/// 让各处 `crate::console!(...)` 的写法照旧能用。
pub use dps::console;

static ASSET_PATH: OnceLock<PathBuf> = OnceLock::new();

/// 找 `assets/` 目录：从 exe 目录开始逐级向上，取第一个存在 `assets/` 的那一层。
///
/// assets 只有一份，放在仓库根（`<repo>/assets`，DPS 与 DPR 共用，见 README §2）：
/// - dev 与打包后：tauri 按 `bundle.resources` 把资源复制到 exe 同目录（`target\debug\assets`），第一轮就命中；
/// - 从别处启动 exe、或资源没被复制时：沿祖先目录找到 `<repo>\assets`；
/// - 都没有时退回 `<exe 目录>/assets`，与旧行为一致。
fn resolve_asset_dir(exe_dir: &Path) -> PathBuf {
    for dir in exe_dir.ancestors() {
        let candidate = dir.join("assets");
        if candidate.is_dir() {
            return candidate;
        }
    }
    exe_dir.join("assets")
}

pub fn build_conf(headless: bool) -> macroquad::window::Conf {
    // 分屏游玩的两个进程要各自按标题找到自己的窗口（见 `split::place_window`），
    // 所以标题带角色后缀；不分屏时仍是 `DPRecorder`。
    let window_title = match std::env::var("DPR_SPLIT_ROLE").as_deref() {
        Ok("secondary") => split::window_title(true),
        Ok("main") => split::window_title(false),
        _ => "DPRecorder",
    };
    macroquad::window::Conf {
        window_title: window_title.to_string(),
        window_width: 1280,
        window_height: 720,
        icon: Some(macroquad::miniquad::conf::Icon {
            medium: ICON,
            big: BIG_ICON,
            small: SMALL_ICON,
        }),
        headless,
        ..Default::default()
    }
}

#[inline]
async fn wrap_async<R>(f: impl Future<Output = Result<R>>) -> Result<R, InvokeError> {
    f.await.map_err(|e| {
        error!("[命令] 执行失败：{}", common::error_chain(&e));
        InvokeError::from_anyhow(e)
    })
}

fn run_wrapped(f: impl Future<Output = Result<()>> + 'static, headless: bool) {
    macroquad::Window::from_config(build_conf(headless), async {
        if let Err(err) = f.await {
            error!("[进程] 执行失败：{}", common::error_chain(&err));
            exit_program(1);
        }
    });
    exit_program(0);
}

fn hide_cmd() {
    #[cfg(all(target_os = "windows", not(debug_assertions)))]
    {
        //unsafe { winapi::um::wincon::FreeConsole() };
        unsafe {
            use std::ptr::null_mut;
            use winapi::um::wincon::GetConsoleWindow;
            use winapi::um::winuser::{ShowWindow, SW_HIDE, SW_MINIMIZE};
            let console_window = GetConsoleWindow();
            if console_window != null_mut() {
                ShowWindow(console_window, SW_MINIMIZE);
            }
        }
    }
}

/// Tauri 在**建窗口前**会把 WebView2 的用户数据目录强制设成 `app_local_data_dir()`
/// （Windows 上就是 `%LOCALAPPDATA%\com.daogemm.dpr`），并且**无条件 `create_dir_all` 它**
/// （`tauri-2.12.2/src/manager/webview.rs:562-577`）——那个目录建不出来就 `?` 直接 panic：
/// `Failed to setup app: 拒绝访问。 (os error 5)`。
///
/// 所以这里在 Builder 之前就用 `WEBVIEW2_USER_DATA_FOLDER` 把 WebView2 引到 **exe 同级的
/// `webview2/`**：程序整目录拷到别处连界面状态（语言、谱面列表、面板位置）一起走，也不依赖
/// `%LOCALAPPDATA%` 的写权限。exe 所在目录写不进去（装进 Program Files 之类）才退回标准目录。
#[cfg(target_os = "windows")]
fn ensure_webview_data_dir(exe_dir: &Path) {
    // 这个函数整个都在日志器注册**之前**跑（tauri-plugin-log 在 Builder::build() 里才装上），
    // 所以诊断信息一律直接走 stderr，跟 resolve_user_dir 一个道理。
    if std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_some() {
        return;
    }
    let portable = exe_dir.join("webview2");
    if std::fs::create_dir_all(&portable).is_ok() && common::is_dir_writable(&portable) {
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &portable);
        return;
    }
    // 退路：标准目录。它也建不出来的话 Tauri 那一步会 panic，所以这里先试一次并说明原因。
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        let standard = local.join("com.daogemm.dpr");
        match std::fs::create_dir_all(&standard) {
            Ok(()) => crate::console!(
                warn,
                "[WebView2] {} 不可写，改用 {}",
                portable.display(),
                standard.display()
            ),
            Err(e) => crate::console!(
                error,
                "[WebView2] {} 与 {} 都不可用（{e}），界面可能无法显示",
                portable.display(),
                standard.display()
            ),
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn ensure_webview_data_dir(_exe_dir: &Path) {}

/// 用户目录解析：**便携优先** —— 先用 `preferred`（exe 同级目录，拷走整个文件夹就能在任何
/// 机器上用），只有它写不进去时（装在 `Program Files`、只读介质、受限沙箱……）才退回
/// 系统标准目录 `fallback`，返回真正要用的那个。
///
/// `migrate = true` 时会把 `fallback` 里已有的用户文件搬一份到 `preferred` —— 不搬的话，
/// 换了目录的用户看到的是一个空预设列表，会以为配置丢了（这比写不进去更让人困惑）。
fn resolve_user_dir(preferred: PathBuf, fallback: PathBuf, migrate: bool) -> PathBuf {
    if common::is_dir_writable(&preferred) {
        if migrate && preferred != fallback {
            copy_user_data(&fallback, &preferred);
        }
        return preferred;
    }
    if std::fs::create_dir_all(&fallback).is_ok() && common::is_dir_writable(&fallback) {
        // 这条要在日志器注册**之前**就出来，所以直接走 stderr（正常的目录选择是静默的）
        crate::console!(
            warn,
            "[目录] {} 不可写，改用 {}",
            preferred.display(),
            fallback.display()
        );
        return fallback;
    }
    crate::console!(
        error,
        "[目录] {} 与 {} 都不可写，设置与输出可能失败",
        preferred.display(),
        fallback.display()
    );
    fallback
}

/// 只搬「不搬就会让人以为数据丢了」的东西，且不覆盖目标已有的文件。
fn copy_user_data(from: &Path, to: &Path) {
    if !from.is_dir() {
        return;
    }
    for name in ["config.toml", "presets.toml", "presets.json"] {
        let (src, dst) = (from.join(name), to.join(name));
        if src.is_file() && !dst.exists() {
            let _ = std::fs::copy(&src, &dst);
        }
    }
    copy_dir_if_absent(&from.join("respack"), &respack_dest(to));
}

/// 资源包的落点：**程序目录**下的 respack（见 `common::respack_dir`），不跟着 config 走。
fn respack_dest(config_fallback: &Path) -> PathBuf {
    config_fallback
        .parent()
        .unwrap_or(config_fallback)
        .join("respack")
}

fn copy_dir_if_absent(from: &Path, to: &Path) {
    if !from.is_dir() {
        return;
    }
    let _ = std::fs::create_dir_all(to);
    let Ok(entries) = std::fs::read_dir(from) else {
        return;
    };
    for entry in entries.flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if src.is_dir() {
            copy_dir_if_absent(&src, &dst);
        } else if !dst.exists() {
            let _ = std::fs::copy(&src, &dst);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() -> Result<()> {
    common::init_console();

    // 文案只有简体中文：不让 `sys_locale` 挑（英文系统上会挑到 en-US，提示就成英文了）。
    // 放在最前面 —— 下面任何 `mtl!` / `tl!` 都比这里晚。
    let zh_cn: dps::l10n::LanguageIdentifier = "zh-CN".parse()?;
    dps::l10n::set_prefered_locale(Some(zh_cn));

    // 必须在 tauri::Builder::build() 之前设置
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            ensure_webview_data_dir(dir);
        }
    }

    // 编辑窗口的预览区里，引擎用 cpal 的 WebAudio 后端自己建 AudioContext 出声；没有用户手势时
    // 浏览器的自动播放策略会把刚建的上下文挂成 suspended，听起来就是「一点声音都没有」。
    // 桌面程序自己的窗口不需要这条策略，于是在建 WebView2 环境之前就把它加进启动参数
    // （WebView2 只认环境创建时的那一份，两个窗口各传一份会撞 0x8007139F）。
    // 调试时外部已经用这个变量传了别的参数（比如 --remote-debugging-port），就在后面追加。
    {
        let mut args = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").unwrap_or_default();
        if !args.contains("autoplay-policy") {
            if !args.is_empty() {
                args.push(' ');
            }
            args.push_str("--autoplay-policy=no-user-gesture-required");
            std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", args);
        }
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let _guard = rt.enter();

    let app =
        tauri::Builder::default()
            .plugin(tauri_plugin_os::init())
            .plugin(tauri_plugin_fs::init())
            .plugin(tauri_plugin_http::init())
            .plugin(tauri_plugin_shell::init())
            .plugin(tauri_plugin_dialog::init())
            .plugin(tauri_plugin_drag::init())
            .plugin(tauri_plugin_opener::init())
            .plugin(
                tauri_plugin_prevent_default::Builder::new()
                    .with_flags(Flags::all().difference(
                        Flags::FIND | Flags::RELOAD | Flags::FOCUS_MOVE | Flags::DEV_TOOLS,
                    ))
                    .build(),
            )
            .manage(TaskQueue::new())
            .invoke_handler(tauri::generate_handler![
                exit_program,
                open_output_folder,
                open_dir,
                reveal_in_dir,
                open_external,
                preview_chart,
                preview_tweakoffset,
                preview_play,
                parse_chart,
                read_chart_asset,
                read_chart_config,
                post_render,
                get_tasks,
                cancel_task,
                pause_task,
                resume_task,
                clear_tasks,
                remove_task,
                move_task,
                get_respacks,
                get_appendants,
                open_respack_folder,
                open_appendant_folder,
                get_presets,
                add_preset,
                remove_preset,
                get_presets_path,
                get_output_dir_path,
                get_temp_dir_path,
            get_noise_player_dir_path,
            get_dpe_output_dir_path,
            get_dpe_modify_dir_path,
            stage_chart_to_editor,
            open_editor_folder,
            list_editor_charts,
            list_editor_chart_files,
            read_editor_chart_file,
            open_editor_window,
            editor_context,
            set_editor_draft,
            list_editor_drafts,
            load_editor_draft,
            create_editor_draft,
            save_editor_draft,
            delete_editor_draft,
            open_editor_drafts_folder,
            import_editor_media,
            import_editor_media_dir,
            read_editor_media,
            parse_editor_lyrics,
            missing_editor_media,
            export_editor_draft,
            dpe_preview,
            dpe_generate_modified,
            malody_convert::dpe_malody_probe,
            malody_convert::dpe_malody_convert,
            malody_convert::get_dpe_malody_dir_path,
                get_app_dir,
                get_respack_dir_path,
                get_appendant_dir_path,
                get_log_dir_path,
                open_log_folder,
                export_log,
                read_config,
                save_config,
                test_output_dir,
                check_rpe_dir,
                set_rpe_dir,
                unset_rpe_dir,
                get_rpe_charts,
                open_app_folder,
                check_ffmpeg,
                get_ffmpeg_path,
                check_ffmpeg_filter,
                download_ffmpeg,
                cancel_ffmpeg_download,
                get_encoder,
                test_encoder,
                export_pez,
                delete_path,
                delete_file,
                delete_autosave,
                save_info,
                read_info,
                noise_player::noise_player_probe,
                noise_player::noise_player_convert,
                noise_player::noise_player_progress,
                noise_player::noise_player_reveal,
        noise_player::noise_player_outputs,
                merge::merge_charts,
                merge::dpe_merge_charts,
            ])
            .on_window_event(|window, event| match event {
                // 关主窗口 = 退出整个程序；编辑窗口是独立窗口，关掉它只关掉它自己。
                WindowEvent::CloseRequested { .. } => {
                    if window.label() != EDITOR_WINDOW_LABEL {
                        exit_program(0);
                    }
                }
                _ => {}
            })
            .build(tauri::generate_context!())
            .expect("error while running tauri application");

    let resolver = app.path();
    let exe = std::env::current_exe()?;
    let exe_dir = exe.parent().unwrap();

    // 便携优先：**用户会直接看到的目录一律先放在程序所在文件夹**
    // ——`config/`（设置与预设）、`respack/`（资源包）、`output/`（渲染输出）、`tmp/`（临时
    // 文件）都在 exe 旁边，整份文件夹拷到别的机器（或打包成绿色版）就能直接用，用户也看得见、
    // 好清理。只有那里写不进去时（装在 `Program Files`、只读介质、受限沙箱）才退回系统标准
    // 目录（`%APPDATA%` / `%LOCALAPPDATA%`），第一次回退/切换时会把已有的
    // config.toml / presets.* / respack 复制过来，免得看起来像「配置全没了」。
    CONFIG_DIR
        .set(ensure_dir(resolve_user_dir(
            exe_dir.join("config"),
            resolver
                .app_config_dir()
                .unwrap_or_else(|_| exe_dir.to_owned()),
            true,
        )))
        .ok();
    // DATA_DIR 目前只用来放默认输出目录的兜底点，所以它本身就是程序目录（输出落到
    // `<程序目录>/output`）；这里不复制任何东西。
    DATA_DIR
        .set(ensure_dir(resolve_user_dir(
            exe_dir.to_owned(),
            resolver
                .app_data_dir()
                .unwrap_or_else(|_| exe_dir.to_owned()),
            false,
        )))
        .ok();

    // 便携布局：用户会直接看到的几个文件夹都先在 exe 旁边建好（打包后一打开文件夹就能看到
    // 该放什么、该去哪儿找输出）。建不出来（exe 目录不可写）就跳过，后面各自的解析会兜底。
    for name in ["config", "respack", "appendant", "output", "tmp"] {
        let dir = exe_dir.join(name);
        if !dir.is_dir() {
            let _ = std::fs::create_dir_all(&dir);
        }
    }

    // 资源包也从旧的「配置目录/respack」搬到程序目录下（`<程序目录>/respack`，见
    // common.rs::respack_dir）。留着旧的那份不动，只是复制，不覆盖目标已有的文件。
    {
        let new = exe_dir.join("respack");
        let old = CONFIG_DIR.get().unwrap().join("respack");
        if new != old && old.is_dir() && common::is_dir_writable(&new) {
            copy_dir_if_absent(&old, &new);
        }
    }

    // let asset_dir = resolver.resolve("assets", BaseDirectory::Config).unwrap();
    // assets 只有一份（仓库根），解析规则见 resolve_asset_dir。
    let asset_dir = resolve_asset_dir(&exe_dir);
    ASSET_PATH.set(asset_dir.clone()).ok();

    let config = common::read_config()?;
    let is_gui = std::env::args().len() == 1;

    // 上次的队列等配置目录登记好之后再读回：`TaskQueue::new()` 比这里早，那时还没有
    // `CONFIG_DIR`，读不到 tasks.json。只有 GUI 进程管这个队列 —— CLI 的渲染 / 预览进程
    // 不该去读它，更不该把别人的任务标成中断。
    if is_gui {
        app.state::<TaskQueue>().start_restore();
    }

    // 「输出日志到控制台」：渲染子进程的 stderr 是「输出」面板的来源，它必须照打；其余进程
    // （GUI 主进程、预览 / 试玩 / 校准偏移子进程）按设置里的开关来。
    let render_role = matches!(
        std::env::args().nth(1).as_deref(),
        Some("render") | Some("--render") | Some("-r")
    );
    common::set_console_log(render_role || config.print_stderr);

    // tempfile（视频落盘、音频混流、封面）以及 ffmpeg 等子进程全都走 std::env::temp_dir()。
    // 根目录：设置里填了临时目录就用它（和 `common::temp_root` 同一处判断），否则用
    // `<exe 目录>/tmp`（和 config/respack/output 一样挨着 exe，用户看得见、好清理，也不会把
    // 几 GB 视频临时文件塞进系统盘）。DPR 这条线的中间产物放根下的 `DPR` 子目录、DPE 用 `DPE`
    // （见 `common::get_temp_dir` / `get_dpe_temp_dir`），所以这里把 `TEMP` 指到 `<根>/DPR`。
    // 写不进去时保留系统临时目录；两者都不可写才警告（一失败就是满屏
    // `拒绝访问。 (os error 5) at path "...\Temp\.tmpXXXXXX"`，视频加载和渲染会直接挂掉）。
    {
        let root = config
            .temp_dir
            .clone()
            .unwrap_or_else(|| exe_dir.join("tmp"));
        // 子进程（渲染进程、以后的预览进程）拿到的 `TMP` 是 `<根>/DPR`，DPE 要的是
        // `<根>/DPE`，所以把根单独交代出去。
        std::env::set_var("DPRECORDER_TEMP_DIR", &root);
        let dir = root.join("DPR");
        let system = std::env::temp_dir();
        if std::fs::create_dir_all(&dir).is_ok() && common::is_dir_writable(&dir) {
            if system != dir {
                std::env::set_var("TMP", &dir);
                std::env::set_var("TEMP", &dir);
                info!("[临时目录] 改用 {}", dir.display());
            }
        } else if !common::is_dir_writable(&system) {
            crate::console!(
                error,
                "[临时目录] {} 与 {} 都不可写，后续临时文件写入会失败",
                dir.display(),
                system.display()
            );
        }
    }

    // GUI 进程始终开订阅：它得把日志写进文件。「显示详细日志」只决定渲染子进程往 stderr
    // 打多细 —— 「输出」面板里看到的就是子进程 stderr。
    if is_gui || config.show_detailed_log {
        // GUI 自己与「输出」面板里的子进程都强制带颜色码：前者常被 .bat 或重定向接走，后者
        // 的 stderr 是管道，按 tty 判断会漏掉颜色（父进程起它时塞了 `DPR_CONSOLE_ANSI`）。
        // CLI 直接跑时交给日志层判断：接终端才上色，重定向到文件就干净。
        if is_gui || std::env::var_os("DPR_CONSOLE_ANSI").is_some() {
            log::register_with_colorize(true);
        } else {
            log::register();
        }
    }

    // 只有 GUI 这个进程写日志文件：渲染子进程与 CLI 渲染只打 stderr，它们的 stderr
    // 由父进程收进「输出」面板，再逐行转写进同一份日志 —— 免得两个进程各写一遍。
    if is_gui {
        match common::init_log_file() {
            Ok(path) => info!("[日志] 日志文件 {}", path.display()),
            Err(err) => warn!("[日志] 日志文件不可用，只输出到终端：{}", common::error_chain(&err)),
        }

        // 启动横幅：一行说清版本和三个别人会去翻的目录。从 .bat 起的时候，控制台里至少能
        // 看出跑的是哪一份程序、配置与输出落在哪儿。它不受「输出日志到控制台」开关管 ——
        // 那个开关默认关着，否则启动时控制台一行都没有，看着像没起来。
        let config_dir = CONFIG_DIR.get().map(|dir| dir.display().to_string()).unwrap_or_default();
        let output = common::default_output_dir().map(|dir| dir.display().to_string()).unwrap_or_default();
        let logs = common::logs_dir().map(|dir| dir.display().to_string()).unwrap_or_default();
        log::startup_banner("DPRecorder", &[("配置", &config_dir), ("输出", &output), ("日志", &logs)]);
    }

    if std::env::args().len() > 1 {
        match std::env::args().nth(1).as_deref().unwrap_or_default() {
            "help" | "--help" | "-help" | "/help" | "-h" | "?" | "--?" | "-?" | "/?" => {
                println!("用法: DPRecorder --render --input <谱面文件> [选项]");
                println!("      DPRecorder --preview --input <谱面文件> [选项]");
                println!("选项:");
                println!("  --input   -i    <文件/路径>   谱面文件");
                println!("  --output  -o    <文件/路径>   输出文件");
                println!("  --config  -c    <文件/JSON>   配置");
                println!("  --info    -ci   <JSON>        谱面信息");
                println!("  --version -V                  版本号");
                exit_program(0);
            }
            "version" | "--version" | "-V" | "-v" => {
                println!("DPRecorder {}", env!("CARGO_PKG_VERSION"));
                exit_program(0);
            }
            "render" => {
                run_wrapped(render::main(false), true);
            }
            "play" => {
                run_wrapped(preview::main(false, false, false), false);
            }
            "preview" => {
                run_wrapped(preview::main(false, false, true), false);
            }
            "tweakoffset" => {
                run_wrapped(preview::main(false, true, true), false);
            }
            "--render" | "-r" => {
                run_wrapped(render::main(true), true);
            }
            "--play" => {
                run_wrapped(preview::main(true, false, false), false);
            }
            "--preview" | "-p" => {
                run_wrapped(preview::main(true, false, true), false);
            }
            "--tweakoffset" | "-t" => {
                run_wrapped(preview::main(true, true, true), false);
            }
            // 编辑器那一半的预览窗口：同一个程序、另一个入口（不单独出 exe）。
            "dpe-preview" => {
                dpe::run(std::env::args().skip(2));
            }
            cmd => {
                info!("[预览] 命令：{cmd:?}");
                let args = std::env::args().nth(1).unwrap_or_default();
                let path = Path::new(&args);
                if path.is_file() || path.is_dir() {
                    info!("[预览] 找到可用路径，开始预览");
                    run_wrapped(preview::main(true, false, true), false);
                    exit_program(0);
                } else {
                    exit_program(1);
                }
            }
        }
    } else {
        hide_cmd();
    }

    app.run(|_, _| {});

    Ok(())
}

#[tauri::command]
fn exit_program(code: i32) {
    /*#[cfg(target_os = "windows")]
    {
        use sysinfo::{ProcessExt, System, SystemExt, PidExt};
        let current_exe = std::env::current_exe().unwrap();
        let exe_name = current_exe.file_name().unwrap().to_str().unwrap();
        let mut system = System::new_all();
        system.refresh_processes();
        for (pid, process) in system.processes() {
            if process.name() == exe_name {
                if pid.as_u32() == std::process::id() {
                    continue;
                }
                process.kill();
            }
        }
    }*/
    std::process::exit(code);
}

/// 用资源管理器打开一个**目录**。
///
/// 为什么不用 `open` crate 的 `that_detached`：目录那条路它先试 `shell_open_folder`
/// （`CoInitialize` + `ILCreateFromPathW` + `SHOpenFolderAndSelectItems`）——在 DPR 的
/// 进程里这条 COM 路线是**没反应**的（同一个环境下「打开文件」好用、「在文件夹中
/// 显示」不好用，差别就在这儿），`CoInitialize` 的返回值它也从不检查；那一步失败之后它
/// 退到 `ShellExecuteExW`，而那个调用的 `fMask` 是 0（缺 MSDN 要求的 `SEE_MASK_FLAG_NO_UI`），
/// 于是失败时由 **Windows 自己弹**一个「Windows 无法访问指定设备、路径或文件」的错误框
/// —— 「右键打不开文件夹」就是它（目录明明在、权限也够，只是这一串调用没走通）。
///
/// 实际排下来的顺序（别改动）：
///   1. 自己调 `ShellExecuteExW`，**动词留空**（系统对目录的默认动词就是打开）。这条
///      能开（「打开文件」一直好用也是同一个调用），而且是 **shell 去起进程**，不牵扯我们
///      自己的标准句柄。
///   2. `ShellExecuteExW` 的文件位换成 `explorer.exe`、参数位给目录：再给 shell 一次机会，
///      只是换个入口（万一默认动词被劫持）。
///   3. `cmd /c start "" <目录>`：**只在前两条都失败时才用，而且它可能假成功** ——
///      日志正是 `cmd /c start ok (still running after 1.5s,
///      assumed ok pid=2064)` 而文件夹根本没开，所以它绝不能排在最前面。
///
/// 不再有「自己 spawn 一个 explorer.exe」那条路：GUI 父进程的标准句柄是无效的，子进程继承
/// 过去会直接起不来（会报「explorer.exe - 应用程序无法正常启动(0xc0000142)」）。
/// 要起 explorer 就让 shell 去起（第 2 条）。
fn open_folder(path: &Path) -> Result<()> {
    if !path.is_dir() {
        bail!("not a directory: {}", path.display());
    }

    #[cfg(not(target_os = "windows"))]
    {
        return open::that_detached(path).map_err(|err| anyhow::anyhow!("{err}"));
    }

    #[cfg(target_os = "windows")]
    {
        let mut fails: Vec<String> = Vec::new();

        match shell_execute(None, path.as_os_str(), None) {
            Ok(()) => {
                open_log(&format!(
                    "open_folder: ShellExecuteExW(默认动词) ok {}",
                    path.display()
                ));
                return Ok(());
            }
            Err(err) => {
                let msg = format!("ShellExecuteExW(默认动词) failed: {err}");
                open_log(&format!("open_folder: {msg}"));
                fails.push(msg);
            }
        }

        match shell_execute(None, std::ffi::OsStr::new("explorer.exe"), Some(path.as_os_str())) {
            Ok(()) => {
                open_log(&format!(
                    "open_folder: ShellExecuteExW(explorer.exe) ok {}",
                    path.display()
                ));
                return Ok(());
            }
            Err(err) => {
                let msg = format!("ShellExecuteExW(explorer.exe) failed: {err}");
                open_log(&format!("open_folder: {msg}"));
                fails.push(msg);
            }
        }

        if has_cmd_special(path) {
            open_log(&format!(
                "open_folder: skip `cmd /c start`（特殊字符）{}",
                path.display()
            ));
        } else {
            match start_folder_via_cmd(path) {
                Ok(note) => {
                    // 这一条报「成功」不代表真开了（见函数注释里的日志），只当最后一道保险
                    open_log(&format!(
                        "open_folder: cmd /c start 报告成功（{note}，不代表真开了）{}",
                        path.display()
                    ));
                    return Ok(());
                }
                Err(err) => {
                    let msg = format!("cmd /c start failed: {err}");
                    open_log(&format!("open_folder: {msg}"));
                    fails.push(msg);
                }
            }
        }

        bail!("failed to open {}: {}", path.display(), fails.join(" | "))
    }
}

/// 路径里有没有 cmd 会当分隔符 / 转义符的字符。
/// Rust 给 Windows 参数加引号时只处理空格、制表和引号，裸的 `&` 之类的会被 cmd 当命令
/// 分隔符切开（重则变成注入），所以这类路径不交给 `cmd /c start`，让 `ShellExecuteExW` 去
///（那条路参数由我们自己传，没有命令行解析问题）。
#[cfg(target_os = "windows")]
fn has_cmd_special(path: &Path) -> bool {
    path.as_os_str()
        .to_string_lossy()
        .chars()
        .any(|c| matches!(c, '"' | '&' | '|' | '^' | '<' | '>' | '%' | '\r' | '\n'))
}

/// `cmd /c start "" <目录>`：最后一道保险。写法与 `start_via_cmd` 一致
///（`start` 的第一个参数是窗口标题，必须显式给空串，否则路径会被当成标题吃掉）。
#[cfg(target_os = "windows")]
fn start_folder_via_cmd(path: &Path) -> anyhow::Result<String> {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("cmd");
    cmd.arg("/c").arg("start").arg("").arg(path);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW：不要闪一个黑框
    run_briefly(cmd, "cmd")
}

/// 让前端打开任意目录（设置页那几个「右键 / 点文件夹图标」要在资源管理器里打开的路径）。
#[tauri::command]
fn open_dir(path: String) -> Result<(), InvokeError> {
    (|| {
        let path = PathBuf::from(path);
        info!("[目录] 打开文件夹 {}", path.display());
        open_folder(&path)
    })()
    .map_err(InvokeError::from_anyhow)
}

/// 在资源管理器里**选中**一个文件。
///
/// 前端原本用 `@tauri-apps/plugin-opener` 的 `revealItemInDir`：它走 COM 的
/// `ILCreateFromPathW` + `SHOpenFolderAndSelectItems`，在 DPR 的进程里失败
///（「打开文件」好用、「在文件夹中显示」没反应，差别就在这条 COM 路线上）。
/// 这里把 `explorer.exe /select,...` **交给 shell 去起**（`ShellExecuteExW` 的文件位是
/// `explorer.exe`）：进程由资源管理器创建，一点也不继承我们这边的标准句柄，也就没有
/// 0xc0000142 那一类毛病。失败（或没能选中）就退到「至少把所在目录打开」。
fn reveal_path(path: &Path) -> Result<()> {
    if !path.exists() {
        bail!("not found: {}", path.display());
    }

    #[cfg(target_os = "windows")]
    {
        // `/select,"<路径>"` 是资源管理器认的写法；整串塞进 ShellExecuteExW 的参数位，让
        // shell 去按它自己的规矩解释引号（我们自己拼命令行反而会踩转义问题）。
        let params = {
            let mut params = std::ffi::OsString::from("/select,\"");
            params.push(path.as_os_str());
            params.push("\"");
            params
        };
        match shell_execute(None, std::ffi::OsStr::new("explorer.exe"), Some(&params)) {
            Ok(()) => {
                open_log(&format!("reveal_path: select ok {}", path.display()));
                return Ok(());
            }
            Err(err) => {
                open_log(&format!(
                    "reveal_path: select failed ({err}) {}，退到打开所在目录",
                    path.display()
                ));
            }
        }
    }

    match path.parent() {
        Some(parent) => open_folder(parent),
        None => bail!("no parent directory: {}", path.display()),
    }
}

/// 让前端「在文件夹中显示」某个文件（任务页 / 批量页 / 设置页那个文件夹图标）。
#[tauri::command]
fn reveal_in_dir(path: String) -> Result<(), InvokeError> {
    (|| {
        let path = PathBuf::from(path);
        info!("[目录] 在文件夹中定位 {}", path.display());
        reveal_path(&path)
    })()
    .map_err(InvokeError::from_anyhow)
}

#[tauri::command]
fn open_output_folder() -> Result<(), InvokeError> {
    (|| {
        let path = get_output_dir()?;
        // 目录可能还没建出来（配置里指了个还没建的目录，或者程序目录下的 output/ 从来没写过东西）：
        // 先按「输出目录」的规矩建出来，explorer 打不开不存在的目录。
        common::test_output_dir(path.clone())?;
        info!("[输出] 打开输出文件夹 {}", path.display());
        open_folder(&path)?;
        Ok(())
    })()
    .map_err(InvokeError::from_anyhow)
}

/// 用系统默认浏览器打开一个外部链接（`http` / `https` / `mailto`）。
///
/// 为什么不用 `tauri-plugin-opener` 的 `openUrl`：它最终走 `open` crate 的
/// `that_detached`，在 Windows 上用 `ShellExecuteExW` 时 `fMask` 保持 0，缺了 MSDN
/// 要求的 `SEE_MASK_NOASYNC`（「调用线程没有消息循环时必须指定这个 flag」）。
/// Tauri 的命令线程没有消息泵、invoke 又立刻返回，于是启动请求被**静默丢弃**：
/// 不报错、浏览器也不出来（就是「点击链接无法唤醒浏览器」那个毛病）。
/// 对照：同一个 crate 打开**目录**走的是 `shell_open_folder`（`CoInitialize` +
/// `SHOpenFolderAndSelectItems`），所以「打开输出文件夹」一直是好的。
///
/// 直接打开失败后，几条路依次试、每条都记日志：
///   1. `cmd /c start "" <url>`（**主路**，它确实能把请求送到 Edge）；
///   2. 自己调 `ShellExecuteExW` 并显式带上 `SEE_MASK_NOASYNC`（见下）；
///   3. `rundll32 url.dll,FileProtocolHandler <url>`。
/// 三条都起不来才把错误回给前端（红色 toast）。日志写在
/// `<exe 目录>/tmp/dpr-open-external.log`，排查「点了打不开」时先看这个文件。
///
/// **坑**：从 `DPRecorder.exe` 里调 `ShellExecuteExW` 会**返回 TRUE 却什么都
/// 不发生**（日志里连着十来次 `ShellExecuteExW: ok`，而 Edge 的 User Data 一个字节都没动、
/// `msedge.exe` 进程数始终是 0）。所以它只能当兜底，绝不能放在 `cmd /c start` 前面 —— 放前面
/// 时它那句「成功」会直接把后面的路全部吃掉，用户看到的就是「点了没反应」。
#[tauri::command]
fn open_external(url: String) -> Result<(), InvokeError> {
    (|| {
        let lower = url.to_ascii_lowercase();
        let allowed = lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("mailto:");
        // cmd 会解析这些字符，URL 里出现就拒绝，避免命令行注入
        let risky = url
            .chars()
            .any(|c| matches!(c, '"' | '&' | '|' | '^' | '<' | '>' | '\r' | '\n'));
        if !allowed || risky {
            open_log(&format!("refused url={url}"));
            bail!("refused to open url: {}", url);
        }
        info!("[目录] 打开外部链接 {}", url);
        open_log(&format!("--- open_external url={url}"));

        let mut fails: Vec<String> = Vec::new();

        match start_via_cmd(&url) {
            Ok(note) => {
                open_log(&format!("cmd /c start: {note}"));
                return Ok(());
            }
            Err(err) => {
                let msg = format!("cmd /c start failed: {err}");
                open_log(&msg);
                fails.push(msg);
            }
        }

        match shell_execute(None, std::ffi::OsStr::new(&url), None) {
            Ok(()) => {
                open_log("ShellExecuteExW: ok");
                return Ok(());
            }
            Err(err) => {
                let msg = format!("ShellExecuteExW failed: {err}");
                open_log(&msg);
                fails.push(msg);
            }
        }

        match start_via_rundll32(&url) {
            Ok(note) => {
                open_log(&format!("rundll32: {note}"));
                return Ok(());
            }
            Err(err) => {
                let msg = format!("rundll32 failed: {err}");
                open_log(&msg);
                fails.push(msg);
            }
        }

        open_log(&format!("all failed: {}", fails.join(" | ")));
        bail!("failed to open {}: {}", url, fails.join(" | "))
    })()
    .map_err(InvokeError::from_anyhow)
}

/// 自己调 `ShellExecuteExW`，并显式带上 MSDN 要求的 `SEE_MASK_NOASYNC`
///（「调用线程没有消息循环时必须指定这个 flag」）与 `SEE_MASK_FLAG_NO_UI`（失败时别让
/// Windows 自己弹框），这正是 `open` crate 缺的那一步。失败时用 `GetLastError` 拿到原因，
/// 能一路冒到前端的 toast 上。
///
/// `verb` / `params` 都可以留空（空动词 = 系统默认动词，打开目录就是它）。调用方
/// （`open_folder` / `reveal_path` / `open_external`）都把它当**兜底**：它
/// 从 `DPRecorder.exe` 调可能返回 TRUE 却什么都不发生（原因未查明，可能是这个进程/线程没有 shell
/// 需要的 COM 环境），所以每条路里它都排在 `cmd /c start` 后面。
#[cfg(target_os = "windows")]
fn shell_execute(
    verb: Option<&str>,
    file: &std::ffi::OsStr,
    params: Option<&std::ffi::OsStr>,
) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use winapi::um::shellapi::{
        ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
    };
    use winapi::um::winuser::SW_SHOWNORMAL;

    let wide = |text: &std::ffi::OsStr| -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    };
    let verb_wide = verb.map(|v| wide(std::ffi::OsStr::new(v)));
    let file_wide = wide(file);
    let params_wide = params.map(|p| wide(p));
    // SAFETY：`SHELLEXECUTEINFOW` 全零就是合法初值（cbSize/fMask/lpFile/nShow 随后填齐，
    // 其余句柄字段保持 null），三个宽字符串缓冲区都活到调用结束。
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
    info.lpVerb = verb_wide.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
    info.lpFile = file_wide.as_ptr();
    info.lpParameters = params_wide.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
    info.nShow = SW_SHOWNORMAL;
    let ok = unsafe { ShellExecuteExW(&mut info) };
    if ok == 0 {
        bail!("{}", std::io::Error::last_os_error());
    }
    Ok(())
}

/// `cmd /c start "" <url>`：**主路**。cmd 有自己的消息泵，`start` 按系统关联唤起默认浏览器。
/// 命令行照平时那样传参数即可（Rust 会把那个空串转义成 `""`，这条命令确实把请求送到了
/// Edge，别改成 `raw_arg` 手拼——那是没验证过的写法）。
fn start_via_cmd(url: &str) -> anyhow::Result<String> {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("cmd");
    // `start` 的第一个参数是窗口标题，必须显式给一个空串，否则 URL 会被当成标题
    cmd.arg("/c").arg("start").arg("").arg(url);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW：不要闪一个黑框
    run_briefly(cmd, "cmd")
}

/// `rundll32 url.dll,FileProtocolHandler <url>`：最老牌的「用系统默认程序打开」路子。
fn start_via_rundll32(url: &str) -> anyhow::Result<String> {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("rundll32");
    cmd.arg("url.dll,FileProtocolHandler").arg(url);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.creation_flags(0x08000000);
    run_briefly(cmd, "rundll32")
}

/// 起一个「只是去把活交给系统」的进程，最多等 1.5 秒看它自己退不退；超时说明它还在
/// 干活（也可能已经把活交出去了），当成成功，别把用户卡在这儿。
fn run_briefly(mut cmd: std::process::Command, what: &str) -> anyhow::Result<String> {
    let mut child = cmd.spawn().with_context(|| format!("spawn {what}"))?;
    let pid = child.id();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(format!("exit=0 pid={pid}")),
            Ok(Some(status)) => bail!("exit={:?} pid={pid}", status.code()),
            Ok(None) => {}
            Err(err) => bail!("try_wait: {err} pid={pid}"),
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            return Ok(format!("still running after 1.5s, assumed ok pid={pid}"));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

/// 打开链接的排障日志：`<exe 目录>/tmp/dpr-open-external.log`（写不进去就退到系统临时目录）。
/// 只用来排查「点了没反应」，所以失败一律静默。
fn open_log(line: &str) {
    use std::io::Write;
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .map(|d| d.join("tmp"))
        .filter(|d| std::fs::create_dir_all(d).is_ok())
        .unwrap_or_else(std::env::temp_dir);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("dpr-open-external.log"))
    {
        let _ = writeln!(file, "[{stamp}] {line}");
    }
}

#[tauri::command]
async fn parse_chart(path: &Path) -> Result<ChartInfo, InvokeError> {
    wrap_async(async move {
        let mut fs: Box<dyn FileSystem + Send + Sync + 'static> = fs::fs_from_file(path)
            .with_context(|| mtl!("read-chart-failed", "path" => path.display().to_string()))?;
        let info = fs::load_info(fs.deref_mut())
            .await
            .with_context(|| mtl!("load-info-failed", "path" => path.display().to_string()))?;
        //let info_display = format!("{}\n", serde_json::to_string(&info)?);
        //info_display!("{:?}", info);
        Ok(info)
    })
    .await
}

/// 读谱面根上的 `config.toml`（只有 DPR 认它，引擎不读）：谱面自带的强制渲染配置。
///
/// 没有这个文件返回 `null`；文件在但读不出来则报错，由前端提示 —— 静默忽略会让
/// 写错键名的谱面看起来「配置没生效」，用户查不出原因。
#[tauri::command]
async fn read_chart_config(
    path: std::path::PathBuf,
) -> Result<Option<serde_json::Map<String, serde_json::Value>>, InvokeError> {
    wrap_async(chart_config::read(&path)).await
}

pub fn cmd_hidden(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let cmd = tokio::process::Command::new(program);
    /*#[cfg(target_os = "windows")] // Without terminal, there is no log
    {
        let mut cmd = cmd;
        #[cfg(not(debug_assertions))]
        cmd.creation_flags(0x08000000);
        cmd
    }
    #[cfg(not(target_os = "windows"))]*/
    cmd
}

/// 读取谱面包里的一个资源（曲绘等），返回**原始字节**。
///
/// 为什么不能走 `convertFileSrc`：`ChartInfo.illustration` 是**谱面包内的相对路径**
/// （RPE 的 `Picture: illustration.png`，默认值是 `background.png`），不是磁盘绝对路径。
/// 直接喂给 `convertFileSrc` 只会得到一个指向不存在文件的坏 URL（界面表现为裂图）。
/// 谱面既可能是文件夹也可能是 `.pez` 压缩包，所以这里统一按谱面自己的 `FileSystem`
/// 把字节读出来，前端再包成 Blob URL。
///
/// 返回 `tauri::ipc::Response` ⇒ 走 IPC 的裸字节通道，前端 `invoke` 直接拿到 `ArrayBuffer`。
/// 没有曲绘（`name` 为空）时返回**长度 0** 的字节，前端按「没曲绘」处理。
#[tauri::command]
async fn read_chart_asset(path: String, name: String) -> Result<tauri::ipc::Response, InvokeError> {
    wrap_async(async move {
        if name.is_empty() {
            return Ok(tauri::ipc::Response::new(Vec::<u8>::new()));
        }
        let mut fs: Box<dyn FileSystem + Send + Sync + 'static> =
            fs::fs_from_file(Path::new(&path))
                .with_context(|| mtl!("read-chart-failed", "path" => path.clone()))?;
        let bytes = fs
            .load_file(&name)
            .await
            .with_context(|| mtl!("read-asset-failed", "path" => format!("{path} / {name}")))?;
        Ok(tauri::ipc::Response::new(bytes))
    })
    .await
}

/// 用主程序的预览入口起一个窗口看生成的谱面（Editor 的输出列表上「预览」按这个走）。
///
/// 预览是同一个程序的另一个入口（`DPRecorder dpe-preview`），不单独出 exe；窗口仍要独立进程，
/// 因为一个进程只开一个窗口。`--standalone` 让它别读 stdin —— 这里的 stdin 给的是 null，
/// 预览进程一读就是 EOF，会当成「父进程没了」立刻退出。
///
/// 谱面包自带的 `config.toml` 在这里合一次：渲染页与噪域播放器的预览都由前端
/// `withChartConfig` 合过，DPE 这边也得合 —— 纯配置谱面（改动全写在 `config.toml` 里）
/// 不合就会显示成原谱。
#[tauri::command]
async fn dpe_preview(path: String, config: render::RenderConfig) -> Result<(), InvokeError> {
    wrap_async(async move {
        let exe = std::env::current_exe()?;
        let config = with_chart_config(config, Path::new(&path)).await?;
        // 把界面上的那份渲染配置整份交给预览进程：DPR 自己的预览也是这么给的
        // （`preview_chart` 把 RenderParams 写进子进程 stdin），这样两边的画面一模一样。
        let mut prpr_config = config.to_config();
        prpr_config.res_pack_path = common::resolve_respack_path(config.res_pack_path.clone());
        prpr_config.volume_bgm = prpr_config.volume_music;
        prpr_config.interactive = false;
        let config_path = common::get_dpe_temp_dir()?.join("preview-config.json");
        std::fs::write(&config_path, serde_json::to_vec(&prpr_config)?)
            .with_context(|| format!("写入预览配置失败：{}", config_path.display()))?;

        let mut cmd = cmd_hidden(exe);
        cmd.arg("dpe-preview")
            .arg("--input")
            .arg(&path)
            .arg("--play")
            .arg("--standalone");
        cmd.arg("--config").arg(&config_path);
        // 与 DPR 自己的预览同一条设置：全屏谱面预览开着就全屏起窗
        if common::read_config().map(|it| it.preview_fullscreen).unwrap_or(true) {
            cmd.arg("--fullscreen");
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()?;
        Ok(())
    })
    .await
}

/// 把谱面包自带的 `config.toml`（经白名单过滤）盖到一份渲染配置上。
///
/// 语义与前端 `withChartConfig` 一致：谱面写明的键优先，其余仍按传进来的那份配置走。
async fn with_chart_config(
    config: render::RenderConfig,
    path: &Path,
) -> Result<render::RenderConfig> {
    let Some(forced) = chart_config::read(path).await? else {
        return Ok(config);
    };
    let mut merged = serde_json::to_value(&config)?;
    if let (serde_json::Value::Object(base), serde_json::Value::Object(forced)) =
        (&mut merged, serde_json::Value::Object(forced))
    {
        for (key, value) in forced {
            base.insert(key, value);
        }
    }
    Ok(serde_json::from_value(merged)?)
}

#[tauri::command]
async fn preview_chart(params: RenderParams) -> Result<(), InvokeError> {
    wrap_async(async move {
        let mut child = cmd_hidden(std::env::current_exe()?)
            .arg("preview")
            .stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;

        let mut stdin = child.stdin.take().unwrap();
        let info = format!("{}\n", serde_json::to_string(&params)?);
        info!("[预览] 参数：{}", info);
        stdin.write_all(info.as_bytes()).await?;

        Ok(())
    })
    .await
}

#[tauri::command]
async fn preview_tweakoffset(params: RenderParams) -> Result<Option<f32>, InvokeError> {
    wrap_async(async move {
        let mut child = cmd_hidden(std::env::current_exe()?)
            .arg("tweakoffset")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        let mut stdin = child.stdin.take().unwrap();
        let info = format!("{}\n", serde_json::to_string(&params)?);
        info!("[预览] 校准偏移参数：{}", info);
        stdin.write_all(info.as_bytes()).await?;

        // Read and process stdout to get the offset value
        let stdout = child.stdout.take().unwrap();
        let mut stdout_lines = tokio::io::BufReader::new(stdout).lines();
        let mut offset = None;

        loop {
            if let Some(stdout_result) = stdout_lines.next_line().await? {
                if let Ok(result) = serde_json::from_str::<f32>(&stdout_result) {
                    offset = Some(result)
                }
            } else {
                break;
            }
        }

        let status = child.wait().await?;
        if !status.success() {
            error!("[预览] 子进程退出：{}", status);
        }

        Ok(offset)
    })
    .await
}

#[tauri::command]
async fn preview_play(mut params: RenderParams) -> Result<(), InvokeError> {
    // 同 `post_render`：试玩窗口也跟着设置页的「高性能模式」走（勾了就用独显玩）。
    if let Ok(app) = common::read_config() {
        params.config.high_performance = app.high_performance;
    }
    wrap_async(async move {
        // 临时排查用：把这次试玩的开关状态记到 exe 旁的 tmp/split-debug.txt。
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let tmp = dir.join("tmp");
                let _ = std::fs::create_dir_all(&tmp);
                let _ = std::fs::write(
                    tmp.join("split-debug.txt"),
                    format!(
                        "playSplitView={} pure={} fixed={}\n",
                        params.config.play_split_view,
                        params.config.pure_config_chart,
                        params.config.fixed_lane_chart
                    ),
                );
            }
        }
        // 「游玩分屏」：主窗口放原谱面，副窗口放改过的谱面。
        // 两个窗口必须是两个进程（macroquad 一个进程只能开一个窗口）。
        // 不看「谱面修改」里有没有功能打开：开关打开就分屏，两个窗口显示不同内容由配置决定。
        if !params.config.play_split_view {
            spawn_play_child(&params, None).await?;
            return Ok(());
        }

        // 端口由父进程挑：绑 0 让系统分配，拿到号就放掉，两个子进程谁先起都能重试着连上。
        let port = {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
            listener.local_addr()?.port()
        };
        let mut plain = params.clone();
        plain.config.clear_chart_mods();
        plain.config.play_split_view = false;
        let mut changed = params;
        changed.config.play_split_view = false;

        spawn_play_child(&plain, Some(("main", port))).await?;
        spawn_play_child(&changed, Some(("secondary", port))).await?;
        Ok(())
    })
    .await
}

/// 起一个试玩进程，把谱面信息与配置 JSON 从 stdin 递进去。
///
/// `split` 是 `(角色, 端口)`：分屏时两个子进程靠这两个环境变量认出自己的角色，
/// 主进程监听端口发播放时刻，副进程连上去跟时刻（见 `split` 模块）。
async fn spawn_play_child(params: &RenderParams, split: Option<(&str, u16)>) -> Result<()> {
    let mut command = cmd_hidden(std::env::current_exe()?);
    command
        .arg("play")
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some((role, port)) = split {
        command
            .env("DPR_SPLIT_ROLE", role)
            .env("DPR_SPLIT_PORT", port.to_string());
    }
    let mut child = command.spawn()?;

    let mut stdin = child.stdin.take().unwrap();
    let info = format!("{}\n", serde_json::to_string(params)?);
    info!("[预览] 播放参数：{}", info);
    stdin.write_all(info.as_bytes()).await?;
    Ok(())
}

#[tauri::command]
async fn post_render(queue: State<'_, TaskQueue>, mut params: RenderParams) -> Result<(), InvokeError> {
    // 「高性能模式」住在 app 配置（设置页）里，而渲染时它得落到这个渲染进程的
    // `RenderConfig` 上：在这里注入，渲染页 / 批量页两条路都吃得到，前端零改动，
    // CLI（`dpr --render -c` / 直接喂 JSON）也不受影响。
    if let Ok(app) = common::read_config() {
        params.config.high_performance = app.high_performance;
    }
    wrap_async(async move {
        queue.post(params).await?;
        Ok(())
    })
    .await
}

#[tauri::command]
async fn get_tasks(queue: State<'_, TaskQueue>) -> Result<Vec<TaskView>, InvokeError> {
    wrap_async(async move { Ok(queue.tasks().await) }).await
}

#[tauri::command]
async fn cancel_task(queue: State<'_, TaskQueue>, id: u32) -> Result<(), InvokeError> {
    queue.cancel(id).await;
    Ok(())
}

#[tauri::command]
async fn pause_task(queue: State<'_, TaskQueue>, id: u32) -> Result<(), InvokeError> {
    queue.pause(id).await;
    Ok(())
}

#[tauri::command]
async fn resume_task(queue: State<'_, TaskQueue>, id: u32) -> Result<(), InvokeError> {
    queue.resume(id).await;
    Ok(())
}

#[tauri::command]
async fn clear_tasks(queue: State<'_, TaskQueue>) -> Result<(), InvokeError> {
    queue.clear().await;
    Ok(())
}

/// 删除一条任务。参数用 `id` 而不是显示下标（任务列表可以拖动，下标会同时取决于显示顺序，
/// 所以全线按 id 走；`id` 只增不减，见 `task.rs` 的 `TaskQueue`）。
#[tauri::command]
async fn remove_task(
    queue: State<'_, TaskQueue>,
    id: u32,
    remove_file: bool,
) -> Result<(), InvokeError> {
    wrap_async(async move {
        queue.remove_with_output(id, remove_file).await?;
        Ok(())
    })
    .await
}

/// 调整任务在列表里的位置，`index` 是**移动之后**的下标（0 = 下一个渲染）。
/// 渲染顺序就是任务列表从上到下，所以拖动任务条就能调整顺序 —— 只有还在等待渲染的
/// 任务允许拖，前端会拦住，后端只改顺序。
#[tauri::command]
async fn move_task(queue: State<'_, TaskQueue>, id: u32, index: usize) -> Result<(), InvokeError> {
    queue.move_task(id, index).await;
    Ok(())
}

/// 程序认得的七个包：包名（`appendant/` 下的 zip 或同名目录）、界面显示的文件名、必需文件表。
///
/// `appendant/` 里的皮肤不进资源包下拉框 —— 那是一组程序自己按场合挑的包（official 当默认、
/// retri 给「报应SP」、spas 给「痉挛SP」、qzk 给「QZKAGO」、rr 给「Rr」），不是给用户选的。
const APPENDANT_PACKS: [(&str, &str, &[&str]); 9] = [
    (appendant::OFFICIAL, "appendant.official.zip", appendant::SKIN_FILES),
    (appendant::OLD, "appendant.old.zip", appendant::SKIN_FILES),
    (appendant::RETRI, "appendant.retri.zip", appendant::SKIN_FILES),
    (appendant::LAVA, "appendant.lava.zip", appendant::LAVA_FILES),
    (appendant::WATER, "appendant.water.zip", appendant::WATER_FILES),
    (appendant::SPAS, "appendant.spas.zip", appendant::SPAS_FILES),
    (appendant::QZK, "appendant.qzk.zip", appendant::QZK_FILES),
    (appendant::RR, "appendant.rr.zip", appendant::SKIN_FILES),
    (appendant::CP, "appendant.cp.zip", appendant::CP_FILES),
];

#[derive(Serialize)]
struct RespackInfo {
    name: String,
    path: String,
    /// 能不能用：zip 在、且必需文件齐全。缺文件的包照样列出来（用户得能选中它去修），
    /// 只是 `present` 为 false，界面据此标红并提示重新下载。
    present: bool,
    /// 包里缺哪些文件（齐全时为空）。
    missing: Vec<String>,
    /// 包里带不带按键皮肤（`SKIN_FILES` 齐全）：界面用它在资源包那一栏点一个按键图标，
    /// 有就亮到原色、没有就压成淡色。内置的「默认」项自带按键贴图，所以恒为 true。
    keys: bool,
    /// 包里带不带完整的噪域皮肤（`noise_on.*` 与 `noise_off.*` 两档齐全）：带了就用它当噪域
    /// 皮肤，缺一档就整包当没带（照旧用官方噪域）。界面拿它标个图标，不拦选择。
    noise: bool,
}

/// `respack/` 里的皮肤包候选（zip 文件，或装着一个包的目录）。
fn pack_candidates(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = dir.read_dir() else {
        return Vec::new();
    };
    let mut list: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let is_zip = path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
            let is_pack_dir = path.is_dir() && path.join("info.yml").is_file();
            (is_pack_dir || (path.is_file() && is_zip)).then(|| (entry.file_name().to_string_lossy().into_owned(), path))
        })
        .collect();
    list.sort_by(|x, y| x.0.cmp(&y.0));
    list
}

/// 资源包下拉框的候选：`respack/` 里的 zip 与**同名的皮肤目录**（用户自己做的皮肤包），再加上
/// 前端的「默认」项。
#[tauri::command]
fn get_respacks() -> Result<Vec<RespackInfo>, InvokeError> {
    (|| {
        let dir = respack_dir()?;
        Ok(pack_candidates(&dir)
            .into_iter()
            .map(|(name, path)| {
                let missing: Vec<String> = appendant::missing_files(&path, appendant::SKIN_FILES)
                    .into_iter()
                    .map(str::to_owned)
                    .collect();
                RespackInfo {
                    name,
                    path: path.display().to_string(),
                    present: missing.is_empty(),
                    keys: missing.is_empty(),
                    missing,
                    noise: appendant::has_noise_skin(&path),
                }
            })
            .collect())
    })()
    .map_err(InvokeError::from_anyhow)
}

/// 附带资源包的状态：目录 + 每个已知包在不在、缺不缺文件。
#[derive(Serialize)]
struct AppendantInfo {
    /// 包文件名（`appendant.official.zip` / `appendant.lava.zip` …）。
    file: String,
    /// 包所在位置（zip 或同名目录）；缺包时给出「该放哪儿」的 zip 路径。
    path: String,
    /// 包在不在（`<包名>.zip` 或同名目录都算在）。
    found: bool,
    /// 能不能用：包在且必需文件齐全。缺文件的包按缺包处理 —— 加载到一半失败比提前提示更难查。
    present: bool,
    /// 包里缺哪些文件；包整个不在时就是整张必需文件表。
    missing: Vec<String>,
}

#[derive(Serialize)]
struct AppendantStatus {
    /// `<程序目录>/appendant` 的绝对路径。
    dir: String,
    items: Vec<AppendantInfo>,
}

#[tauri::command]
fn get_appendants() -> Result<AppendantStatus, InvokeError> {
    (|| {
        let dir = appendant_dir()?;
        let items = APPENDANT_PACKS
            .iter()
            .map(|(name, file, required)| {
                let pack = appendant::pack_in(&dir, name);
                let missing: Vec<String> = match &pack {
                    Some(pack) => appendant::missing_files(pack, required).into_iter().map(str::to_owned).collect(),
                    None => required.iter().map(|it| (*it).to_owned()).collect(),
                };
                AppendantInfo {
                    file: (*file).to_owned(),
                    path: pack
                        .as_ref()
                        .map(|it| it.display().to_string())
                        .unwrap_or_else(|| dir.join(file).display().to_string()),
                    found: pack.is_some(),
                    present: missing.is_empty(),
                    missing,
                }
            })
            .collect();
        Ok(AppendantStatus {
            dir: dir.display().to_string(),
            items,
        })
    })()
    .map_err(InvokeError::from_anyhow)
}

#[tauri::command]
fn open_respack_folder() -> Result<(), InvokeError> {
    (|| {
        let path = respack_dir()?;
        info!("[目录] 打开资源包文件夹 {}", path.display());
        // 从来没放过资源包时这个目录可能还不存在，先建出来再打开
        std::fs::create_dir_all(&path)
            .with_context(|| format!("failed to create {}", path.display()))?;
        open_folder(&path)?;
        Ok(())
    })()
    .map_err(InvokeError::from_anyhow)
}

#[tauri::command]
fn open_appendant_folder() -> Result<(), InvokeError> {
    (|| {
        let path = appendant_dir()?;
        info!("[目录] 打开附加包文件夹 {}", path.display());
        // 缺包检查 / 设置页都可能先于任何下载来开这个目录：不存在就先建出来
        std::fs::create_dir_all(&path)
            .with_context(|| format!("failed to create {}", path.display()))?;
        open_folder(&path)?;
        Ok(())
    })()
    .map_err(InvokeError::from_anyhow)
}

#[tauri::command]
async fn get_presets() -> Result<HashMap<String, RenderConfig>, InvokeError> {
    (|| {
        let toml_file = get_presets_toml_file()?;
        if toml_file.exists() {
            let presets: HashMap<String, RenderConfig> =
                toml::from_str(&std::fs::read_to_string(toml_file)?)?;
            return Ok(presets);
        }

        // Compatible with old versions
        let json_file = get_presets_json_file()?;
        if json_file.exists() {
            let presets: HashMap<String, RenderConfig> =
                serde_json::from_reader(BufReader::new(File::open(json_file)?))?;
            return Ok(presets);
        }

        Ok(HashMap::new())
    })()
    .map_err(InvokeError::from_anyhow)
}

#[tauri::command]
async fn add_preset(name: String, config: RenderConfig) -> Result<(), InvokeError> {
    let mut presets = get_presets().await?;
    wrap_async(async move {
        if presets.insert(name, config).is_some() {
            bail!(mtl!("preset-exists"));
        }
        save_presets(&presets).await?;
        Ok(())
    })
    .await
}

#[tauri::command]
async fn remove_preset(name: String) -> Result<(), InvokeError> {
    let mut presets = get_presets().await?;
    wrap_async(async move {
        if presets.remove(&name).is_none() {
            bail!(mtl!("preset-not-found"));
        }
        save_presets(&presets).await?;
        Ok(())
    })
    .await
}

/// 「预设配置」文件（`config/presets.toml`）的绝对路径 —— 设置页里显示 / 在资源管理器里定位用。
/// 前端只拿它显示，不直接读写这个文件（写入仍然走 `add_preset` / `remove_preset`）。
#[tauri::command]
fn get_presets_path() -> Result<String, InvokeError> {
    // 直接 map / map_err，不要包成 `(|| Ok(x?))`：那样闭包的返回类型推断不出来
    // （报 `no method named map_err found for closure`）
    get_presets_toml_file()
        .map(|path| path.to_string_lossy().to_string())
        .map_err(InvokeError::from_anyhow)
}

/// 输出目录的**有效路径**（设置里没填就是默认那个「程序目录下的 output/」）——
/// 设置页「自定义输出目录」那格空着时拿它当占位提示，这样默认也看得到完整路径。
#[tauri::command]
fn get_output_dir_path() -> Result<String, InvokeError> {
    common::get_output_dir()
        .map(|path| path.to_string_lossy().to_string())
        .map_err(InvokeError::from_anyhow)
}

/// 临时目录根的有效路径：设置里没填就是系统临时目录（`<程序目录>/tmp`）。
/// 设置页那格空着时拿它当占位提示 —— 默认值也看得见、也能一键打开。
#[tauri::command]
fn get_temp_dir_path() -> Result<String, InvokeError> {
    common::temp_root()
        .map(|path| path.to_string_lossy().to_string())
        .map_err(InvokeError::from_anyhow)
}

/// 工具页「噪域播放器」的输出目录：`<程序目录>/dpe/noisePlayer`（DPE 的产物目录）。
/// 工具页那格空着时拿它当占位提示 —— 默认值看得见、也能一键打开。
#[tauri::command]
fn get_noise_player_dir_path() -> Result<String, InvokeError> {
    common::noise_player_dir()
        .map(|path| path.to_string_lossy().to_string())
        .map_err(InvokeError::from_anyhow)
}

/// DPE 的输出根：`<程序目录>/dpe`。路径固定，设置页只显示不给改。
#[tauri::command]
fn get_dpe_output_dir_path() -> Result<String, InvokeError> {
    common::dpe_output_root()
        .map(|path| path.to_string_lossy().to_string())
        .map_err(InvokeError::from_anyhow)
}

/// 谱面修改的输出目录：`<程序目录>/dpe/modify`。
#[tauri::command]
fn get_dpe_modify_dir_path() -> Result<String, InvokeError> {
    common::dpe_output_dir(common::DPE_MODIFY_DIR)
        .map(|path| path.to_string_lossy().to_string())
        .map_err(InvokeError::from_anyhow)
}

/// 编辑窗口加载谱面：把源谱面复制/解包进 `<程序目录>/dpe/editor/charts`，返回**解包后目录**的路径。
///
/// 编辑窗口只动这份副本，用户拖进来的原文件不受影响；重名时由
/// [`common::stage_chart_in_editor`] 另起名字，覆盖不到已有的目录。
#[tauri::command]
fn stage_chart_to_editor(path: String) -> Result<String, InvokeError> {
    (|| {
        let dest = common::stage_chart_in_editor(Path::new(&path))?;
        info!("[编辑窗口] 摊开谱面 {} → {}", path, dest.display());
        Ok(dest.to_string_lossy().to_string())
    })()
    .map_err(InvokeError::from_anyhow)
}

/// 编辑窗口的谱面目录：`<程序目录>/dpe/editor/charts`，打开它（没有就先建）。
#[tauri::command]
fn open_editor_folder() -> Result<(), InvokeError> {
    (|| {
        let dir = common::editor_charts_dir()?;
        info!("[编辑窗口] 打开谱面列表文件夹 {}", dir.display());
        open_folder(&dir)?;
        Ok(())
    })()
    .map_err(InvokeError::from_anyhow)
}

/// 编辑窗口的标签：独立于主窗口的一个 webview（`index.html?editor=1`）。
const EDITOR_WINDOW_LABEL: &str = "editor";

/// 主窗口又点了另一张谱面 / 换了草稿时发给编辑窗口的事件：窗口开着就换内容，不另开第二个。
const EDITOR_DRAFT_EVENT: &str = "editor-draft-changed";

/// 编辑窗口当前在编辑的草稿 id。
///
/// 不塞进窗口的 URL：窗口起来之后本来就要读草稿内容，统一从这里取省一次编解码。
static EDITOR_DRAFT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 切到某个草稿（编辑窗口自己新建 / 切换草稿时调）。
///
/// 记下来是为了「主窗口再点同一张谱面」时能认出这个窗口已经在编辑那个草稿了。
#[tauri::command]
fn set_editor_draft(id: String) -> Result<(), InvokeError> {
    *EDITOR_DRAFT.lock().unwrap() = Some(id);
    Ok(())
}

/// 决定这次要开哪个草稿：给了 id 就用它，只给谱面路径就找一份引用了它的草稿、没有就新建。
async fn resolve_editor_draft(path: Option<String>, draft: Option<String>) -> Result<String> {
    if let Some(id) = draft {
        return Ok(id);
    }
    let Some(path) = path else {
        return Ok(drafts::create(None)?.id);
    };
    if let Some(existing) = drafts::find_by_chart(&path) {
        return Ok(existing.id);
    }

    // 新草稿：只把用户点名的那份谱面挂到「谱面」轨道上，名字先照曲名起
    // （解析不出来就留自动编号的名字）。音乐与曲绘由用户自己往轨道上放。
    // 路径可能是一个摊开的谱面包目录，这时挑出目录里真正的谱面文件。
    let mut created = drafts::create(None)?;
    if let Ok(info) = load_editor_chart_info(Path::new(&path)).await {
        if !info.name.trim().is_empty() {
            created.name = info.name.clone();
        }
    }
    let chart = if Path::new(&path).is_dir() {
        drafts::chart_file_in(Path::new(&path))
    } else {
        Some(Path::new(&path).to_path_buf())
    };
    if let Some(chart) = chart {
        if let Ok(media) = drafts::media_from_path(&chart) {
            created.assign(media);
        }
    }
    Ok(drafts::save(created)?.id)
}

/// 打开编辑窗口 —— 独立窗口，不是主窗口里的一页。
///
/// 三种进法：`draft` 给了就直接开那个草稿；只给 `path`（主窗口「谱面修改」点了一张谱面）
/// 就找引用这份谱面的草稿、没有就新建一个把谱面挂上去；两个都没给就开一个新草稿。
///
/// 已经开着时不再开第二个：把焦点切过去，再发 `editor-draft-changed` 让它换草稿
/// （窗口位置、大小都是用户自己调的，关掉重开就全丢了）。
///
/// **必须是 `async fn`**：同步命令跑在主线程上，而建窗口要等主线程的
/// 事件循环，同步写法会把两边互相等死（界面看起来没反应）。
#[tauri::command]
async fn open_editor_window(
    app: tauri::AppHandle,
    path: Option<String>,
    draft: Option<String>,
) -> Result<(), InvokeError> {
    let id = resolve_editor_draft(path, draft)
        .await
        .map_err(InvokeError::from_anyhow)?;
    *EDITOR_DRAFT.lock().unwrap() = Some(id.clone());

    (|| {
        if let Some(window) = app.get_webview_window(EDITOR_WINDOW_LABEL) {
            window.set_focus()?;
            window.emit(EDITOR_DRAFT_EVENT, id.clone())?;
            return Ok(());
        }

        tauri::WebviewWindowBuilder::new(
            &app,
            EDITOR_WINDOW_LABEL,
            tauri::WebviewUrl::App("index.html?editor=1".into()),
        )
        // 无系统标题栏：和主窗口一样，标题栏由页面自己画（那条 `.editor-titlebar`
        // 同时是拖拽区与窗口按钮），否则会多出一条重复的标题栏。
        .title("DPEditor")
        .decorations(false)
        .inner_size(1600.0, 1000.0)
        .min_inner_size(1100.0, 700.0)
        .build()?;

        info!("[编辑窗口] 打开编辑窗口：草稿 {id}");
        Ok(())
    })()
    .map_err(InvokeError::from_anyhow)
}

/// 编辑窗口启动时问「我要编辑哪个草稿」。
#[tauri::command]
fn editor_context() -> Option<String> {
    EDITOR_DRAFT.lock().unwrap().clone()
}

/// 草稿列表（最近改过的排前面），编辑窗口的「草稿」菜单用它。
#[tauri::command]
fn list_editor_drafts() -> Result<Vec<drafts::EditorDraft>, InvokeError> {
    drafts::list().map_err(InvokeError::from_anyhow)
}

/// 读一个草稿。
#[tauri::command]
fn load_editor_draft(id: String) -> Result<drafts::EditorDraft, InvokeError> {
    drafts::load(&id).map_err(InvokeError::from_anyhow)
}

/// 新建一个空草稿（名字缺省时自动编号）。
#[tauri::command]
fn create_editor_draft(name: Option<String>) -> Result<drafts::EditorDraft, InvokeError> {
    drafts::create(name).map_err(InvokeError::from_anyhow)
}

/// 保存草稿（轨道上放了什么、名字改没改都在这里面）。
#[tauri::command]
fn save_editor_draft(draft: drafts::EditorDraft) -> Result<drafts::EditorDraft, InvokeError> {
    drafts::save(draft).map_err(InvokeError::from_anyhow)
}

/// 删掉一个草稿（连同它那个目录）。
#[tauri::command]
fn delete_editor_draft(id: String) -> Result<(), InvokeError> {
    (|| Ok(drafts::remove(&id)?))().map_err(InvokeError::from_anyhow)
}

/// 打开草稿目录：`<程序目录>/dpe/editor/drafts`。
#[tauri::command]
fn open_editor_drafts_folder() -> Result<(), InvokeError> {
    (|| {
        let dir = drafts::drafts_dir()?;
        info!("[编辑窗口] 打开草稿目录 {}", dir.display());
        open_folder(&dir)?;
        Ok(())
    })()
    .map_err(InvokeError::from_anyhow)
}

/// 把用户挑中的文件包成素材项（**不复制文件**，只记路径）。
///
/// 认不出种类的那些跳过：一次挑十个文件里混了一个 `.txt`，不该让整次导入失败；
/// 一个都没认出来才报错，由前端提示支持哪些格式。
///
/// 拖进来的是文件夹时把它里面的素材摊开收进来（往后最多三十一层，跳过以 `.` 开头的项），
/// 但目录本身永远不当成「一张谱面」——谱面轨道只收谱面文件。`.pez` / `.zip` 不按文件收：
/// 解开成一层文件夹，素材库按包名建文件夹装它里面的东西。
#[tauri::command]
fn import_editor_media(paths: Vec<String>) -> Result<Vec<drafts::MediaInDir>, InvokeError> {
    (|| Ok(drafts::media_import(&paths)?))().map_err(InvokeError::from_anyhow)
}

/// 把一个文件夹里认得出的素材全部包成素材项（同样只记路径，不复制）。
///
/// 用给素材库的「导入素材 → 选文件夹」：谱面包目录里那些 json / 音频 / 曲绘一次就都进素材库
/// （往后最多三十一层），另外带回每一项在选中目录下的相对目录，素材库照它建出真实的文件夹层级；
/// 目录里的 `.pez` / `.zip` 解开成一层文件夹，不按压缩包文件出现。
#[tauri::command]
fn import_editor_media_dir(dir: String) -> Result<Vec<drafts::MediaInDir>, InvokeError> {
    (|| Ok(drafts::media_import_in_dir(Path::new(&dir))?))().map_err(InvokeError::from_anyhow)
}

/// 读一个素材文件的原始字节（预览区把音频 / 曲绘塞进播放器，曲绘也用它出缩略图）。
#[tauri::command]
fn read_editor_media(path: String) -> Result<tauri::ipc::Response, InvokeError> {
    (|| {
        let bytes =
            std::fs::read(&path).with_context(|| format!("读取素材失败：{path}"))?;
        Ok(tauri::ipc::Response::new(bytes))
    })()
    .map_err(InvokeError::from_anyhow)
}

/// 解析一份歌词文件（`.srt` / `.lrc` / `.ass` / `.ssa` / `.vtt`）：一句一条，带回起止时间。
///
/// 编辑窗口拿到这些句子后一句一个片段地铺到一条特效轨上（每个片段是一个文字噪域），
/// 所以这里只负责读时间与文字；`.lrc` 没有结束时间，由前端按下一句的起点补。
#[tauri::command]
fn parse_editor_lyrics(path: String) -> Result<Vec<lyrics::LyricLine>, InvokeError> {
    (|| Ok(lyrics::parse(Path::new(&path))?))().map_err(InvokeError::from_anyhow)
}

/// 挑出已经不存在的素材路径（草稿只记路径，原文件被挪走或删掉时要能看出来）。
#[tauri::command]
fn missing_editor_media(paths: Vec<String>) -> Vec<String> {
    paths
        .into_iter()
        .filter(|path| !Path::new(path).exists())
        .collect()
}

/// 编辑窗口列表里的一张谱面：摊开后的目录 + 解析出来的谱面信息。
#[derive(Serialize)]
struct EditorChartEntry {
    path: String,
    info: ChartInfo,
}

/// 扫一遍编辑窗口的谱面目录，`<程序目录>/dpe/editor/charts` 下**每个子目录就是一张谱面**。
///
/// 列表以这个目录为准：重名目录（`名字` 与 `名字 (2)`）各自一张，不按曲名合并；
/// 目录里多出来的（用户自己拷进去的、重新解包的）也照列，少了的由前端去掉卡片。
/// 单个目录解析不了（不是谱面、文件被删了一半）只跳过它，不让整张列表失败。
#[tauri::command]
async fn list_editor_charts() -> Result<Vec<EditorChartEntry>, InvokeError> {
    wrap_async(async move {
        let dir = common::editor_charts_dir()?;
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("读取谱面列表文件夹失败：{}", dir.display()))?
        {
            let path = entry?.path();
            if path.is_dir() {
                paths.push(path);
            }
        }
        // 目录名的顺序就是默认顺序（前端还会按自己的排序规则再排一次）
        paths.sort();

        let mut charts = Vec::with_capacity(paths.len());
        for path in paths {
            match load_editor_chart_info(&path).await {
                Ok(info) => charts.push(EditorChartEntry {
                    path: path.to_string_lossy().to_string(),
                    info,
                }),
                Err(err) => warn!(
                    "[编辑窗口] 跳过无法解析的谱面 {}：{}",
                    path.display(),
                    common::error_chain(&err)
                ),
            }
        }
        Ok(charts)
    })
    .await
}

/// 网页播放器要的谱面资源清单：谱面里的全部文件，相对路径用 `/` 分隔。
///
/// 谱面既可能是**目录**（摊开的一份），也可能是 `.pez` / `.zip`（草稿直接引用用户那份包），
/// 两种都要能列：目录走递归，包就读 zip 的条目表。从解包缓存里摊出来的谱面包（素材库把压缩包
/// 解开的那些）按**整包**列：谱面轨道只拿了包里的某个 json，播放器却还要包里的曲绘与配置。
///
/// 播放器（`DPS/dps/src/bin/wasm_player.rs` 编出来的 wasm）在浏览器里没有文件系统，谱面
/// 资源由前端读成字节、塞进它的内存文件系统，所以要先把文件名列出来。
#[tauri::command]
async fn list_editor_chart_files(path: String) -> Result<Vec<String>, InvokeError> {
    let root = PathBuf::from(&path);
    let package = if root.is_dir() { Some(root.clone()) } else { editor_unpacked_root(&root) };
    if let Some(package) = package {
        return wrap_async(async move {
            let mut files = Vec::new();
            collect_editor_chart_files(&package, &package, &mut files)?;
            files.sort();
            Ok(files)
        })
        .await;
    }
    (|| {
        let file = std::fs::File::open(&root)
            .with_context(|| format!("打不开谱面包：{}", root.display()))?;
        let mut archive = match zip::ZipArchive::new(file) {
            Ok(archive) => archive,
            // 散装谱面：只有一个 json / pec 文件，没有包可拆。
            Err(_) => {
                let name = root
                    .file_name()
                    .map(|it| it.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if name.is_empty() {
                    bail!("{} 不是合法的谱面包", root.display());
                }
                return Ok(vec![name]);
            }
        };
        let mut files = Vec::with_capacity(archive.len());
        for index in 0..archive.len() {
            let entry = archive.by_index(index)?;
            if entry.is_dir() {
                continue;
            }
            files.push(entry.name().replace('\\', "/"));
        }
        files.sort();
        Ok(files)
    })()
    .map_err(InvokeError::from_anyhow)
}

/// 谱面文件是不是从解包缓存里摊出来的：是就返回那个包解出来的根目录。
///
/// 压缩包解开后谱面轨道拿到的只是包里的某个 json，播放器却要整包（曲绘、`info.txt` 这些），
/// 所以按缓存目录的第一段找回包的根。
fn editor_unpacked_root(path: &Path) -> Option<PathBuf> {
    let cache = common::editor_unpacked_dir().ok()?;
    let rel = path.strip_prefix(&cache).ok()?;
    let root = cache.join(rel.components().next()?);
    root.is_dir().then_some(root)
}

/// 把 `/` 分隔的条目名拼到目录上；空段、`.`、`..` 一律拒绝（不许跳出谱面包）。
fn join_entry(dir: &Path, name: &str) -> Result<PathBuf> {
    let mut full = dir.to_path_buf();
    for seg in name.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            bail!("非法的谱面文件名：{name}");
        }
        full.push(seg);
    }
    Ok(full)
}

/// 递归收集谱面目录里的文件（相对路径用 `/`），读不了的子项跳过。
fn collect_editor_chart_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("读取谱面目录失败：{}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_editor_chart_files(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// 读谱面里的一个文件，按原始字节回给前端（网页播放器把它们塞进 wasm 内存文件系统）。
///
/// 目录与 `.pez` / `.zip` 都认：目录按相对路径拼，包按 zip 条目名取；
/// 散装谱面（单个 `.json`）的条目名就是文件名本身，解包缓存里摊出来的则相对包的根。
#[tauri::command]
fn read_editor_chart_file(path: String, name: String) -> Result<tauri::ipc::Response, InvokeError> {
    (|| {
        let root = PathBuf::from(&path);
        if !root.is_dir() {
            // 解包缓存里的谱面包：条目名相对包根，谱面本身可能就在包里的子目录下
            if let Some(package) = editor_unpacked_root(&root) {
                let full = join_entry(&package, &name)?;
                let bytes = std::fs::read(&full)
                    .with_context(|| format!("读取谱面文件失败：{}", full.display()))?;
                return Ok(tauri::ipc::Response::new(bytes));
            }
            let file = std::fs::File::open(&root)
                .with_context(|| format!("打不开谱面包：{}", root.display()))?;
            let mut archive = match zip::ZipArchive::new(file) {
                Ok(archive) => archive,
                // 散装谱面只有一个文件，条目名就是文件名本身。
                Err(_) => {
                    let single = root
                        .file_name()
                        .map(|it| it.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if single != name {
                        bail!("{} 里没有 {name}", root.display());
                    }
                    let bytes = std::fs::read(&root)
                        .with_context(|| format!("读取谱面文件失败：{}", root.display()))?;
                    return Ok(tauri::ipc::Response::new(bytes));
                }
            };
            let mut entry = archive
                .by_name(&name)
                .with_context(|| format!("谱面包里没有 {name}"))?;
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            std::io::copy(&mut entry, &mut bytes)
                .with_context(|| format!("读取谱面包条目失败：{name}"))?;
            return Ok(tauri::ipc::Response::new(bytes));
        }

        let full = join_entry(&root, &name)?;
        let bytes = std::fs::read(&full)
            .with_context(|| format!("读取谱面文件失败：{}", full.display()))?;
        Ok(tauri::ipc::Response::new(bytes))
    })()
    .map_err(InvokeError::from_anyhow)
}

/// 导出包里塞的空谱面：谱面轨道空着也得有一张，播放器没有谱面起不来。
///
/// 与预览里塞的那张一致：PGR 格式，一条判定线，没有任何音符。
const EMPTY_EDITOR_CHART: &str = r#"{"formatVersion":3,"offset":0,"judgeLineList":[{"bpm":120,"speedEvents":[],"notesAbove":[],"notesBelow":[],"judgeLineMoveEvents":[],"judgeLineRotateEvents":[],"judgeLineDisappearEvents":[]}]}"#;

/// 导出包里一个条目的来源：磁盘上的文件按路径现读，内存里生成的（`info.txt`、空谱面）直接给字节。
enum ExportEntry {
    File(PathBuf),
    Bytes(Vec<u8>),
}

/// 把草稿导出成 `<dpe>/editor/output/<曲名>_<草稿名>.pez`，返回文件的绝对路径。
///
/// 条件是**放了音乐 + 写了曲名**：播放器没有音乐就起不来（引擎的谱面长度取音乐长度），
/// 没有曲名的包在任何列表里都只能显示成文件名。谱面、曲绘都可以空着 —— 谱面空着就塞一张
/// 空谱面，曲绘空着就黑屏。
///
/// 包里的内容就是「谱面包 + 音乐 + 曲绘 + 一份 `info.txt`」：谱面包按整包带上（目录递归、
/// 压缩包读条目、解包缓存里摊出来的按包根找回整包），音乐与曲绘按原文件名铺在根上，
/// `info.txt` 最后写，盖掉包里自带的那份。
///
/// `block_areas` 是编辑器里特效轨铺出来的噪域方块（谱面顶层 `blockAreaList` 的原生条目）：
/// 引擎只认谱面里的噪域，别的入口都没有，所以在这一步并进谱面字节。只动 `.json` 谱面，
/// `.pec` 没有噪域这个字段，原样带出去。
#[tauri::command]
fn export_editor_draft(
    draft: drafts::EditorDraft,
    block_areas: Option<serde_json::Value>,
) -> Result<String, InvokeError> {
    (|| {
        let title = draft.info.name.trim();
        if title.is_empty() {
            bail!("还没有写曲名，导出不了");
        }
        let music = draft.music.as_ref().context("还没有放音乐，导出不了")?;
        let music_path = PathBuf::from(&music.path);
        if !music_path.is_file() {
            bail!("音乐文件不在了：{}", music_path.display());
        }
        let music_name = file_name_of(&music_path)?;

        let mut entries: Vec<(String, ExportEntry)> = Vec::new();
        // 谱面包：条目名相对包根
        if let Some(chart) = draft.chart.as_ref() {
            let chart_path = PathBuf::from(&chart.path);
            let package = if chart_path.is_dir() {
                Some(chart_path.clone())
            } else {
                editor_unpacked_root(&chart_path)
            };
            match package {
                Some(root) => {
                    let mut files = Vec::new();
                    collect_editor_chart_files(&root, &root, &mut files)?;
                    for name in files {
                        let source = join_entry(&root, &name)?;
                        push_export_entry(&mut entries, name, ExportEntry::File(source));
                    }
                }
                None if chart_path.is_file() => {
                    match std::fs::File::open(&chart_path).map(zip::ZipArchive::new) {
                        // 还没解开过的 `.pez` / `.zip`：条目整份读进内存，谱面包大多只有几 MB
                        Ok(Ok(mut archive)) => {
                            for index in 0..archive.len() {
                                let mut entry = archive.by_index(index)?;
                                if entry.is_dir() {
                                    continue;
                                }
                                let name = entry.name().replace('\\', "/");
                                let mut bytes = Vec::with_capacity(entry.size() as usize);
                                std::io::copy(&mut entry, &mut bytes)
                                    .with_context(|| format!("读取谱面包条目失败：{name}"))?;
                                push_export_entry(&mut entries, name, ExportEntry::Bytes(bytes));
                            }
                        }
                        // 散装谱面（单个 json / pec）：条目名就是文件名本身
                        _ => {
                            push_export_entry(
                                &mut entries,
                                file_name_of(&chart_path)?,
                                ExportEntry::File(chart_path.clone()),
                            );
                        }
                    }
                }
                // 谱面文件已经不在了：当作没放谱面
                None => {}
            }
        }

        // 谱面轨道上挂着的那份，包里没有就按包里的第一个谱面找
        let wanted = draft
            .chart
            .as_ref()
            .map(|it| PathBuf::from(&it.path))
            .and_then(|it| it.file_name().map(|it| it.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let chart_name = entries
            .iter()
            .map(|(name, _)| name.clone())
            .find(|name| !wanted.is_empty() && (*name == wanted || name.ends_with(&format!("/{wanted}"))))
            .or_else(|| {
                entries
                    .iter()
                    .map(|(name, _)| name.clone())
                    .find(|name| is_editor_chart_name(name))
            });
        let chart_name = match chart_name {
            Some(name) => name,
            None => {
                push_export_entry(
                    &mut entries,
                    "editor-empty.json".to_owned(),
                    ExportEntry::Bytes(EMPTY_EDITOR_CHART.as_bytes().to_vec()),
                );
                "editor-empty.json".to_owned()
            }
        };

        // 特效轨铺出来的噪域并进谱面字节：引擎没有别的入口能加噪域（见 `inject_block_areas`）。
        // 材质跟着每条噪域走（条目里的 `material`），不用再往包里塞渲染配置。
        if let Some(areas) = block_areas.as_ref().and_then(|it| it.as_array()) {
            if !areas.is_empty() {
                inject_block_areas(&mut entries, &chart_name, areas)?;
            }
        }

        // 音乐、曲绘铺在包根上，按原文件名
        push_export_entry(&mut entries, music_name.clone(), ExportEntry::File(music_path));
        let mut cover_name = None;
        if let Some(cover) = draft.cover.as_ref() {
            let cover_path = PathBuf::from(&cover.path);
            if cover_path.is_file() {
                let name = file_name_of(&cover_path)?;
                push_export_entry(&mut entries, name.clone(), ExportEntry::File(cover_path));
                cover_name = Some(name);
            }
        }

        // `info.txt` 最后写，盖掉包里自带的那份
        let mut info = String::from("#\n");
        info.push_str(&format!("Name: {title}\n"));
        for (key, value) in [
            ("Level", draft.info.difficulty.trim()),
            ("Charter", draft.info.charter.trim()),
            ("Composer", draft.info.composer.trim()),
        ] {
            if !value.is_empty() {
                info.push_str(&format!("{key}: {value}\n"));
            }
        }
        info.push_str(&format!("Chart: {chart_name}\nMusic: {music_name}\n"));
        if let Some(cover) = cover_name.as_ref() {
            info.push_str(&format!("Image: {cover}\n"));
        }
        push_export_entry(
            &mut entries,
            "info.txt".to_owned(),
            ExportEntry::Bytes(info.into_bytes()),
        );

        let draft_name = match draft.name.trim() {
            "" => "未命名草稿".to_owned(),
            name => common::safe_file_name(name),
        };
        let output = common::editor_output_dir()?
            .join(format!("{}_{draft_name}.pez", common::safe_file_name(title)));

        let mut pack = common::ZipPack::create(&output, 9)?;
        let mut buffer = vec![0u8; 1 << 20];
        for (name, entry) in entries {
            match entry {
                ExportEntry::Bytes(bytes) => pack.bytes(&name, &bytes)?,
                ExportEntry::File(path) => {
                    let mut source = std::fs::File::open(&path)
                        .with_context(|| format!("读取文件失败：{}", path.display()))?;
                    pack.entry(&name, |out| {
                        loop {
                            let read = std::io::Read::read(&mut source, &mut buffer)?;
                            if read == 0 {
                                return Ok(());
                            }
                            std::io::Write::write_all(out, &buffer[..read])?;                        }
                    })?;
                }
            }
        }
        pack.finish()?;
        Ok(output.to_string_lossy().into_owned())
    })()
    .map_err(InvokeError::from_anyhow)
}

/// 导出包里的条目名：重名时后写的盖掉先写的（`info.txt`、音乐、曲绘要压过包里那份）。
fn push_export_entry(entries: &mut Vec<(String, ExportEntry)>, name: String, entry: ExportEntry) {
    match entries.iter_mut().find(|(it, _)| *it == name) {
        Some(slot) => slot.1 = entry,
        None => entries.push((name, entry)),
    }
}

/// 把噪域方块追加到导出谱面的顶层 `blockAreaList`（PGR 与 RPE 都读这个字段）。
///
/// 谱面本来就有噪域时接在它后面：编辑器加的特效叠在谱面自带的上面，跟预览里看到的一致。
/// `.pec` 没有噪域字段、读不动的谱面也不算错 —— 这两种情况原样带出去，别把用户的谱面改坏。
fn inject_block_areas(
    entries: &mut Vec<(String, ExportEntry)>,
    chart_name: &str,
    areas: &[serde_json::Value],
) -> Result<()> {
    if !chart_name.to_ascii_lowercase().ends_with(".json") {
        return Ok(());
    }
    let Some(slot) = entries.iter_mut().find(|(name, _)| name == chart_name) else {
        return Ok(());
    };
    let bytes = match &slot.1 {
        ExportEntry::Bytes(bytes) => bytes.clone(),
        ExportEntry::File(path) => {
            std::fs::read(path).with_context(|| format!("读取谱面文件失败：{}", path.display()))?
        }
    };
    let Ok(mut chart) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Ok(());
    };
    let Some(object) = chart.as_object_mut() else {
        return Ok(());
    };
    let list = object
        .entry("blockAreaList")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    match list.as_array_mut() {
        Some(list) => list.extend(areas.iter().cloned()),
        // 手改过的谱面把 `blockAreaList` 写成了别的东西：整份换成编辑器算出来的那些
        None => *list = serde_json::Value::Array(areas.to_vec()),
    }
    slot.1 = ExportEntry::Bytes(
        serde_json::to_vec(&chart).context("把噪域写进谱面失败")?,
    );
    Ok(())
}

/// 文件名的文本形式；没有文件名（路径以 `..` 结尾之类）就是出错。
fn file_name_of(path: &Path) -> Result<String> {
    path.file_name()
        .map(|it| it.to_string_lossy().into_owned())
        .filter(|it| !it.is_empty())
        .with_context(|| format!("文件没有文件名：{}", path.display()))
}

/// 条目名是不是谱面文件（与 [`drafts::classify`] 同一个口径：`json` / `pec`）。
fn is_editor_chart_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .map(|it| {
            let extension = it.to_string_lossy().to_ascii_lowercase();
            extension == "json" || extension == "pec"
        })
        .unwrap_or(false)
}

/// 读一张摊开后的谱面的信息（与 `parse_chart` 同一条路，只是不经过命令层）。
async fn load_editor_chart_info(path: &Path) -> Result<ChartInfo> {
    let mut fs: Box<dyn FileSystem + Send + Sync + 'static> = fs::fs_from_file(path)
        .with_context(|| mtl!("read-chart-failed", "path" => path.display().to_string()))?;
    fs::load_info(fs.deref_mut())
        .await
        .with_context(|| mtl!("load-info-failed", "path" => path.display().to_string()))
}

/// DPE「生成谱面」：复制源 `.pez`，把谱面修改写进它的 `config.toml`，落到
/// `<程序目录>/dpe/modify/<原名>.(<修改1>→<修改2>).pez`，返回新文件的绝对路径。
///
/// 修改以 `config.toml` 的形式跟着谱面走：渲染页导入这张谱面时会把它们合并进工作
/// 配置（`chart_config::read`），所以不必改动谱面本体，也就不受「引擎没有谱面写出
/// 能力」的限制。源包里原有的 `config.toml` 先经白名单过滤再与新修改合并，
/// 用户自己写的东西不会丢。
///
/// `labels` 是生效的修改名（按落地顺序，纯配置谱面时就是「纯配置谱面」）：文件名与难度
/// 都由它拼，口径同渲染页的 `chartModMarker`。
#[tauri::command]
async fn dpe_generate_modified(
    source: String,
    labels: Vec<String>,
    entries: serde_json::Map<String, serde_json::Value>,
) -> Result<String, InvokeError> {
    async fn inner(
        source: String,
        labels: Vec<String>,
        entries: serde_json::Map<String, serde_json::Value>,
    ) -> anyhow::Result<String> {
        let source = std::path::PathBuf::from(source);
        if !source.is_file() {
            anyhow::bail!("谱面包不存在：{}", source.display());
        }
        let stem = source
            .file_stem()
            .map(|it| it.to_string_lossy().to_string())
            .filter(|it| !it.is_empty())
            .ok_or_else(|| anyhow::anyhow!("谱面包没有文件名：{}", source.display()))?;

        let labels = labels
            .into_iter()
            .map(|it| it.trim().to_owned())
            .filter(|it| !it.is_empty())
            .collect::<Vec<_>>();

        // 源谱面若已经是「（内容）」式的修改链（这个命令自己生成过的、或融合结果），把内容
        // 当成链首接在后面，而不是再套一层括号。读不出谱面信息也照常生成，只是难度不改。
        let (info, lead) = if labels.is_empty() {
            (None, None)
        } else {
            match fs::fs_from_file(&source) {
                Ok(mut pack) => match fs::load_info(pack.deref_mut()).await {
                    Ok(info) => {
                        let lead = chain_lead(&info);
                        (Some(modified_info(info, lead.as_deref(), &labels)), lead)
                    }
                    Err(_) => (None, None),
                },
                Err(_) => (None, None),
            }
        };

        // 文件名里不允许出现的字符一律抹掉；一个名字都没剩下时用兜底的名字。
        let names = lead
            .iter()
            .map(String::as_str)
            .chain(labels.iter().map(String::as_str))
            .map(|it| {
                it.chars()
                    .filter(|it| !matches!(it, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
                    .collect::<String>()
                    // 链首是难度文本，里面的 ` → ` 带空格；文件名里统一成不带空格的 `→`。
                    .replace(" → ", "→")
            })
            .map(|it| it.trim().to_owned())
            .filter(|it| !it.is_empty())
            .collect::<Vec<_>>();
        // 源文件名若已经带着上一次生成的「(修改链)」尾巴，先裁掉再拼新链；否则会出现
        // `X.SP(陈列按键).(陈列按键→随机位置→长条填充).pez` 这种旧标识叠着的名字。
        let stem = strip_old_marker(&stem, &names);

        let suffix = if names.is_empty() {
            "修改".to_owned()
        } else {
            format!("({})", names.join("→"))
        };

        let dir = common::dpe_output_dir(common::DPE_MODIFY_DIR)?;
        let dest = dir.join(format!("{stem}.{suffix}.pez"));

        let mut merged = match crate::chart_config::read(&source).await {
            Ok(Some(config)) => config,
            // 谱面自带配置坏了不该挡住生成：丢掉它，从空表开始。
            Ok(None) | Err(_) => serde_json::Map::new(),
        };
        for (key, value) in entries {
            merged.insert(key, value);
        }
        crate::chart_config::write_generated(&source, &dest, &merged, info.as_ref())?;
        Ok(dest.to_string_lossy().to_string())
    }

    inner(source, labels, entries)
        .await
        .map_err(InvokeError::from_anyhow)
}

/// 源谱面的难度文本若已经是 `（内容）` 这样的修改链，取出括号里的内容当作链首。
///
/// 三处（`diff` / `level` / `difficulty`）都可能是它，取第一个命中的；内容里还带括号的
/// 不算一条链（那是谱面作者自己写的括号说明），不接。
fn chain_lead(info: &ChartInfo) -> Option<String> {
    [info.diff.trim(), info.level.trim(), info.difficulty.trim()]
        .into_iter()
        .find_map(|it| {
            let inner = it.strip_prefix('（')?.strip_suffix('）')?.trim();
            (!inner.is_empty() && !inner.contains('（') && !inner.contains('）'))
                .then(|| inner.to_owned())
        })
}

/// 把源文件名末尾上一次生成的「(修改链)」裁掉。
///
/// 生成过的谱面包名形如 `曲名.谱师.0.SP.(陈列按键→随机位置)`。在它上面再生成一次时若直接
/// 往后接，同一个标识就会出现两遍：`X.SP.(陈列按键).(陈列按键→随机位置→长条填充)`。
/// 因此只要末尾括号里的内容确实是这次新链的开头（链首或前几段，见 `names`），就把它连同
/// 前面的分隔符一起裁掉；谱面自带的括号说明（`Song (Live).pez` 这类）不动。
fn strip_old_marker(stem: &str, names: &[String]) -> String {
    if names.is_empty() {
        return stem.to_owned();
    }
    let chain = names
        .iter()
        .flat_map(|it| it.split('→'))
        .map(str::trim)
        .filter(|it| !it.is_empty())
        .collect::<Vec<_>>();

    let mut current = stem.to_owned();
    // 旧名字可能叠过几层，只要还对得上就继续往前裁。
    while let Some(open) = current.rfind(|it| it == '(' || it == '（') {
        let tail = &current[open..];
        let inner = tail
            .strip_prefix('(')
            .and_then(|it| it.strip_suffix(')'))
            .or_else(|| tail.strip_prefix('（').and_then(|it| it.strip_suffix('）')))
            .map(str::trim);
        let Some(inner) = inner else { break };
        let segs = inner
            .split('→')
            .map(str::trim)
            .filter(|it| !it.is_empty())
            .collect::<Vec<_>>();
        let shared = segs.len().min(chain.len());
        if shared == 0 || segs[..shared] != chain[..shared] {
            break;
        }
        let head = current[..open]
            .trim_end_matches(|it: char| it == '.' || it == '_' || it == '-' || it.is_whitespace())
            .trim_end()
            .to_owned();
        if head.is_empty() {
            break;
        }
        current = head;
    }
    current
}

/// 生成包的谱面信息：难度标识与难度文本都写成修改链 `（A → B）`，定数照旧。
///
/// `info.yml` 的约定是 `level` 放难度文本、`difficulty` 放定数，`load_info` 拿前者盖后者，
/// 而界面上的难度标识取自 `diff` —— 三处写同一份才不会留下旧难度。`lead` 是源谱面已有的
/// 那一环（见 [`chain_lead`]），摆在链首，与 `chartSide.ts::chartModMarker` 同一口径。
fn modified_info(mut info: ChartInfo, lead: Option<&str>, labels: &[String]) -> ChartInfo {
    let mut parts: Vec<&str> = Vec::with_capacity(labels.len() + 1);
    parts.extend(lead);
    parts.extend(labels.iter().map(String::as_str));
    if parts.is_empty() {
        return info;
    }
    let text = format!("（{}）", parts.join(" → "));
    info.diff = text.clone();
    info.level = text.clone();
    info.difficulty = text;
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info_with(diff: &str, level: &str, difficulty: &str) -> ChartInfo {
        ChartInfo {
            diff: diff.to_owned(),
            level: level.to_owned(),
            difficulty: difficulty.to_owned(),
            ..Default::default()
        }
    }

    fn labels(items: &[&str]) -> Vec<String> {
        items.iter().map(|it| it.to_string()).collect()
    }

    #[test]
    fn chain_lead_takes_one_parenthesis_layer() {
        // 已经改过一次的谱面：把括号里的整条链接上去，别套两层括号。
        let info = info_with("（随机流速 → 随机按键）", "（随机流速 → 随机按键）", "（随机流速 → 随机按键）");
        assert_eq!(chain_lead(&info).as_deref(), Some("随机流速 → 随机按键"));

        // 融合结果单独占一环。
        let merged = info_with("（谱面融合）", "（谱面融合）", "（谱面融合）");
        assert_eq!(chain_lead(&merged).as_deref(), Some("谱面融合"));

        // 原谱难度、空括号、括号里还嵌括号的，都不算一条链。
        assert_eq!(chain_lead(&info_with("IN", "IN Lv.15", "15")), None);
        assert_eq!(chain_lead(&info_with("（）", "", "")), None);
        assert_eq!(chain_lead(&info_with("（自制（改））", "", "")), None);
    }

    #[test]
    fn modified_info_appends_to_existing_chain() {
        let info = modified_info(
            info_with("（随机流速 → 随机按键）", "（随机流速 → 随机按键）", "（随机流速 → 随机按键）"),
            Some("随机流速 → 随机按键"),
            &labels(&["随机位置"]),
        );
        assert_eq!(info.diff, "（随机流速 → 随机按键 → 随机位置）");
        assert_eq!(info.level, "（随机流速 → 随机按键 → 随机位置）");
        assert_eq!(info.difficulty, "（随机流速 → 随机按键 → 随机位置）");

        // 原谱难度被整串换掉，链首为空时只剩这次开的修改。
        let plain = modified_info(info_with("IN", "IN Lv.15", "15"), None, &labels(&["随机流速", "随机按键"]));
        assert_eq!(plain.diff, "（随机流速 → 随机按键）");
    }

    #[test]
    fn strip_old_marker_drops_previous_chain() {
        let mods = labels(&["陈列按键", "随机位置", "长条填充"]);

        // 上一次生成的结局：括号紧贴难度标识，或前面带一个点。
        assert_eq!(strip_old_marker("Message.くるぶっこちゃん.0.SP(陈列按键)", &mods), "Message.くるぶっこちゃん.0.SP");
        assert_eq!(strip_old_marker("Message.(陈列按键→随机位置)", &mods), "Message");

        // 谱面自带的括号说明（内容对不上修改链）不裁。
        assert_eq!(strip_old_marker("Song (Live)", &mods), "Song (Live)");
        assert_eq!(strip_old_marker("Song", &mods), "Song");
    }

    /// 导出把三条轨道上的东西和一个 `info.txt` 都装进一个 `.pez`：
    /// 谱面按原文件名、音乐与曲绘铺在根上，`info.txt` 写曲名与三条引用。
    #[test]
    fn export_packs_tracks_and_info_txt() {
        let dir = std::env::temp_dir().join(format!("dpr-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let chart = dir.join("chart.json");
        let music = dir.join("song.mp3");
        let cover = dir.join("cover.png");
        std::fs::write(&chart, EMPTY_EDITOR_CHART).unwrap();
        std::fs::write(&music, b"fake audio").unwrap();
        std::fs::write(&cover, b"fake image").unwrap();

        let draft = drafts::EditorDraft {
            info: drafts::DraftInfo {
                name: "测试曲".to_owned(),
                difficulty: "IN Lv.15".to_owned(),
                ..Default::default()
            },
            chart: Some(drafts::media_from_path(&chart).unwrap()),
            music: Some(drafts::media_from_path(&music).unwrap()),
            cover: Some(drafts::media_from_path(&cover).unwrap()),
            ..Default::default()
        };
        let saved = export_editor_draft(draft.clone(), None).expect("导出应当成功");
        assert!(saved.ends_with("测试曲_未命名草稿.pez"), "文件名：{saved}");

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&saved).unwrap()).unwrap();
        let mut names = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_owned())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, vec!["chart.json", "cover.png", "info.txt", "song.mp3"]);

        let mut info = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("info.txt").unwrap(), &mut info).unwrap();
        assert!(info.starts_with("#\n"), "info.txt：{info}");
        assert!(info.contains("Name: 测试曲\n"));
        assert!(info.contains("Level: IN Lv.15\n"));
        assert!(info.contains("Chart: chart.json\n"));
        assert!(info.contains("Music: song.mp3\n"));
        assert!(info.contains("Image: cover.png\n"));

        // 没有曲名 / 没有音乐都不许导出
        let mut no_title = draft.clone();
        no_title.info.name = "  ".to_owned();
        assert!(export_editor_draft(no_title, None).is_err());
        let mut no_music = draft;
        no_music.music = None;
        assert!(export_editor_draft(no_music, None).is_err());

        let _ = std::fs::remove_file(&saved);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 程序（exe）所在文件夹的绝对路径 —— 设置页把「自定义输出目录 / 预设配置文件路径」显示成
/// 相对写法（`.\output`、`.\config\presets.toml`）时拿它当基准。
/// 悬停提示里给的仍是完整路径，所以这个基准只影响显示，不影响任何读写。
/// 开发时它是 `target\debug`，打包后就是安装目录 —— `output/` 与 `config/` 都住在那里。
#[tauri::command]
fn get_app_dir() -> Result<String, InvokeError> {
    let exe = std::env::current_exe().map_err(|e| {
        InvokeError::from_anyhow(anyhow::anyhow!("failed to locate the executable: {e}"))
    })?;
    let dir = exe.parent().ok_or_else(|| {
        InvokeError::from_anyhow(anyhow::anyhow!(
            "the executable has no parent directory: {}",
            exe.display()
        ))
    })?;
    Ok(dir.to_string_lossy().to_string())
}

/// 「日志」目录（默认 `<程序目录>/logs`）的绝对路径 —— 设置页那一格拿它显示，
/// 导出日志时当前端选保存位置的默认目录。目录由 `common::logs_dir()` 解析并保证存在。
#[tauri::command]
fn get_log_dir_path() -> Result<String, InvokeError> {
    common::logs_dir()
        .map(|path| path.to_string_lossy().to_string())
        .map_err(InvokeError::from_anyhow)
}

/// 在资源管理器里打开日志目录（日志文件只在启动时写，用户要自己看/拷走）。
#[tauri::command]
fn open_log_folder() -> Result<(), InvokeError> {
    (|| {
        let dir = common::logs_dir()?;
        open_folder(&dir)
    })()
    .map_err(InvokeError::from_anyhow)
}

/// 把一段日志写成纯文本文件（「输出」面板里那份可能带 ANSI 颜色码）。
/// 保存位置由前端 `dialog.save` 选，这里只负责剥掉颜色码再落盘 —— 导出的文件
/// 是给人贴进 issue 的，不该带转义序列。
#[tauri::command]
fn export_log(output_path: String, content: String) -> Result<(), InvokeError> {
    (|| {
        let path = Path::new(&output_path);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(path, common::strip_ansi(&content))?;
        info!("[日志] 导出日志 {}", path.display());
        Ok(())
    })()
    .map_err(InvokeError::from_anyhow)
}

/// 「资源包」目录（`<程序目录>/respack`）的绝对路径 —— 设置页那一格拿它显示成相对写法 `.\respack`。
/// 目录本身由 `common::respack_dir()` 解析并保证
/// 存在（启动时的便携布局也会在 exe 旁边建出 `respack/`，见 `setup`），这里只读不写。
#[tauri::command]
fn get_respack_dir_path() -> Result<String, InvokeError> {
    respack_dir()
        .map(|path| path.to_string_lossy().to_string())
        .map_err(InvokeError::from_anyhow)
}

/// 「附带资源包」目录（`<程序目录>/appendant`）的绝对路径 —— 设置页那一格拿它显示成相对写法 `.\appendant`。
/// 目录本身由 `common::appendant_dir()` 解析并保证存在，这里只读不写。
#[tauri::command]
fn get_appendant_dir_path() -> Result<String, InvokeError> {
    appendant_dir()
        .map(|path| path.to_string_lossy().to_string())
        .map_err(InvokeError::from_anyhow)
}

#[tauri::command]
async fn read_config() -> Result<AppConfig, InvokeError> {
    common::read_config().map_err(InvokeError::from_anyhow)
}

#[tauri::command]
async fn save_config(config: AppConfig) -> Result<(), InvokeError> {
    common::save_config(config).map_err(InvokeError::from_anyhow)
}

#[tauri::command]
async fn test_output_dir(dir: PathBuf) -> Result<(), InvokeError> {
    common::test_output_dir(dir).map_err(InvokeError::from_anyhow)
}

#[derive(Serialize)]
pub struct RPEChartInfo {
    name: String,
    id: String,
    path: String,
    illustration: String,
    charter: String,
    #[serde(skip)]
    modified: SystemTime,
}

#[tauri::command]
fn check_rpe_dir(path: PathBuf) -> Result<bool, InvokeError> {
    (|| {
        let valid = common::check_rpe_dir(path);
        Ok(valid)
    })()
    .map_err(InvokeError::from_anyhow)
}

#[tauri::command]
fn set_rpe_dir(path: PathBuf) -> Result<(), InvokeError> {
    (|| {
        common::set_rpe_dir(Some(path))?;
        Ok(())
    })()
    .map_err(InvokeError::from_anyhow)
}

#[tauri::command]
fn unset_rpe_dir() -> Result<(), InvokeError> {
    (|| {
        common::set_rpe_dir(None)?;
        Ok(())
    })()
    .map_err(InvokeError::from_anyhow)
}

#[tauri::command]
fn get_rpe_charts() -> Result<Option<Vec<RPEChartInfo>>, InvokeError> {
    (|| {
        let Ok(dir) = get_rpe_dir() else {
            return Ok(None);
        };
        let mut results = Vec::new();
        let mut name = None;
        let mut id = None;
        let mut chart = None;
        let mut illustration = None;
        let mut charter = None;
        macro_rules! commit {
            () => {
                (|| {
                    let id = id.take()?;
                    let path = dir.join("Resources").join(&id);
                    let metadata = path.join(chart.take()?).metadata().ok();

                    let modified = metadata
                        .and_then(|m| m.modified().ok())
                        .unwrap_or(SystemTime::UNIX_EPOCH);

                    results.push(RPEChartInfo {
                        name: name.take()?,
                        id,
                        path: path.display().to_string(),
                        illustration: path.join(illustration.take()?).display().to_string(),
                        charter: charter.take()?,
                        modified,
                    });
                    Some(())
                })()
            };
        }

        {
            use regex::Regex;
            static ONLY_NUM: OnceLock<Regex> = OnceLock::new();
            let only_num = ONLY_NUM.get_or_init(|| Regex::new(r"^\d+$").unwrap());
            let mut folders = Vec::new();
            let path = dir.join("Resources");
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    if let Some(folder_name) = path.file_name().and_then(|n| n.to_str()) {
                        if only_num.is_match(folder_name) {
                            folders.push(path);
                        }
                    }
                }
            }
            for folder in folders {
                info!(
                    "[谱面] 发现谱面文件夹 {}",
                    folder.file_name().unwrap_or_default().to_string_lossy()
                );
                if !folder.join("info.txt").exists() {
                    let folder_name = folder
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    warn!("[谱面] {} 里没有 info.txt", folder_name);
                    results.push(RPEChartInfo {
                        name: folder_name,
                        id: "Empty folder".to_string(),
                        path: folder.display().to_string(),
                        illustration: "".to_string(),
                        charter: "".to_string(),
                        modified: SystemTime::UNIX_EPOCH,
                    });
                    continue;
                }
                for line in BufReader::new(File::open(folder.join("info.txt"))?).lines() {
                    let line = line?;
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if line == "#" {
                        commit!();
                        continue;
                    }
                    let Some((key, value)) = line.split_once(':') else {
                        continue;
                    };
                    *(match key {
                        "Name" => &mut name,
                        "Path" => &mut id,
                        "Chart" => &mut chart,
                        "Picture" => &mut illustration,
                        "Charter" => &mut charter,
                        _ => continue,
                    }) = Some(value.trim().to_owned());
                }
                commit!();
            }
        }

        results.sort_by(|a, b| b.modified.cmp(&a.modified));

        Ok(Some(results))
    })()
    .map_err(InvokeError::from_anyhow)
}

#[tauri::command]
fn open_app_folder() -> Result<(), InvokeError> {
    (|| {
        let exe_path = std::env::current_exe()?;
        let path = exe_path.parent().unwrap();
        info!("[目录] 打开程序所在文件夹 {}", path.display());
        open_folder(path)?;
        Ok(())
    })()
    .map_err(InvokeError::from_anyhow)
}

#[tauri::command]
fn check_ffmpeg() -> Result<bool, InvokeError> {
    (|| Ok(render::find_ffmpeg()?.is_some()))().map_err(InvokeError::from_anyhow)
}

/// 探测到的 ffmpeg **所在目录**（没探测到就是 `None`）—— 设置页「FFmpeg 路径」那格空着时
/// 拿它当真值显示，跟输出目录、预设文件那两格一个做法。那格写的就是**目录**，
/// 不再是 exe 本身（见 `render::resolve_ffmpeg_exe`）。
#[tauri::command]
fn get_ffmpeg_path() -> Result<Option<String>, InvokeError> {
    (|| Ok(render::find_ffmpeg_dir()?))().map_err(InvokeError::from_anyhow)
}

/// 启动时自动下载 FFmpeg，成功后返回 ffmpeg 的完整路径。
///
/// 进度走 `ffmpeg-download` 事件（见 `ffmpeg_download::Progress`），前端弹窗照着画进度条。
#[tauri::command]
async fn download_ffmpeg(app: tauri::AppHandle) -> Result<String, InvokeError> {
    ffmpeg_download::download_ffmpeg(&app)
        .await
        .map_err(InvokeError::from_anyhow)
}

/// 取消正在进行的自动下载（弹窗上的「取消」）。只置一个开关，下载循环自己收尾。
#[tauri::command]
fn cancel_ffmpeg_download() {
    ffmpeg_download::cancel_download();
}

#[tauri::command]
async fn check_ffmpeg_filter(ffmpeg: Option<String>) -> bool {
    // 设置页那格给的是 **ffmpeg 所在目录**，这里换算成可执行文件再验；
    // 传 exe 路径过来的老调用方也照旧能用。
    let ffmpeg = if let Some(ffmpeg) = ffmpeg {
        render::resolve_ffmpeg_exe(ffmpeg)
    } else if let Ok(Some(ffmpeg)) = render::find_ffmpeg() {
        render::resolve_ffmpeg_exe(ffmpeg)
    } else {
        return false;
    };

    info!("[编码器] ffmpeg 路径：{}", ffmpeg.display());

    if !render::test_ffmpeg(&ffmpeg) {
        return false;
    }

    let output = Command::new(&ffmpeg)
        .arg("-filters")
        .output()
        .await
        .expect("failed get output");

    let stderr = String::from_utf8(output.stderr).unwrap_or_default();
    if !stderr.contains("ffmpeg") {
        error!("[编码器] 找不到 ffmpeg：把 ffmpeg.exe 放到程序目录，或在设置里填它所在的目录");
        return false;
    }

    let stdout = String::from_utf8(output.stdout).unwrap_or_default();
    let filter_required = ["aresample", "alimiter", "acompressor", "volume"];
    let mut complete = true;
    for i in filter_required {
        if !stdout.contains(i) {
            error!("[编码器] 这份 ffmpeg 缺滤镜 {i}：换一份完整版（full build，带 libmp3lame / libx264 的那类）");
            complete = false;
        }
    }
    return complete;
}

#[tauri::command]
async fn get_encoder(hevc: bool) -> Result<Option<String>, InvokeError> {
    (|| {
        let Some(ffmpeg) = render::find_ffmpeg()? else {
            bail!("找不到 FFmpeg：把 ffmpeg.exe 放到程序目录下，或在设置里指定它所在的目录")
        };
        let config = RenderConfig {
            hevc,
            ..RenderConfig::default()
        };
        let encoder_list = if config.hevc {
            ENCODER_LIST_HEVC
        } else {
            ENCODER_LIST_AVC
        };
        Ok(render::get_encoder(&ffmpeg, &config, encoder_list, false))
    })()
    .map_err(InvokeError::from_anyhow)
}

#[tauri::command]
async fn test_encoder(encoder: &str) -> Result<bool, InvokeError> {
    (|| {
        let Some(ffmpeg) = render::find_ffmpeg()? else {
            bail!("找不到 FFmpeg：把 ffmpeg.exe 放到程序目录下，或在设置里指定它所在的目录")
        };
        Ok(render::test_encoder(&ffmpeg, encoder))
    })()
    .map_err(InvokeError::from_anyhow)
}

#[tauri::command]
async fn export_pez(
    chart_path: String,
    output_path: String,
    minify: bool,
) -> Result<(), InvokeError> {
    wrap_async(async move {
        info!("[谱面] 导出谱面包：{} -> {}", chart_path, output_path);
        let chart_path = PathBuf::from(chart_path);
        let output_path = PathBuf::from(output_path);

        if !chart_path.exists() || !chart_path.is_dir() {
            bail!("Not a directory");
        }

        let mut files = collect_chart_files(chart_path.clone(), chart_path.clone())?;
        if minify {
            let mut fs: Box<dyn FileSystem + Send + Sync + 'static> =
                fs::fs_from_file(&chart_path)
                    .with_context(|| mtl!("read-chart-failed", "path" => chart_path.display().to_string()))?;
            let info = fs::load_info(fs.deref_mut())
                .await
                .with_context(|| mtl!("load-info-failed", "path" => chart_path.display().to_string()))?;
            let chart_data = tokio::fs::read_to_string(chart_path.join(&info.chart)).await?;
            let chart_json: dps::parse::RPEChart = serde_json::from_str(&chart_data)?;
            let chart_minified = serde_json::to_string(&chart_json)?;
            let chart_minified_path = chart_path.join("tempfile_minified.json");
            tokio::fs::write(&chart_minified_path, chart_minified).await?;

            if let Err(_) = fs.load_file("info.yml").await {
                let info_path = chart_path.join("tempfile_info.yml");
                let info_yaml = serde_yaml::to_string(&info)?;
                tokio::fs::write(&info_path, info_yaml).await?;
                files.insert(String::from("info.yml"), info_path);
            };

            files.remove(&info.chart);
            files.insert(info.chart, chart_minified_path);
        }

        let res_path = chart_path.parent().unwrap().join("shaders").join("pr");

        let extra_file = chart_path.join("extra.json");
        if extra_file.exists() {
            let mut shaders = Vec::new();
            let extra: Extra = serde_json::from_str(&std::fs::read_to_string(extra_file)?)?;
            for effect in extra.effects {
                if let Some(shader) = effect.shader.strip_prefix("/") {
                    let shader = shader.to_string();
                    if shader.ends_with("_pr.glsl") && !shaders.contains(&shader) {
                        shaders.push(shader);
                    }
                }
            }
            for shader in shaders {
                let shader_path = chart_path.join(&shader);
                let rpe_shader_path = res_path.join(&shader);
                if !shader_path.exists() && rpe_shader_path.exists() {
                    files.insert(shader, rpe_shader_path);
                }
            }
        }
        info!("[输出] 文件：{files:?}");

        create_zip(output_path, files).await?;
        Ok(())
    })
    .await
}

#[tauri::command]
async fn delete_path(path: String) -> Result<(), InvokeError> {
    wrap_async(async move {
        let path = PathBuf::from(path);
        if path.exists() && path.is_dir() {
            info!("[输出] 删除 {}", path.display());
            tokio::fs::remove_dir_all(path).await?;
        } else {
            bail!("Not a directory");
        }
        Ok(())
    })
    .await
}

/// 删掉一个文件（目录不归它管，见 `delete_path`）。
/// 文件本来就不在也算成功 —— 「取消渲染并移除文件」可能遇到还没落盘的输出。
/// 文件被占着（刚取消渲染时 ffmpeg 还没退出、或者播放器开着这个视频）时
/// 会退避重试几秒再报错，见 `common::remove_file_retry`。
#[tauri::command]
async fn delete_file(path: String) -> Result<(), InvokeError> {
    wrap_async(async move {
        let path = PathBuf::from(path);
        if path.is_file() {
            info!("[输出] 删除 {}", path.display());
            remove_file_retry(&path).await?;
        }
        Ok(())
    })
    .await
}

#[tauri::command]
async fn delete_autosave(path: String) -> Result<(), InvokeError> {
    wrap_async(async move {
        let path = PathBuf::from(path);
        if path.exists() && path.is_dir() {
            let mut entries = tokio::fs::read_dir(&path).await?;

            while let Some(entry) = entries.next_entry().await? {
                let file_path = entry.path();
                if let Some(file_name) = file_path.file_name().and_then(|s| s.to_str()) {
                    if file_name.starts_with("AutoSave_") {
                        info!("[输出] 删除 {}", file_path.display());
                        tokio::fs::remove_file(file_path).await?;
                    }
                }
            }
        } else {
            bail!("Not a directory");
        }
        Ok(())
    })
    .await
}

#[tauri::command]
async fn save_info(path: String, info: ChartInfo) -> Result<(), InvokeError> {
    wrap_async(async move {
        let file = PathBuf::from(path);
        info!("[配置] 保存 {}", file.display());
        let string = serde_yaml::to_string(&info)?;
        std::fs::write(file, string)?;
        Ok(())
    })
    .await
}

#[tauri::command]
async fn read_info(path: String) -> Result<ChartInfo, InvokeError> {
    wrap_async(async move {
        let file = PathBuf::from(path);
        info!("[配置] 读取 {}", file.display());
        let info = serde_yaml::from_reader(BufReader::new(std::fs::File::open(file)?))?;
        Ok(info)
    })
    .await
}
