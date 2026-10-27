use crate::{
    config::Config,
    core::{BadNote, Chart, NOTE_WIDTH_RATIO_BASE, Note, NoteKind, Point, Resource, Vector, block_touch_blocked},
    ext::{NotNanExt, get_viewport},
};
use macroquad::prelude::{
    utils::{register_input_subscriber, repeat_all_miniquad_input},
    *,
};
use miniquad::{EventHandler, MouseButton};
use once_cell::sync::Lazy;
use rustc_hash::{FxHashMap, FxHashSet};
use sasa::{PlaySfxParams, Sfx};
use serde::Serialize;
use std::{cell::RefCell, collections::HashMap, num::FpCategory};

pub const FLICK_SPEED_THRESHOLD: f32 = 0.8;
pub const LIMIT_PERFECT: f64 = 0.08;
pub const LIMIT_GOOD: f64 = 0.18;
pub const LIMIT_BAD: f64 = 0.22;
pub const UP_TOLERANCE: f64 = 0.05;
pub const DIST_FACTOR: f64 = 0.2;
const LATE_OFFSET: f64 = 0.13;

pub fn play_sfx(sfx: &mut Sfx, config: &Config) {
    if config.volume_sfx <= 1e-2 {
        return;
    }
    // 渲染期间不实时出声（见 `crate::ext::audio_silent`）：成片里的按键音轨由渲染器按谱面
    // 音头离线合成，跟这里的实时播放无关。
    if crate::ext::audio_silent() {
        return;
    }
    let _ = sfx.play(PlaySfxParams {
        amplifier: config.volume_sfx,
    });
}

#[cfg(all(not(target_os = "windows"), not(target_os = "ios")))]
fn get_uptime() -> f64 {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    let ret = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) };
    assert!(ret == 0);
    time.tv_sec as f64 + time.tv_nsec as f64 * 1e-9
}

#[cfg(target_os = "ios")]
fn get_uptime() -> f64 {
    use crate::objc::*;
    unsafe {
        let process_info: ObjcId = msg_send![class!(NSProcessInfo), processInfo];
        msg_send![process_info, systemUptime]
    }
}

#[cfg(target_os = "windows")]
fn get_uptime() -> f64 {
    miniquad::native::windows::get_uptime()
}

#[derive(Debug, Clone)]
pub enum HitSound {
    None,
    Click,
    Flick,
    Drag,
    Custom(String),
}

impl HitSound {
    pub fn play(&self, res: &mut Resource) {
        match self {
            HitSound::None => {}
            HitSound::Click => {
                if check_hitsound(&mut res.played_hitsounds_count, "click") {
                    play_sfx(&mut res.sfx_click, &res.config)
                }
            },
            HitSound::Flick => {
                if check_hitsound(&mut res.played_hitsounds_count, "flick") {
                    play_sfx(&mut res.sfx_flick, &res.config)
                }
            },
            HitSound::Drag => {
                if check_hitsound(&mut res.played_hitsounds_count, "drag") {
                    play_sfx(&mut res.sfx_drag, &res.config)
                }
            },
            HitSound::Custom(s) => {
                if let Some(sfx) = res.extra_sfxs.get_mut(s) {
                    if check_hitsound(&mut res.played_hitsounds_count, s) {
                        play_sfx(sfx, &res.config)
                    }
                }
            }
        }
    }

    pub fn default_from_kind(kind: &NoteKind) -> Self {
        match kind {
            NoteKind::Click => HitSound::Click,
            NoteKind::Flick => HitSound::Flick,
            NoteKind::Drag => HitSound::Drag,
            NoteKind::Hold { .. } => HitSound::Click,
        }
    }
}

fn check_hitsound(map: &mut HashMap<String, u8, rustc_hash::FxBuildHasher>, sfx: &str) -> bool {
    let count = map.entry(sfx.to_string()).or_insert(0);
    if *count < 5 {
        *count += 1;
        true
    } else {
        false
    }
}

pub struct FlickTracker {
    threshold: f32,
    last_point: Point,
    last_delta: Option<Vector>,
    last_time: f32,
    flicked: bool,
    stopped: bool,
}

impl FlickTracker {
    pub fn new(_dpi: u32, time: f32, point: Point) -> Self {
        // TODO maybe a better approach?
        let dpi = 275;
        Self {
            threshold: FLICK_SPEED_THRESHOLD * dpi as f32 / 386.,
            last_point: point,
            last_delta: None,
            last_time: time,
            flicked: false,
            stopped: true,
        }
    }

    pub fn push(&mut self, time: f32, position: Point) {
        let delta = position - self.last_point;
        self.last_point = position;
        if let Some(last_delta) = &self.last_delta {
            let dt = time - self.last_time;
            let speed = delta.dot(last_delta) / dt;
            if speed < self.threshold {
                self.stopped = true;
            }
            if self.stopped && !self.flicked {
                self.flicked = delta.magnitude() / dt >= self.threshold * 2.;
            }
            // if speed < self.threshold || self.stopped {
            // self.stopped = delta.magnitude() / dt < self.threshold * 5.;
            // self.flicked = self.threshold <= speed;
            // if self.flicked {
            // warn!("new flick!");
            // }
            // }
        }
        self.last_delta = Some(delta.normalize());
        self.last_time = time;
    }
}

#[derive(Debug)]
pub enum JudgeStatus {
    NotJudged,
    PreJudge,
    Judged,
    Hold(bool, f64, f64, bool, f64), // perfect, at, diff, pre-judge, up-time
}

#[repr(u8)]
#[derive(Debug, Copy, Clone, Serialize)]
pub enum Judgement {
    Perfect,
    Good,
    Bad,
    Miss,
}

#[cfg(not(feature = "closed"))]
#[derive(Default)]
pub(crate) struct JudgeInner {
    diffs: Vec<f64>,

    combo: u32,
    max_combo: u32,
    counts: [u32; 4],
    num_of_notes: u32,
}

#[cfg(not(feature = "closed"))]
impl JudgeInner {
    pub fn new(num_of_notes: u32) -> Self {
        Self {
            diffs: Vec::new(),

            combo: 0,
            max_combo: 0,
            counts: [0; 4],
            num_of_notes,
        }
    }

    pub fn commit(&mut self, what: Judgement, diff: f64) {
        use Judgement::*;
        if matches!(what, Judgement::Good) {
            self.diffs.push(diff);
        }
        self.counts[what as usize] += 1;
        match what {
            Perfect | Good => {
                self.combo += 1;
                if self.combo > self.max_combo {
                    self.max_combo = self.combo;
                }
            }
            _ => {
                self.combo = 0;
            }
        }
    }

    pub fn reset(&mut self) {
        self.combo = 0;
        self.max_combo = 0;
        self.counts = [0; 4];
        self.diffs.clear();
    }

    pub fn accuracy(&self) -> f64 {
        (self.counts[0] as f64 + self.counts[1] as f64 * 0.65) / self.num_of_notes as f64
    }

    pub fn real_time_accuracy(&self) -> f64 {
        let cnt = self.counts.iter().sum::<u32>();
        if cnt == 0 {
            return 1.;
        }
        (self.counts[0] as f64 + self.counts[1] as f64 * 0.65) / cnt as f64
    }

    pub fn score(&self) -> f64 {
        if self.counts[0] == self.num_of_notes {
            1_000_000.0
        } else {
            (0.9 * self.accuracy() + self.max_combo as f64 / self.num_of_notes as f64 * 0.1) * 1_000_000.
        }
    }

    pub fn result(&self) -> PlayResult {
        let early = self.diffs.iter().filter(|it| **it < 0.).count() as u32;
        PlayResult {
            score: self.score(),
            accuracy: self.accuracy(),
            max_combo: self.max_combo,
            num_of_notes: self.num_of_notes,
            counts: self.counts,
            early,
            late: self.diffs.len() as u32 - early,
            std: 0.,
        }
    }

    pub fn combo(&self) -> u32 {
        self.combo
    }

    pub fn counts(&self) -> [u32; 4] {
        self.counts
    }
}

#[cfg(feature = "closed")]
mod inner;
#[cfg(feature = "closed")]
use inner::*;

/// 「噪域吞触摸」的挡标记维护：返回这根手指这一帧是否算被噪域挡着。
///
/// 官方的语义是「按下时在噪域里 → 一直挡到松手」：中途滑出噪域也不解除，
/// 抬起 / 取消才清掉。所以除了 `inside`（这一帧真的落在噪域里）之外还要看历史。
fn finger_blocked(blocked_finger_ids: &mut FxHashSet<u64>, id: u64, phase: TouchPhase, inside: bool) -> bool {
    if matches!(phase, TouchPhase::Started | TouchPhase::Ended | TouchPhase::Cancelled) {
        // 手指 id 会被系统立刻复用，上一次触摸的挡标记不能跨生命周期带过来
        blocked_finger_ids.remove(&id);
    }
    if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
        return false;
    }
    if inside {
        blocked_finger_ids.insert(id);
    }
    blocked_finger_ids.contains(&id)
}

#[repr(C)]
pub struct Judge {
    // notes of each line in order
    // LinkedList::drain_filter is unstable...
    pub notes: Vec<(Vec<u32>, usize)>,
    pub trackers: FxHashMap<u64, FlickTracker>,
    pub last_time: f64,
    /// 「噪域吞触摸」：当前被噪域挡住的触点 id（按下时落在噪域里 → 挡到松手）。
    blocked_finger_ids: FxHashSet<u64>,

    key_down_count: u32,

    /// 「落点模拟」（`config.touch_sim`）：AUTOPLAY 的替代驱动 —— 每帧为到点的按键合成一根
    /// 「自动点击的手指」，判定走真实游玩那条链路。
    sim_fingers: Vec<SimFinger>,
    /// 已经替它按过的按键 `(判定线 id, 按键 id)`，防止同一颗按键被按第二下。
    sim_pressed: FxHashSet<(u32, u32)>,
    /// 合成手指的 id 序号（加在 `SIM_FINGER_ID_BASE` 上，跟真实触点 id 分开）。
    sim_finger_seq: u64,

    pub(crate) inner: JudgeInner,
    pub judgements: RefCell<Vec<(f64, u32, u32, Result<Judgement, bool>)>>,
}

/// 合成触点的 id 基址：跟真实触点的 id 拉开，免得撞号。
const SIM_FINGER_ID_BASE: u64 = 0x5349_4d00_0000_0000;

/// 一颗「落点模拟」合成出来的手指。
///
/// 按下那一刻把落点钉在按键中心，之后就定在屏幕上不动 —— 判定完全走真实游玩那条链路，
/// 所以判定线把按键带走时它就跟不上，而别的按键被带到这根手指底下时也能被它接住。
struct SimFinger {
    /// 合成触点的 id（走 `Touch::id` 那条路）。
    id: u64,
    /// 钉住的落点，判定空间（`Touch::position` 的约定：y 轴与谱面相反）。
    pos: Vec2,
    /// 渲染用的谱面坐标 `(x, y)` 与判定半宽 —— 也就是判定线走多远会断连的那个范围。
    chart: (f32, f32, f32),
    /// 它代按的按键，用来判断是否已经判成 Miss（落点转红）。
    anchor: (u32, u32),
    /// 什么时候抬手指（谱面时刻）。
    release: f64,
    /// 这一帧的触点相位。
    phase: TouchPhase,
    /// 已经发过 `Ended`，下一帧摘掉。
    retired: bool,
}

static SUBSCRIBER_ID: Lazy<usize> = Lazy::new(register_input_subscriber);
thread_local! {
    static TOUCHES: RefCell<(Vec<Touch>, i32, u32)> = RefCell::default();
}

impl Judge {
    pub fn new(chart: &Chart) -> Self {
        let notes = chart
            .lines
            .iter()
            .map(|line| {
                let mut idx: Vec<u32> = (0..(line.notes.len() as u32)).filter(|it| !line.notes[*it as usize].fake).collect();
                idx.sort_unstable_by_key(|id| line.notes[*id as usize].time.not_nan());
                (idx, 0)
            })
            .collect();
        Self {
            notes,
            trackers: FxHashMap::with_capacity_and_hasher(16, Default::default()),
            blocked_finger_ids: FxHashSet::default(),
            last_time: 0.,

            key_down_count: 0,

            sim_fingers: Vec::new(),
            sim_pressed: FxHashSet::default(),
            sim_finger_seq: 0,

            inner: JudgeInner::new(chart.lines.iter().map(|it| it.notes.iter().filter(|it| !it.fake).count() as u32).sum()),
            judgements: RefCell::new(Vec::new()),
        }
    }

    pub fn reset(&mut self) {
        self.notes.iter_mut().for_each(|it| it.1 = 0);
        self.trackers.clear();
        self.blocked_finger_ids.clear();
        self.sim_fingers.clear();
        self.sim_pressed.clear();
        self.inner.reset();
        self.judgements.borrow_mut().clear();
    }

    /// 暂停 / 回放时调用：忘掉「哪些手指正被噪域挡着」，恢复播放后重新按当前位置判定。
    pub fn clear_blocked_fingers(&mut self) {
        self.blocked_finger_ids.clear();
    }

    /// 播放头回退到 `time` 之后调用：把判定进度退回到 `time`。
    ///
    /// 回退点之后已经判过的记录要撤掉（`judgements` 里逐条删），分数 / 连击 / 准确率按剩下的
    /// 记录重建 —— 只挪游标不撤记录的话，那段音符会被判第二次，分数直接算重。
    /// 回退点之前的判定原样保留。
    pub fn rewind_to(&mut self, chart: &Chart, time: f64) {
        let num_of_notes = chart.lines.iter().map(|it| it.notes.iter().filter(|it| !it.fake).count() as u32).sum();
        let mut inner = JudgeInner::new(num_of_notes);
        let mut judgements = self.judgements.borrow_mut();
        judgements.retain(|(t, line_id, note_id, what)| {
            let note_time = chart.lines[*line_id as usize].notes[*note_id as usize].time;
            let kept = note_time < time;
            if let (true, Ok(what)) = (kept, what) {
                // 长条的按下动作记成 `Err(..)`，不进 `inner`，这里同样跳过。
                // 第二项是「判定时刻 - 音符时刻」，用来还原 early / late 统计的符号。
                inner.commit(what.clone(), t - note_time);
            }
            kept
        });
        drop(judgements);
        self.inner = inner;
        for (line, (idx, cursor)) in chart.lines.iter().zip(self.notes.iter_mut()) {
            // `idx` 在 `Judge::new` 里按音符时刻排过序，`partition_point` 就是第一个 `time >= time`
            // 的音符的位置。
            *cursor = idx.partition_point(|id| line.notes[*id as usize].time < time);
        }
        self.trackers.clear();
        self.blocked_finger_ids.clear();
        self.sim_fingers.clear();
        self.sim_pressed.clear();
        self.key_down_count = 0;
        // 触摸事件的时间戳按「上一帧时刻 → 这一帧时刻」插值，回退后不跟上的话会插到未来去。
        self.last_time = time;
    }

    pub fn commit(&mut self, t: f64, what: Judgement, line_id: u32, note_id: u32, diff: f64) {
        self.judgements.borrow_mut().push((t, line_id, note_id, Ok(what)));
        self.inner.commit(what, diff);
    }

    #[inline]
    pub fn accuracy(&self) -> f64 {
        self.inner.accuracy()
    }

    #[inline]
    pub fn real_time_accuracy(&self) -> f64 {
        self.inner.real_time_accuracy()
    }

    #[inline]
    pub fn score(&self) -> f64 {
        self.inner.score()
    }

    pub(crate) fn on_new_frame() {
        let mut handler = Handler(Vec::new(), 0, 0);
        repeat_all_miniquad_input(&mut handler, *SUBSCRIBER_ID);
        handler.finalize();
        TOUCHES.with(|it| {
            *it.borrow_mut() = (handler.0, handler.1, handler.2);
        });
    }

    fn rotate_vec2(vec: Vec2, angle_rad: f32) -> Vec2 {
        let cos_theta = angle_rad.cos();
        let sin_theta = angle_rad.sin();

        Vec2::new(
            vec.x * cos_theta - vec.y * sin_theta,
            vec.x * sin_theta + vec.y * cos_theta,
        )
    }

    fn touch_transform(flip_x: bool, scale: f32, angle: f32, low_resolution_mode: bool) -> impl Fn(&mut Touch) {
        let vp = get_viewport();
        move |touch| {
            let p = if low_resolution_mode {
                vec2(touch.position.x / 2., touch.position.y / 2.)
            } else {
                touch.position
            };
            touch.position = vec2(
                (p.x - vp.0 as f32) / vp.2 as f32 * 2. - 1.,
                ((p.y - (vp.3 as f32 - (vp.1 + vp.3) as f32)) / vp.3 as f32 * 2. - 1.) / (vp.2 as f32 / vp.3 as f32),
            );
            if flip_x {
                touch.position.x *= -1.;
            }
            touch.position = Self::rotate_vec2(touch.position, angle);
            touch.position /= scale;
        }
    }

    pub fn get_touches(scale: f32, low_resolution_mode: bool) -> Vec<Touch> {
        TOUCHES.with(|it| {
            let guard = it.borrow();
            let tr = Self::touch_transform(false, scale, 0., low_resolution_mode);
            guard
                .0
                .iter()
                .cloned()
                .map(|mut it| {
                    tr(&mut it);
                    it
                })
                .collect()
        })
    }

    /// 「屏幕进水」：把血量归零的按键判成 MISS（AUTOPLAY 也一样 —— 所以放在
    /// `autoplay()` 早退之前）。
    fn update_water_death(&mut self, res: &mut Resource, chart: &mut Chart) {
        if !res.config.water_screen {
            return;
        }
        let t = res.time;
        for (line_id, line) in chart.lines.iter_mut().enumerate() {
            for (index, note) in line.notes.iter_mut().enumerate() {
                if note.water.pending_miss {
                    note.water.pending_miss = false;
                    note.judge = JudgeStatus::Judged;
                    self.commit(t, Judgement::Miss, line_id as u32, index as u32, 0.25);
                }
            }
        }
    }

    pub fn update(&mut self, res: &mut Resource, chart: &mut Chart, bad_notes: &mut Vec<BadNote>, angle: f32) {
        res.played_hitsounds_count.clear();
        // 「落点模拟」：落点每帧重填（`update` 末尾填），别让上一帧的留着。
        res.sim_touch_points.clear();
        self.update_water_death(res, chart);
        if res.config.autoplay() {
            if res.config.touch_sim {
                // 「落点模拟」不走 AUTOPLAY 那套无条件 Perfect：合成触点下去走真实判定，
                // 于是落点固定、长条照常被判定线带走、一个落点也可能接住别的按键。
                self.sim_fingers_update(res, chart);
            } else {
                self.auto_play_update(res, chart);
                return;
            }
        }
        let x_diff_max: f64 = if res.config.full_scrrn_judge() {
            2. / res.config.chart_ratio as f64
        } else {
            0.21 / (16. / 9.) * 2.
        };
        let spd = res.config.speed as f64;

        let uptime = get_uptime();

        let t = res.time;
        // TODO optimize
        let mut touches: HashMap<u64, Touch> = {
            let mut touches = touches();
            let btn = MouseButton::Left;
            let id = button_to_id(btn);
            if is_mouse_button_pressed(btn) {
                let p = mouse_position();
                touches.push(Touch {
                    id,
                    phase: TouchPhase::Started,
                    position: vec2(p.0, p.1),
                    time: f64::NEG_INFINITY,
                });
            } else if is_mouse_button_down(btn) {
                let p = mouse_position();
                touches.push(Touch {
                    id,
                    phase: TouchPhase::Moved,
                    position: vec2(p.0, p.1),
                    time: f64::NEG_INFINITY,
                });
            } else if is_mouse_button_released(btn) {
                let p = mouse_position();
                touches.push(Touch {
                    id,
                    phase: TouchPhase::Ended,
                    position: vec2(p.0, p.1),
                    time: f64::NEG_INFINITY,
                });
            }
            let tr = Self::touch_transform(res.config.flip_x(), res.config.chart_ratio, angle, res.config.low_resolution_mode);
            touches
                .into_iter()
                .map(|mut it| {
                    tr(&mut it);
                    (it.id, it)
                })
                .collect()
        };
        let (events, keys_down) = TOUCHES.with(|it| {
            let guard = it.borrow();
            (guard.0.clone(), guard.2)
        });
        self.key_down_count = self.key_down_count.saturating_add_signed(TOUCHES.with(|it| it.borrow().1));
        {
            fn to_local(Vec2 { x, y }: Vec2) -> Point {
                Point::new(x / screen_width() * 2. - 1., y / screen_height() * 2. - 1.)
            }
            let delta = (t / spd - self.last_time) as f64 / (events.len() + 1) as f64;
            let mut t = self.last_time as f64;
            for Touch {
                id,
                phase,
                position: p,
                time,
            } in events.into_iter()
            {
                t += delta;
                let t = t as f32;
                let p = to_local(p);
                match phase {
                    TouchPhase::Started => {
                        self.trackers.insert(id, FlickTracker::new(res.dpi, t, p));
                        touches
                            .entry(id)
                            .or_insert_with(|| Touch {
                                id,
                                phase: TouchPhase::Started,
                                position: vec2(p.x, p.y),
                                time,
                            })
                            .phase = TouchPhase::Started;
                    }
                    TouchPhase::Moved | TouchPhase::Stationary => {
                        if let Some(tracker) = self.trackers.get_mut(&id) {
                            tracker.push(t, p);
                        }
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled => {
                        self.trackers.remove(&id);
                    }
                }
            }
        }
        let mut touches: Vec<Touch> = touches
            .into_values()
            .map(|mut it| {
                it.time = if it.time.is_infinite() {
                    f64::NEG_INFINITY
                } else {
                    t as f64 - (uptime - it.time) * spd as f64
                };
                it
            })
            .collect();
        // 「噪域吞触摸」：落在 `Active` 相位噪域里的手指直接从触点列表里摘掉 ——
        // 下面的按键因此打不中（官方语义：噪域是一层禁止打击的区域），同时把它记进
        // `chart.blocked_touches`，噪域渲染会在触点位置点亮 hover 光晕。
        //
        // 官方一旦判定这根手指被噪域挡住，就一直挡到松手：中途滑出噪域也不会解除，
        // 光晕跟着手指走。所以挡住的触点是**持续**的，不能只看当前位置。
        chart.blocked_touches.clear();
        if !chart.block_areas.is_empty() || !chart.next_block_areas.is_empty() {
            let aspect = res.aspect_ratio;
            let areas = &chart.block_areas;
            let next = &chart.next_block_areas;
            let live: Vec<u64> = touches.iter().map(|touch| touch.id).collect();
            // 已经抬起来的触摸不会再走到下面那趟 retain，这里顺手清掉它的挡标记
            self.blocked_finger_ids.retain(|id| live.contains(id));
            let mut blocked = Vec::new();
            touches.retain(|touch| {
                // 触点 y 轴与 chart 相反（触摸是屏幕坐标，chart 的 y 向上）
                let p = Vector::new(touch.position.x, -touch.position.y);
                // 已经被挡住的触点不再重复测试（它可能已经滑出噪域），仍然算挡着
                let inside = !self.blocked_finger_ids.contains(&touch.id) && block_touch_blocked(areas, next, p, t, aspect);
                if finger_blocked(&mut self.blocked_finger_ids, touch.id, touch.phase, inside) {
                    blocked.push((touch.id, p));
                    false
                } else {
                    true
                }
            });
            chart.blocked_touches = blocked;
        }
        // 「落点模拟」：把合成的手指塞进触点列表，后面完全按真实游玩判定。
        // 放在噪域过滤之后 —— 模拟不跟噪域联动。
        if res.config.touch_sim {
            for finger in &self.sim_fingers {
                touches.push(Touch {
                    id: finger.id,
                    phase: finger.phase,
                    position: finger.pos,
                    time: f64::NEG_INFINITY,
                });
            }
        }
        // pos[line][touch]
        let mut pos = Vec::<Vec<Option<Point>>>::with_capacity(chart.lines.len());
        for id in 0..chart.lines.len() {
            chart.lines[id].object.set_time(t);
            let inv = chart.lines[id].now_transform(res, &chart.lines).try_inverse().unwrap();
            pos.push(
                touches
                    .iter()
                    .map(|touch| {
                        let p = touch.position;
                        let p = inv.transform_point(&Point::new(p.x, -p.y));
                        fn ok(f: f32) -> bool {
                            matches!(f.classify(), FpCategory::Zero | FpCategory::Subnormal | FpCategory::Normal)
                        }
                        if ok(p.x) && ok(p.y) {
                            Some(p)
                        } else {
                            None
                        }
                    })
                    .collect(),
            );
        }
        let time_of = |touch: &Touch| {
            if touch.time.is_infinite() {
                t
            } else {
                touch.time
            }
        };
        let mut judgements = Vec::new();
        // clicks & flicks
        for (id, touch) in touches.iter().enumerate() {
            let click = touch.phase == TouchPhase::Started;
            // 「落点模拟」的合成手指没有真实滑动轨迹（`trackers` 只从系统触摸事件里建），
            // 所以它按住期间的 Moved 直接当作一次滑动，让它按真实规则接住滑键。
            let sim_finger = res.config.touch_sim && self.sim_fingers.iter().any(|it| it.id == touch.id);
            let flick = matches!(touch.phase, TouchPhase::Moved | TouchPhase::Stationary)
                && (self.trackers.get_mut(&touch.id).map_or(false, |it| it.flicked) || sim_finger);
            if !(click || flick) {
                continue;
            }
            let t = time_of(touch);
            let mut closest = (None, x_diff_max, LIMIT_BAD, LIMIT_BAD + (x_diff_max / NOTE_WIDTH_RATIO_BASE - 1.).max(0.) * DIST_FACTOR, 0.);
            for (line_id, ((line, pos), (idx, st))) in chart.lines.iter_mut().zip(pos.iter()).zip(self.notes.iter_mut()).enumerate() {
                let Some(pos) = pos[id] else { continue; };
                for id in &idx[*st..] {
                    let note = &mut line.notes[*id as usize];
                    if !matches!(note.judge, JudgeStatus::NotJudged | JudgeStatus::PreJudge) {
                        continue;
                    }
                    if !click && matches!(note.kind, NoteKind::Click | NoteKind::Hold { .. }) {
                        continue;
                    }
                    let dt = (note.time - t) / spd;
                    if dt.abs() >= closest.3.abs() {
                        break;
                    }
                    // let dt = if dt < 0. { (dt + EARLY_OFFSET).min(0.).abs() } else { dt };
                    let x = &mut note.object.translation.0;
                    x.set_time(t);
                    let posx = pos.x;
                    let dist = (x.now() - posx).abs() as f64;
                    // 判定半宽：默认按按键宽度算；「落点宽度」填了就固定用那个值。
                    let judge_half = res.config.touch_sim_half().unwrap_or((x_diff_max - NOTE_WIDTH_RATIO_BASE) + NOTE_WIDTH_RATIO_BASE * note.judge_scale);
                    if dist > judge_half {
                        continue;
                    }
                    if dt.abs() >
                        if matches!(note.kind, NoteKind::Click) {
                            LIMIT_BAD // LIMIT_BAD - LIMIT_PERFECT * (dist - 0.9).max(0.)
                        } else {
                            LIMIT_GOOD
                        }
                    {
                        continue;
                    }
                    let dist_key = if res.config.full_scrrn_judge() {
                        (dist / NOTE_WIDTH_RATIO_BASE - 1.).max(0.) * 0.01
                    } else {
                        (dist / NOTE_WIDTH_RATIO_BASE - 1.).max(0.) * DIST_FACTOR
                    };
                    let key = if matches!(note.kind, NoteKind::Flick | NoteKind::Drag) { // Low Priority
                        dt.abs() + LIMIT_BAD
                    } else if dt < -LIMIT_GOOD { // Prevent Late Bad
                        dt.abs()
                    } else if dt < 0.0 {
                        (dt + LATE_OFFSET).min(0.0).abs() // Protect Late Good
                    } else {
                        dt.abs()
                    };
                    let key = key + dist_key;
                    if key < closest.3 {
                        closest = (Some((line_id, *id)), dist, dt, key, posx);
                    }
                }
            }
            if let (Some((line_id, id)), _, dt, _, posx) = closest {
                let can_protect_note = |note: &mut Note| {
                    let x = &mut note.object.translation.0;
                    x.set_time(t);
                    let judge_time = t - note.time;
                    matches!(note.kind, NoteKind::Drag | NoteKind::Flick)
                        && judge_time >= -LIMIT_GOOD
                        && judge_time <= LIMIT_BAD
                        && (x.now() - posx).abs() as f64 <= (x_diff_max - NOTE_WIDTH_RATIO_BASE) + NOTE_WIDTH_RATIO_BASE * note.judge_scale // note_dist <= x_diff_max
                        && !note.protected
                        && !note.fake
                };
                let lines = &mut chart.lines;
                if matches!(lines[line_id].notes[id as usize].kind, NoteKind::Drag) {
                    // debug!("reject by drag");
                    continue;
                }
                if click {
                    if dt > LIMIT_PERFECT {
                        let mut any = false;
                        lines.iter_mut()
                            .flat_map(|line| line.notes.iter_mut())
                            .for_each(|note| {
                                if can_protect_note(note) {
                                    note.protected = true;
                                    any = true;
                                }
                            });
                        if any {
                            continue;
                        }
                    }
                    // click & hold
                    let note = &mut lines[line_id].notes[id as usize];
                    let dt = dt.abs();
                    // 「落点模拟」下把滑键当点击判：固定落点不会滑动，滑动手势永远凑不出来。
                    if matches!(note.kind, NoteKind::Flick) && !res.config.touch_sim {
                        // debug!("reject by flick");
                        continue; // to next loop
                    }
                    if dt <= LIMIT_GOOD || matches!(note.kind, NoteKind::Hold { .. }) {
                        match note.kind {
                            NoteKind::Click | NoteKind::Flick => {
                                note.judge = JudgeStatus::Judged;
                                judgements.push((if dt <= LIMIT_PERFECT { Judgement::Perfect } else { Judgement::Good }, line_id, id, Some(t)));
                                #[cfg(feature = "play")]
                                if res.config.health_mode.is_some() {
                                    res.health.on_judge(Judgement::Perfect);
                                }
                            }
                            NoteKind::Hold { .. } => {
                                play_sfx(&mut res.sfx_click, &res.config);
                                self.judgements.borrow_mut().push((t, line_id as _, id, Err(dt <= LIMIT_PERFECT)));
                                note.judge = JudgeStatus::Hold(dt <= LIMIT_PERFECT, t, t, false, f64::INFINITY);
                            }
                            _ => unreachable!(),
                        };
                    } else {
                        // prevent extra judgements
                        if matches!(note.judge, JudgeStatus::NotJudged) {
                            // keep the note after bad judgement
                            note.judge = JudgeStatus::PreJudge;
                            judgements.push((Judgement::Bad, line_id, id, None));
                            #[cfg(feature = "play")]
                            if res.config.health_mode.is_some() {
                                res.health.on_judge(Judgement::Bad);
                            }
                        }
                    }
                } else {
                    // flick
                    lines[line_id].notes[id as usize].judge = JudgeStatus::PreJudge;
                    if let Some(tracker) = self.trackers.get_mut(&touch.id) {
                        tracker.flicked = false;
                    }
                }
            }
        }
        for _ in 0..keys_down {
            // find the earliest not judged click / hold note
            if let Some((line_id, id)) = chart
                .lines
                .iter()
                .zip(self.notes.iter())
                .enumerate()
                .filter_map(|(line_id, (line, (idx, st)))| {
                    idx[*st..]
                        .iter()
                        .cloned()
                        .find(|id| {
                            let note = &line.notes[*id as usize];
                            matches!(note.judge, JudgeStatus::NotJudged) && matches!(note.kind, NoteKind::Click | NoteKind::Hold { .. })
                        })
                        .map(|id| (line_id, id))
                })
                .min_by_key(|(line_id, id)| chart.lines[*line_id].notes[*id as usize].time.not_nan())
            {
                let note = &mut chart.lines[line_id].notes[id as usize];
                let dt = (t - note.time).abs() / spd;
                if dt <= if matches!(note.kind, NoteKind::Click) { LIMIT_BAD } else { LIMIT_GOOD } {
                    match note.kind {
                        NoteKind::Click => {
                            note.judge = JudgeStatus::Judged;
                            let judge = if dt <= LIMIT_PERFECT {
                                    Judgement::Perfect
                                } else if dt <= LIMIT_GOOD {
                                    Judgement::Good
                                } else {
                                    Judgement::Bad
                                };
                            judgements.push((judge, line_id, id, None));
                            #[cfg(feature = "play")]
                            if res.config.health_mode.is_some() {
                                res.health.on_judge(judge);
                            }
                        }
                        NoteKind::Hold { .. } => {
                            if note.water.death_total <= 0. {
                                note.hitsound.play(res);
                            } else {
                                eprintln!(
                                    "[water] 已溺水死亡的按键不再播放打击音效 note_t={:.3}",
                                    note.time
                                );
                            }
                            self.judgements.borrow_mut().push((t, line_id as _, id, Err(dt <= LIMIT_PERFECT)));
                            note.judge = JudgeStatus::Hold(dt <= LIMIT_PERFECT, t, (t - note.time) / spd, false, f64::INFINITY);
                        }
                        _ => unreachable!(),
                    };
                }
            } else {
                break;
            }
        }
        for (line_id, ((line, pos), (idx, st))) in chart.lines.iter_mut().zip(pos.iter()).zip(self.notes.iter()).enumerate() {
            line.object.set_time(t);
            for id in &idx[*st..] {
                let note = &mut line.notes[*id as usize];
                let x_diff_max = res.config.touch_sim_half().unwrap_or((x_diff_max - NOTE_WIDTH_RATIO_BASE) + NOTE_WIDTH_RATIO_BASE * note.judge_scale as f64);
                if let NoteKind::Hold { end_time, .. } = &note.kind {
                    if let JudgeStatus::Hold(.., ref mut pre_judge, ref mut up_time) = note.judge {
                        if (*end_time - t) / spd <= LIMIT_BAD {
                            *pre_judge = true;
                            continue;
                        }
                        let x = &mut note.object.translation.0;
                        x.set_time(t);
                        let x = x.now();
                        if self.key_down_count == 0 && !pos.iter().any(|it| it.map_or(false, |it| (it.x - x).abs() <= x_diff_max as f32)) {
                            if t > *up_time + UP_TOLERANCE {
                                note.judge = JudgeStatus::Judged;
                                judgements.push((Judgement::Miss, line_id, *id, None));
                                #[cfg(feature = "play")]
                                if res.config.health_mode.is_some() {
                                    res.health.on_judge(Judgement::Miss);
                                }
                            } else if up_time.is_infinite() {
                                *up_time = t;
                            }
                        } else {
                            *up_time = f64::INFINITY;
                        }
                        continue;
                    }
                }
                if !matches!(note.judge, JudgeStatus::NotJudged) {
                    continue;
                }
                // process miss
                let dt = (t - note.time) / spd;
                if dt > LIMIT_BAD {
                    // 「落点模拟」：渲染窗口开始（`play_start_time`）之前的按键真实玩法也按不到，
                    // 按 AUTOPLAY 的惯例记 Perfect，否则一开录就是整片 Miss。
                    if res.config.touch_sim && note.time <= res.config.play_start_time {
                        note.judge = JudgeStatus::Judged;
                        judgements.push((Judgement::Perfect, line_id, *id, None));
                        continue;
                    }
                    note.judge = JudgeStatus::Judged;
                    judgements.push((Judgement::Miss, line_id, *id, None));
                    #[cfg(feature = "play")]
                    if res.config.health_mode.is_some() {
                        res.health.on_judge(Judgement::Miss);
                    }
                    continue;
                }
                if -dt > LIMIT_BAD {
                    break;
                }
                // 滑键只认“手指按在屏幕上经过它”，真实输入靠 `key_down_count`；「落点模拟」的合成
                // 手指不进那个计数，所以模拟时一律当作按着。
                let finger_down = self.key_down_count != 0 || res.config.touch_sim;
                if !matches!(note.kind, NoteKind::Drag) && (!finger_down || !matches!(note.kind, NoteKind::Flick)) {
                    continue;
                }
                let dt = dt.abs();
                let x = &mut note.object.translation.0;
                x.set_time(t);
                let x = x.now();
                if finger_down
                    || pos.iter().any(|it| {
                        it.map_or(false, |it| {
                            let dx = (it.x - x).abs() as f64;
                            dx <= x_diff_max && dt <= (LIMIT_BAD - LIMIT_PERFECT * (dx - 0.9).max(0.))
                        })
                    })
                {
                    note.judge = JudgeStatus::PreJudge;
                }
            }
        }
        // process pre-judge
        for (line_id, (line, (idx, st))) in chart.lines.iter_mut().zip(self.notes.iter()).enumerate() {
            line.object.set_time(t);
            for id in &idx[*st..] {
                let note = &mut line.notes[*id as usize];
                if let JudgeStatus::Hold(perfect, .., diff, true, _) = note.judge {
                    if let NoteKind::Hold { end_time, .. } = &note.kind {
                        if *end_time <= t {
                            note.judge = JudgeStatus::Judged;
                            let judge = if perfect { Judgement::Perfect } else { Judgement::Good };
                            judgements.push((judge, line_id, *id, Some(diff)));
                            #[cfg(feature = "play")]
                            if res.config.health_mode.is_some() {
                                res.health.on_judge(judge);
                            }
                            continue;
                        }
                    }
                }
                // TODO adjust
                let ghost_t = t + LIMIT_GOOD;
                if matches!(note.kind, NoteKind::Click) {
                    if ghost_t < note.time {
                        break;
                    }
                } else if t < note.time {
                    continue;
                }
                if matches!(note.judge, JudgeStatus::PreJudge) {
                    let diff = if let JudgeStatus::Hold(.., diff, _, _) = note.judge {
                        Some(diff)
                    } else {
                        None
                    };
                    note.judge = JudgeStatus::Judged;
                    if !matches!(note.kind, NoteKind::Click) {
                        judgements.push((Judgement::Perfect, line_id, *id, diff));
                        #[cfg(feature = "play")]
                        if res.config.health_mode.is_some() {
                            res.health.on_judge(Judgement::Perfect);
                        }
                    }
                }
            }
        }
        for (judgement, line_id, id, diff) in judgements {
            let line = &mut chart.lines[line_id];
            let note = &mut line.notes[id as usize];
            line.object.set_time(t);
            note.object.set_time(t);
            let line = &chart.lines[line_id];
            let note = &line.notes[id as usize];
            let mut note_transform = note.object.now(res);
            if !note.above {
                note_transform.append_nonuniform_scaling_mut(&Vector::new(1.0, -1.0));
            }
            let line_tr = line.now_transform(res, &chart.lines);
            self.commit(
                t,
                judgement,
                line_id as _,
                id,
                if matches!(judgement, Judgement::Miss) {
                    0.25
                } else if matches!(note.kind, NoteKind::Drag | NoteKind::Flick) {
                    0.
                } else {
                    (diff.unwrap_or(t) - note.time) / spd
                },
            );
            if matches!(note.kind, NoteKind::Hold { .. }) {
                continue;
            }
            if match judgement {
                Judgement::Perfect => {
                    let color = note
                        .hit_fx_color
                        .now_opt()
                        .unwrap_or_else(|| Note::hit_fx_color(res, true));
                    res.with_model(line_tr * note_transform, |res| res.emit_at_origin(note.rotation(line), color));
                    true
                }
                Judgement::Good => {
                    let color = note
                        .hit_fx_color
                        .now_opt()
                        .unwrap_or_else(|| Note::hit_fx_color(res, false));
                    res.with_model(line_tr * note_transform, |res| res.emit_at_origin(note.rotation(line), color));
                    true
                }
                Judgement::Bad => {
                    if !matches!(note.kind, NoteKind::Hold { .. }) {
                        bad_notes.push(BadNote {
                            time: t,
                            kind: note.kind.clone(),
                            matrix: {
                                let incline_sin = line.incline.now_opt().map(|it| it.to_radians().sin()).unwrap_or_default();
                                let mut note_transform = note.now_transform(
                                    res,
                                    &line.ctrl_obj.borrow_mut(),
                                    (note.height - line.height.now()) as f32 / res.aspect_ratio * note.speed as f32,
                                    incline_sin,
                                    true, true
                                );
                                if !note.above {
                                    note_transform.append_nonuniform_scaling_mut(&Vector::new(1.0, -1.0));
                                }
                                line_tr * note_transform
                            },
                        });
                    }
                    false
                }
                _ => false,
            } {
                if note.water.death_total <= 0. {
                    note.hitsound.play(res);
                } else {
                    eprintln!(
                        "[water] 已溺水死亡的按键不再播放打击音效 note_t={:.3}",
                        note.time
                    );
                }
            }
        }
        for (line, (idx, st)) in chart.lines.iter().zip(self.notes.iter_mut()) {
            while idx
                .get(*st)
                .map_or(false, |id| matches!(line.notes[*id as usize].judge, JudgeStatus::Judged))
            {
                *st += 1;
            }
        }
        // 「落点模拟」：抬完的手指摘掉，剩下的落点交给渲染（`Chart::render` 画在按键之上）。
        // 手指锚定的按键被判成 Miss / 已经结束时落点转红。
        for finger in self.sim_fingers.iter_mut() {
            if matches!(finger.phase, TouchPhase::Ended) {
                finger.retired = true;
            }
        }
        if res.config.touch_sim {
            res.sim_touch_points.extend(self.sim_fingers.iter().filter(|it| !it.retired).map(|it| {
                let (x, y, half) = it.chart;
                let anchor = &chart.lines[it.anchor.0 as usize].notes[it.anchor.1 as usize];
                (x, y, half, matches!(anchor.judge, JudgeStatus::Judged))
            }));
        }
        self.last_time = t / spd;
    }

    fn auto_play_update(&mut self, res: &mut Resource, chart: &mut Chart) {
        let t = res.time - res.config.judge_offset;
        let perfect_fx = Note::hit_fx_color(res, true);
        let good_fx = Note::hit_fx_color(res, false);
        let (judge_type, judge_type_hold, judge_time, fx_color) = if res.config.all_bad {
            (Judgement::Bad, Judgement::Good, LIMIT_BAD, Color::new(0., 0., 0., 0.))
        } else if res.config.all_good {
            (Judgement::Good, Judgement::Good, LIMIT_GOOD, good_fx)
        } else {
            (Judgement::Perfect, Judgement::Perfect, 0., perfect_fx)
        };
        //let spd = res.config.speed;
        let mut judgements = Vec::new();
        for (line_id, (line, (idx, st))) in chart.lines.iter_mut().zip(self.notes.iter_mut()).enumerate() {
            for id in &idx[*st..] {
                let note = &mut line.notes[*id as usize];
                if let JudgeStatus::Hold(..) = note.judge {
                    if let NoteKind::Hold { end_time, .. } = note.kind {
                        if t >= end_time {
                            note.judge = JudgeStatus::Judged;
                            judgements.push((line_id, *id));
                            #[cfg(feature = "play")]
                            if res.config.health_mode.is_some() {
                                res.health.on_judge(Judgement::Perfect);
                            }
                            continue;
                        }
                    }
                }
                if !matches!(note.judge, JudgeStatus::NotJudged) {
                    continue;
                }
                if note.time > t {
                    break;
                }
                note.judge = if matches!(note.kind, NoteKind::Hold { .. }) {
                    if note.time >= res.config.play_start_time && !res.disable_hit_fx {
                        if note.water.death_total <= 0. {
                            note.hitsound.play(res);
                        } else {
                            eprintln!(
                                "[water] 已溺水死亡的按键不再播放打击音效 note_t={:.3}",
                                note.time
                            );
                        }
                    }
                    self.judgements.borrow_mut().push((t, line_id as _, *id, Err(true)));
                    // AutoPlay 无需输出打击时间差
                    // JudgeStatus::Hold(true, t, (t - note.time) / spd, false, f32::INFINITY)
                    JudgeStatus::Hold(true, t, judge_time, true, f64::INFINITY)
                } else {
                    judgements.push((line_id, *id));
                    #[cfg(feature = "play")]
                    if res.config.health_mode.is_some() {
                        res.health.on_judge(Judgement::Perfect);
                    }
                    JudgeStatus::Judged
                };
            }
            while idx
                .get(*st)
                .map_or(false, |id| matches!(line.notes[*id as usize].judge, JudgeStatus::Judged))
            {
                *st += 1;
            }
        }
        for (line_id, id) in judgements.into_iter() {
            let mut note_transform = {
                let line = &mut chart.lines[line_id];
                let note = &mut line.notes[id as usize];
                let nt = if matches!(note.kind, NoteKind::Hold { .. }) { t } else { note.time };
                line.object.set_time(nt);
                note.object.set_time(nt);
                note.object.now(res)
            };
            let line = &chart.lines[line_id];
            let note = &line.notes[id as usize];
            if !note.above {
                note_transform.append_nonuniform_scaling_mut(&Vector::new(1.0, -1.0));
            }
            match note.kind {
                NoteKind::Click => {
                    let color = if let Some(color) = note.hit_fx_color.now_opt() {
                        color
                    } else {
                        fx_color
                    };
                    self.commit(t, judge_type, line_id as _, id, 0.);
                    if note.time >= res.config.play_start_time && !res.disable_hit_fx {
                        res.with_model(line.now_transform(res, &chart.lines) * note_transform, |res| {
                            res.emit_at_origin(note.rotation(line), color)
                        });
                        if !res.config.all_bad {
                            if note.water.death_total <= 0. {
                                note.hitsound.play(res)
                            } else {
                                eprintln!(
                                    "[water] 已溺水死亡的按键不再播放打击音效 note_t={:.3}",
                                    note.time
                                );
                            }
                        }
                    }
                }
                NoteKind::Hold { .. } => {
                    self.commit(t, judge_type_hold, line_id as _, id, 0.);
                }
                _ => {
                    let color = if let Some(color) = note.hit_fx_color.now_opt() {
                        color
                    } else {
                        fx_color
                    };
                    self.commit(t, Judgement::Perfect, line_id as _, id, 0.);
                    if note.time >= res.config.play_start_time && !res.disable_hit_fx {
                        res.with_model(line.now_transform(res, &chart.lines) * note_transform, |res| {
                            res.emit_at_origin(note.rotation(line), color)
                        });
                        if note.water.death_total <= 0. {
                            note.hitsound.play(res)
                        } else {
                            eprintln!(
                                "[water] 已溺水死亡的按键不再播放打击音效 note_t={:.3}",
                                note.time
                            );
                        }
                    }
                },
            };
        }
    }

    /// 「落点模拟」：替这一帧到点、还没被按过的按键各合成一根「自动点击的手指」。
    ///
    /// 落点在按下那一刻由按键中心算出，之后钉在屏幕上不再移动；手指按住期间由 `update` 塞进
    /// 真实判定的触点列表，判定完全复用真实游玩那条链路 —— 所以长条被判定线带走会断连，而
    /// 别的按键被带到这根手指底下时也能被它接住。
    fn sim_fingers_update(&mut self, res: &mut Resource, chart: &mut Chart) {
        let t = res.time;
        let x_diff_max: f64 = if res.config.full_scrrn_judge() {
            2. / res.config.chart_ratio as f64
        } else {
            0.21 / (16. / 9.) * 2.
        };
        // 上一帧已经抬完的摘掉。
        self.sim_fingers.retain(|it| !it.retired);
        // 手上的手指推进相位：到点就抬，否则第一帧之后一直是 Moved。
        for finger in self.sim_fingers.iter_mut() {
            finger.phase = if t >= finger.release {
                TouchPhase::Ended
            } else if matches!(finger.phase, TouchPhase::Started) {
                TouchPhase::Moved
            } else {
                finger.phase
            };
        }
        for line in chart.lines.iter_mut() {
            line.object.set_time(t);
        }
        // 这一帧该按下的按键。
        let mut starts = Vec::new();
        for (line_id, line) in chart.lines.iter().enumerate() {
            for (note_id, note) in line.notes.iter().enumerate() {
                if note.fake || note.time > t {
                    continue;
                }
                if !matches!(note.judge, JudgeStatus::NotJudged | JudgeStatus::PreJudge) {
                    continue;
                }
                let key = (line_id as u32, note_id as u32);
                if !self.sim_pressed.contains(&key) {
                    starts.push(key);
                }
            }
        }
        for (line_id, note_id) in starts {
            let (rel, scale, note_time, release) = {
                let note = &mut chart.lines[line_id as usize].notes[note_id as usize];
                note.object.set_time(t);
                let release = match note.kind {
                    // 长条一直按到 HOLDTIME 结束（断连也不提前松手）。
                    NoteKind::Hold { end_time, .. } => end_time,
                    _ => note.time + 1e-3,
                };
                (note.object.now(res), note.judge_scale as f64, note.time, release)
            };
            let chart_pos = {
                let line_tr = chart.lines[line_id as usize].now_transform(res, &chart.lines);
                (line_tr * rel).transform_point(&Point::default())
            };
            // 判定半宽：默认与真实判定同一套（`(x_diff_max - NOTE_WIDTH) + NOTE_WIDTH * scale`），
            // 也就是判定线把按键带离多远会断连 —— 画出来的方框就是这个范围；「落点宽度」填了就用它。
            let half = res
                .config
                .touch_sim_half()
                .map_or(((x_diff_max - NOTE_WIDTH_RATIO_BASE) + NOTE_WIDTH_RATIO_BASE * scale) as f32, |it| it as f32);
            self.sim_fingers.push(SimFinger {
                id: SIM_FINGER_ID_BASE | self.sim_finger_seq,
                // `Touch::position` 的 y 轴与谱面相反。
                pos: vec2(chart_pos.x, -chart_pos.y),
                chart: (chart_pos.x, chart_pos.y, half),
                anchor: (line_id, note_id),
                release: release.max(note_time),
                phase: TouchPhase::Started,
                retired: false,
            });
            self.sim_finger_seq += 1;
            self.sim_pressed.insert((line_id, note_id));
        }
    }


    pub fn commit_all(&mut self, chart: &mut Chart) {
        for _ in chart.lines.iter()
            .flat_map(|it| it.notes.iter())
            .filter(|it| !it.fake && matches!(it.judge, JudgeStatus::NotJudged | JudgeStatus::PreJudge))
        {
            self.commit(0., Judgement::Perfect, 0, 0, 0.);
        }
    }

    #[inline]
    pub fn result(&self) -> PlayResult {
        self.inner.result()
    }

    #[inline]
    pub fn combo(&self) -> u32 {
        self.inner.combo()
    }

    #[inline]
    pub fn counts(&self) -> [u32; 4] {
        self.inner.counts()
    }
}

struct Handler(Vec<Touch>, i32, u32);
impl Handler {
    fn finalize(&mut self) {
        if is_mouse_button_down(MouseButton::Left) {
            self.0.push(Touch {
                id: button_to_id(MouseButton::Left),
                phase: TouchPhase::Moved,
                position: mouse_position().into(),
                time: f64::NEG_INFINITY,
            });
        }
    }
}

/// 操作键（空格、方向键、回车 / 退格 / Esc / Tab、修饰键、F1~F25、编辑与锁定键）不算打击输入。
///
/// 引擎把任意按键的按下都计进「手指按下数」（`Handler.1` / `Handler.2`），判定那边就当成一次
/// 全屏触摸：游玩时按空格暂停、按方向键跳转，都会顺带打中按键。这些键只用来操作，按下不该
/// 产生触摸，所以在按下 / 松开两处一起跳过（两处必须一致，否则按下数会算错）。
fn is_function_key(keycode: KeyCode) -> bool {
    use KeyCode::*;
    matches!(
        keycode,
        Space
            | Escape
            | Enter
            | KpEnter
            | Tab
            | Backspace
            | Insert
            | Delete
            | Right
            | Left
            | Down
            | Up
            | PageUp
            | PageDown
            | Home
            | End
            | CapsLock
            | ScrollLock
            | NumLock
            | PrintScreen
            | Pause
            | F1
            | F2
            | F3
            | F4
            | F5
            | F6
            | F7
            | F8
            | F9
            | F10
            | F11
            | F12
            | F13
            | F14
            | F15
            | F16
            | F17
            | F18
            | F19
            | F20
            | F21
            | F22
            | F23
            | F24
            | F25
            | LeftShift
            | LeftControl
            | LeftAlt
            | LeftSuper
            | RightShift
            | RightControl
            | RightAlt
            | RightSuper
    )
}

fn button_to_id(button: MouseButton) -> u64 {
    u64::MAX
        - match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::Unknown => 3,
        }
}

impl EventHandler for Handler {
    fn update(&mut self, _: &mut miniquad::Context) {}
    fn draw(&mut self, _: &mut miniquad::Context) {}
    fn touch_event(&mut self, _: &mut miniquad::Context, phase: miniquad::TouchPhase, id: u64, x: f32, y: f32, time: f64) {
        self.0.push(Touch {
            id,
            phase: phase.into(),
            position: vec2(x, y),
            time,
        });
    }

    fn mouse_button_down_event(&mut self, _ctx: &mut miniquad::Context, button: MouseButton, x: f32, y: f32) {
        self.0.push(Touch {
            id: button_to_id(button),
            phase: TouchPhase::Started,
            position: vec2(x, y),
            time: f64::NEG_INFINITY,
        });
    }

    fn mouse_button_up_event(&mut self, _ctx: &mut miniquad::Context, button: MouseButton, x: f32, y: f32) {
        self.0.push(Touch {
            id: button_to_id(button),
            phase: TouchPhase::Ended,
            position: vec2(x, y),
            time: f64::NEG_INFINITY,
        });
    }

    fn key_down_event(&mut self, _ctx: &mut miniquad::Context, keycode: KeyCode, _keymods: miniquad::KeyMods, repeat: bool) {
        if !repeat && !is_function_key(keycode) {
            self.1 += 1;
            self.2 += 1;
        }
    }

    fn key_up_event(&mut self, _ctx: &mut miniquad::Context, keycode: KeyCode, _keymods: miniquad::KeyMods) {
        if !is_function_key(keycode) {
            self.1 -= 1;
        }
    }
}

#[derive(Default)]
pub struct PlayResult {
    pub score: f64,
    pub accuracy: f64,
    pub max_combo: u32,
    pub num_of_notes: u32,
    pub counts: [u32; 4],
    pub early: u32,
    pub late: u32,
    pub std: f32,
}

pub fn icon_index(score: u32, full_combo: bool) -> usize {
    match (score, full_combo) {
        (x, _) if x >= 1000000 => 0,
        (_, true) => 1,
        (x, _) if x < 700000 => 7,
        (x, _) if x < 820000 => 6,
        (x, _) if x < 880000 => 5,
        (x, _) if x < 920000 => 4,
        (x, _) if x < 960000 => 3,
        (_, false) => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 「噪域挡住一根手指就挡到松手」：滑出噪域不解除，抬起 / 取消才解除；
    /// 复用的手指 id 不带旧标记。
    #[test]
    fn blocked_finger_persists_until_release() {
        use TouchPhase::*;
        let mut blocked = FxHashSet::default();
        assert!(finger_blocked(&mut blocked, 7, Started, true), "按下时落在噪域里");
        assert!(finger_blocked(&mut blocked, 7, Stationary, false), "滑出噪域仍然挡着");
        assert!(finger_blocked(&mut blocked, 7, Moved, false), "继续按着仍然挡着");
        assert!(!finger_blocked(&mut blocked, 8, Started, false), "另一根手指没被挡");
        assert!(!finger_blocked(&mut blocked, 7, Ended, true), "抬起解除");
        assert!(!finger_blocked(&mut blocked, 7, Started, false), "复用的 id 不带旧标记");
        assert!(finger_blocked(&mut blocked, 7, Moved, true), "重新按进噪域又挡住");
        assert!(!finger_blocked(&mut blocked, 7, Cancelled, true), "取消解除");
    }
}