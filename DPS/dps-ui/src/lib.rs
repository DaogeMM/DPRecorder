dps::tl_file!("common" ttl crate::);

#[cfg(feature = "closed")]
mod inner;

mod charts_view;
mod client;
mod data;
mod icons;
mod images;
mod login;
mod mp;
mod page;
mod popup;
mod rate;
mod scene;
mod tags;
// UML 界面描述语言的解析与元素都还没有调用方（事件页接入时才用），先整体留用。
#[allow(dead_code)]
mod uml;

use anyhow::Result;
use data::Data;
use macroquad::prelude::*;
use dps::{
    build_conf,
    core::init_assets,
    l10n::set_prefered_locale,
    log,
    scene::{show_error, show_message},
    time::TimeManager,
    ui::{FontArc, TextPainter},
    Main,
};
use scene::MainScene;
use std::sync::{mpsc, Mutex};
// 帧率统计只在没有 `play` 特性的构建里画。
#[cfg(not(feature = "play"))]
use std::collections::VecDeque;
// 陀螺仪与安卓的 JNI 回调只在安卓上用。
#[cfg(target_os = "android")]
use {
    dps::gyro::{GYRO, GyroData},
    nalgebra::Vector3,
    std::time::Duration,
};
use tracing::{error, debug, info};

/// 联网功能总开关。
///
/// 关掉时所有联网入口都不画（登录、消息、活动、在线谱面、联机、评级与上传），
/// 启动与浏览也不再发请求；代码仍留在下面，改回 `true` 就能恢复。
pub const ONLINE: bool = false;

static ACTIVITY_LIFECYCLE: Mutex<Option<mpsc::Sender<bool>>> = Mutex::new(None);
static ACTIVITY_FOUCUS: Mutex<Option<mpsc::Sender<bool>>> = Mutex::new(None);
static ANTI_ADDICTION_CALLBACK: Mutex<Option<mpsc::Sender<i32>>> = Mutex::new(None);
static DATA_PATH: Mutex<Option<String>> = Mutex::new(None);
static CACHE_DIR: Mutex<Option<String>> = Mutex::new(None);
pub static mut DATA: Option<Data> = None;

#[cfg(feature = "closed")]
pub async fn load_res(name: &str) -> Vec<u8> {
    let bytes = load_file(name).await.unwrap();
    inner::resolve_data(bytes)
}

pub fn sync_data() {
    // 多语言暂时关掉：界面固定显示简体中文，设置里的语言项也已经拿掉。
    // 恢复时把这两行换回按 `get_data().language` 选语言。
    set_prefered_locale("zh-CN".parse().ok());
    get_data_mut().language = Some("zh-CN".to_owned());
    let _ = client::set_access_token_sync(get_data().tokens.as_ref().map(|it| &*it.0));
}

pub fn set_data(data: Data) {
    unsafe {
        DATA = Some(data);
    }
}

pub fn get_data() -> &'static Data {
    // 走 `addr_of!` 而不是直接取引用：直接借 `static mut` 会触发 `static_mut_refs`。
    unsafe { (*std::ptr::addr_of!(DATA)).as_ref().unwrap() }
}

pub fn get_data_mut() -> &'static mut Data {
    unsafe { (*std::ptr::addr_of_mut!(DATA)).as_mut().unwrap() }
}

pub fn save_data() -> Result<()> {
    std::fs::write(format!("{}/data.json", dir::root()?), serde_json::to_string(get_data())?)?;
    Ok(())
}

mod dir {
    use anyhow::Result;

    use crate::{CACHE_DIR, DATA_PATH};

    fn ensure(s: &str) -> Result<String> {
        let s = format!("{}/{}", DATA_PATH.lock().unwrap().as_ref().map(|it| it.as_str()).unwrap_or("."), s);
        let path = std::path::Path::new(&s);
        if !path.exists() {
            std::fs::create_dir_all(path)?;
        }
        Ok(s)
    }

    pub fn cache() -> Result<String> {
        if let Some(cache) = &*CACHE_DIR.lock().unwrap() {
            ensure(cache)
        } else {
            ensure("cache")
        }
    }

    pub fn cache_image_local() -> Result<String> {
        ensure(&format!("{}/image", cache()?))
    }

    pub fn root() -> Result<String> {
        ensure("data")
    }

    pub fn charts() -> Result<String> {
        ensure("data/charts")
    }

    pub fn custom_charts() -> Result<String> {
        ensure("data/charts/custom")
    }

    pub fn downloaded_charts() -> Result<String> {
        ensure("data/charts/download")
    }

    pub fn respacks() -> Result<String> {
        ensure("data/respack")
    }
}

/// 数据目录的绝对路径（`dir::root()` 给的是相对 exe 目录的写法，横幅与日志落点要绝对路径）。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn data_dir() -> Option<std::path::PathBuf> {
    let root = dir::root().ok()?;
    let root = std::path::Path::new(&root);
    if root.is_absolute() {
        Some(root.to_path_buf())
    } else {
        Some(std::env::current_dir().ok()?.join(root.to_string_lossy().trim_start_matches("./")))
    }
}

/// 本次运行的日志文件：先放 `<程序目录>/logs`，不可写就退回数据目录下的 `logs/`，
/// 返回给横幅显示的路径。两处都写不了就返回一句说明 —— 程序照常跑，日志只打终端。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn init_log_file() -> String {
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join("logs"))) {
        candidates.push(dir);
    }
    if let Some(data) = data_dir() {
        candidates.push(data.join("logs"));
    }
    for dir in candidates {
        if let Ok(path) = log::init_log_file(&dir, "dps") {
            return path.display().to_string();
        }
    }
    "不可写".to_owned()
}

async fn the_main() -> Result<()> {
    // 控制台先摆好：代码页切 UTF-8、开 ANSI 转义，再注册日志层 —— 第一条日志就该是彩色的。
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    log::init_console();

    log::register();

    init_assets();

    // 启动横幅直打 stderr，不受「日志输出到控制台」那个开关管：从 .bat 起的时候，
    // 至少能看出跑的是哪一份程序、数据和日志落在哪儿。
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let data = data_dir().map(|dir| dir.display().to_string()).unwrap_or_default();
        let logs = init_log_file();
        log::startup_banner("DPSimulator", &[("数据", &data), ("日志", &logs)]);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let _guard = rt.enter();

    #[cfg(target_os = "ios")]
    unsafe {
        use dps::objc::*;
        #[allow(improper_ctypes)]
        extern "C" {
            pub fn NSSearchPathForDirectoriesInDomains(
                directory: std::os::raw::c_ulong,
                domain_mask: std::os::raw::c_ulong,
                expand_tilde: bool,
            ) -> *mut NSArray<*mut NSString>;
        }
        let directories = NSSearchPathForDirectoriesInDomains(5, 1, true);
        let first: &mut NSString = msg_send![directories, firstObject];
        let path = first.as_str().to_owned();
        *DATA_PATH.lock().unwrap() = Some(path);
        *CACHE_DIR.lock().unwrap() = Some("Caches".to_owned());
    }

    let dir = dir::root()?;
    let mut data: Data = std::fs::read_to_string(format!("{dir}/data.json"))
        .map_err(anyhow::Error::new)
        .and_then(|s| Ok(serde_json::from_str(&s)?))
        .unwrap_or_default();
    data.init().await?;
    set_data(data);
    sync_data();

    let activity_lifecycle = {
        let (tx, rx) = mpsc::channel();
        *ACTIVITY_LIFECYCLE.lock().unwrap() = Some(tx);
        rx
    };

    let activity_foucus = {
        let (tx, rx) = mpsc::channel();
        *ACTIVITY_FOUCUS.lock().unwrap() = Some(tx);
        rx
    };

    let anti_addiction_callback = {
        let (tx, rx) = mpsc::channel();
        *ANTI_ADDICTION_CALLBACK.lock().unwrap() = Some(tx);
        rx
    };

    unsafe { get_internal_gl() }
        .quad_context
        .display_mut()
        .set_pause_resume_listener(on_pause_resume);

    if let Some(me) = &get_data().me {
        anti_addiction_action("startup", Some(format!("Phigros-{}", me.id)));
    }

    // 与 `SharedState` 的画笔共用 `font.ttf`：界面与游戏文字用同一份字体，省得再带一个字体文件。
    let font = FontArc::try_from_vec(load_file("font.ttf").await?)?;
    let mut painter = TextPainter::new(font);

    let mut main = Main::new(Box::new(MainScene::new().await?), TimeManager::default(), None).await?;

    let tm = TimeManager::default();

    #[cfg(not(feature = "play"))]
    let mut frame_times: VecDeque<(f64, u32)> = VecDeque::new();
    let mut fps_last_update_sec: u32 = 0;

    let mut exit_time = f64::INFINITY;

    'app: loop {
        let frame_start = tm.real_time();
        let res = || -> Result<()> {
            main.update()?;
            main.render(&mut painter)?;
            if let Ok(paused) = activity_lifecycle.try_recv() {
                if paused {
                    main.pause()?;
                } else {
                    main.resume()?;
                }
            } else if let Ok(paused) = activity_foucus.try_recv() {
                if paused {
                    main.foucus_pause()?;
                } else {
                    main.foucus_resume()?;
                }
            }
            Ok(())
        }();
        if let Err(err) = res {
            //error!("uncaught error: {err:?}");
            eprint!("uncaught error: {err:?}");
            show_error(err);
        }
        if main.should_exit() {
            break 'app;
        }

        if let Ok(code) = anti_addiction_callback.try_recv() {
            info!("anti addiction callback: {code}");
            match code {
                // login success
                500 => {
                    anti_addiction_action("enterGame", None);
                }
                // switch account
                1001 => {
                    anti_addiction_action("exit", None);
                    get_data_mut().me = None;
                    get_data_mut().tokens = None;
                    let _ = save_data();
                    sync_data();
                    use crate::login::L10N_LOCAL;
                    show_message(crate::login::tl!("logged-out")).ok();
                }
                // period restrict
                1030 => {
                    show_and_exit("你当前为未成年账号，已被纳入防沉迷系统。根据国家相关规定，周五、周六、周日及法定节假日 20 点 - 21 点之外为健康保护时段。当前时间段无法游玩，请合理安排时间。");
                    exit_time = frame_start;
                }
                // duration limit
                1050 => {
                    show_and_exit("你当前为未成年账号，已被纳入防沉迷系统。根据国家相关规定，周五、周六、周日及法定节假日 20 点 - 21 点之外为健康保护时段。你已达时间限制，无法继续游戏。");
                    exit_time = frame_start;
                }
                // stopped
                9002 => {
                    show_and_exit("必须实名认证方可进行游戏。");
                    exit_time = frame_start;
                }
                _ => {}
            }
        }

        let frame_end = tm.real_time();
        let now_fps = (1. / (frame_end - frame_start)) as u32;

        #[cfg(not(feature = "play"))]
        {
            frame_times.push_back((frame_end, now_fps));
            while frame_times.front().is_some_and(|it| frame_end - it.0 > 1.0) {
                frame_times.pop_front();
            }
        }

        if frame_end > exit_time + 5. {
            break;
        }

        next_frame().await;
        #[cfg(not(feature = "play"))]
        let flash_end = tm.real_time();

        let fps_now_sec = frame_end as u32;
        // FPS 是调试信息：每秒一条会把控制台刷满，降到 debug —— 默认不上屏，仍进日志文件，
        // 设了 `RUST_LOG` 才在终端里看得见。
        #[cfg(feature = "play")]
        if fps_now_sec != fps_last_update_sec {
            fps_last_update_sec = fps_now_sec;
            debug!("FPS {}", now_fps);
        }
        #[cfg(not(feature = "play"))]
        if fps_last_update_sec != fps_now_sec {
            fps_last_update_sec = fps_now_sec;
            let real_fps = frame_times.len() as u32;
            let real_now_fps = (1. / (flash_end - frame_start)) as u32;
            let avg_fps = frame_times.iter().map(|(_, fps)| fps).sum::<u32>() / real_fps;
            let min_fps = frame_times.iter().map(|(_, fps)| fps).min().unwrap_or(&0);
            debug!("| AVG: {}|{} NOW: {}|{}, MIN: {}", real_fps, avg_fps, real_now_fps, now_fps, min_fps);
        }
    }
    Ok(())
}

fn show_and_exit(msg: &str) {
    dps::ui::Dialog::simple(msg)
        .buttons(vec!["确定".to_owned()])
        .listener(|_| std::process::exit(0))
        .show();
}

fn build_global_window_conf() -> Conf {
    let mut conf = build_conf();

    #[cfg(target_os = "windows")]
    {
        conf.fullscreen = dir::root()
            .ok()
            .and_then(|r| std::fs::read_to_string(std::path::Path::new(&r).join("data.json")).ok())
            .and_then(|s| serde_json::from_str::<Data>(&s).ok())
            .is_some_and(|d| d.config.fullscreen_mode);
    }

    conf
}

#[no_mangle]
pub extern "C" fn quad_main() {
    macroquad::Window::from_config(build_global_window_conf(), async {
        if let Err(err) = the_main().await {
            error!("Error: {:?}", err);
        }
    });
}

fn on_pause_resume(pause: bool) {
    if let Some(tx) = ACTIVITY_LIFECYCLE.lock().unwrap().as_mut() {
        let _ = tx.send(pause);
    }
}

#[cfg(target_os = "android")]
unsafe fn string_from_java(env: *mut ndk_sys::JNIEnv, s: ndk_sys::jstring) -> String {
    let get_string_utf_chars = (**env).GetStringUTFChars.unwrap();
    let release_string_utf_chars = (**env).ReleaseStringUTFChars.unwrap();

    let ptr = (get_string_utf_chars)(env, s, ::std::ptr::null::<ndk_sys::jboolean>() as _);
    let res = std::ffi::CStr::from_ptr(ptr).to_str().unwrap().to_owned();
    (release_string_utf_chars)(env, s, ptr);

    res
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_quad_1native_QuadNative_libActivityOnPause(_: *mut std::ffi::c_void, _: *const std::ffi::c_void) {
    anti_addiction_action("leaveGame", None);
    if let Some(tx) = ACTIVITY_LIFECYCLE.lock().unwrap().as_mut() {
        let _ = tx.send(true);
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_quad_1native_QuadNative_libActivityOnResume(_: *mut std::ffi::c_void, _: *const std::ffi::c_void) {
    anti_addiction_action("enterGame", None);
    if let Some(tx) = ACTIVITY_LIFECYCLE.lock().unwrap().as_mut() {
        let _ = tx.send(false);
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_quad_1native_QuadNative_libActivityOnWindowFocusChanged(_: *mut std::ffi::c_void, _: *const std::ffi::c_void, has_focus: ndk_sys::jboolean) {
    if let Some(tx) = ACTIVITY_FOUCUS.lock().unwrap().as_mut() {
        let _ = tx.send(has_focus == 0);
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_quad_1native_QuadNative_libActivityOnDestroy(_: *mut std::ffi::c_void, _: *const std::ffi::c_void) {
    // std::process::exit(0);
}

#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn Java_quad_1native_QuadNative_setDataPath(_: *mut std::ffi::c_void, _: *const std::ffi::c_void, path: ndk_sys::jstring) {
    let env = crate::miniquad::native::attach_jni_env();
    *DATA_PATH.lock().unwrap() = Some(string_from_java(env, path));
}

#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn Java_quad_1native_QuadNative_setTempDir(_: *mut std::ffi::c_void, _: *const std::ffi::c_void, path: ndk_sys::jstring) {
    let env = crate::miniquad::native::attach_jni_env();
    let path = string_from_java(env, path);
    std::env::set_var("TMPDIR", path.clone());
    *CACHE_DIR.lock().unwrap() = Some(path);
}

#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn Java_quad_1native_QuadNative_setDpi(_: *mut std::ffi::c_void, _: *const std::ffi::c_void, dpi: ndk_sys::jint) {
    dps::core::DPI_VALUE.store(dpi as _, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn Java_quad_1native_QuadNative_setChosenFile(_: *mut std::ffi::c_void, _: *const std::ffi::c_void, file: ndk_sys::jstring) {
    use dps::scene::CHOSEN_FILE;

    let env = crate::miniquad::native::attach_jni_env();
    CHOSEN_FILE.lock().unwrap().1 = Some(string_from_java(env, file));
}

#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn Java_quad_1native_QuadNative_markImport(_: *mut std::ffi::c_void, _: *const std::ffi::c_void) {
    use dps::scene::CHOSEN_FILE;

    CHOSEN_FILE.lock().unwrap().0 = Some("_import".to_owned());
}

#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn Java_quad_1native_QuadNative_markImportRespack(_: *mut std::ffi::c_void, _: *const std::ffi::c_void) {
    use dps::scene::CHOSEN_FILE;

    CHOSEN_FILE.lock().unwrap().0 = Some("_import_respack".to_owned());
}

#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn Java_quad_1native_QuadNative_setInputText(_: *mut std::ffi::c_void, _: *const std::ffi::c_void, text: ndk_sys::jstring) {
    use dps::scene::INPUT_TEXT;

    let env = crate::miniquad::native::attach_jni_env();
    INPUT_TEXT.lock().unwrap().1 = Some(string_from_java(env, text));
}

#[cfg(not(all(target_os = "android", feature = "aa")))]
pub fn anti_addiction_action(_action: &str, _arg: Option<String>) {}

#[cfg(all(target_os = "android", feature = "aa"))]
pub fn anti_addiction_action(action: &str, arg: Option<String>) {
    unsafe {
        let env = miniquad::native::attach_jni_env();
        let ctx = ndk_context::android_context().context();
        let class = (**env).GetObjectClass.unwrap()(env, ctx);
        let method =
            (**env).GetMethodID.unwrap()(env, class, b"antiAddiction\0".as_ptr() as _, b"(Ljava/lang/String;Ljava/lang/String;)V\0".as_ptr() as _);
        let action = std::ffi::CString::new(action.to_owned()).unwrap();
        let arg = arg.map(|it| std::ffi::CString::new(it).unwrap());
        (**env).CallVoidMethod.unwrap()(
            env,
            ctx,
            method,
            (**env).NewStringUTF.unwrap()(env, action.as_ptr()),
            arg.map(|it| (**env).NewStringUTF.unwrap()(env, it.as_ptr()))
                .unwrap_or_else(|| std::ptr::null_mut()),
        );
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn Java_quad_1native_QuadNative_antiAddictionCallback(
    _: *mut std::ffi::c_void,
    _: *const std::ffi::c_void,
    #[allow(dead_code)] code: ndk_sys::jint,
) {
    if cfg!(feature = "aa") {
        if let Some(tx) = ANTI_ADDICTION_CALLBACK.lock().unwrap().as_mut() {
            let _ = tx.send(code);
        }
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn Java_quad_1native_QuadNative_updateGyroScope(
    env: ndk_sys::JNIEnv,
    _class: ndk_sys::jclass,
    x: ndk_sys::jfloat,
    y: ndk_sys::jfloat,
    z: ndk_sys::jfloat,
    timestamp: ndk_sys::jlong,
) {
    let set_gyro_data = GyroData {
        angular_velocity: Vector3::new(x, y, z),
        timestamp: Duration::from_nanos(timestamp as u64),
    };
    GYRO.lock().unwrap().update_gyroscope(set_gyro_data);
}

#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn Java_quad_1native_QuadNative_updateGravity(
    env: ndk_sys::JNIEnv,
    _class: ndk_sys::jclass,
    roll: ndk_sys::jfloat,
    pitch: ndk_sys::jfloat,
    yaw: ndk_sys::jfloat,
) {
    if let mut gyro_data = GYRO.lock().unwrap() {
        gyro_data.update_gravity(Vector3::new(roll, pitch, yaw));
    }
}
