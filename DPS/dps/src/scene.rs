crate::tl_file!("scene" ttl);

mod ending;
pub use ending::{EndingScene, RecordUpdateState};

pub mod game;
pub use game::{GameMode, GameScene, SimpleRecord};

mod loading;
pub use loading::{BasicPlayer, LoadingScene, UpdateFn, UploadFn};

use crate::{
    ext::{draw_image, screen_aspect, LocalTask, RectExt, SafeTexture, ScaleType},
    judge::Judge,
    time::TimeManager,
    ui::{BillBoard, Dialog, Message, MessageHandle, MessageKind, TextPainter, Ui},
};
use anyhow::{Error, Result};
use cfg_if::cfg_if;
use macroquad::prelude::*;
use std::{any::Any, cell::RefCell, sync::Mutex};
use tracing::warn;

#[derive(Default)]
pub enum NextScene {
    #[default]
    None,
    Pop,
    PopN(usize),
    PopWithResult(Box<dyn Any>),
    PopNWithResult(usize, Box<dyn Any>),
    Exit,
    Overlay(Box<dyn Scene>),
    Replace(Box<dyn Scene>),
}

thread_local! {
    pub static BILLBOARD: RefCell<(BillBoard, TimeManager)> = RefCell::new((BillBoard::new(), TimeManager::default()));
    pub static DIALOG: RefCell<Option<Dialog>> = RefCell::new(None);
    #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
    static TEXT_INPUT: RefCell<Option<TextInput>> = const { RefCell::new(None) };
}

#[inline]
pub fn show_error(error: Error) {
    warn!("show error: {error:?}");
    Dialog::error(error).show();
}

pub struct MessageBuilder {
    content: String,
    kind: MessageKind,
    duration: f32,
}

impl MessageBuilder {
    pub fn new(content: String) -> Self {
        Self {
            content,
            kind: MessageKind::Info,
            duration: 2.,
        }
    }

    #[inline]
    pub fn kind(mut self, kind: MessageKind) -> Self {
        self.kind = kind;
        self
    }

    #[inline]
    pub fn duration(mut self, t: f32) -> Self {
        self.duration = t;
        self
    }

    #[inline]
    pub fn ok(self) -> Self {
        self.kind(MessageKind::Ok)
    }

    #[inline]
    pub fn warn(self) -> Self {
        self.kind(MessageKind::Warn)
    }

    #[inline]
    pub fn error(self) -> Self {
        self.kind(MessageKind::Error)
    }

    fn show(&mut self) -> MessageHandle {
        BILLBOARD.with(|it| {
            let mut guard = it.borrow_mut();
            let (msg, handle) = Message::new(std::mem::take(&mut self.content), guard.1.now() as _, self.duration, self.kind.clone());
            guard.0.add(msg);
            handle
        })
    }

    #[inline]
    pub fn handle(mut self) -> MessageHandle {
        let handle = self.show();
        std::mem::forget(self);
        handle
    }
}

impl Drop for MessageBuilder {
    fn drop(&mut self) {
        self.show();
    }
}

#[inline]
pub fn show_message(msg: impl Into<String>) -> MessageBuilder {
    MessageBuilder::new(msg.into())
}

pub static INPUT_TEXT: Mutex<(Option<String>, Option<String>)> = Mutex::new((None, None));
#[cfg(not(target_arch = "wasm32"))]
pub static CHOSEN_FILE: Mutex<(Option<String>, Option<String>)> = Mutex::new((None, None));

#[inline]
pub fn request_input(id: impl Into<String>, text: &str, title: impl Into<String>) {
    let title = title.into();
    request_input_full(id, text, false, title.as_str(), "");
}
#[inline]
pub fn request_password(id: impl Into<String>, text: &str, title: impl Into<String>) {
    let title = title.into();
    request_input_full(id, text, true, title.as_str(), "");
}
pub fn request_input_full(id: impl Into<String>, #[allow(unused_variables)] text: &str, #[allow(unused_variables)] is_password: bool, #[allow(unused_variables)] title: &str, #[allow(unused_variables)] hint: &str) {
    *INPUT_TEXT.lock().unwrap() = (Some(id.into()), None);
    cfg_if! {
        if #[cfg(target_os = "android")] {
            unsafe {
                let env = miniquad::native::attach_jni_env();
                let ctx = ndk_context::android_context().context();
                let class = (**env).GetObjectClass.unwrap()(env, ctx);
                let method = (**env).GetMethodID.unwrap()(env, class, b"inputText\0".as_ptr() as _, b"(Ljava/lang/String;ZLjava/lang/String;Ljava/lang/String;)V\0".as_ptr() as _);

                let jtext = (**env).NewStringUTF.unwrap()(env, std::ffi::CString::new(text.to_owned()).unwrap().as_ptr());
                let jtitle = (**env).NewStringUTF.unwrap()(env, std::ffi::CString::new(title.to_owned()).unwrap().as_ptr());
                let jhint = (**env).NewStringUTF.unwrap()(env, std::ffi::CString::new(hint.to_owned()).unwrap().as_ptr());
                (**env).CallVoidMethod.unwrap()(env, ctx, method, jtext, is_password as std::ffi::c_uint, jtitle, jhint);
            }
        } else if #[cfg(target_os = "ios")] {
            unsafe {
                use crate::objc::*;
                let view_ctrl = *miniquad::native::ios::VIEW_CTRL_OBJ.lock().unwrap();

                let alert: ObjcId = msg_send![
                    class!(UIAlertController),
                    alertControllerWithTitle: str_to_ns(ttl!("input"))
                    message: str_to_ns(ttl!("input-msg"))
                    preferredStyle: 1
                ];

                let action: ObjcId = msg_send![
                    class!(UIAlertAction),
                    actionWithTitle: str_to_ns("Cancel")
                    style: 1
                    handler: 0
                ];
                let _: () = msg_send![alert, addAction: action];
                let action: ObjcId = msg_send![
                    class!(UIAlertAction),
                    actionWithTitle: str_to_ns("OK")
                    style: 0
                    handler: ConcreteBlock::new({
                        let alert = alert; // TODO strong ptr?
                        move |_: ObjcId| {
                            let fields: ObjcId = msg_send![alert, textFields];
                            let field: ObjcId = msg_send![fields, firstObject];
                            let text: *const NSString = msg_send![field, text];
                            INPUT_TEXT.lock().unwrap().1 = Some((*text).as_str().to_owned());
                        }
                    }).copy()
                ];
                let _: () = msg_send![alert, addAction: action];

                let text = text.to_owned();
                let _: () = msg_send![alert, addTextFieldWithConfigurationHandler: ConcreteBlock::new(move |field: ObjcId| {
                    let _: () = msg_send![field, setPlaceholder: str_to_ns(ttl!("input-hint"))];
                    let _: () = msg_send![field, setText: str_to_ns(&text)];
                    if is_password {
                        let _: () = msg_send![field, setSecureTextEntry: runtime::YES];
                    }
                }).copy()];

                let _: () = msg_send![
                    view_ctrl as ObjcId,
                    presentViewController: alert
                    animated: runtime::YES
                    completion: 0 as ObjcId
                ];
            }
        } else {
            // 桌面端：弹一个键盘输入框，而不是把剪贴板当结果直接返回。
            // 用 request id 去重：`Ui::input` 每帧都会调 `request_input`，重复入队会让框里的内容被不断重置。
            TEXT_INPUT.with(|it| {
                let mut guard = it.borrow_mut();
                if let Some(input) = guard.as_mut() {
                    if input.label == title {
                        return;
                    }
                }
                *guard = Some(TextInput::new(title.to_owned(), text.to_owned(), is_password, hint.to_owned()));
            });
        }
    }
}

pub fn take_input() -> Option<(String, String)> {
    let mut w = INPUT_TEXT.lock().unwrap();
    w.0.clone().zip(std::mem::take(&mut w.1))
}

/// 当前还没被消费的输入请求 id（`request_input` 发起、`take_input` 消费）。
/// 桌面端输入框确认后，场景靠它判断这次输入是不是自己的。
pub fn pending_input_id() -> Option<String> {
    INPUT_TEXT.lock().unwrap().0.clone()
}

/// 桌面端的文本输入框。
///
/// Android / iOS 走系统的原生输入框（见 `request_input_full`），收完结果塞进 `INPUT_TEXT`。
/// 桌面端过去没有这个通路：`request_input_full` 直接把**剪贴板**当结果返回，等于强制复制粘贴。
/// 这里补一个键盘输入框，行为对齐原生输入框：Enter 确认 / Esc 取消 / Backspace 删字符，
/// 空输入按取消处理（这样就不会再把旧值当成「用户输入的内容」覆盖回去）。
#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
struct TextInput {
    label: String,
    text: String,
    password: bool,
    hint: String,
    pending: Option<Option<String>>,
    /// 刚弹框、还没吃到回车的前几帧不吃键盘 —— 以免弹出前那一下点击/按键顺带把框关掉。
    age: u32,
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
impl TextInput {
    fn new(label: String, text: String, password: bool, hint: String) -> Self {
        Self {
            label,
            text,
            password,
            hint,
            pending: None,
            age: 0,
        }
    }

    fn shown(&self) -> String {
        if self.password {
            "*".repeat(self.text.chars().count())
        } else {
            self.text.clone()
        }
    }

    fn step(&mut self) {
        self.age = self.age.saturating_add(1);
        if self.pending.is_some() || self.age <= 2 {
            return;
        }
        while let Some(ch) = get_char_pressed() {
            if !ch.is_control() {
                self.text.push(ch);
            }
        }
        if is_key_pressed(KeyCode::Backspace) {
            self.text.pop();
        }
        if is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::KpEnter) {
            self.pending = Some(if self.text.is_empty() {
                None
            } else {
                Some(self.text.clone())
            });
        } else if is_key_pressed(KeyCode::Escape) {
            self.pending = Some(None);
        }
    }

    /// 取走这一帧的用户动作：`Some(None)` = 取消，`Some(Some(text))` = 确认。
    fn take(&mut self) -> Option<Option<String>> {
        self.pending.take()
    }

    fn render(&self, ui: &mut Ui) {
        // 和 `Dialog::render` 同一套坐标：`Ui::camera()` 的 zoom 是 `(1., -w/h)`，
        // 所以 x ∈ [-1, 1]、y ∈ [-ui.top, ui.top]（16:9 时 top = 0.5625），**不是像素**。
        let w = 0.9;
        let h = 0.34;
        ui.fill_rect(ui.screen_rect(), Color::new(0., 0., 0., 0.35));
        let panel = Rect::new(-w / 2., -h / 2., w, h);
        ui.fill_path(&panel.rounded(0.02), Color::new(0.13, 0.13, 0.13, 0.95));
        ui.text(self.label.clone())
            .pos(0., panel.y + 0.045)
            .anchor(0.5, 0.)
            .size(0.55)
            .max_width(w - 0.06)
            .color(WHITE)
            .draw();
        let bx = Rect::new(panel.x + 0.04, -0.045, w - 0.08, 0.09);
        ui.fill_path(&bx.rounded(0.02), Color::new(0.05, 0.05, 0.05, 1.));
        let caret = |x: f32| Rect::new(x, bx.center().y - 0.028, 0.005, 0.056);
        let shown = self.shown();
        if shown.is_empty() {
            let mut x = bx.x + 0.02;
            if !self.hint.is_empty() {
                let r = ui
                    .text(self.hint.clone())
                    .pos(x, bx.center().y)
                    .anchor(0., 0.5)
                    .size(0.44)
                    .max_width(bx.w - 0.04)
                    .color(Color::new(0.45, 0.45, 0.45, 1.))
                    .draw();
                x = r.right() + 0.008;
            }
            ui.fill_rect(caret(x), WHITE);
        } else {
            let r = ui
                .text(shown)
                .pos(bx.x + 0.02, bx.center().y)
                .anchor(0., 0.5)
                .size(0.44)
                .max_width(bx.w - 0.04)
                .color(WHITE)
                .draw();
            ui.fill_rect(caret(r.right() + 0.008), WHITE);
        }
        ui.text(format!("{}  ·  Esc {}", ttl!("ok"), ttl!("cancel")))
            .pos(0., panel.bottom() - 0.025)
            .anchor(0.5, 1.)
            .size(0.36)
            .color(Color::new(0.6, 0.6, 0.6, 1.))
            .draw();
    }
}

pub fn return_input(id: String, text: String) {
    *INPUT_TEXT.lock().unwrap() = (Some(id), Some(text));
}

#[cfg(not(target_arch = "wasm32"))]
pub fn request_file(id: impl Into<String>) {
    *CHOSEN_FILE.lock().unwrap() = (Some(id.into()), None);
    cfg_if! {
        if #[cfg(target_os = "android")] {
            unsafe {
                let env = miniquad::native::attach_jni_env();
                let ctx = ndk_context::android_context().context();
                let class = (**env).GetObjectClass.unwrap()(env, ctx);
                let method = (**env).GetMethodID.unwrap()(env, class, b"chooseFile\0".as_ptr() as _, b"()V\0".as_ptr() as _);
                (**env).CallVoidMethod.unwrap()(env, ctx, method);
            }
        } else if #[cfg(target_os = "ios")] {
            use once_cell::sync::Lazy;
            unsafe {
                use crate::objc::*;
                static PICKER_DELEGATE: Lazy<u64> = Lazy::new(|| unsafe {
                    let mut decl = ClassDecl::new("PickerDelegate", class!(NSObject)).unwrap();
                    extern "C" fn document_picker(_: &Object, _: Sel, _: ObjcId, documents: ObjcId) {
                        unsafe {
                            let url: ObjcId = msg_send![documents, firstObject];
                            let need_close: bool = msg_send![url, startAccessingSecurityScopedResource];
                            let mut error: ObjcId = std::ptr::null_mut();
                            let data: ObjcId = msg_send![class!(NSData), dataWithContentsOfURL: url options: 2 error: &mut error as *mut ObjcId];
                            if need_close {
                                let _: () = msg_send![url, stopAccessingSecurityScopedResource];
                            }
                            if data.is_null() {
                                show_message(ttl!("read-file-failed")).error();
                                if !error.is_null() {
                                    let msg: *const NSString = msg_send![error, localizedDescription];
                                    show_error(Error::msg((*msg).as_str()).context(ttl!("read-file-failed")));
                                }
                            } else {
                                extern "C" {
                                    #[allow(improper_ctypes)]
                                    pub fn NSTemporaryDirectory() -> *mut NSString;
                                }
                                let dir = NSTemporaryDirectory();
                                let uuid: ObjcId = msg_send![class!(NSUUID), UUID];
                                let uuid: *mut NSString = msg_send![uuid, UUIDString];
                                let path = format!("{}{}", (*dir).as_str(), (*uuid).as_str());
                                let _: () = msg_send![data, writeToFile: str_to_ns(&path) atomically: YES];
                                CHOSEN_FILE.lock().unwrap().1 = Some(path);
                            }
                        }
                    }
                    decl.add_method(sel!(documentPicker: didPickDocumentsAtURLs:), document_picker as extern "C" fn(&Object, Sel, ObjcId, ObjcId));
                    decl.register() as *const _ as _
                });

                let picker: ObjcId = msg_send![class!(UIDocumentPickerViewController), alloc];
                let picker: ObjcId = if available("14.0.0") {
                    let tp_cls = class!(UTType);
                    let ext = |e: &str| {
                        let tp: ObjcId = msg_send![tp_cls, typeWithFilenameExtension: str_to_ns(e)];
                        std::mem::transmute::<_, ShareId<NSObject>>(ShareId::from_ptr(tp))
                    };
                    let types = NSArray::from_slice(&[ext("zip"), ext("pez"), ext("jpg"), ext("png"), ext("jpeg"), ext("json"), ext("mp3"), ext("ogg")]);
                    let types: ObjcId = std::mem::transmute(types);
                    msg_send![picker, initForOpeningContentTypes: types]
                } else {
                    let ext = |e: &str| str_to_ns(e);
                    let types = NSArray::from_vec(vec![ext("public.image"), ext("public.archive")]);
                    let types: ObjcId = std::mem::transmute(types);
                    msg_send![picker, initWithDocumentTypes: types inMode: 0]
                };
                let dlg_obj: ObjcId = msg_send![*PICKER_DELEGATE as ObjcId, alloc];
                let dlg_obj: ObjcId = msg_send![dlg_obj, init];
                let _: () = msg_send![picker, setDelegate: dlg_obj];

                let view_ctrl = *miniquad::native::ios::VIEW_CTRL_OBJ.lock().unwrap();
                let _: () = msg_send![
                    view_ctrl as ObjcId,
                    presentViewController: picker
                    animated: runtime::YES
                    completion: 0 as ObjcId
                ];
            }
        } else { // desktop
            CHOSEN_FILE.lock().unwrap().1 = rfd::FileDialog::new().pick_file().map(|it| it.display().to_string());
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn take_file() -> Option<(String, String)> {
    let mut w = CHOSEN_FILE.lock().unwrap();
    w.0.clone().zip(std::mem::take(&mut w.1))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn return_file(id: String, file: String) {
    *CHOSEN_FILE.lock().unwrap() = (Some(id), Some(file));
}

pub trait Scene {
    fn enter(&mut self, _tm: &mut TimeManager, _target: Option<RenderTarget>) -> Result<()> {
        Ok(())
    }
    fn pause(&mut self, _tm: &mut TimeManager) -> Result<()> {
        Ok(())
    }
    fn resume(&mut self, _tm: &mut TimeManager) -> Result<()> {
        Ok(())
    }
    fn foucus_pause(&mut self, _tm: &mut TimeManager) -> Result<()> {
        Ok(())
    }
    fn foucus_resume(&mut self, _tm: &mut TimeManager) -> Result<()> {
        Ok(())
    }
    fn on_result(&mut self, _tm: &mut TimeManager, _result: Box<dyn Any>) -> Result<()> {
        Ok(())
    }
    /// 桌面端输入框把结果送回来。返回 `true` 表示这个场景认领了这次输入。
    /// 原生平台（android/ios）不经过这条路，保持默认实现即可。
    fn take_input(&mut self, _tm: &mut TimeManager, _text: Option<String>) -> Result<bool> {
        Ok(false)
    }
    fn touch(&mut self, _tm: &mut TimeManager, _touch: &Touch) -> Result<bool> {
        Ok(false)
    }
    fn update(&mut self, tm: &mut TimeManager) -> Result<()>;
    fn render(&mut self, tm: &mut TimeManager, ui: &mut Ui) -> Result<()>;
    fn next_scene(&mut self, _tm: &mut TimeManager) -> NextScene {
        NextScene::None
    }
}

pub trait RenderTargetChooser {
    fn choose(&mut self) -> Option<RenderTarget>;
}
impl RenderTargetChooser for Option<RenderTarget> {
    fn choose(&mut self) -> Option<RenderTarget> {
        *self
    }
}
impl<F: FnMut() -> Option<RenderTarget>> RenderTargetChooser for F {
    fn choose(&mut self) -> Option<RenderTarget> {
        self()
    }
}

pub struct Main {
    pub scenes: Vec<Box<dyn Scene>>,
    times: Vec<f64>,
    target_chooser: Box<dyn RenderTargetChooser>,
    tm: TimeManager,
    paused: bool,
    last_update_time: f64,
    should_exit: bool,
    pub top_level: bool,
    touches: Option<Vec<Touch>>,
    pub viewport: Option<(i32, i32, i32, i32)>,
}

impl Main {
    pub async fn new(mut scene: Box<dyn Scene>, mut tm: TimeManager, mut target_chooser: impl RenderTargetChooser + 'static) -> Result<Self> {
        simulate_mouse_with_touch(false);
        scene.enter(&mut tm, target_chooser.choose())?;
        let last_update_time = tm.now();
        macro_rules! load_tex {
            ($path:literal) => {
                SafeTexture::from(Texture2D::from_image(&load_image($path).await?))
            };
        }
        let icons = [load_tex!("info.png"), load_tex!("warn.png"), load_tex!("ok.png"), load_tex!("error.png")];
        BILLBOARD.with(|it| it.borrow_mut().0.set_icons(icons));
        Ok(Self {
            scenes: vec![scene],
            times: Vec::new(),
            target_chooser: Box::new(target_chooser),
            tm,
            paused: false,
            last_update_time,
            should_exit: false,
            top_level: true,
            touches: None,
            viewport: None,
        })
    }

    pub fn update(&mut self) -> Result<()> {
        self.update_with_mutate(|_| {})
    }

    pub fn update_with_mutate(&mut self, f: impl Fn(&mut Touch)) -> Result<()> {
        if self.paused {
            return Ok(());
        }
        match self.scenes.last_mut().unwrap().next_scene(&mut self.tm) {
            NextScene::None => {}
            NextScene::Pop => {
                self.scenes.pop();
                self.tm.seek_to(self.times.pop().unwrap());
                self.scenes.last_mut().unwrap().enter(&mut self.tm, self.target_chooser.choose())?;
            }
            NextScene::PopN(num) => {
                for _ in 0..num {
                    self.scenes.pop();
                    self.tm.seek_to(self.times.pop().unwrap());
                }
                self.scenes.last_mut().unwrap().enter(&mut self.tm, self.target_chooser.choose())?;
            }
            NextScene::PopWithResult(result) => {
                self.scenes.pop();
                self.tm.seek_to(self.times.pop().unwrap());
                self.scenes.last_mut().unwrap().on_result(&mut self.tm, result)?;
                self.scenes.last_mut().unwrap().enter(&mut self.tm, self.target_chooser.choose())?;
            }
            NextScene::PopNWithResult(num, result) => {
                for _ in 0..num {
                    self.scenes.pop();
                    self.tm.seek_to(self.times.pop().unwrap());
                }
                self.scenes.last_mut().unwrap().on_result(&mut self.tm, result)?;
                self.scenes.last_mut().unwrap().enter(&mut self.tm, self.target_chooser.choose())?;
            }
            NextScene::Exit => {
                self.should_exit = true;
            }
            NextScene::Overlay(mut scene) => {
                self.times.push(self.tm.now());
                scene.enter(&mut self.tm, self.target_chooser.choose())?;
                self.scenes.push(scene);
            }
            NextScene::Replace(mut scene) => {
                scene.enter(&mut self.tm, self.target_chooser.choose())?;
                *self.scenes.last_mut().unwrap() = scene;
            }
        }
        Judge::on_new_frame();
        let mut touches = Judge::get_touches(1.0, false);
        touches.iter_mut().for_each(f);
        #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
        let mut input_action = None;
        #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
        {
            // 输入框开着的时候，这一帧的点击只用来关掉它，不再穿透给场景。
            let input_active = TEXT_INPUT.with(|it| {
                let active = it.borrow().is_some();
                if active {
                    touches.clear();
                }
                active
            });
            if input_active {
                input_action = TEXT_INPUT.with(|it| {
                    let mut guard = it.borrow_mut();
                    let input = guard.as_mut().unwrap();
                    input.step();
                    input.take()
                });
            }
        }
        if !touches.is_empty() {
            let now = self.tm.now();
            let delta = (now - self.last_update_time) / touches.len() as f64;
            let start_time = self.tm.start_time;
            let mut last_err = None;
            DIALOG.with(|it| -> Result<()> {
                let mut index = 1;
                touches.retain_mut(|touch| {
                    let t = self.last_update_time + (index + 1) as f64 * delta;
                    index += 1;
                    let mut guard = it.borrow_mut();
                    if let Some(dialog) = guard.as_mut() {
                        if !dialog.touch(touch, t as _) {
                            drop(guard);
                            *it.borrow_mut() = None;
                        }
                        false
                    } else {
                        drop(guard);
                        self.tm.seek_to(t);
                        match self.scenes.last_mut().unwrap().touch(&mut self.tm, touch) {
                            Ok(val) => !val,
                            Err(err) => {
                                warn!("failed to handle touch: {:?}", err);
                                last_err = Some(err);
                                false
                            }
                        }
                    }
                });
                Ok(())
            })?;
            if let Some(err) = last_err {
                return Err(err);
            }
            self.tm.start_time = start_time;
        }
        self.touches = Some(touches);
        self.last_update_time = self.tm.now();
        DIALOG.with(|it| {
            if let Some(dialog) = it.borrow_mut().as_mut() {
                dialog.update(self.last_update_time as _);
            }
        });
        #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
        {
            if let Some(text) = input_action {
                let _ = TEXT_INPUT.with(|it| it.borrow_mut().take());
                // 从栈顶往下找第一个认领这次输入的场景。
                for scene in self.scenes.iter_mut().rev() {
                    if scene.take_input(&mut self.tm, text.clone())? {
                        break;
                    }
                }
                return Ok(());
            }
            // 输入框开着的时候冻结场景：免得在改开始/结束时间时谱面还在自己往前走。
            if TEXT_INPUT.with(|it| it.borrow().is_some()) {
                return Ok(());
            }
        }
        self.scenes.last_mut().unwrap().update(&mut self.tm)?;
        Ok(())
    }

    pub fn render(&mut self, painter: &mut TextPainter) -> Result<()> {
        if self.paused {
            return Ok(());
        }
        let mut ui = Ui::new(painter, self.viewport);
        ui.set_touches(self.touches.take().unwrap());
        ui.scope(|ui| self.scenes.last_mut().unwrap().render(&mut self.tm, ui))?;
        if self.top_level {
            push_camera_state();
            set_camera(&ui.camera());
            let mut gl = unsafe { get_internal_gl() };
            gl.flush();
            // gl.quad_gl.render_pass(None);
            // gl.quad_gl.viewport(None);
            BILLBOARD.with(|it| {
                let mut guard = it.borrow_mut();
                let t = guard.1.now() as f32;
                guard.0.render(&mut ui, t);
            });
            DIALOG.with(|it| {
                if let Some(dialog) = it.borrow_mut().as_mut() {
                    dialog.render(&mut ui, self.tm.now() as _);
                }
            });
            #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
            TEXT_INPUT.with(|it| {
                if let Some(input) = it.borrow().as_ref() {
                    input.render(&mut ui);
                }
            });
            pop_camera_state();
        }
        Ok(())
    }

    /// 当前播放时刻（游戏内时间，秒）。
    pub fn time(&self) -> f64 {
        self.tm.now()
    }

    /// 把播放时刻拨到 `time`：分屏的副窗口靠它跟着主窗口走。
    pub fn seek_time(&mut self, time: f64) {
        self.tm.seek_to(time);
    }

    pub fn pause(&mut self) -> Result<()> {        self.paused = true;
        self.scenes.last_mut().unwrap().pause(&mut self.tm)
    }

    pub fn foucus_pause(&mut self) -> Result<()> {
        self.scenes.last_mut().unwrap().foucus_pause(&mut self.tm)
    }

    pub fn resume(&mut self) -> Result<()> {
        self.paused = false;
        self.scenes.last_mut().unwrap().resume(&mut self.tm)
    }

    pub fn foucus_resume(&mut self) -> Result<()> {
        self.scenes.last_mut().unwrap().foucus_resume(&mut self.tm)
    }

    pub fn should_exit(&self) -> bool {
        self.should_exit
    }
}

fn draw_background(tex: Texture2D, dim: bool) {
    let asp = screen_aspect();
    let top = 1. / asp;
    draw_image(tex, Rect::new(-1., -top, 2., top * 2.), ScaleType::CropCenter);
    if dim {
        draw_rectangle(-1., -top, 2., top * 2., Color::new(0., 0., 0., 0.5));
    }
}

pub type LocalSceneTask = LocalTask<Result<NextScene>>;
