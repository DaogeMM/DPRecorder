//! 「谱面修改」：纯配置谱面、定轨谱面，把谱面本身改出花样的按键替换、随机按键与流速、长条填充、
//! 长条延长、复制音符、随机位置、陈列按键（后七个与谱面修改工具 modify 里的同名功能一一对应），
//! 以及「隐藏按键」这一项自己的东西（modify 里没有对应功能）。
//!
//! 这些都同一层：谱面文件不动，只改内存里刚解析出来的判定线与按键，所以预览、渲染、分屏走的
//! 是同一份转换，入口只有一个 —— [`crate::core::Chart::apply_chart_mods`]。
//!
//! 按键那一层还可以同时开多个，**落地顺序就是界面上勾选的先后**（配置里记在 `chart_mod_order`）：
//! 顺序会改结果，例如先「复制音符」再「长条填充」是拆出来的按键各复制一份，反过来则是复制出来的
//! 长按各自被拆。没在 `chart_mod_order` 里提到的功能排在后面，按界面行序 —— 老的 config.toml
//! 没有这一项，照样能跑。
//!
//! 四类**互斥**（[`Mods::from_config`] 里兜底）：纯配置谱面 / 定轨谱面 / 隐藏按键 / 按键那一层，
//! 同一时刻只让一类生效。前三类各自把整张谱面重排一遍，和别的类叠在一起只会互相覆盖；界面按同一
//! 规则联动开关，手改 config.toml 同时开了几类时按界面行序取最靠前的那一类。
//!
//! 其中「纯配置谱面」「定轨谱面」改判定线布局、「陈列按键」会把整张谱面收到一条基准线上，这三项由
//! [`crate::core::Chart::apply_chart_mods`] 自己落地；其余各项只动按键自己，走 [`apply_step`]。
//!
//! 与 modify 的差别有三处：
//!
//! * 那边的时间是「1/32 拍」的整数 tick，这里一律是秒，只在长条填充按 `BpmList` 现算 tick；
//! * 那边遇到贴图判定线（带 `Texture`）就整条跳过，这里由 [`JudgeLineKind`] 直接判断，贴图 / 文字 /
//!   画笔判定线以及挂界面元素的判定线一律不动 —— 它们承载的是画面元素，不是谱面；
//! * 那边把 `holdTime` 为 0 的长按原样写回谱面，这里换成长按时要补一小截，否则按键根本看不见。

use crate::{
    config::Config,
    core::{
        fixed_lane::Lanes, pure_config::SideLines, Anim, AnimFloat, AnimFloatF64, AnimVector, BpmList, JudgeLine, JudgeLineCache, JudgeLineKind, Keyframe, Note,
        NoteKind, Object, StaticTween, Vector,
    },
    judge::{HitSound, JudgeStatus},
};
use rand::Rng;
use rand_pcg::{rand_core::SeedableRng, Pcg32};

/// 1/32 拍在 1 BPM 下的时长（秒）：谱面修改工具 modify 里的 `TIME_BASE`，`60 / 32`。
const TIME_BASE: f64 = 60. / 32.;
/// 随机类功能的固定种子：同一张谱面每次渲染出来的随机结果一致，成片可复现。
const RANDOM_SEED: u64 = 0x5EED_CAFE;
/// 把点按 / 拖动 / 滑动换成 HOLD 时补的长度（1/32 拍）。modify 那边这类按键的 `holdTime` 是 0，
/// 写回谱面后只剩一根看不见的线，换成长按之后等于没换。
const CONVERT_HOLD_TICKS: f64 = 4.;

/// 目标键型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NoteType {
    Tap,
    Drag,
    Hold,
    Flick,
}

impl NoteType {
    /// 配置里存的是键型名字（`tap` / `drag` / `hold` / `flick`），认不出来的按点按算。
    pub fn from_name(name: &str) -> Self {
        match name {
            "drag" => Self::Drag,
            "hold" => Self::Hold,
            "flick" => Self::Flick,
            _ => Self::Tap,
        }
    }

    /// 这个按键本来就是不是这个键型（是就不用改，长按更不该被截短）。
    fn matches_kind(self, kind: &NoteKind) -> bool {
        matches!(
            (kind, self),
            (NoteKind::Click, Self::Tap) | (NoteKind::Drag, Self::Drag) | (NoteKind::Flick, Self::Flick) | (NoteKind::Hold { .. }, Self::Hold)
        )
    }
}

/// 按键替换：四种源键型各自换成哪种键型（换成自己 = 这种按键不动）。
#[derive(Clone, Copy)]
pub struct Replace {
    /// `TAP` 换成哪种键型。
    pub tap: NoteType,
    /// `DRAG` 换成哪种键型。
    pub drag: NoteType,
    /// `HOLD` 换成哪种键型。
    pub hold: NoteType,
    /// `FLICK` 换成哪种键型。
    pub flick: NoteType,
}

impl Replace {
    /// 这个按键该换成哪种键型。
    fn target(&self, kind: &NoteKind) -> NoteType {
        match kind {
            NoteKind::Click => self.tap,
            NoteKind::Drag => self.drag,
            NoteKind::Hold { .. } => self.hold,
            NoteKind::Flick => self.flick,
        }
    }
}

/// 长条填充的参数。
pub struct Spread {
    /// 拆出来的按键换成哪种键型。
    pub note_type: NoteType,
    /// 拆分间隔，单位是 1/32 拍（谱面修改工具里的一「tick」）。
    pub interval: f64,
}

/// 陈列按键的参数：所有按键收到一条基准线上的四个站位上。
pub struct Arrange {
    /// 四条站位的横向间隔（世界单位，半个屏幕宽是 1）：站位移在 `±gap` 与 `±3 * gap`。
    pub gap: f32,
    /// 基准线钉在屏幕纵向的哪个高度，与「纯配置谱面」同一个口径：`0` 屏幕底边、`1` 屏幕顶边。
    pub line_y: f32,
    /// 基准线的按键流速。
    pub speed: f64,
}

/// 「纯配置谱面」的参数：给 `None` 就是这个功能没开。
#[derive(Clone, Copy)]
pub struct Pure {
    /// 判定线钉在屏幕纵向的哪个高度：`0` 屏幕底边、`1` 屏幕顶边、`0.5` 正中。
    pub line_y: f32,
    /// 按键流速：判定线高度与长按尾端都按它算。
    pub speed: f64,
    /// 左右两条竖判定线，给 `None` 就是没有竖线。
    pub side: Option<SideLines>,
    /// 按键噪域：判定时刻落在噪域里的按键在渲染时长出一块噪域矩形。
    pub noise: bool,
}

/// 「谱面修改」里的一个功能，连同它的参数。
///
/// 这几项同属一类、走同一个入口（`Chart::apply_chart_mods`），但**落地顺序由用户勾选的先后决定**：
/// [`Mods`] 按那个顺序把它们存成一串，入口逐个落地。顺序确实会改结果 —— 先「复制音符」再
/// 「长条填充」是拆出来的按键各复制一份，反过来则是复制出来的长按各自被拆。
///
/// 定轨谱面不在这一串里（[`Mods::lane`]）：它得紧跟在纯配置谱面后面落地，见 `Chart::apply_chart_mods`；
/// 隐藏按键也不在（[`Mods::hide`]）：它只改可见性，永远最后一步。
pub enum Step {
    /// 纯配置谱面：判定线拍平，只留按键的横向位置。
    Pure(Pure),
    /// 陈列按键：整张谱面收到一条基准线的四个站位上。
    Arrange(Arrange),
    /// 按键替换：四种源键型各自的映射。
    Replace(Replace),
    /// 随机按键与流速。
    Random {
        /// 每个按键随机换一种键型。
        note_type: bool,
        /// 每个按键随机乘一个流速倍率，范围是这两个数（小的在前）。
        speed: Option<(f64, f64)>,
    },
    /// 随机位置：横向随机的幅度（世界单位，半个屏幕宽是 1）。
    RandomPos(f32),
    /// 长条填充。
    Spread(Spread),
    /// 长条延长：在音乐结束前留的空白（秒）。
    Extend(f64),
    /// 复制音符：复制后一共几份，以及这几份以中轴左右对称散开的总跨度。
    Copy {
        /// 复制后一共几份。
        times: usize,
        /// 这几份散开的总跨度（世界单位，半个屏幕宽是 1；0 = 全部叠在原位）。
        offset: f32,
    },
}

impl Step {
    /// 配置里记的就是这个名字（`chart_mod_order` 的元素）。
    pub fn name(&self) -> &'static str {
        match self {
            Self::Pure(_) => "pure",
            Self::Arrange(_) => "arrange",
            Self::Replace(_) => "replace",
            Self::Random { .. } => "random",
            Self::RandomPos(_) => "random_pos",
            Self::Spread(_) => "spread",
            Self::Extend(_) => "extend",
            Self::Copy { .. } => "copy",
        }
    }
}

/// 「谱面修改」这一类的全部功能。四类互斥（纯配置谱面 / 定轨谱面 / 隐藏按键 / 按键级的那几个），
/// 同一时刻只有一类会落在里面；按键那几个按用户勾选的先后排好。
pub struct Mods {
    /// 依次要落地的功能；空表就是一个功能都没开（或走的是下面两类之一）。
    pub steps: Vec<Step>,
    /// 定轨谱面：单独一类，与纯配置谱面互斥，所以它要么一步落地、要么不在。
    pub lane: Option<Lanes>,
    /// 隐藏按键：单独一类，与上面两类和其它功能都互斥，落地时只改可见性。
    pub hide: bool,
}

impl Mods {
    /// 从 dps 配置里收出这一趟要落地的功能。
    ///
    /// 四类**互斥**：纯配置谱面 / 定轨谱面 / 隐藏按键 / 按键级的那些功能（陈列按键、按键替换、随机、
    /// 随机位置、长条填充、长条延长、复制音符）—— 同一时刻只让一类生效。界面按同一规则联动开关，
    /// 这里兜底手改的 config.toml：同时开了几类时，按界面行序取最靠前的那一类
    /// （纯配置谱面 → 定轨谱面 → 隐藏按键 → 按键那一类）。
    ///
    /// 按键那一类的顺序照 `chart_mod_order` 走 —— 那是界面按勾选先后写进来的；表里没提到的功能
    /// 按界面行序续在后面，所以老的 config.toml（没有这一项）照样能跑，只是顺序退回界面行序。
    pub fn from_config(config: &Config) -> Self {
        if config.pure_config_chart {
            return Self {
                steps: vec![Step::Pure(Pure {
                    line_y: config.pure_config_chart_line_y,
                    speed: config.pure_config_chart_speed,
                    side: config.pure_config_chart_side_lines.then(|| SideLines {
                        angle: config.pure_config_chart_angle,
                        x: config.pure_config_chart_side_x,
                    }),
                    noise: config.pure_config_chart_noise,
                })],
                lane: None,
                hide: false,
            };
        }
        if config.fixed_lane_chart {
            return Self {
                steps: Vec::new(),
                lane: Some(Lanes {
                    gap: config.fixed_lane_gap,
                    line_y: config.fixed_lane_line_y,
                    speed: config.fixed_lane_speed,
                    interval: config.fixed_lane_interval,
                    third: config.fixed_lane_third,
                }),
                hide: false,
            };
        }
        if config.chart_mod_hide {
            return Self {
                steps: Vec::new(),
                lane: None,
                hide: true,
            };
        }

        // 先按界面行序把它们收全，再按配置里记的顺序挑出来
        let mut all = Vec::new();
        if config.chart_mod_arrange {
            all.push(Step::Arrange(Arrange {
                gap: config.chart_mod_arrange_gap,
                line_y: config.chart_mod_arrange_line_y,
                speed: config.chart_mod_arrange_speed,
            }));
        }
        if config.chart_mod_replace {
            all.push(Step::Replace(Replace {
                tap: NoteType::from_name(&config.chart_mod_replace_tap),
                drag: NoteType::from_name(&config.chart_mod_replace_drag),
                hold: NoteType::from_name(&config.chart_mod_replace_hold),
                flick: NoteType::from_name(&config.chart_mod_replace_flick),
            }));
        }
        let random_speed = (config.chart_mod_random && config.chart_mod_random_speed)
            .then_some((config.chart_mod_random_speed_min, config.chart_mod_random_speed_max));
        if config.chart_mod_random && (config.chart_mod_random_type || random_speed.is_some()) {
            all.push(Step::Random {
                note_type: config.chart_mod_random_type,
                speed: random_speed,
            });
        }
        if config.chart_mod_random_pos {
            all.push(Step::RandomPos(config.chart_mod_random_pos_range));
        }
        if config.chart_mod_spread_hold {
            all.push(Step::Spread(Spread {
                note_type: NoteType::from_name(&config.chart_mod_spread_hold_note_type),
                interval: config.chart_mod_spread_hold_interval,
            }));
        }
        if config.chart_mod_extend_hold {
            all.push(Step::Extend(config.chart_mod_extend_hold_blank));
        }
        if config.chart_mod_copy {
            all.push(Step::Copy {
                times: config.chart_mod_copy_times,
                offset: config.chart_mod_copy_offset,
            });
        }

        let mut steps = Vec::with_capacity(all.len());
        for name in &config.chart_mod_order {
            if let Some(index) = all.iter().position(|it| it.name() == name) {
                steps.push(all.remove(index));
            }
        }
        steps.append(&mut all);
        Self {
            steps,
            lane: None,
            hide: false,
        }
    }

    /// 一个功能都没开：`GameScene` 据此跳过整步转换。
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.lane.is_none() && !self.hide
    }
}

/// 随机类功能共用的随机数发生器：固定种子，同一张谱面每次渲染出来的随机结果一致。
pub(crate) fn rng() -> Pcg32 {
    Pcg32::seed_from_u64(RANDOM_SEED)
}

/// 把「按键那一层」的一个功能落到 `lines` 上。
///
/// 纯配置谱面 / 定轨谱面 / 陈列按键要改判定线布局、还要补绘制顺序，由
/// [`crate::core::Chart::apply_chart_mods`] 自己处理，落到这里只会跳过。
///
/// * `bpm_list` —— 长条填充与换成长按都要把秒换算成 1/32 拍 tick，用谱面自己的 BPM 表。
/// * `track_end` —— 音乐结束的时刻（秒）：长条延长把长按尾端钉在它前面留白处。
pub(crate) fn apply_step(lines: &mut [JudgeLine], bpm_list: &mut BpmList, step: &Step, track_end: f64, rng: &mut Pcg32) {
    let editable_lines = lines.iter_mut().filter(|it| editable(it));
    match step {
        Step::Replace(replace) => {
            for line in editable_lines {
                replace_types(line, replace, bpm_list);
            }
        }
        Step::Random { note_type, speed } => {
            for line in editable_lines {
                randomize(line, *note_type, *speed, bpm_list, rng);
            }
        }
        Step::Spread(spread) => {
            for line in editable_lines {
                spread_holds(line, spread, bpm_list);
            }
        }
        Step::Extend(blank) => {
            let end_time = (track_end - blank.max(0.)).max(0.);
            for line in editable_lines {
                extend_holds(line, end_time);
            }
        }
        Step::Copy { times, offset } => {
            for line in editable_lines {
                copy_notes(line, *times, *offset);
            }
        }
        Step::RandomPos(range) => {
            for line in editable_lines {
                randomize_positions(line, *range, rng);
            }
        }
        Step::Pure(_) | Step::Arrange(_) => {}
    }
}

/// 全部功能落地之后的收尾：重算双押提示，重建改动过的判定线缓存。
///
/// 缓存按 `above` / `speed` / 长按尾端分桶（见 `JudgeLineCache`），这三样都可能被前面的功能改过，
/// 所以统一在这里重建一次。
pub(crate) fn finish(lines: &mut [JudgeLine]) {
    refresh_double_hints(lines);
    for line in lines.iter_mut().filter(|it| editable(it)) {
        line.cache = JudgeLineCache::new(&mut line.notes);
    }
}

/// 重算「双押提示」的标记。
///
/// 标记原本只在解析时算一次（同一时刻的按键数大于 1，见 [`crate::parse::process_lines`]），
/// 谱面修改又会新增（长条填充）与复制（复制音符）按键，解析时那份已经过时 —— 不重算的话
/// 新造出来的双押永远不带提示。先清掉旧标记再算，避免按键被改少之后留下过时的提示。
fn refresh_double_hints(lines: &mut [JudgeLine]) {
    for note in lines.iter_mut().flat_map(|it| it.notes.iter_mut()) {
        note.multiple_hint = false;
    }
    crate::parse::process_lines(lines);
}

/// 一条判定线能不能改：挂界面元素的判定线与贴图 / 文字 / 画笔判定线承载的是画面元素，不是谱面。
fn editable(line: &JudgeLine) -> bool {
    line.attach_ui.is_none() && matches!(line.kind, JudgeLineKind::Normal)
}

/// 按键替换：每种源键型各自换成映射里的键型（换成自己就是不动）。
fn replace_types(line: &mut JudgeLine, replace: &Replace, bpm_list: &mut BpmList) {
    let mut curve = line.height.clone();
    for note in line.notes.iter_mut() {
        let note_type = replace.target(&note.kind);
        if note_type.matches_kind(&note.kind) {
            continue;
        }
        // 长按换成别的键型时流速回到 1：长按身上的流速常带着修饰，换到点按上会快得离谱
        if matches!(note.kind, NoteKind::Hold { .. }) {
            note.speed = 1.;
        }
        let hold_len = CONVERT_HOLD_TICKS / ticks_per_sec(bpm_list, note.time);
        set_note_type(note, note_type, hold_len, &mut curve);
    }
}

/// 随机按键 / 随机按键与流速：每个按键随机键型，再随机乘一个流速倍率。
///
/// 流速倍率按谱面修改工具的写法乘在按键自己身上（长按连尾端一起乘），于是它在屏幕上的移动快慢
/// 变了，判定时刻不变 —— 按键高度与判定线高度是同一个倍率，两者相交的时刻只由时间决定。
fn randomize(line: &mut JudgeLine, note_type: bool, speed: Option<(f64, f64)>, bpm_list: &mut BpmList, rng: &mut Pcg32) {
    let mut curve = line.height.clone();
    for note in line.notes.iter_mut() {
        if note_type {
            let note_type = match rng.random_range(0..4) {
                0 => NoteType::Tap,
                1 => NoteType::Drag,
                2 => NoteType::Hold,
                _ => NoteType::Flick,
            };
            if !note_type.matches_kind(&note.kind) {
                // 长按换成别的键型时流速回到 1（同「按键替换」）
                if matches!(note.kind, NoteKind::Hold { .. }) {
                    note.speed = 1.;
                }
                let hold_len = CONVERT_HOLD_TICKS / ticks_per_sec(bpm_list, note.time);
                set_note_type(note, note_type, hold_len, &mut curve);
            }
        }
        if let Some((min, max)) = speed {
            let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
            let factor = lo + (hi - lo) * rng.random::<f64>();
            note.speed *= factor;
            if let NoteKind::Hold { end_speed: Some(end_speed), .. } = &mut note.kind {
                *end_speed *= factor;
            }
        }
    }
}

/// 长条填充：长按按固定间隔拆成一串按键，其余按键换成选定的键型。
///
/// 拆出来的按键沿用原来那一个的横向位置与上下方归属，纵向改由判定线的高度曲线给出 —— 与 modify
/// 里 `make_note` 只带 `positionX` / `floorPosition`、丢掉 `yOffset` 的做法一致。
fn spread_holds(line: &mut JudgeLine, spread: &Spread, bpm_list: &mut BpmList) {
    let interval = spread.interval.round().max(1.);
    let mut curve = line.height.clone();
    let mut notes = Vec::with_capacity(line.notes.len());
    for note in line.notes.drain(..) {
        // 只拆长按：点按 / 拖动 / 滑动原样留着（modify 里那种「顺手把非长按换成目标键型」的口径
        // 会把整张谱面的键型洗一遍，这里不做）。
        if let NoteKind::Hold { end_time, .. } = note.kind {
            split_hold(&mut notes, &note, end_time, spread.note_type, interval, &mut curve, bpm_list);
        } else {
            notes.push(note);
        }
    }
    line.notes = notes;
}

/// 把一个长按按 `interval`（1/32 拍）拆成一串按键，推在 `out` 后面。
///
/// 拆出来的每个按键自己也是「间隔 + 1」拍的长按（键型由调用方定），与 modify 里 `split_hold`
/// 写回谱面的 `holdTime` 一致。
fn split_hold(out: &mut Vec<Note>, template: &Note, end_time: f64, note_type: NoteType, interval: f64, curve: &mut AnimFloatF64, bpm_list: &mut BpmList) {
    let ticks = ticks_per_sec(bpm_list, template.time);
    let start = (template.time * ticks).floor();
    let end = start + ((end_time - template.time).max(0.) * ticks).floor();
    let x = note_x(template);
    let mut tick = start;
    while tick <= end {
        let time = tick / ticks;
        let mut note = new_note(time, x, template.above, template.fake);
        note.height = curve_at(curve, time);
        set_note_type(&mut note, note_type, (interval + 1.) / ticks, curve);
        out.push(note);
        tick += interval;
    }
}

/// 长条延长：把这条判定线上每个长按的尾端拖到 `end_time`（秒）。
///
/// 尾端的屏幕位置由 `end_height` 定（尾端在判定线上方多高），而判定线本身要在那一刻正好高到那里，
/// 所以先把判定线的高度曲线按原来的平均速度往后接一段，再按接好的曲线写回 `end_height`。只改
/// `end_time` 的话判定线停在原来那一帧，长按的尾端永远等不到线。
fn extend_holds(line: &mut JudgeLine, end_time: f64) {
    if end_time <= 0. || !line.notes.iter().any(|it| matches!(it.kind, NoteKind::Hold { .. })) {
        return;
    }
    extend_curve(&mut line.height, end_time);
    let mut curve = line.height.clone();
    let end_height = curve_at(&mut curve, end_time);
    for note in line.notes.iter_mut() {
        // 起点就在目标之后的长按不动：它的尾端本来就在目标之后
        if note.time >= end_time {
            continue;
        }
        if let NoteKind::Hold { end_time: tail, end_height: tail_height, .. } = &mut note.kind {
            *tail = end_time;
            *tail_height = end_height;
        }
    }
}

/// 把判定线高度曲线接到 `target` 秒：沿 `next` 链走到最后一段动画，按它的平均速度往后补一帧。
///
/// 返回是否真的补上了 —— 链上的动画都是空的就无处可补，此时长按会被压成零长度（尾端仍按时判定）。
fn extend_curve(curve: &mut AnimFloatF64, target: f64) -> bool {
    if let Some(next) = curve.next.as_deref_mut() {
        // 末端在嵌套事件那一层（RPE 的嵌套事件是叠加出来的），先交给它；它没有帧就退回来补自己这条
        if extend_curve(next, target) {
            return true;
        }
    }
    append_keyframe(curve, target)
}

/// 在动画末尾补一帧：值按「末值 / 末时刻」的平均速度外推。
///
/// 谱面的高度曲线是速度事件积分出来的（从 0 起算、一直线性涨到结束），所以末段的平均速度就是
/// `末值 / 末时刻`；末帧的补间函数要改成线性（`2`），否则谱面里常写的「保持」会让追加的这一段变成阶跃。
fn append_keyframe(curve: &mut AnimFloatF64, target: f64) -> bool {
    let (last_time, last_value) = match curve.keyframes.last() {
        Some(it) if it.time < target => (it.time, it.value),
        Some(_) => return true,
        None => return false,
    };
    let speed = if last_time > 1e-6 { last_value / last_time } else { 0. };
    let value = last_value + speed * (target - last_time);
    let mut keyframes = curve.keyframes.to_vec();
    if let Some(prev) = keyframes.last_mut() {
        prev.tween = StaticTween::get_rc(2);
    }
    keyframes.push(Keyframe::new(target, value, 0));
    curve.keyframes = keyframes.into_boxed_slice();
    true
}

/// 复制音符：每个按键在同一时刻复制成 `times` 份（键型 / 颜色 / 判定时刻一模一样）。
///
/// `offset` 是这几份之间的总跨度（世界单位）：以原按键的横坐标为中轴左右对称散开，单数时中间
/// 那份落在原位上；`0` 表示全部叠在一起（只是「加厚」）。横坐标只有在需要散开时才重写，
/// 免得把带在按键上的位移动画拍平成常量。
fn copy_notes(line: &mut JudgeLine, times: usize, offset: f32) {
    if times < 2 {
        return;
    }
    let span = offset.max(0.);
    let step = span / (times - 1) as f32;
    let middle = (times - 1) as f32 / 2.;
    let mut notes = Vec::with_capacity(line.notes.len() * times);
    for note in line.notes.drain(..) {
        let base = note_x(&note);
        for index in 0..times {
            let mut copy = note.clone();
            if span > 0. {
                copy.object.translation.0 = AnimFloat::fixed(base + (index as f32 - middle) * step);
            }
            notes.push(copy);
        }
    }
    line.notes = notes;
}

/// 隐藏按键：整颗按键不画出来（判定照常，见 `Note::hidden`）。
pub(crate) fn hide_notes(lines: &mut [JudgeLine]) {
    for line in lines.iter_mut().filter(|it| editable(it)) {
        for note in line.notes.iter_mut() {
            note.hidden = true;
        }
    }
}

/// 随机位置：每个按键的横向位置随机落在 `-range`~`range`（世界单位，半个屏幕宽是 1）。
fn randomize_positions(line: &mut JudgeLine, range: f32, rng: &mut Pcg32) {
    let range = range.clamp(0., 1.);
    for note in line.notes.iter_mut() {
        let x = if range > 0. { rng.random_range(-range..=range) } else { 0. };
        note.object.translation.0 = AnimFloat::fixed(x);
    }
}

/// 陈列按键：把整张谱面的按键按时间收到第一条可改判定线上，每个键型一个固定站位。
///
/// 站位沿用「纯配置谱面」那套拍平（判定线的移动 / 旋转 / 消失全换成常量、按键沿同一条常量速度积分），
/// 横坐标是 `±gap` 与 `±3 * gap`，键型与站位的对应关系同谱面修改工具：
/// 点按 `-3gap`、长按 `-gap`、拖动 `+gap`、滑动 `+3gap`。
///
/// 返回值是要保留的基准线下标 —— 其余判定线的按键都搬走了，`Chart` 那边要把它们从绘制顺序里删掉。
pub(crate) fn arrange_notes(lines: &mut [JudgeLine], arrange: &Arrange) -> Option<usize> {
    let base = lines.iter().position(|it| editable(it))?;
    let gap = arrange.gap.clamp(0., 1.);

    let mut collected = Vec::new();
    for line in lines.iter_mut() {
        if editable(line) {
            collected.append(&mut line.notes);
        }
    }

    let mut arranged = Vec::with_capacity(collected.len());
    for mut note in collected {
        let lane = match note.kind {
            NoteKind::Click => -3. * gap,
            NoteKind::Hold { .. } => -gap,
            NoteKind::Drag => gap,
            NoteKind::Flick => 3. * gap,
        };
        crate::core::pure_config::flatten_note(&mut note, lane, arrange.speed);
        arranged.push(note);
    }

    // 判定线高度积分的终点取最后一个按键之后一秒（与「纯配置谱面」同一个口径）
    let end_time = arranged
        .iter()
        .map(|it| match it.kind {
            NoteKind::Hold { end_time, .. } => end_time,
            _ => it.time,
        })
        .fold(0., f64::max)
        + 1.;

    let line = &mut lines[base];
    line.object.translation = AnimVector::fixed(Vector::new(0., -1. + arrange.line_y.clamp(0., 1.) * 2.));
    line.object.rotation = AnimFloat::fixed(0.);
    // 缩放会把按键相对判定线的位移再乘一遍，固定下来的横向站位就不在站位上了
    line.object.scale = AnimVector::fixed(Vector::new(1., 1.));
    line.object.alpha = AnimFloat::fixed(1.);
    line.height = crate::core::pure_config::speed_curve(end_time, arrange.speed);
    // 父级（含「随父旋转」）会把拍平后的线再一次带走
    line.parent = None;
    line.rotate_with_parent = false;
    line.notes = arranged;
    line.cache = JudgeLineCache::new(&mut line.notes);
    Some(base)
}

/// 把按键换成 `note_type`；换成长按时补一截长度 `hold_len`（秒）。
///
/// 本来就是长按的不动（更不该被截短），`hold_len` 只对原本不是长按的按键起作用；换完键型，音效也
/// 跟着键型走（谱面自带的自定义音效同样被换掉）。
fn set_note_type(note: &mut Note, note_type: NoteType, hold_len: f64, curve: &mut AnimFloatF64) {
    if note_type.matches_kind(&note.kind) {
        return;
    }
    let end_time = match note.kind {
        NoteKind::Hold { end_time, .. } => end_time,
        _ => note.time + hold_len,
    };
    note.kind = match note_type {
        NoteType::Tap => NoteKind::Click,
        NoteType::Drag => NoteKind::Drag,
        NoteType::Flick => NoteKind::Flick,
        NoteType::Hold => NoteKind::Hold {
            end_time,
            end_height: curve_at(curve, end_time),
            end_speed: None,
        },
    };
    note.hitsound = HitSound::default_from_kind(&note.kind);
}

/// 造一个还没定位的新按键：颜色 / 特效动画留空（用资源包默认样式），判定状态是待判定。
fn new_note(time: f64, x: f32, above: bool, fake: bool) -> Note {
    Note {
        object: Object {
            alpha: AnimFloat::fixed(1.),
            scale: AnimVector::fixed(Vector::new(1., 1.)),
            rotation: AnimFloat::fixed(0.),
            translation: AnimVector::fixed(Vector::new(x, 0.)),
        },
        kind: NoteKind::Click,
        hitsound: HitSound::default_from_kind(&NoteKind::Click),
        time,
        height: 0.,
        speed: 1.,
        above,
        multiple_hint: false,
        fake,
        judge: JudgeStatus::NotJudged,
        judge_scale: 1.,
        color: Anim::default(),
        hit_fx_color: Anim::default(),
        protected: false,
        hidden: false,
        pure_config_noise: false,
        water: Default::default(),
    }
}

/// 按键相对判定线的横向位置（世界单位）。谱面里的按键位移是常量动画，按当前游标取值就是它自己
/// 的横向站位。
fn note_x(note: &Note) -> f32 {
    note.object.translation.0.now_opt().unwrap_or_default()
}

/// 判定线高度曲线在 `time` 时刻的值：按键正好站在判定线上时该有的高度。
fn curve_at(curve: &mut AnimFloatF64, time: f64) -> f64 {
    curve.set_time(time);
    curve.now()
}

/// 一秒里有多少个 1/32 拍 tick。
fn ticks_per_sec(bpm_list: &mut BpmList, time: f64) -> f64 {
    bpm_list.now_bpm(time) / TIME_BASE
}
