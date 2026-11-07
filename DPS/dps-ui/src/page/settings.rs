dps::tl_file!("settings");

use super::{NextPage, OffsetPage, Page, SharedState};
use crate::{get_data, get_data_mut, save_data, scene::BGM_VOLUME_UPDATED};
use anyhow::Result;
use macroquad::prelude::*;
use dps::{
    ext::{LocalTask, RectExt, poll_future, validate_combo},
    health::{HealthConfig, HealthType},
    scene::{request_input, return_input, show_error, show_message, take_input},
    ui::{theme, Dialog, DRectButton, Scroll, Slider, Ui}
};
use std::{borrow::Cow, net::ToSocketAddrs, sync::atomic::Ordering};

const ITEM_HEIGHT: f32 = 0.078;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingListType {
    General,
    Audio,
    Chart,
    Other,
    About,
}

pub struct SettingsPage {
    btn_general: DRectButton,
    btn_audio: DRectButton,
    btn_chart: DRectButton,
    btn_other: DRectButton,
    btn_about: DRectButton,
    btn_reset: DRectButton,
    chosen: SettingListType,

    list_general: GeneralList,
    list_audio: AudioList,
    list_chart: ChartList,
    list_other: OtherList,

    scroll: Scroll,
    save_time: f32,
}

impl SettingsPage {
    const SAVE_TIME: f32 = 0.5;

    pub fn new() -> Self {
        Self {
            btn_general: DRectButton::new(),
            btn_audio: DRectButton::new(),
            btn_chart: DRectButton::new(),
            btn_other: DRectButton::new(),
            btn_about: DRectButton::new(),
            btn_reset: DRectButton::new(),
            chosen: SettingListType::General,

            list_general: GeneralList::new(),
            list_audio: AudioList::new(),
            list_chart: ChartList::new(),
            list_other: OtherList::new(),

            scroll: Scroll::new(),
            save_time: f32::INFINITY,
        }
    }

    #[inline]
    fn switch_to_type(&mut self, ty: SettingListType) {
        if self.chosen != ty {
            self.chosen = ty;
            self.scroll.y_scroller.offset = 0.;
        }
    }
}

impl Page for SettingsPage {
    fn label(&self) -> Cow<'static, str> {
        "SETTINGS".into()
    }

    fn exit(&mut self) -> Result<()> {
        BGM_VOLUME_UPDATED.store(true, Ordering::Relaxed);
        if self.save_time.is_finite() {
            save_data()?;
        }
        Ok(())
    }

    fn touch(&mut self, touch: &Touch, s: &mut SharedState) -> Result<bool> {
        let t = s.t;
        if match self.chosen {
            SettingListType::General => self.list_general.top_touch(touch, t),
            SettingListType::Audio => self.list_audio.top_touch(touch, t),
            SettingListType::Chart => self.list_chart.top_touch(touch, t),
            SettingListType::Other => self.list_other.top_touch(touch, t),
            SettingListType::About => false,
        } {
            return Ok(true);
        }

        if self.btn_general.touch(touch, t) {
            self.switch_to_type(SettingListType::General);
            return Ok(true);
        }
        if self.btn_audio.touch(touch, t) {
            self.switch_to_type(SettingListType::Audio);
            return Ok(true);
        }
        if self.btn_chart.touch(touch, t) {
            self.switch_to_type(SettingListType::Chart);
            return Ok(true);
        }
        if self.btn_other.touch(touch, t) {
            self.switch_to_type(SettingListType::Other);
            return Ok(true);
        }
        if self.btn_about.touch(touch, t) {
            self.switch_to_type(SettingListType::About);
            return Ok(true);
        }
        if matches!(self.chosen, SettingListType::About) && self.btn_reset.touch(touch, t) {
            // 整份配置换回默认值；全屏、音量这些运行时状态要立刻跟着改。
            Dialog::plain(tl!("reset-defaults"), tl!("reset-defaults-confirm"))
                .buttons(vec![tl!("cancel").to_string(), tl!("reset").to_string()])
                .listener(|pos| {
                    if pos == 1 {
                        let mut data = get_data_mut();
                        data.config = Default::default();
                        macroquad::window::set_fullscreen(data.config.fullscreen_mode);
                        drop(data);
                        BGM_VOLUME_UPDATED.store(true, Ordering::Relaxed);
                        if let Err(err) = save_data() {
                            show_error(err);
                        }
                        show_message(tl!("reset-done")).ok();
                    }
                })
                .show();
            return Ok(true);
        }
        if self.scroll.touch(touch, t) {
            return Ok(true);
        }
        if let Some(p) = match self.chosen {
            SettingListType::General => self.list_general.touch(touch, t)?,
            SettingListType::Audio => self.list_audio.touch(touch, t)?,
            SettingListType::Chart => self.list_chart.touch(touch, t)?,
            SettingListType::Other => self.list_other.touch(touch, t)?,
            SettingListType::About => None,
        } {
            if p {
                self.save_time = t;
            }
            self.scroll.y_scroller.halt();
            return Ok(true);
        }
        Ok(false)
    }

    fn update(&mut self, s: &mut SharedState) -> Result<()> {
        let t = s.t;
        self.scroll.update(t);
        if match self.chosen {
            SettingListType::General => self.list_general.update(t)?,
            SettingListType::Audio => self.list_audio.update(t)?,
            SettingListType::Chart => self.list_chart.update(t)?,
            SettingListType::Other => self.list_other.update(t)?,
            SettingListType::About => false,
        } {
            self.save_time = t;
        }
        if t > self.save_time + Self::SAVE_TIME {
            save_data()?;
            self.save_time = f32::INFINITY;
        }
        Ok(())
    }

    fn render(&mut self, ui: &mut Ui, s: &mut SharedState) -> Result<()> {
        let t = s.t;
        s.render_fader(ui, |ui, c| {
            ui.tab_rects(
                c,
                t,
                [
                    (&mut self.btn_general, tl!("general"), SettingListType::General),
                    (&mut self.btn_audio, tl!("audio"), SettingListType::Audio),
                    (&mut self.btn_chart, tl!("chart"), SettingListType::Chart),
                    (&mut self.btn_other, tl!("other"), SettingListType::Other),
                    (&mut self.btn_about, tl!("about"), SettingListType::About),
                ]
                .into_iter()
                .map(|(btn, text, ty)| (btn, text, ty == self.chosen)),
            );
        });
        let r = ui.content_rect();
        s.fader.render(ui, t, |ui, c| {
            ui.panel(r, theme::RADIUS_MD, c.a);
            let r = r.feather(-0.01);
            self.scroll.size((r.w, r.h));
            ui.scope(|ui| {
                ui.dx(r.x);
                ui.dy(r.y);
                self.scroll.render(ui, |ui| match self.chosen {
                    SettingListType::General => self.list_general.render(ui, r, t, c),
                    SettingListType::Audio => self.list_audio.render(ui, r, t, c),
                    SettingListType::Chart => self.list_chart.render(ui, r, t, c),
                    SettingListType::Other => self.list_other.render(ui, r, t, c),
                    SettingListType::About => {
                        let pad = 0.04;
                        let bottom = ui
                            .text(tl!("about-content", "version" => env!("CARGO_PKG_VERSION")))
                            .pos(pad, pad)
                            .size(0.55)
                            .multiline()
                            .max_width(r.w - pad * 2.)
                            .color(c)
                            .draw()
                            .bottom();
                        let br = Rect::new(pad, bottom + 0.04, 0.52, 0.1);
                        self.btn_reset.render_text(ui, br, t, c.a, tl!("reset-defaults"), 0.5, false);
                        (r.w, br.bottom() + 0.04)
                    }
                });
            });
        });
        Ok(())
    }

    fn next_page(&mut self) -> NextPage {
        if matches!(self.chosen, SettingListType::Audio) {
            return self.list_audio.next_page().unwrap_or_default();
        }
        NextPage::None
    }
}

fn render_title<'a>(ui: &mut Ui, c: Color, title: impl Into<Cow<'a, str>>, subtitle: Option<Cow<'a, str>>) -> f32 {
    const TITLE_SIZE: f32 = 0.42;
    const SUBTITLE_SIZE: f32 = 0.3;
    const LEFT: f32 = 0.06;
    const SUB_MAX_WIDTH: f32 = 1.4;
    if let Some(subtitle) = subtitle {
        // 两行各占固定的一条：标题贴上、副标题贴下。
        // 按测量高度居中会让两行在窄行高里挤到一起。
        let r = ui
            .text(title.into())
            .pos(LEFT, ITEM_HEIGHT * 0.10)
            .no_baseline()
            .size(TITLE_SIZE)
            .color(c)
            .draw();
        ui.text(subtitle)
            .pos(LEFT, ITEM_HEIGHT * 0.58)
            .no_baseline()
            .size(SUBTITLE_SIZE)
            .max_width(SUB_MAX_WIDTH)
            .color(Color { a: c.a * 0.6, ..c })
            .draw();
        r.right()
    } else {
        ui.text(title.into())
            .pos(LEFT, ITEM_HEIGHT / 2.)
            .anchor(0., 0.5)
            .no_baseline()
            .size(TITLE_SIZE)
            .color(c)
            .draw()
            .right()
    }
}

#[inline]
fn render_switch(ui: &mut Ui, r: Rect, t: f32, c: Color, btn: &mut DRectButton, on: bool) {
    btn.render_switch(ui, r, t, c.a, on);
}

/// 右侧开关列：宽高固定，右端留出面板内边距。
#[inline]
fn right_rect(w: f32) -> Rect {
    const H: f32 = 0.056;
    Rect::new(w - 0.26, (ITEM_HEIGHT - H) / 2., 0.22, H)
}

/// 输入框与滑条列：宽度是开关的两倍，右端与开关列对齐。
#[inline]
fn wide(w: f32) -> Rect {
    const H: f32 = 0.056;
    const WIDTH: f32 = 0.44;
    Rect::new(w - 0.04 - WIDTH, (ITEM_HEIGHT - H) / 2., WIDTH, H)
}

struct GeneralList {
    offline_btn: DRectButton,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fullscreen_btn: DRectButton,
    mp_btn: DRectButton,
    mp_addr_btn: DRectButton,
    anti_aliasing_btn: DRectButton,
    low_resolution_btn: DRectButton,
    insecure_btn: DRectButton,
}

impl GeneralList {
    pub fn new() -> Self {
        Self {
            offline_btn: DRectButton::new(),
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            fullscreen_btn: DRectButton::new(),
            mp_btn: DRectButton::new(),
            mp_addr_btn: DRectButton::new(),
            anti_aliasing_btn: DRectButton::new(),
            low_resolution_btn: DRectButton::new(),
            insecure_btn: DRectButton::new(),
        }
    }

    pub fn top_touch(&mut self, _touch: &Touch, _t: f32) -> bool {
        false
    }

    pub fn touch(&mut self, touch: &Touch, t: f32) -> Result<Option<bool>> {
        let data = get_data_mut();
        let config = &mut data.config;
        if self.offline_btn.touch(touch, t) {
            config.offline_mode ^= true;
            return Ok(Some(true));
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        if self.fullscreen_btn.touch(touch, t) {
            config.fullscreen_mode ^= true;
            macroquad::window::set_fullscreen(config.fullscreen_mode);
            return Ok(Some(true));
        }
        if self.mp_btn.touch(touch, t) {
            config.mp_enabled ^= true;
            return Ok(Some(true));
        }
        if self.mp_addr_btn.touch(touch, t) {
            request_input("mp_addr", &config.mp_address, tl!("item-mp-addr"));
            return Ok(Some(true));
        }
        if self.anti_aliasing_btn.touch(touch, t) {
            config.sample_count = if config.sample_count == 1 { 2 } else { 1 };
            return Ok(Some(true));
        }
        if self.low_resolution_btn.touch(touch, t) {
            config.low_resolution_mode ^= true;
            return Ok(Some(true));
        }
        if self.insecure_btn.touch(touch, t) {
            data.accept_invalid_cert ^= true;
            return Ok(Some(true));
        }
        Ok(None)
    }

    pub fn update(&mut self, t: f32) -> Result<bool> {
        let data = get_data_mut();
        if let Some((id, text)) = take_input() {
            if id == "mp_addr" {
                if let Err(err) = text.to_socket_addrs() {
                    show_error(anyhow::Error::new(err).context(tl!("item-mp-addr-invalid")));
                    return Ok(false);
                } else {
                    data.config.mp_address = text;
                    return Ok(true);
                }
            } else {
                return_input(id, text);
            }
        }
        Ok(false)
    }

    pub fn render(&mut self, ui: &mut Ui, r: Rect, t: f32, c: Color) -> (f32, f32) {
        let w = r.w;
        let mut h = 0.;
        macro_rules! item {
            ($($b:tt)*) => {{
                $($b)*
                ui.dy(ITEM_HEIGHT);
                h += ITEM_HEIGHT;
            }}
        }
        let rr = right_rect(w);

        let data = get_data();
        let config = &data.config;
        item! {
            render_title(ui, c, tl!("item-offline"), Some(tl!("item-offline-sub")));
            render_switch(ui, rr, t, c, &mut self.offline_btn, config.offline_mode);
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        item! {
            render_title(ui, c, tl!("item-fullscreen"), Some(tl!("item-fullscreen-sub")));
            render_switch(ui, rr, t, c, &mut self.fullscreen_btn, config.fullscreen_mode);
        }
        item! {
            render_title(ui, c, tl!("item-mp"), Some(tl!("item-mp-sub")));
            render_switch(ui, rr, t, c, &mut self.mp_btn, config.mp_enabled);
        }
        item! {
            render_title(ui, c, tl!("item-mp-addr"), Some(tl!("item-mp-addr-sub")));
            self.mp_addr_btn.render_text_left(ui, wide(w), t, c.a, &config.mp_address, 0.36, false);
        }
        item! {
            render_title(ui, c, tl!("item-anti-aliasing"), Some(tl!("item-anti-aliasing-sub")));
            render_switch(ui, rr, t, c, &mut self.anti_aliasing_btn, config.sample_count == 2);
        }
        item! {
            render_title(ui, c, tl!("item-low-resolution"), Some(tl!("item-low-resolution-sub")));
            render_switch(ui, rr, t, c, &mut self.low_resolution_btn, config.low_resolution_mode);
        }
        item! {
            render_title(ui, c, tl!("item-insecure"), Some(tl!("item-insecure-sub")));
            render_switch(ui, rr, t, c, &mut self.insecure_btn, data.accept_invalid_cert);
        }
        (w, h)
    }
}

struct AudioList {
    adjust_btn: DRectButton,
    music_slider: Slider,
    sfx_slider: Slider,
    bgm_slider: Slider,
    cali_btn: DRectButton,
    #[cfg(target_os = "android")]
    audio_compatibility_btn: DRectButton,

    cali_task: LocalTask<Result<OffsetPage>>,
    next_page: Option<NextPage>,
}

impl AudioList {
    pub fn new() -> Self {
        Self {
            adjust_btn: DRectButton::new(),
            music_slider: Slider::new(0.0..2.0, 0.05),
            sfx_slider: Slider::new(0.0..2.0, 0.05),
            bgm_slider: Slider::new(0.0..2.0, 0.05),
            cali_btn: DRectButton::new(),
            #[cfg(target_os = "android")]
            audio_compatibility_btn: DRectButton::new(),

            cali_task: None,
            next_page: None,
        }
    }

    pub fn top_touch(&mut self, _touch: &Touch, _t: f32) -> bool {
        false
    }

    pub fn touch(&mut self, touch: &Touch, t: f32) -> Result<Option<bool>> {
        let data = get_data_mut();
        let config = &mut data.config;
        if self.adjust_btn.touch(touch, t) {
            config.auto_tweak_offset ^= true;
            return Ok(Some(true));
        }
        if let wt @ Some(_) = self.music_slider.touch(touch, t, &mut config.volume_music) {
            return Ok(wt);
        }
        if let wt @ Some(_) = self.sfx_slider.touch(touch, t, &mut config.volume_sfx) {
            return Ok(wt);
        }
        let old = config.volume_bgm;
        if let wt @ Some(_) = self.bgm_slider.touch(touch, t, &mut config.volume_bgm) {
            if (config.volume_bgm - old).abs() > 0.001 {
                BGM_VOLUME_UPDATED.store(true, Ordering::Relaxed);
            }
            return Ok(wt);
        }
        if self.cali_btn.touch(touch, t) {
            self.cali_task = Some(Box::pin(OffsetPage::new()));
            return Ok(Some(false));
        }
        #[cfg(target_os = "android")]
        if self.audio_compatibility_btn.touch(touch, t) {
            config.audio_compatibility ^= true;
            return Ok(Some(true));
        }
        Ok(None)
    }

    pub fn update(&mut self, _t: f32) -> Result<bool> {
        if let Some(task) = &mut self.cali_task {
            if let Some(res) = poll_future(task.as_mut()) {
                match res {
                    Err(err) => show_error(err.context(tl!("load-cali-failed"))),
                    Ok(page) => {
                        self.next_page = Some(NextPage::Overlay(Box::new(page)));
                    }
                }
                self.cali_task = None;
            }
        }
        Ok(false)
    }

    pub fn render(&mut self, ui: &mut Ui, r: Rect, t: f32, c: Color) -> (f32, f32) {
        let w = r.w;
        let mut h = 0.;
        macro_rules! item {
            ($($b:tt)*) => {{
                $($b)*
                ui.dy(ITEM_HEIGHT);
                h += ITEM_HEIGHT;
            }}
        }
        let rr = right_rect(w);

        let data = get_data();
        let config = &data.config;
        item! {
            render_title(ui, c, tl!("item-auto-latency"), Some(tl!("item-auto-latency-sub")));
            render_switch(ui, rr, t, c, &mut self.adjust_btn, config.auto_tweak_offset);
        }
        item! {
            render_title(ui, c, tl!("item-music"), Some(tl!("item-music-sub")));
            self.music_slider.render(ui, wide(w), t,c, config.volume_music, format!("{:.2}", config.volume_music));
        }
        item! {
            render_title(ui, c, tl!("item-sfx"), Some(tl!("item-sfx-sub")));
            self.sfx_slider.render(ui, wide(w), t, c, config.volume_sfx, format!("{:.2}", config.volume_sfx));
        }
        item! {
            render_title(ui, c, tl!("item-bgm"), Some(tl!("item-bgm-sub")));
            self.bgm_slider.render(ui, wide(w), t, c, config.volume_bgm, format!("{:.2}", config.volume_bgm));
        }
        item! {
            render_title(ui, c, tl!("item-cali"), Some(tl!("item-cali-sub")));
            self.cali_btn.render_text(ui, rr, t, c.a, format!("{:.0}ms", config.offset * 1000.), 0.5, true);
        }
        #[cfg(target_os = "android")]
        item! {
            render_title(ui, c, tl!("item-audio-compatibility"), Some(tl!("item-audio-compatibility-sub")));
            render_switch(ui, rr, t, c, &mut self.audio_compatibility_btn, config.audio_compatibility);
        }
        (w, h)
    }

    pub fn next_page(&mut self) -> Option<NextPage> {
        self.next_page.take()
    }
}

struct ChartList {
    show_acc_btn: DRectButton,
    dc_pause_btn: DRectButton,
    dhint_btn: DRectButton,
    aggressive_chart_btn: DRectButton,
    aggressive_note_btn: DRectButton,
    aggressive_particle_btn: DRectButton,
    speed_slider: Slider,
    size_slider: Slider,
    render_extra_btn: DRectButton,
}

impl ChartList {
    pub fn new() -> Self {
        Self {
            show_acc_btn: DRectButton::new(),
            dc_pause_btn: DRectButton::new(),
            dhint_btn: DRectButton::new(),
            aggressive_chart_btn: DRectButton::new(),
            aggressive_note_btn: DRectButton::new(),
            aggressive_particle_btn: DRectButton::new(),
            speed_slider: Slider::new(0.1..2.0, 0.05),
            size_slider: Slider::new(0.0..5.0, 0.005),
            render_extra_btn: DRectButton::new(),
        }
    }

    pub fn top_touch(&mut self, _touch: &Touch, _t: f32) -> bool {
        false
    }

    pub fn touch(&mut self, touch: &Touch, t: f32) -> Result<Option<bool>> {
        let data = get_data_mut();
        let config = &mut data.config;
        if self.show_acc_btn.touch(touch, t) {
            config.show_acc ^= true;
            return Ok(Some(true));
        }
        if self.dc_pause_btn.touch(touch, t) {
            config.double_click_to_pause ^= true;
            return Ok(Some(true));
        }
        if self.dhint_btn.touch(touch, t) {
            config.render_double_hint ^= true;
            return Ok(Some(true));
        }
        if self.aggressive_chart_btn.touch(touch, t) {
            config.aggressive_chart ^= true;
            if !config.aggressive_chart {
                config.aggressive_note = false;
                config.aggressive_particle = false;
            }
            return Ok(Some(true));
        }
        if self.aggressive_note_btn.touch(touch, t) {
            config.aggressive_note ^= true;
            if config.aggressive_note {
                config.aggressive_chart = true;
            }
            return Ok(Some(true));
        }
        if self.aggressive_particle_btn.touch(touch, t) {
            config.aggressive_particle ^= true;
            if config.aggressive_particle {
                config.aggressive_chart = true;
            }
            return Ok(Some(true));
        }
        if let wt @ Some(_) = self.speed_slider.touch(touch, t, &mut config.speed) {
            return Ok(wt);
        }
        if let wt @ Some(_) = self.size_slider.touch(touch, t, &mut config.note_scale) {
            return Ok(wt);
        }
        if self.render_extra_btn.touch(touch, t) {
            config.render_extra ^= true;
            return Ok(Some(true));
        }
        Ok(None)
    }

    pub fn update(&mut self, _t: f32) -> Result<bool> {
        Ok(false)
    }

    pub fn render(&mut self, ui: &mut Ui, r: Rect, t: f32, c: Color) -> (f32, f32) {
        let w = r.w;
        let mut h = 0.;
        macro_rules! item {
            ($($b:tt)*) => {{
                $($b)*
                ui.dy(ITEM_HEIGHT);
                h += ITEM_HEIGHT;
            }}
        }
        let rr = right_rect(w);

        let data = get_data();
        let config = &data.config;
        item! {
            render_title(ui, c, tl!("item-show-acc"), Some(tl!("item-show-acc-sub")));
            render_switch(ui, rr, t, c, &mut self.show_acc_btn, config.show_acc);
        }
        item! {
            render_title(ui, c, tl!("item-dc-pause"), Some(tl!("item-dc-pause-sub")));
            render_switch(ui, rr, t, c, &mut self.dc_pause_btn, config.double_click_to_pause);
        }
        item! {
            render_title(ui, c, tl!("item-dhint"), Some(tl!("item-dhint-sub")));
            render_switch(ui, rr, t, c, &mut self.dhint_btn, config.render_double_hint);
        }
        item! {
            render_title(ui, c, tl!("item-aggressive-chart"), Some(tl!("item-aggressive-chart-sub")));
            render_switch(ui, rr, t, c, &mut self.aggressive_chart_btn, config.aggressive_chart);
        }
        item! {
            render_title(ui, c, tl!("item-aggressive-note"), Some(tl!("item-aggressive-note-sub")));
            render_switch(ui, rr, t, c, &mut self.aggressive_note_btn, config.aggressive_note);
        }
        item! {
            render_title(ui, c, tl!("item-aggressive-particle"), Some(tl!("item-aggressive-particle-sub")));
            render_switch(ui, rr, t, c, &mut self.aggressive_particle_btn, config.aggressive_particle);
        }
        item! {
            render_title(ui, c, tl!("item-speed"), Some(tl!("item-speed-sub")));
            self.speed_slider.render(ui, wide(w), t,c, config.speed, format!("{:.2}", config.speed));
        }
        item! {
            render_title(ui, c, tl!("item-note-size"), Some(tl!("item-note-size-sub")));
            self.size_slider.render(ui, wide(w), t,c, config.note_scale, format!("{:.3}", config.note_scale));
        }
        item! {
            render_title(ui, c, tl!("item-render-extra"), Some(tl!("item-render-extra-sub")));
            render_switch(ui, rr, t, c, &mut self.render_extra_btn, config.render_extra);
        }
        (w, h)
    }
}

struct OtherList {
    chart_debug_line_slider: Slider,
    chart_debug_note_slider: Slider,
    touch_debug_btn: DRectButton,
    chart_ratio_slider: Slider,
    fade_slider: Slider,
    watermark: DRectButton,
    combo_btn: DRectButton,
    roman_btn: DRectButton,
    chinese_btn: DRectButton,
    rotation_mode: DRectButton,
    #[cfg(feature = "play")]
    shake_play_mode_btn: DRectButton,

    health_mode_btn: DRectButton,
}

impl OtherList {
    pub fn new() -> Self {
        Self {
            chart_debug_line_slider: Slider::new(0.0..1.0, 0.05),
            chart_debug_note_slider: Slider::new(0.0..1.0, 0.05),
            touch_debug_btn: DRectButton::new(),
            chart_ratio_slider: Slider::new(0.05..1.0, 0.05),
            fade_slider: Slider::new(-2.0..2.0, 0.05),
            watermark: DRectButton::new(),
            combo_btn: DRectButton::new(),
            roman_btn: DRectButton::new(),
            chinese_btn: DRectButton::new(),
            rotation_mode: DRectButton::new(),
            #[cfg(feature = "play")]
            shake_play_mode_btn: DRectButton::new(),
            #[cfg(feature = "play")]
            health_mode_btn: DRectButton::new(),
        }
    }

    pub fn top_touch(&mut self, _touch: &Touch, _t: f32) -> bool {
        false
    }

    pub fn touch(&mut self, touch: &Touch, t: f32) -> Result<Option<bool>> {
        let data = get_data_mut();
        let config = &mut data.config;
        if let wt @ Some(_) = self.chart_debug_line_slider.touch(touch, t, &mut config.chart_debug_line) {
            return Ok(wt);
        }
        if let wt @ Some(_) = self.chart_debug_note_slider.touch(touch, t, &mut config.chart_debug_note) {
            return Ok(wt);
        }
        if self.touch_debug_btn.touch(touch, t) {
            config.touch_debug ^= true;
            return Ok(Some(true));
        }
        if let wt @ Some(_) = self.chart_ratio_slider.touch(touch, t, &mut config.chart_ratio) {
            return Ok(wt);
        }
        if let wt @ Some(_) = self.fade_slider.touch(touch, t, &mut config.fade) {
            return Ok(wt);
        }
        if self.watermark.touch(touch, t) {
            request_input("watermark", &config.watermark, tl!("item-watermark"));
            return Ok(Some(true));
        }
        if self.combo_btn.touch(touch, t) {
            request_input("combo", &config.combo, tl!("item-combo"));
            return Ok(Some(true));
        }
        if self.roman_btn.touch(touch, t) {
            config.roman ^= true;
            if config.roman && config.roman == config.chinese {
                config.chinese = !config.roman;
            }
            return Ok(Some(true));
        }
        if self.chinese_btn.touch(touch, t) {
            config.chinese ^= true;
            if config.chinese && config.chinese == config.roman {
                config.roman = !config.chinese;
            }
            return Ok(Some(true));
        }
        if self.rotation_mode.touch(touch, t) {
            config.rotation_mode ^= true;
            return Ok(Some(true));
        }
        #[cfg(feature = "play")]
        if self.shake_play_mode_btn.touch(touch, t) {
            config.shake_play_mode ^= true;
            return Ok(Some(true));
        }
        #[cfg(feature = "play")]
        if self.health_mode_btn.touch(touch, t) {
            let text = if let Some(health_mode) = config.health_mode.clone() {
                health_mode.to_json()?
            } else {
                String::new()
            };
            request_input("health-mode", &text, tl!("item-health-mode"));
            return Ok(Some(true));
        }
        Ok(None)
    }

    pub fn update(&mut self, _t: f32) -> Result<bool> {
        let data = get_data_mut();
        if let Some((id, text)) = take_input() {
            if id == "watermark" {
                data.config.watermark = text;
                return Ok(true);
            } else {
                return_input(id, text);
            }
        }
        if let Some((id, text)) = take_input() {
            if id == "combo" {
                if validate_combo(&text) || text.len() > 50 {
                    show_message(tl!("not-combo")).error();
                    return Ok(false);
                }
                data.config.combo = text;
                return Ok(true);
            } else {
                return_input(id, text);
            }
        }
        #[cfg(feature = "play")]
        if let Some((id, text)) = take_input() {
            if id == "health-mode" {
                if text.trim().is_empty() {
                    data.config.health_mode = None;
                    return Ok(true);
                }
                match HealthConfig::from_json(&text) {
                    Ok(health_mode) => {
                        data.config.health_mode = Some(health_mode);
                        return Ok(true);
                    }
                    Err(_) => {
                        show_message(tl!("illegal-input")).error();
                        return Ok(false);
                    }
                }
            } else {
                return_input(id, text);
            }
        }
        Ok(false)
    }

    pub fn render(&mut self, ui: &mut Ui, r: Rect, t: f32, c: Color) -> (f32, f32) {
        let w = r.w;
        let mut h = 0.;
        macro_rules! item {
            ($($b:tt)*) => {{
                $($b)*
                ui.dy(ITEM_HEIGHT);
                h += ITEM_HEIGHT;
            }}
        }
        let rr = right_rect(w);

        let data = get_data();
        let config = &data.config;
        item! {
            render_title(ui, c, tl!("item-chart-debug-line"), Some(tl!("item-chart-debug-line-sub")));
            self.chart_debug_line_slider.render(ui, wide(w), t,c, config.chart_debug_line, format!("{:.2}", config.chart_debug_line));
        }
        item! {
            render_title(ui, c, tl!("item-chart-debug-note"), Some(tl!("item-chart-debug-note-sub")));
            self.chart_debug_note_slider.render(ui, wide(w), t,c, config.chart_debug_note, format!("{:.2}", config.chart_debug_note));
        }
        item! {
            render_title(ui, c, tl!("item-touch-debug"), Some(tl!("item-touch-debug-sub")));
            render_switch(ui, rr, t, c, &mut self.touch_debug_btn, config.touch_debug);
        }
        item! {
            render_title(ui, c, tl!("item-chart_ratio"), Some(tl!("item-chart_ratio-sub")));
            self.chart_ratio_slider.render(ui, wide(w), t,c, config.chart_ratio, format!("{:.2}", config.chart_ratio));
        }
        item! {
            render_title(ui, c, tl!("item-fade"), Some(tl!("item-fade-sub")));
            self.fade_slider.render(ui, wide(w), t,c, config.fade, format!("{:.2}", config.fade));
        }
        item! {
            render_title(ui, c, tl!("item-watermark"), Some(tl!("item-watermark-sub")));
            self.watermark.render_text_left(ui, wide(w), t, c.a, &config.watermark, 0.36, false);
        }
        item! {
            render_title(ui, c, tl!("item-combo"), Some(tl!("item-combo-sub")));
            self.combo_btn.render_text_left(ui, wide(w), t, c.a, &config.combo, 0.36, false);
        }
        item! {
            render_title(ui, c, tl!("item-roman"), Some(tl!("item-roman-sub")));
            render_switch(ui, rr, t, c, &mut self.roman_btn, config.roman);
        }
        item! {
            render_title(ui, c, tl!("item-chinese"), Some(tl!("item-chinese-sub")));
            render_switch(ui, rr, t, c, &mut self.chinese_btn, config.chinese);
        }
        item! {
            render_title(ui, c, tl!("item-rotation-mode"), Some(tl!("item-rotation-mode-sub")));
            render_switch(ui, rr, t, c, &mut self.rotation_mode, config.rotation_mode);
        }
        #[cfg(feature = "play")]
        item! {
            render_title(ui, c, tl!("item-shake-play-mode"), Some(tl!("item-shake-play-mode-sub")));
            render_switch(ui, rr, t, c, &mut self.shake_play_mode_btn, config.shake_play_mode);
        }
        #[cfg(feature = "play")]
        item! {
            render_title(ui, c, tl!("item-health-mode"), Some(tl!("item-health-mode-sub")));
            let text = match config.health_mode.clone().map(|it| it.mode) {
                None => "OFF",
                Some(HealthType::Classic{}) => "classic",
                Some(HealthType::ComboHeal{ .. }) => "comboHeal",
                Some(HealthType::SpeedBased{ .. }) => "speedBased",
            };
            self.health_mode_btn.render_text(ui, rr, t, c.a, text, 0.4, false);
        }
        (w, h)
    }
}
