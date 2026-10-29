//! 「谱面修改」：纯配置谱面、定轨谱面，把谱面本身改出花样的按键替换、随机按键、随机流速、固定流速、
//! 长条填充、长条延长、复制音符、随机位置、陈列按键（后九个与谱面修改工具 modify 里的同名功能一一对应），
//! 以及「隐藏按键」这一项自己的东西（modify 里没有对应功能）。
//!
//! 这些都同一层：谱面文件不动，只改内存里刚解析出来的判定线与按键，所以预览、渲染、分屏走的
//! 是同一份转换，入口只有一个 —— [`crate::core::Chart::apply_chart_mods`]。
//!
//! 按键那一层还可以同时开多个，**落地顺序就是界面上勾选的先后**（配置里记在 `chart_mod_order`）：
//! 顺序会改结果，例如先「复制音符」再「长条填充」是拆出来的按键各复制一份，反过来则是复制出来的
//! 长按各自被拆。没在 `chart_mod_order` 里提到的功能排在后面，按界面行序 —— 老的 config.toml
//! 没有这一项，照样能跑。固定流速不在这一串里：它把整张谱面的下落节奏换掉，和别的功能叠在一起没有
//! 意义，界面上它也放在「测试」那一栏。
//!
//! 六类**互斥**（[`Mods::from_config`] 里兜底）：纯配置谱面 / 定轨谱面 / 隐藏按键 / 陈列按键 / 固定流速 /
//! 按键那一层，同一时刻只让一类生效。前五类各自把整张谱面（或全部按键的下落节奏、可见性）改一遍，和
//! 别的类叠在一起只会互相覆盖；界面按同一规则联动开关，手改 config.toml 同时开了几类时按界面行序取
//! 最靠前的那一类。
//!
//! 其中「纯配置谱面」「定轨谱面」改判定线布局、「陈列按键」会把整张谱面收到一条基准线上，这三项由
//! [`crate::core::Chart::apply_chart_mods`] 自己落地；「固定流速」和其余各项走 [`apply_step`] —— 它虽然
//! 也改判定线，但改的是每条线各自的高度曲线，不需要重排绘制顺序。
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
/// 隐藏按键也不在（[`Mods::hide`]）：它只改可见性，永远最后一步；固定流速也不在：它单独一类，界面上
/// 归在「测试」那一栏（见 [`Mods::from_config`]）。
pub enum Step {
    /// 纯配置谱面：判定线拍平，只留按键的横向位置。
    Pure(Pure),
    /// 陈列按键：整张谱面收到一条基准线的四个站位上。
    Arrange(Arrange),
    /// 按键替换：四种源键型各自的映射。
    Replace(Replace),
    /// 随机按键：每个按键随机换一种键型。
    RandomType,
    /// 随机流速：每个按键随机乘一个流速倍率，范围是这两个数（小的在前）。
    RandomSpeed(f64, f64),
    /// 固定流速：整张谱面的流速钉成这一个值。
    FixedSpeed(f64),
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
            Self::RandomType => "random",
            Self::RandomSpeed(..) => "random_speed",
            Self::FixedSpeed(_) => "fixed_speed",
            Self::RandomPos(_) => "random_pos",
            Self::Spread(_) => "spread",
            Self::Extend(_) => "extend",
            Self::Copy { .. } => "copy",
        }
    }
}

/// 「谱面修改」这一类的全部功能。六类互斥（纯配置谱面 / 定轨谱面 / 隐藏按键 / 陈列按键 / 固定流速 /
/// 按键级的那几个），同一时刻只有一类会落在里面；按键那几个按用户勾选的先后排好。
pub struct Mods {
    /// 依次要落地的功能；空表就是一个功能都没开（或走的是下面两类之一）。
    pub steps: Vec<Step>,
    /// 定轨谱面：单独一类，所以它要么一步落地、要么不在。
    pub lane: Option<Lanes>,
    /// 隐藏按键：单独一类，落地时只改可见性。
    pub hide: bool,
}

impl Mods {
    /// 从 dps 配置里收出这一趟要落地的功能。
    ///
    /// 六类**互斥**：纯配置谱面 / 定轨谱面 / 隐藏按键 / 陈列按键 / 固定流速 / 按键级的那些功能（按键替换、
    /// 随机、随机位置、长条填充、长条延长、复制音符）—— 同一时刻只让一类生效。界面按同一规则联动开关，
    /// 这里兜底手改的 config.toml：同时开了几类时，按界面行序取最靠前的那一类
    /// （纯配置谱面 → 定轨谱面 → 隐藏按键 → 陈列按键 → 固定流速 → 按键那一类）。
    ///
    /// 按键那一类的顺序照 `chart_mod_order` 走 —— 那是界面按勾选先后写进来的；表里没提到的功能
    /// 按界面行序续在后面，所以老的 config.toml（没有这一项）照样能跑，只是顺序退回界面行序。
    /// 固定流速不在这串里：它不参与排序，开了就单独一步落地。
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
        if config.chart_mod_arrange {
            return Self {
                steps: vec![Step::Arrange(Arrange {
                    gap: config.chart_mod_arrange_gap,
                    line_y: config.chart_mod_arrange_line_y,
                    speed: config.chart_mod_arrange_speed,
                })],
                lane: None,
                hide: false,
            };
        }
        if config.chart_mod_fixed_speed {
            return Self {
                steps: vec![Step::FixedSpeed(config.chart_mod_fixed_speed_value)],
                lane: None,
                hide: false,
            };
        }

        // 先按界面行序把它们收全，再按配置里记的顺序挑出来
        let mut all = Vec::new();
        if config.chart_mod_replace {
            all.push(Step::Replace(Replace {
                tap: NoteType::from_name(&config.chart_mod_replace_tap),
                drag: NoteType::from_name(&config.chart_mod_replace_drag),
                hold: NoteType::from_name(&config.chart_mod_replace_hold),
                flick: NoteType::from_name(&config.chart_mod_replace_flick),
            }));
        }
        if config.chart_mod_random {
            all.push(Step::RandomType);
        }
        if config.chart_mod_random_speed {
            all.push(Step::RandomSpeed(config.chart_mod_random_speed_min, config.chart_mod_random_speed_max));
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
        Step::RandomType => {
            for line in editable_lines {
                randomize_types(line, bpm_list, rng);
            }
        }
        Step::RandomSpeed(min, max) => {
            for line in editable_lines {
                randomize_speeds(line, (*min, *max), rng);
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
        Step::FixedSpeed(speed) => {
            for line in editable_lines {
                fix_speed(line, *speed, track_end);
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

/// 随机按键：每个按键随机换一种键型（长按换成别的键型时流速回到 1，同「按键替换」）。
fn randomize_types(line: &mut JudgeLine, bpm_list: &mut BpmList, rng: &mut Pcg32) {
    let mut curve = line.height.clone();
    for note in line.notes.iter_mut() {
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
}

/// 随机流速：每个按键随机乘一个流速倍率。
///
/// 流速倍率按谱面修改工具的写法乘在按键自己身上（长按连尾端一起乘），于是它在屏幕上的移动快慢
/// 变了，判定时刻不变 —— 按键高度与判定线高度是同一个倍率，两者相交的时刻只由时间决定。
fn randomize_speeds(line: &mut JudgeLine, (min, max): (f64, f64), rng: &mut Pcg32) {
    let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
    for note in line.notes.iter_mut() {
        let factor = lo + (hi - lo) * rng.random::<f64>();
        note.speed *= factor;
        if let NoteKind::Hold { end_speed: Some(end_speed), .. } = &mut note.kind {
            *end_speed *= factor;
        }
    }
}

/// 固定流速：判定线高度换成一条常量速度的积分，按键与长按尾端按同一条积分重算高度。
///
/// 判定线的高度是速度事件积分出来的，按键的 `height` 又是**同一个积分**在它自己时刻的值 —— 两者
/// 必须同源，否则按键到判定时刻不在判定线上（线走到哪、键走到哪对不上）。所以这里不能只把判定线
/// 拍平：按键的高度、长按尾端的高度与尾端流速都要一起换到新积分上。
///
/// 长按尾端的 `end_speed` 取同一个流速（尾端相对判定线的下降速度），`note.speed` 归 1：长按的可见
/// 长度于是只由时间差与流速决定，谱面原来的变速事件不再把长按拉长或压扁。
///
/// 例外是「原地判定」的按键，见 [`frozen_on_line`]：它们本来就靠判定线带着走，改了流速也不能让它们
/// 从远处落下来。长条不在此列（身体长度靠流速撑着），见 [`fix_speed_note`]。
fn fix_speed(line: &mut JudgeLine, speed: f64, track_end: f64) {
    if line.notes.is_empty() {
        return;
    }
    // 流速不是正数时判定线一动不动、按键永远落不下来，正数才能积分
    let speed = speed.max(1e-3);
    let end_time = line
        .notes
        .iter()
        .map(|it| match it.kind {
            NoteKind::Hold { end_time, .. } => end_time,
            _ => it.time,
        })
        .fold(track_end.max(0.), f64::max)
        + 1.;
    // 判断「原地判定」要拿原来的高度曲线比，拍平之后就没有依据了
    let original = line.height.clone();
    line.height = crate::core::pure_config::speed_curve(end_time, speed);
    for note in line.notes.iter_mut() {
        fix_speed_note(note, speed, &original);
    }
}

/// 判定「原地判定」的取样窗口：按键判定前这几秒。判定线在这段时间里一直停在按键所在的高度，
/// 就说明按键是被判定线带着走的（自己不用下落），判定前在屏幕上也一动不动。
const FROZEN_WINDOW: [f64; 6] = [0., 0.25, 0.5, 1., 1.5, 2.5];

/// 高度差小于这个数就算「贴在同一个高度上」。谱面高度单位，屏幕上远不到一个像素。
const FROZEN_EPS: f64 = 1e-3;

/// 这个按键是不是「原地判定」的：判定前一段时间里判定线一动不动，而按键一直就贴在它上面。
///
/// 谱面里有按键不是自然落下来的：判定线长时间停在同一个高度，按键从一开始就贴在线上，靠判定线
/// 带着走。它的屏幕位置只由判定线决定，所以固定流速不能让它按新流速从远处落下来 —— 那样它会在
/// 判定前突然出现在半空中，和原来看到的完全不是一回事。
///
/// 判据只看原来的高度曲线：按键的 `height` 是判定时刻那条曲线的值，如果判定前它也在同一个高度上，
/// 那它这段时间就是贴在线上不动的。反过来，正常下落的按键离判定线还有一段距离，判定前的高度差
/// 远大于 [`FROZEN_EPS`]，不会被误判。
fn frozen_on_line(height: &AnimFloatF64, note: &Note) -> bool {
    let mut curve = height.clone();
    FROZEN_WINDOW.iter().all(|before| {
        curve.set_time((note.time - before).max(0.));
        (curve.now() - note.height).abs() < FROZEN_EPS
    })
}

/// 把一个按键换到常量速度积分上。
///
/// `height` 与判定线的高度曲线取自同一条积分，所以按键在判定时刻正好落在判定线上；`speed` 归 1，
/// 免得按键自带的流速倍率再把它从线上带走。
///
/// 原地判定的按键反着处理：`speed` 归 0，让判定线的位置直接决定它画在哪（见 [`Note::speed`] 在
/// 绘制里的用法：偏移是 `(height - 判定线高度) * speed`，归 0 就只剩判定线），高度留着原来的值 ——
/// 流速 0 时它不参与定位，留着反而保住了原来的位置。
///
/// 长条不套「原地判定」：长条的身体长度是 `(尾端高度 - 高度) * speed`，流速归 0 就把身体压成一条
/// 线（只剩头部那一小片在判定线上）。所以长条一律按新流速正常下落，判定时刻照样落在判定线上，
/// 可见长度也只由时间差与流速决定。
fn fix_speed_note(note: &mut Note, speed: f64, line_height: &AnimFloatF64) {
    // 本来就绑在判定线上的按键（流速 0）一直贴着线，判定线换成什么形状都不影响它
    if note.speed == 0. {
        return;
    }
    if !matches!(note.kind, NoteKind::Hold { .. }) && frozen_on_line(line_height, note) {
        note.speed = 0.;
        return;
    }
    note.height = crate::core::pure_config::height_at(note.time, speed);
    note.speed = 1.;
    if let NoteKind::Hold { end_time, end_height, end_speed } = &mut note.kind {
        *end_height = crate::core::pure_config::height_at(*end_time, speed);
        *end_speed = Some(speed);
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
        // 音效跟着原来那个按键走：谱面自带的自定义音效要跟着一起拆开，不然拆完只剩内置 click
        note.hitsound = template.hitsound.clone();
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
/// 本来就是长按的不动（更不该被截短），`hold_len` 只对原本不是长按的按键起作用。音效只换内置那几种：
/// 谱面自带的自定义音效（RPE 的 `hitsound`）跟着按键走，换键型不该把谱面自备的音听丢。
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
    if !matches!(note.hitsound, HitSound::Custom(_)) {
        note.hitsound = HitSound::default_from_kind(&note.kind);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn hold_note(hitsound: HitSound) -> Note {
        let mut note = new_note(1., 0., true, false);
        note.hitsound = hitsound;
        note.kind = NoteKind::Hold { end_time: 2., end_height: 0., end_speed: None };
        note
    }

    fn height_curve() -> AnimFloatF64 {
        AnimFloatF64::new(vec![Keyframe::new(0., 0., 2), Keyframe::new(10., 10., 0)])
    }

    // 换键型只改内置音效：谱面自带的 hitsound（RPE 的 Custom）不能被换掉，换了听起来就不是这张谱了
    #[test]
    fn set_note_type_keeps_custom_hitsound() {
        let mut curve = height_curve();
        let mut note = hold_note(HitSound::Custom(String::from("boom.ogg")));
        set_note_type(&mut note, NoteType::Flick, 0.1, &mut curve);
        assert!(matches!(note.kind, NoteKind::Flick));
        assert!(matches!(note.hitsound, HitSound::Custom(_)));
    }

    // 内置音效要跟着新键型走，不然点按会响拖动的声音
    #[test]
    fn set_note_type_swaps_builtin_hitsound() {
        let mut curve = height_curve();
        let mut note = hold_note(HitSound::Click);
        set_note_type(&mut note, NoteType::Drag, 0.1, &mut curve);
        assert!(matches!(note.kind, NoteKind::Drag));
        assert!(matches!(note.hitsound, HitSound::Drag));
    }

    // 固定流速之后按键必须还在判定线上：两条高度取自同一条常量速度积分，且按键自己的流速归 1
    #[test]
    fn fix_speed_puts_note_on_line() {
        let speed = 3.;
        let mut note = hold_note(HitSound::Click);
        note.height = 123.;
        note.speed = 2.5;
        fix_speed_note(&mut note, speed, &height_curve());
        let mut curve = crate::core::pure_config::speed_curve(10., speed);
        // 高度曲线内部按 f32 补间，直接算与曲线取值在末位上差 1e-7 量级（屏幕上不足 1e-5 像素），
        // 这里按 1e-5 比 —— 要比的是「两者同源」，不是浮点末位
        assert!((note.height - curve_at(&mut curve, note.time)).abs() < 1e-5);
        assert_eq!(note.speed, 1.);
        if let NoteKind::Hold { end_time, end_height, end_speed } = note.kind {
            assert!((end_height - curve_at(&mut curve, end_time)).abs() < 1e-5);
            // 尾端按同一个流速相对判定线下降，长按的可见长度只由时间差与流速决定
            assert_eq!(end_speed, Some(speed));
        } else {
            panic!("长按被改成了别的键型");
        }
    }

    // 原地判定的按键：判定前判定线一动不动、按键一直贴在线上，固定流速之后它还得贴着线 ——
    // 流速归 0 由判定线带着走，高度留着原来的值（流速 0 时它只由判定线定位）
    #[test]
    fn fix_speed_keeps_a_note_sitting_still_on_the_line() {
        let mut note = new_note(4., 0., true, false);
        note.height = 3.;
        note.speed = 1.;
        fix_speed_note(&mut note, 3., &AnimFloatF64::fixed(3.));
        assert_eq!(note.speed, 0.);
        assert_eq!(note.height, 3.);
    }

    // 长条不套「原地判定」：流速归 0 会把身体压成一条线，所以它照样落到新积分上，尾端也换新流速
    #[test]
    fn fix_speed_gives_a_frozen_hold_the_new_fall() {
        let mut note = hold_note(HitSound::Click);
        note.time = 4.;
        note.height = 3.;
        note.speed = 1.;
        fix_speed_note(&mut note, 3., &AnimFloatF64::fixed(3.));
        assert_eq!(note.speed, 1.);
        assert!((note.height - crate::core::pure_config::height_at(note.time, 3.)).abs() < 1e-3);
        if let NoteKind::Hold { end_time, end_height, end_speed } = note.kind {
            assert!((end_height - crate::core::pure_config::height_at(end_time, 3.)).abs() < 1e-3);
            assert_eq!(end_speed, Some(3.));
        } else {
            panic!("长按被改成了别的键型");
        }
    }

    // 判定线停着、但按键还在半空（没贴在线上）时不算原地判定：固定流速照样把它换到新积分上
    #[test]
    fn fix_speed_moves_a_note_hovering_above_a_still_line() {
        let mut note = hold_note(HitSound::Click);
        note.time = 4.;
        note.height = 40.;
        note.speed = 1.;
        fix_speed_note(&mut note, 3., &AnimFloatF64::fixed(3.));
        assert_eq!(note.speed, 1.);
        assert!((note.height - crate::core::pure_config::height_at(note.time, 3.)).abs() < 1e-3);
    }

    // 本来就绑在判定线上的按键（流速 0）原样留着：它一直贴着线，判定线换成什么形状都不影响它
    #[test]
    fn fix_speed_keeps_a_bound_note() {
        let mut note = hold_note(HitSound::Click);
        note.height = 42.;
        note.speed = 0.;
        fix_speed_note(&mut note, 3., &height_curve());
        assert_eq!(note.speed, 0.);
        assert_eq!(note.height, 42.);
    }

    /// `example/` 下的一个文件（与别处同名工具同一套找法）；没有示例谱面时返回 `None`。
    fn example_path(rel: &str) -> Option<std::path::PathBuf> {
        let mut candidates: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(dir) = std::env::var("DPS_EXAMPLE_DIR") {
            if !dir.is_empty() {
                candidates.push(std::path::PathBuf::from(dir));
            }
        }
        let mut cur = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        while let Some(parent) = cur.parent() {
            candidates.push(parent.join("example"));
            cur = parent;
        }
        candidates.into_iter().find_map(|dir| {
            let path = dir.join(rel);
            path.is_file().then_some(path)
        })
    }

    // 拿真谱面验固定流速：这张官谱有近两千条非 1 速的速度事件，落完之后每条可编辑判定线的高度都
    // 必须是一条常量速度的积分，每个按键（连长按尾端）都落在同一条曲线上。只拍平判定线、不改按键，
    // 或者两者用了不同的积分，按键就会在判定时刻脱离判定线，这条断言会挂。
    #[test]
    fn fix_speed_flattens_a_real_chart() {
        let Some(path) = example_path("pez/DesultorySignals.technoplanet.0.IN.pez") else {
            eprintln!("skip: example chart not found");
            return;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skip: example chart not found");
            return;
        };
        let text = {
            use std::io::Read;
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
            let mut text = String::new();
            zip.by_name("IN.json").unwrap().read_to_string(&mut text).unwrap();
            text
        };
        let mut chart = crate::parse::parse_phigros(&text, crate::core::ChartExtra::default()).unwrap();
        let speed = 2.8;
        let mods = Mods { steps: vec![Step::FixedSpeed(speed)], lane: None, hide: false };
        chart.apply_chart_mods(&mods, 16. / 9., 200.);

        let mut checked = 0usize;
        let mut frozen = 0usize;
        for line in chart.lines.iter_mut() {
            if !editable(line) {
                continue;
            }
            let mut curve = line.height.clone();
            let expect = |t: f64| crate::core::pure_config::height_at(t, speed);
            for note in line.notes.iter() {
                checked += 1;
                // 判定线的高度曲线上，判定时刻的值就是按键该在的位置
                assert!(
                    (curve_at(&mut curve, note.time) - expect(note.time)).abs() < 1e-3,
                    "判定线在 {} 秒不是常量流速",
                    note.time
                );
                // 原地判定 / 本来绑线的按键留在原地：流速归 0，位置只由判定线决定
                if note.speed == 0. {
                    frozen += 1;
                    continue;
                }
                assert!(
                    (note.height - expect(note.time)).abs() < 1e-3,
                    "按键在 {} 秒离开了判定线：{} != {}",
                    note.time,
                    note.height,
                    expect(note.time)
                );
                assert_eq!(note.speed, 1.);
                if let NoteKind::Hold { end_time, end_height, end_speed } = note.kind {
                    assert!((curve_at(&mut curve, end_time) - expect(end_time)).abs() < 1e-3);
                    assert!((end_height - expect(end_time)).abs() < 1e-3);
                    assert_eq!(end_speed, Some(speed));
                }
            }
        }
        // 这张谱面有一千多个按键；一条都没检查到说明找错谱面或解析失败
        assert!(checked > 1000, "只检查了 {checked} 个按键");
        // 这条规则只该命中少数特例（这张谱面有几条判定线中途停住，上面躺着的按键算原地判定）
        assert!(frozen * 10 < checked, "{frozen} / {checked} 个按键被当成了原地判定");
    }

    // 长条填充拆出来的每个按键都沿用原来那根长按的音效
    #[test]
    fn split_hold_keeps_custom_hitsound() {
        let mut bpm_list = BpmList::new(vec![(0., 200.)]);
        let mut curve = height_curve();
        let template = hold_note(HitSound::Custom(String::from("boom.ogg")));
        let mut out = Vec::new();
        split_hold(&mut out, &template, 2., NoteType::Tap, 4., &mut curve, &mut bpm_list);
        assert!(!out.is_empty());
        assert!(out.iter().all(|it| matches!(it.hitsound, HitSound::Custom(_))));
    }
}
