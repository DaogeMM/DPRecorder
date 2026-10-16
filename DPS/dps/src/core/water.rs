//! 「屏幕进水」的水层（高级 → 更多）。
//!
//! 由 `scene::game` 在噪域图层（`Chart::render_block_area`）之后、同一台相机里调用一次。
//! 那台相机的基础空间是 `x ∈ [-1, 1]`、`y ∈ [-1/aspect, 1/aspect]`、**y 轴朝上**，所以这里
//! 直接用这套坐标按格平铺整屏；y 轴朝上会让 macroquad 的贴图上下颠倒，因此绘制时带
//! `flip_y: true`。
//!
//! 为什么不复用噪域那条「全屏四边形 + 官方材质 + 掩码纹理」的通路：需求只要「一层会流动的
//! 贴图」，而噪域管线要靠掩码决定形状，还得把位移 / 火花 / 描边 / 辉光 / 场景快照一层层关掉，
//! 顺带会牵扯「简化噪域」「噪域文字」等开关的语义。自己按格平铺虽然 quad 多（16:9、1/16 格
//! 约 29×18 = 522 个），但语义干净、也不碰噪域的任何状态。

use std::cell::RefCell;

use macroquad::prelude::*;
use once_cell::sync::Lazy;

use crate::core::{
    note::{Note, NoteKind},
    CtrlObject, Matrix, Point, Resource,
};
use crate::judge::JudgeStatus;

/// 屏幕**上方**额外多铺的水行数：抵消网格向下平移造成的顶部空缺（见 [`draw_water`]）。
const WATER_EXTRA_ROWS: i32 = 2;

/// MC 水贴图（16×16）。和岩浆噪域一样用 `include_bytes!` 打进二进制。
static WATER_TEX: Lazy<Texture2D> = Lazy::new(|| {
    let source = image::load_from_memory(include_bytes!("../../../../assets/Minecraft/water.png"))
        .expect("water.png 解不开")
        .to_rgba8();
    let texture = Texture2D::from_rgba8(source.width() as u16, source.height() as u16, source.as_raw());
    texture.set_filter(FilterMode::Nearest);
    texture
});

/// 画水层。`water_screen` 关闭时直接返回（不碰任何状态）。
pub fn draw_water(res: &mut Resource) {
    if !res.config.water_screen {
        return;
    }
    let aspect = res.aspect_ratio;
    if !aspect.is_finite() || aspect <= 0. {
        return;
    }
    // 基础空间里整屏高 = 2/aspect；一格水 = 屏幕高 / water_cell_scale（方形格，宽同高）。
    let screen_h = 2. / aspect;
    let cell = screen_h / res.config.water_cell_scale.max(0.01);
    if !cell.is_finite() || cell <= 0. {
        return;
    }
    let top = screen_h * 0.5;
    // 从上往下流：整张网格向下平移（y 轴朝上 ⇒ y 递减），只取 [0, cell) 的余数即可无限循环。
    let shift = if res.config.water_flow_speed > 0. {
        (res.time * res.config.water_flow_speed as f64 * screen_h as f64).rem_euclid(cell as f64) as f32
    } else {
        0.
    };
    let color = Color::new(1., 1., 1., res.config.water_opacity.clamp(0., 1.));
    let cols = (2. / cell).ceil() as i32 + 1;
    // 屏幕内需要的行数，再往上多铺 [`WATER_EXTRA_ROWS`] 行、往下多留 1 行：整张网格会向下平移
    // `shift`（0 ≤ shift < cell），最上面那一行被推出屏幕后，不补就会在顶部留下一条高 `shift`
    // 的水空缺（流动时会一闪一闪）。多画的几行落在屏幕外，由 GPU 直接裁掉。
    let rows = (screen_h / cell).ceil() as i32 + WATER_EXTRA_ROWS + 2;
    for i in 0..rows {
        // `flip_y: true` 时 macroquad 把贴图的第一行放在矩形上边 ⇒ 这里的 y 是矩形**下边**。
        let y = top + cell * WATER_EXTRA_ROWS as f32 - shift - cell * (i + 1) as f32;
        if y + cell < -top {
            break;
        }
        for j in 0..cols {
            let x = -1. + cell * j as f32;
            if x > 1. + cell {
                break;
            }
            draw_texture_ex(
                *WATER_TEX,
                x,
                y,
                color,
                DrawTextureParams {
                    dest_size: Some(vec2(cell, cell)),
                    flip_y: true,
                    ..Default::default()
                },
            );
        }
    }
}


// ---------------------------------------------------------------------------
// 「屏幕进水」第 3/4 部分：受伤音效
// ---------------------------------------------------------------------------

/// `hit` 文件夹里的两个音效，和贴图一样 `include_bytes!` 打进二进制。
///
/// 公开是为了让 `DPR` 的渲染器能用**同一份素材**把进水音效写进视频音轨
/// （见 [`take_sfx_events`]）。
pub const HIT_SFX_BYTES: [&[u8]; 2] = [
    include_bytes!("../../../../assets/Minecraft/hit/hit-1.mp3"),
    include_bytes!("../../../../assets/Minecraft/hit/hit-2.mp3"),
];

/// 受伤音效的近似时长（秒）：用来把「同一时刻最多几个并发」落到实处。
const HIT_SFX_LEN: f64 = 0.4;

/// 一个 `sasa::Sfx` 内部能同时叠放多少声：创建时传的是 [`crate::core::BUFFER_SIZE`]，
/// `sasa` 的 `Sfx` 用一条环形缓冲记录每个还没播完的 `(位置, 参数)`，每次 `play` 只往里推一条，
/// 渲染时把所有条目混起来 —— 所以**一个实例本身就能叠 1024 个并发**，实例数按需换算即可。
const HIT_SFX_VOICES: usize = crate::core::BUFFER_SIZE;

/// 每个素材最多建几个实例（换算出来的实例数超过它就截断，防止配置写得太离谱）。
const HIT_SFX_MAX_INSTANCES: usize = 8;

/// 并发上限的硬上限：配置里的 `water_sfx_max` 会被夹到 `1..=HIT_SFX_MAX_CONCURRENCY`
/// （= 单实例能叠的声数 × 每个素材的实例数上限）。
const HIT_SFX_MAX_CONCURRENCY: usize = HIT_SFX_VOICES * HIT_SFX_MAX_INSTANCES;

/// 音效事件日志的条数上限（只用来给渲染器补音轨，超出就丢弃，不参与实时播放）。
const SFX_EVENT_MAX: usize = 4096;

/// 「因溺水而死亡的按键」日志的条数上限（给渲染器去掉这些按键的打击音效用）。
const DROWNED_NOTE_MAX: usize = 4096;

/// 受伤闪红的时长（秒）。
pub const WATER_FLASH_TIME: f32 = 0.15;

/// 死亡时的最大倾斜角度（弧度）。
pub const WATER_DEATH_TILT: f32 = 0.5;

thread_local! {
    /// 受伤音效的 `Sfx` 实例池：按并发上限建，同一个音效也能叠放多次
    static HIT_SFX: RefCell<Vec<sasa::Sfx>> = RefCell::new(Vec::new());
    /// 最近几次受伤音效的播放时刻，用来限制并发数
    static HIT_TIMES: RefCell<Vec<f64>> = RefCell::new(Vec::new());
    /// 真正播放过的音效事件 `(播放时刻, 素材下标, 音量)`，供渲染器写进视频音轨
    static SFX_EVENTS: RefCell<Vec<(f64, usize, f32)>> = RefCell::new(Vec::new());
    /// 因溺水而死亡的按键身份 `(谱面时刻, 类型标签, 横向位置)`，供渲染器去掉它们的打击音效
    static DROWNED_NOTES: RefCell<Vec<(f64, u8, f64)>> = RefCell::new(Vec::new());
}

/// 取出并清空「真正播放过的受伤/死亡音效」事件日志。
///
/// 每条形如 `(播放时刻（秒，即 `res.time`）, `HIT_SFX_BYTES` 的下标, 音量)`。
/// 实时播放时没人取，日志会一直涨（到 [`SFX_EVENT_MAX`] 为止），所以渲染器**开始渲染前**
/// 应该先调用一次把它清空，渲染结束后再调用一次拿到这一遍的视频里该响哪几下。
pub fn take_sfx_events() -> Vec<(f64, usize, f32)> {
    SFX_EVENTS.with(|cell| std::mem::take(&mut *cell.borrow_mut()))
}

/// 取出并清空「因溺水而死亡的按键」日志，每条形如 `(谱面时刻, 类型标签, 横向位置)`。
///
/// 用法与 [`take_sfx_events`] 相同：渲染器**开始渲染前**先清一次，渲染结束后再取一次。
/// 渲染成片的打击音轨是按谱面音头离线合成的（与判定无关），所以已经溺水死亡、画面上早就
/// 变红消失的按键也会照样排上打击音 —— 渲染器拿这份日志把它们的音效去掉。
pub fn take_drowned_notes() -> Vec<(f64, u8, f64)> {
    DROWNED_NOTES.with(|cell| std::mem::take(&mut *cell.borrow_mut()))
}

/// 按键类型标签（`0` Click / `1` Drag / `2` Flick / `3` Hold），给渲染器把打击音效条目对上号。
pub fn note_kind_tag(kind: &NoteKind) -> u8 {
    match kind {
        NoteKind::Click => 0,
        NoteKind::Drag => 1,
        NoteKind::Flick => 2,
        NoteKind::Hold { .. } => 3,
    }
}

/// 记一条「这个按键溺水死了」（渲染器据此在视频音轨里去掉它的打击音效）。
fn log_drowned_note(note: &Note) {
    DROWNED_NOTES.with(|cell| {
        let mut v = cell.borrow_mut();
        if v.len() < DROWNED_NOTE_MAX {
            v.push((note.time, note_kind_tag(&note.kind), note.height));
        }
    });
}

/// 配置里的并发上限，夹到 `1..=HIT_SFX_MAX_CONCURRENCY`（小数向下取整，非法值当 1）。
fn hit_sfx_limit(res: &Resource) -> usize {
    (res.config.water_sfx_max as i64).clamp(1, HIT_SFX_MAX_CONCURRENCY as i64) as usize
}

/// 并发上限换算成「每个素材要建几个实例」：一个实例能叠 [`HIT_SFX_VOICES`] 个并发，
/// 向上取整后夹到 `1..=HIT_SFX_MAX_INSTANCES`（默认的 500 个并发只需要 1 个实例）。
fn hit_sfx_instances(limit: usize) -> usize {
    ((limit + HIT_SFX_VOICES - 1) / HIT_SFX_VOICES).clamp(1, HIT_SFX_MAX_INSTANCES)
}

/// 把实例池建到「每个素材 `per_material` 个」；已经够了就直接返回。
fn init_hit_sfx(res: &mut Resource, per_material: usize) {
    let need = per_material.max(1) * HIT_SFX_BYTES.len();
    HIT_SFX.with(|cell| {
        if cell.borrow().len() >= need {
            return;
        }
        let mut list = cell.borrow_mut();
        while list.len() < need {
            let bytes = HIT_SFX_BYTES[list.len() % HIT_SFX_BYTES.len()];
            match sasa::AudioClip::new(bytes.to_vec()) {
                Ok(clip) => match res.audio.create_sfx(clip, Some(crate::core::BUFFER_SIZE)) {
                    Ok(sfx) => list.push(sfx),
                    Err(err) => {
                        #[cfg(debug_assertions)]
                        eprintln!("[water] create_sfx 失败：{err:?}");
                        break;
                    }
                },
                Err(err) => {
                    #[cfg(debug_assertions)]
                    eprintln!("[water] 受伤音效解码失败：{err:?}");
                    break;
                }
            }
        }
        #[cfg(debug_assertions)]
        eprintln!("[water] 受伤音效就绪 {}/{}", list.len(), need);
    });
}

/// 受伤时随机播一个 `hit` 音效。
///
/// 并发上限用「最近 [`HIT_SFX_LEN`] 秒内已经放了几次」近似：已经到 `water_sfx_max` 次就这次不响。
/// 音量只吃 `water_sfx_volume`（不吃全局 `volume_sfx`，因为后者的默认值是 0，会让这个
/// 功能默认听起来是哑的）。
pub fn play_hurt(res: &mut Resource) {
    play_hit_sound(res, "受伤");
}

/// 溺水死亡时也播一次同样的受击音效（共用素材、音量与并发上限）。
pub fn play_death(res: &mut Resource) {
    play_hit_sound(res, "死亡");
}

fn play_hit_sound(res: &mut Resource, tag: &str) {
    if !res.config.water_screen {
        return;
    }
    let max = hit_sfx_limit(res);
    init_hit_sfx(res, hit_sfx_instances(max));
    let volume = res.config.water_sfx_volume.clamp(0., 1.);
    if volume <= 1e-2 {
        return;
    }
    let now = res.time;
    let allowed = HIT_TIMES.with(|cell| {
        let mut v = cell.borrow_mut();
        v.retain(|t| now - *t < HIT_SFX_LEN);
        if v.len() >= max {
            false
        } else {
            v.push(now);
            true
        }
    });
    if !allowed {
        return;
    }
    let n = HIT_SFX.with(|cell| cell.borrow().len());
    if n == 0 {
        return;
    }
    // 随机挑一个实例（用时间做伪随机，不引入 rand 依赖）
    let idx = (((now * 977.13).fract().abs() * n as f64) as usize).min(n - 1);
    HIT_TIMES.with(|cell| {
        if cell.borrow().len() <= max && cfg!(debug_assertions) {
            eprintln!(
                "[water] {tag}音效 #{idx} @ t={now:.3} volume={volume:.2}（并发 {} / 上限 {max}）",
                cell.borrow().len()
            );
        }
    });
    SFX_EVENTS.with(|cell| {
        let mut v = cell.borrow_mut();
        if v.len() < SFX_EVENT_MAX {
            v.push((now, idx, volume));
        }
    });
    HIT_SFX.with(|cell| {
        // 渲染期间只记事件、不实时出声（见 `crate::ext::audio_silent`）—— 成片里的进水音轨由
        // 上面那圈 `SFX_EVENTS` 日志在渲染结束后离线合成。
        if crate::ext::audio_silent() {
            return;
        }
        if let Some(sfx) = cell.borrow_mut().get_mut(idx) {
            let _ = sfx.play(sasa::PlaySfxParams { amplifier: volume });
        }
    });
}


// ---------------------------------------------------------------------------
// 「屏幕进水」的按键水下状态（原来散在 `core/note.rs` 的 `Note` 上）
// ---------------------------------------------------------------------------

/// 每个按键一份：血量、泡水时长、受伤闪红 / 死亡动画的剩余与总时长、待判 MISS 标记。
///
/// 原来这 8 个字段直接挂在 `Note` 上，更新逻辑也写在 `Note::update_water` 里；现在整块挪到
/// 本模块，`Note` 只留一个 `pub water: WaterState` 字段，更新由 [`update`] 负责。
#[derive(Clone, Copy, Debug)]
pub struct WaterState {
    /// 剩余血量；`< 0` 表示本局还没初始化
    pub hp: f32,
    /// 已经在水里连续泡了多久（秒），用来实现「入水后一段时间才溺水」
    pub in_water: f32,
    /// 受伤闪红的剩余时长（秒）
    pub flash: f32,
    /// 死亡动画的剩余时长（秒）；`> 0` 期间不消失、常红、倾斜、淡出
    pub death: f32,
    /// 死亡动画的总时长（秒），用来算倾斜 / 淡出进度
    pub death_total: f32,
    /// 上一帧的谱面时间（秒）；`< 0` 表示还没记过
    pub last: f64,
    /// 下一次扣血（受伤事件）的倒计时（秒）
    pub hurt_cd: f32,
    /// 血量归零后等 `Judge::update` 判 MISS 的标记
    pub pending_miss: bool,
}

/// **注意**：默认值不是 0，而是两个哨兵 —— `hp = -1.`（本局还没初始化，首次进屏幕时取
/// `water_hp`）与 `last = -1.`（还没记过时间，第一帧只对表不扣血）。抽离成 `WaterState` 时
/// 若直接用 `#[derive(Default)]`，两个哨兵都变成 `0.`：`hp` 一上来就 `<= 0.`，`update` 会在
/// 第 279 行直接 `return`，于是**永远不会溺水**（水层还在画，按键却永不扣血）。
impl Default for WaterState {
    fn default() -> Self {
        Self {
            hp: -1.,
            in_water: 0.,
            flash: 0.,
            death: 0.,
            death_total: 0.,
            last: -1.,
            hurt_cd: 0.,
            pending_miss: false,
        }
    }
}

/// 「屏幕进水」：按键血量。
///
/// 规则：按键只要**出现在屏幕里**（中心点落在屏幕范围内）就按 `water_damage_interval` 的节奏
/// 扣血（每这么多秒扣 `water_damage` 点）；每次扣血算一次「受伤」（闪红 + 随机 `hit` 音效）；
/// **已经判定过的按键（`JudgeStatus::Judged`）不再扣血**；血量归零则进入死亡
/// 动画：常红 + 倾斜 + 淡出 + 白色粒子，并把 `water_pending_miss` 交给 `Judge::update`
/// 判一次 MISS（AUTOPLAY 也一样）。
///
/// 位置公式与 `Note::render` 完全一致（同一套 `base = height - line_height`），否则
/// 「在屏幕里」会和眼睛看到的差一截。
pub fn update(note: &mut Note, res: &mut Resource, parent_tr: &Matrix, ctrl_obj: &mut CtrlObject, line_height: f64) {
    if !res.config.water_screen {
        note.water.last = -1.;
        note.water.in_water = 0.;
        return;
    }
    if note.water.last < 0. {
        // 第一帧只对表、不扣血
        note.water.last = res.time;
        if note.water.hp < 0. {
            note.water.hp = res.config.water_hp.max(0.);
        }
        return;
    }
    let dt = (res.time - note.water.last).clamp(0., 0.25) as f32;
    note.water.last = res.time;
    note.water.flash = (note.water.flash - dt).max(0.);
    note.water.hurt_cd = (note.water.hurt_cd - dt).max(0.);
    if note.water.death > 0. {
        note.water.death = (note.water.death - dt).max(0.);
        return;
    }
    if note.water.hp < 0. {
        note.water.hp = res.config.water_hp.max(0.);
    }
    if note.water.hp <= 0. {
        return;
    }
    note.init_ctrl_obj(ctrl_obj, line_height);
    let aspect_ratio = res.aspect_ratio as f64;
    let spd = note.speed * ctrl_obj.y.now_opt().unwrap_or(1.) as f64;
    let line_height = line_height / aspect_ratio * spd;
    let height = note.height / aspect_ratio * spd;
    let base = (height - line_height) as f32;
    let transform = *parent_tr * note.now_transform(res, ctrl_obj, base, 0., true, true);
    // 是否「画在屏幕里」必须与渲染用同一套坐标：`render` 的 `draw` 闭包里是
    // `res.world_to_screen(Point::default())`（吃相机缩放 `chart_ratio`、进入动画与视口）
    // 再比 `|x| ≤ 1.15 × chart_ratio`、`|y| × chart_ratio × aspect ≤ 1.01`。
    // 以前这里直接拿**模型空间**的原点比 1.15 / 1.01 —— 坐标系不同（谱面缩放
    // `chart_ratio` ≠ 1 时差很多），于是「明明画在水里」的按键会被判成不在水里，
    // 表现就是长时间不扣血（表演用 Drag 尤其明显）。
    let mut on_screen = false;
    res.with_model(transform, |res: &mut Resource| {
        let pt = res.world_to_screen(Point::default());
        on_screen = pt.x.abs() <= 1.15 * res.config.chart_ratio
            && pt.y.abs() * res.config.chart_ratio * res.aspect_ratio <= 1.01;
    });
    // 长条（Hold）判定之后本体是贴在判定线上的：音头坐标可能已经飞出屏幕，但条身还画在
    // 水里。这里补一个「按键停在判定线上」的探针点 —— 与 `render_force_linger` 里 Hold 分支
    // 用的是同一个 `base = 0.` 变换 —— 只要它还画在屏幕里就算在水里，否则长条一判定就不再扣血。
    if !on_screen && res.time >= note.time {
        if let NoteKind::Hold { .. } = note.kind {
            let line_tr = *parent_tr * note.now_transform(res, ctrl_obj, 0., 0., true, false);
            res.with_model(line_tr, |res: &mut Resource| {
                let pt = res.world_to_screen(Point::default());
                on_screen = pt.x.abs() <= 1.15 * res.config.chart_ratio
                    && pt.y.abs() * res.config.chart_ratio * res.aspect_ratio <= 1.01;
            });
        }
    }
    if !on_screen {
        // 不在水里：这一帧都不累加，但**是否清零**分两类 ——
        // * Click / Drag / Flick：不清零。表演用的 Drag 会被 `moveEvents` 带出屏幕再带
        //   回来，一出去就把泡过的时间清零的话，它长期在水里也永远攒不够
        //   `water_drown_delay`，表现就是「长时间未扣血」。
        // * Hold：清零。长条会长时间贴在判定线附近进进出出，累计口径会让它一进水就
        //   立刻淹死（否则「扣血一次就直接死了」），维持原来的「出屏重新计时」。
        if matches!(note.kind, NoteKind::Hold { .. }) {
            note.water.in_water = 0.;
        }
        return;
    }
    // 「已经判定过的按键不再溺水」：判定过的按键分数 / combo 早就结算了，再让它掉血、变红、
    // 播死亡音效（尤其判定后还被画在屏幕上的那些）会被当成 BUG ⇒ 直接不扣血。
    // 判定**之前**一律照旧（表演用的按键长期泡在水里也照样溺水）；正被按住的长条是
    // `JudgeStatus::Hold` 而不是 `Judged`，仍然会溺水。
    if matches!(note.judge, JudgeStatus::Judged) {
        return;
    }
    // 「入水后一段时间才溺水」：在水里连续泡够 `water_drown_delay` 秒才开始扣血
    note.water.in_water += dt;
    if note.water.in_water < res.config.water_drown_delay.max(0.) {
        return;
    }
    // 扣血节奏：泡够入水延迟之后每 `water_damage_interval` 秒扣一次 `water_damage`，
    // 同时闪一次红 + 播一次受伤音效（间隔夹到 ≥ 0.01 秒，防止配置成 0 时一帧扣好多次）。
    if note.water.hurt_cd > 0. {
        return;
    }
    note.water.hurt_cd = res.config.water_damage_interval.max(0.01);
    note.water.hp -= res.config.water_damage.max(0.);
    note.water.flash = WATER_FLASH_TIME;
    play_hurt(res);
    if note.water.hp <= 0. {
        note.water.hp = 0.;
        let total = res.config.water_death_time.max(0.05);
        // 所有类型（含 Hold 长条）都播同一套溺水死亡动画：常红 + 倾斜 + 淡出 + 白色粒子。
        // 注意这不是官方那套「判定后长条淡化」——`water_death > 0` 期间 `render` 会直接改写
        // `color.a` 与颜色，官方那条淡化残影被绕过（见 `note.rs` 的 draw 闭包）。
        note.water.death = total;
        note.water.death_total = total;
        note.water.pending_miss = !matches!(note.judge, JudgeStatus::Judged);
        // 记下这个按键溺水死了：渲染器据此在视频音轨里去掉它自己的打击音效
        log_drowned_note(note);
        // 记下这个按键溺水死了：渲染器据此在视频音轨里去掉它自己的打击音效
                // 死亡时也播一次受击音效（与受伤共用素材、音量与并发上限）
        play_death(res);
        let count = res.config.water_death_particles.max(0.) as usize;
        if count > 0 && res.config.particle {
            let old = res.emitter.particle_count;
            res.emitter.particle_count = count;
            // 按需求：溺水死亡后**只有散落的白色粒子**，没有中心特效
            // （`emit_squares_at_origin` 只发扩散散落粒子，不发中心标记粒子）。
            res.with_model(transform, |res| res.emit_squares_at_origin(WHITE));
            res.emitter.particle_count = old;
        }
    }
}
