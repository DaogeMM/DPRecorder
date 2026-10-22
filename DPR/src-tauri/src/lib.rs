// Prevents additional console window on Windows in release, DO NOT REMOVE!!
// #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

dps::tl_file!("main" mtl);

mod common;
mod icon;
mod ipc;
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
use tauri::{ipc::InvokeError, Manager, State, WindowEvent};
use tauri_plugin_prevent_default::Flags;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    process::Command,
};
use tracing::{error, info, warn};

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
        error!("{e:?}");
        InvokeError::from_anyhow(e)
    })
}

fn run_wrapped(f: impl Future<Output = Result<()>> + 'static, headless: bool) {
    macroquad::Window::from_config(build_conf(headless), async {
        if let Err(err) = f.await {
            error!("{err:?}");
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
/// `Failed to setup app: 拒绝访问。 (os error 5)`。受限环境（沙箱、受控文件夹、只读配置）里
/// 写不进 `%LOCALAPPDATA%`，所以这里提前做两件事：
///   ① 尽力把标准目录先建出来（建成功了 Tauri 那步就是空操作）；
///   ② 标准目录不可用时，用 `WEBVIEW2_USER_DATA_FOLDER` 把 WebView2 引到 exe 同级的 `webview2/`
///      （便携：拷走整个文件夹连界面状态一起走，且一定写得进去）。
#[cfg(target_os = "windows")]
fn ensure_webview_data_dir(exe_dir: &Path) {
    // 这个函数整个都在日志器注册**之前**跑（tauri-plugin-log 在 Builder::build() 里才装上），
    // 所以诊断信息一律直接走 stderr，跟 resolve_user_dir 一个道理。
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        let standard = local.join("com.daogemm.dpr");
        if !standard.exists() {
            match std::fs::create_dir_all(&standard) {
                Ok(()) => eprintln!("已预建 WebView2 标准数据目录 {}", standard.display()),
                Err(e) => eprintln!(
                    "警告: WebView2 标准数据目录 {} 建不出来（{e}），改用 exe 同级目录",
                    standard.display()
                ),
            }
        }
    }
    if std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_some() {
        return;
    }
    let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else {
        return;
    };
    let standard = local.join("com.daogemm.dpr");
    if common::is_dir_writable(&standard) {
        return;
    }
    let fallback = exe_dir.join("webview2");
    if common::is_dir_writable(&fallback) {
        let _ = std::fs::create_dir_all(&fallback);
        eprintln!(
            "警告: WebView2 默认数据目录不可写，改用 {}",
            fallback.display()
        );
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &fallback);
    } else {
        eprintln!(
            "警告: WebView2 默认数据目录与 {} 都不可写，界面可能无法显示",
            fallback.display()
        );
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
        eprintln!(
            "警告: {} 不可写，改用 {}",
            preferred.display(),
            fallback.display()
        );
        return fallback;
    }
    eprintln!(
        "警告: {} 与 {} 都不可写，设置与输出可能失败",
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
    // 必须在 tauri::Builder::build() 之前设置
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            ensure_webview_data_dir(dir);
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
                get_app_dir,
                get_respack_dir_path,
                get_appendant_dir_path,
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
                get_encoder,
                test_encoder,
                export_pez,
                delete_path,
                delete_file,
                delete_autosave,
                save_info,
                read_info,
            ])
            .on_window_event(|_, event| match event {
                //WindowEvent::CloseRequested { api, .. } => {
                WindowEvent::CloseRequested { .. } => {
                    /*event
                    .window()
                    .app_handle()
                    .tray_handle()
                    .get_item("toggle")
                    .set_title(mtl!("tray-show"))
                    .unwrap();*/
                    exit_program(0);
                    //event.window().hide().unwrap();
                    //api.prevent_close();
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

    // tempfile（视频落盘、音频混流、封面）以及 ffmpeg 等子进程全都走 std::env::temp_dir()。
    // 便携起见**优先用 `<exe 目录>/tmp`**（和 config/respack/output 一样挨着 exe，用户看得见、
    // 好清理，也不会把几 GB 视频临时文件塞进系统盘），只有那里写不进去时才保留系统临时目录；
    // 两者都不可写才警告（一失败就是满屏 `拒绝访问。 (os error 5) at path "...\Temp\.tmpXXXXXX"`，
    // 视频加载和渲染会直接挂掉）。
    {
        let dir = exe_dir.join("tmp");
        let system = std::env::temp_dir();
        if std::fs::create_dir_all(&dir).is_ok() && common::is_dir_writable(&dir) {
            if system != dir {
                std::env::set_var("TMP", &dir);
                std::env::set_var("TEMP", &dir);
                info!("临时目录改用 {}", dir.display());
            }
        } else if !common::is_dir_writable(&system) {
            eprintln!(
                "警告: 临时目录 {} 与 {} 都不可写，后续临时文件写入会失败",
                dir.display(),
                system.display()
            );
        }
    }

    let config = common::read_config()?;
    if config.show_detailed_log {
        log::register_with_colorize(true);
    }

    if std::env::args().len() > 1 {
        match std::env::args().nth(1).as_deref().unwrap_or_default() {
            "help" | "--help" | "-help" | "/help" | "-h" | "?" | "--?" | "-?" | "/?" => {
                println!("Usage: dpr --render --input <input file> [options]");
                println!("Usage: dpr --preview --input <input file> [options]");
                println!("Options:");
                println!("  --input   -i    <file/path>    Chart file");
                println!("  --output  -o    <file/path>    Output file");
                println!("  --config  -c    <file/json>    Config");
                println!("  --info    -ci   </json>        Chart Info");
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
            cmd => {
                info!("Command: {cmd:?}");
                let args = std::env::args().nth(1).unwrap_or_default();
                let path = Path::new(&args);
                if path.is_file() || path.is_dir() {
                    info!("Find a valid path, start preview");
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
        info!("Opening folder: {}", path.display());
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
        info!("Revealing in folder: {}", path.display());
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
        info!("Opening output folder: {}", path.display());
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
/// **坑**：从 `dpr.exe` 里调 `ShellExecuteExW` 会**返回 TRUE 却什么都
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
        info!("Opening external url: {}", url);
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
/// 从 `dpr.exe` 调可能返回 TRUE 却什么都不发生（原因未查明，可能是这个进程/线程没有 shell
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
        let mut fs: Box<dyn FileSystem + Send + Sync + 'static> =
            fs::fs_from_file(path).with_context(|| mtl!("read-chart-failed"))?;
        let info = fs::load_info(fs.deref_mut())
            .await
            .with_context(|| mtl!("load-info-failed"))?;
        //let info_display = format!("{}\n", serde_json::to_string(&info)?);
        //info_display!("{:?}", info);
        Ok(info)
    })
    .await
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
            fs::fs_from_file(Path::new(&path)).with_context(|| mtl!("read-chart-failed"))?;
        let bytes = fs.load_file(&name).await?;
        Ok(tauri::ipc::Response::new(bytes))
    })
    .await
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
        info!("preview: {}", info);
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
        info!("preview tweakoffset: {}", info);
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
            error!("Child process exited with {}", status);
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
        // 不看纯配置 / 定轨有没有开：开关打开就分屏，两个窗口显示不同内容由配置决定。
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
        plain.config.pure_config_chart = false;
        plain.config.fixed_lane_chart = false;
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
    info!("preview play: {}", info);
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

/// 程序认得的六个包：包名（`appendant/` 下的 zip 或同名目录）、界面显示的文件名、必需文件表。
///
/// `appendant/` 里的皮肤不进资源包下拉框 —— 那是一组程序自己按场合挑的包（official 当默认、
/// retri 给「报应SP」、spas 给「痉挛SP」、qzk 给「QZKAGO」），不是给用户选的。
const APPENDANT_PACKS: [(&str, &str, &[&str]); 7] = [
    (appendant::OFFICIAL, "appendant.official.zip", appendant::SKIN_FILES),
    (appendant::OLD, "appendant.old.zip", appendant::SKIN_FILES),
    (appendant::RETRI, "appendant.retri.zip", appendant::SKIN_FILES),
    (appendant::LAVA, "appendant.lava.zip", appendant::LAVA_FILES),
    (appendant::WATER, "appendant.water.zip", appendant::WATER_FILES),
    (appendant::SPAS, "appendant.spas.zip", appendant::SPAS_FILES),
    (appendant::QZK, "appendant.qzk.zip", appendant::QZK_FILES),
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
}

/// 资源包下拉框的候选：`respack/` 里的 zip 与**同名的皮肤目录**（用户自己做的皮肤包），再加上
/// 前端的「默认」项。目录要有 `info.yml` 才认，免得把随便一个子目录列成资源包。
#[tauri::command]
fn get_respacks() -> Result<Vec<RespackInfo>, InvokeError> {
    (|| {
        let dir = respack_dir()?;
        let mut list = Vec::new();
        let Ok(entries) = dir.read_dir() else {
            return Ok(list);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_zip = path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
            let is_pack_dir = path.is_dir() && path.join("info.yml").is_file();
            if !is_pack_dir && !(path.is_file() && is_zip) {
                continue;
            }
            let missing: Vec<String> = appendant::missing_files(&path, appendant::SKIN_FILES)
                .into_iter()
                .map(str::to_owned)
                .collect();
            list.push(RespackInfo {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: path.display().to_string(),
                present: missing.is_empty(),
                missing,
            });
        }
        list.sort_by(|x, y| x.name.cmp(&y.name));
        Ok(list)
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
        info!("Opening respack folder: {}", path.display());
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
        info!("Opening appendant folder: {}", path.display());
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
                    "Found chart folder: {}",
                    folder.file_name().unwrap_or_default().to_string_lossy()
                );
                if !folder.join("info.txt").exists() {
                    let folder_name = folder
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    warn!("Not found info.txt: {}", folder_name);
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
        info!("Opening current exe folder: {}", path.display());
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

    info!("ffmpeg: {}", ffmpeg.display());

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
        error!("Missing ffmpeg");
        return false;
    }

    let stdout = String::from_utf8(output.stdout).unwrap_or_default();
    let filter_required = ["aresample", "alimiter", "acompressor", "volume"];
    let mut complete = true;
    for i in filter_required {
        if !stdout.contains(i) {
            error!("Missing lib: {}, Please check FFmpeg availability", i);
            complete = false;
        }
    }
    return complete;
}

#[tauri::command]
async fn get_encoder(hevc: bool) -> Result<Option<String>, InvokeError> {
    (|| {
        let Some(ffmpeg) = render::find_ffmpeg()? else {
            bail!("FFmpeg not found")
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
            bail!("FFmpeg not found")
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
        info!("Exporting PEZ: {} -> {}", chart_path, output_path);
        let chart_path = PathBuf::from(chart_path);
        let output_path = PathBuf::from(output_path);

        if !chart_path.exists() || !chart_path.is_dir() {
            bail!("Not a directory");
        }

        let mut files = collect_chart_files(chart_path.clone(), chart_path.clone())?;
        if minify {
            let mut fs: Box<dyn FileSystem + Send + Sync + 'static> =
                fs::fs_from_file(&chart_path).with_context(|| mtl!("read-chart-failed"))?;
            let info = fs::load_info(fs.deref_mut())
                .await
                .with_context(|| mtl!("load-info-failed"))?;
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
        info!("files: {:?}", files);

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
            info!("delete: {}", path.display());
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
            info!("delete: {}", path.display());
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
                        info!("delete: {}", file_path.display());
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
        info!("save: {}", file.display());
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
        info!("read: {}", file.display());
        let info = serde_yaml::from_reader(BufReader::new(std::fs::File::open(file)?))?;
        Ok(info)
    })
    .await
}
