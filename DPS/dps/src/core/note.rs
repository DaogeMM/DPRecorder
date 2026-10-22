use super::{chart::ChartSettings, BpmList, CtrlObject, JudgeLine, Matrix, Object, Point, Resource, Vector};
use crate::{
    core::{Anim, HEIGHT_RATIO},
    ext::{parse_alpha, WHITE_TEXTURE},
    judge::JudgeStatus,
    parse::RPE_HEIGHT,
    ui::Ui,
};

pub use crate::judge::HitSound;
use macroquad::prelude::*;

const FADEOUT_TIME: f64 = 0.16;
const BAD_TIME: f64 = 0.5;

#[derive(Clone, Debug)]
pub enum NoteKind {
    Click,
    Hold {
        end_time: f64,
        end_height: f64,
        end_speed: Option<f64>,
    },
    Flick,
    Drag,
}

impl NoteKind {
    pub fn order(&self) -> i8 {
        match self {
            Self::Hold { .. } => 0,
            Self::Drag => 1,
            Self::Click => 2,
            Self::Flick => 3,
        }
    }
}

pub struct Note {
    pub object: Object,
    pub kind: NoteKind,
    pub hitsound: HitSound,
    pub time: f64,
    pub height: f64,
    pub speed: f64,

    pub above: bool,
    pub multiple_hint: bool,
    pub fake: bool,
    pub judge: JudgeStatus,
    pub judge_scale: f64,
    pub color: Anim<Color>,
    pub hit_fx_color: Anim<Color>,
    pub protected: bool,
    /// 「纯配置谱面」：这个按键在**判定时刻**的屏幕位置落在谱面的噪域里，于是在它周围画一块噪域矩形
    /// （「反向强制定位」那个观感）。标记在转换时算好（`core::pure_config`），之后不再变；只用于渲染，
    /// 不进二进制谱面。
    pub pure_config_noise: bool,
    /// 「屏幕进水」：这个按键的水下状态（血量 / 泡水时长 / 受伤闪红 / 死亡动画 …）。
    /// 状态定义与更新逻辑都在 `core::water`（`WaterState` / `water::update`）。
    pub water: crate::core::water::WaterState,
}

unsafe impl Sync for Note {}
unsafe impl Send for Note {}

pub struct RenderConfig<'a> {
    pub settings: &'a ChartSettings,
    pub ctrl_obj: &'a mut CtrlObject,
    pub line_height: f64,
    pub appear_before: f64,
    pub invisible_time: f64,
    pub draw_below: bool,
    pub incline_sin: f32,
    /// 判定线在这一帧的旋转角（度），只给按键调试文本用。
    pub line_angle: f32,
}

/// 「按键调试」（`chart_debug_note > 0`）画在按键旁边的那段文本。
///
/// 内容由 `chart_debug_note_fields` 决定：逗号分隔的名单，大小写不敏感、未知项忽略。可选
/// `line`（判定线序号）、`time`（判定时刻，长按再带尾端时刻）、`height`（判定线高度，长按带
/// 尾端高度）、`base`（离判定线的高度差）、`angle`（判定线这一帧的旋转角，单位度）、`speed`、
/// `above`、`fake`。第一行是数值，第二行只放 `speed` / `above` / `below` / `fake` 这些标记，
/// 没有标记就不留空行。名单为空时只剩纯空白串，调用方仍按原样画（等于不显示文本）。
fn debug_note_text(res: &Resource, note: &Note, line_id: usize, base: f64, angle: f32) -> String {
    let fields = res.config.chart_debug_note_fields.as_str();
    let on = |name: &str| fields.split(',').any(|it| it.trim().eq_ignore_ascii_case(name));
    let (end_time, end_height, end_speed) = match &note.kind {
        NoteKind::Hold {
            end_time,
            end_height,
            end_speed,
        } => (Some(*end_time), Some(*end_height), *end_speed),
        _ => (None, None, None),
    };
    let mut text = String::new();
    if on("line") {
        text.push_str(&format!("[{}] ", line_id));
    }
    if on("time") {
        match end_time {
            Some(end) => text.push_str(&format!("t:{:.2}({:.2}) ", note.time, end)),
            None => text.push_str(&format!("t:{:.2} ", note.time)),
        }
    }
    if on("height") {
        match end_height {
            Some(end) => text.push_str(&format!("h:{:.2}({:.2}) ", note.height, end)),
            None => text.push_str(&format!("h:{:.2} ", note.height)),
        }
    }
    if on("base") {
        text.push_str(&format!("[{:.2}] ", base));
    }
    if on("angle") {
        text.push_str(&format!("angle:{:.1} ", angle));
    }
    let mut marks = String::new();
    // 速度和原来一样只在「不是默认值」时出现：长按连尾端速度一起显示。
    let hide_speed = note.speed == 1.0 && end_speed.is_none();
    if on("speed") && !hide_speed {
        marks.push_str(&format!(" v: {}", note.speed));
        if let Some(end) = end_speed {
            marks.push_str(&format!("({})", end));
        }
    }
    if on("above") {
        marks.push_str(if note.above { " above" } else { " below" });
    }
    if on("fake") && note.fake {
        marks.push_str(" fake");
    }
    if !marks.is_empty() {
        text.push('\n');
        text.push_str(&marks);
    }
    text
}

fn draw_tex(res: &Resource, texture: Texture2D, order: i8, x: f32, y: f32, color: Color, mut params: DrawTextureParams, clip: bool) {
    let Vec2 { x: w, y: h } = params.dest_size.unwrap();
    if h < 0. {
        return;
    }
    let mut p = [Point::new(x, y), Point::new(x + w, y), Point::new(x + w, y + h), Point::new(x, y + h)];
    if clip {
        if y + h <= 0. {
            return;
        }
        if y <= 0. {
            let r = -y / (y + h);
            p[0].y = 0.;
            p[1].y = 0.;
            let mut source = params.source.unwrap_or_else(|| Rect::new(0., 0., 1., 1.));
            source.y += source.h * r;
            params.source = Some(source);
        }
    }
    params.flip_y ^= true;
    draw_tex_pts(res, texture, order, p, color, params);
}
fn draw_tex_pts(res: &Resource, texture: Texture2D, order: i8, p: [Point; 4], color: Color, params: DrawTextureParams) {
    let mut p = p.map(|it| res.world_to_screen(it));
    if p[0].x.min(p[1].x.min(p[2].x.min(p[3].x))) > 1. / res.config.chart_ratio
        || p[0].x.max(p[1].x.max(p[2].x.max(p[3].x))) < -1. / res.config.chart_ratio
        || p[0].y.min(p[1].y.min(p[2].y.min(p[3].y))) > 1. / res.config.chart_ratio
        || p[0].y.max(p[1].y.max(p[2].y.max(p[3].y))) < -1. / res.config.chart_ratio
    {
        return;
    }
    let Rect { x: sx, y: sy, w: sw, h: sh } = params.source.unwrap_or(Rect { x: 0., y: 0., w: 1., h: 1. });

    if params.flip_x {
        p.swap(0, 1);
        p.swap(2, 3);
    }
    if params.flip_y {
        p.swap(0, 3);
        p.swap(1, 2);
    }

    #[rustfmt::skip]
    let vertices = [
        Vertex::new(p[0].x, p[0].y, 0., sx     , sy     , color),
        Vertex::new(p[1].x, p[1].y, 0., sx + sw, sy     , color),
        Vertex::new(p[2].x, p[2].y, 0., sx + sw, sy + sh, color),
        Vertex::new(p[3].x, p[3].y, 0., sx     , sy + sh, color),
    ];
    res.note_buffer
        .borrow_mut()
        .push((order, texture.raw_miniquad_texture_handle().gl_internal_id()), vertices);
}

fn draw_center(res: &Resource, tex: Texture2D, order: i8, scale: f32, color: Color) {
    let hf = vec2(scale, tex.height() * scale / tex.width());
    draw_tex(
        res,
        tex,
        order,
        -hf.x,
        -hf.y,
        color,
        DrawTextureParams {
            dest_size: Some(hf * 2.),
            ..Default::default()
        },
        false,
    );
}

/// 判定范围：以音符中心为中心的半透明矩形。
///
/// - Tap / Hold：半宽 = `judge_range_width` × 屏幕高度 / 10（默认 1.9）。
/// - Flick / Drag：半宽 = `judge_range_width_flick` × 屏幕高度 / 10 —— **和 Tap / Hold 同一
///   单位**（默认 2.1，比 Tap / Hold 略宽一点），只是两类音符分开配。
/// - 长度一律 = `judge_range_length` × 屏幕宽度（世界坐标里屏幕宽度恒为 2.0，默认 5.0
///   即「5 倍屏幕宽」）。颜色 / 透明度 / 显现时间 / 总开关全部读配置。
///
/// 顶点顺序是 左下 → 右下 → 右上 → 左上，对应 NoteBuffer::push 里的索引 [0,1,2, 0,2,3]。
/// 它必须和音符共用同一套模型变换：调用点都写在音符的 with_model 作用域内，所以这里跟
/// draw_tex_pts 一样用 res.world_to_screen 把局部坐标过一遍当前模型栈（音符的位置 / 旋转 /
/// 判定线变换全都已经包含在里面了）。矩形中心取音符局部坐标 (0, y)。
///
/// order = -1 让它排在所有音符（0..=3）之前；判定线是立即绘制、不进缓冲区的，
/// 因此图层顺序为「判定线 < 判定范围 < 音符」。
fn draw_judge_range(res: &Resource, kind: &NoteKind, y: f32, alpha: f32) {
    // 屏幕尺寸（世界坐标单位）：相机 zoom = vec2(1., -config.aspect_ratio)（core/resource.rs:698），
    // 世界坐标要过 ±1/zoom 才进裁剪空间，所以可见横向恒为 ±1 ——宽度就是 2.0，与宽高比无关；
    // 纵向是 ±1/aspect_ratio，即高度 2/aspect_ratio。
    // 注意 res.aspect_ratio 是「宽/高」（core/resource.rs:877 取 min(配置, vp.宽/vp.高)），
    // 所以宽度**不能**写成 2. * res.aspect_ratio，那会把可见宽度高估 aspect_ratio 倍。
    let screen_height = 2. / res.aspect_ratio;
    let screen_width = 2.;
    let hw = if matches!(kind, NoteKind::Flick | NoteKind::Drag) {
        // Flick / Drag 与 Tap / Hold 同一单位：半宽 = 配置值 × 屏幕高度 / 10。
        res.config.judge_range_width_flick * screen_height / 10.
    } else {
        res.config.judge_range_width * screen_height / 10.
    };
    let hh = res.config.judge_range_length * screen_width / 2.;
    let p = [
        Point::new(-hw, y - hh),
        Point::new(hw, y - hh),
        Point::new(hw, y + hh),
        Point::new(-hw, y + hh),
    ];
    let p = p.map(|it| res.world_to_screen(it));
    let c = res.config.judge_range_color;
    let color = Color::new(c[0], c[1], c[2], alpha);
    #[rustfmt::skip]
    let vertices = [
        Vertex::new(p[0].x, p[0].y, 0., 0., 0., color),
        Vertex::new(p[1].x, p[1].y, 0., 1., 0., color),
        Vertex::new(p[2].x, p[2].y, 0., 1., 1., color),
        Vertex::new(p[3].x, p[3].y, 0., 0., 1., color),
    ];
    let tex = WHITE_TEXTURE.raw_miniquad_texture_handle().gl_internal_id();

    // 让位给噪域：噪域那份掩码的 R 通道 = 方块覆盖度（1 = 这里被噪域盖住），材质据此把自己
    // 的 alpha 削到 0（见 core/shaders/judge_range.glsl），也就是「噪域把判定范围的重叠部分
    // 抹去」。`u_view` 是掩码画布在帧缓冲里的矩形，掩码按视口烘、分辨率与帧缓冲不同。
    //
    // 画布是**上一帧**烘出来的（`Chart::render_block_area` 每帧重铺一次），因为判定范围在
    // `Chart::render` 里、噪域管线之前绘制，同帧的覆盖度还拿不到；播放中差一帧（1/60 s）
    // 肉眼看不出来，暂停 / 单帧导出时前后两帧的 placements 完全一样、画布已经收敛，
    // 所以导出结果与「同帧」等价。
    //
    // 拿不到画布时（噪域没开、或管线没建起来）照旧进 NoteBuffer，与音符一起批量绘制。
    // `judge_range_noise_occlusion` 关掉时整个跳过这条通路：判定范围照常叠在噪域之上。
    if res.config.judge_range_noise_occlusion {
        if let (Some(mat), Some(paint)) = (res.judge_range_mat, res.block_area_paint) {
            mat.set_texture("u_paint", paint);
            mat.set_uniform("u_view", crate::core::block_area::judge_paint_view());
            gl_use_material(mat);
            unsafe {
                let gl = get_internal_gl();
                gl.quad_gl.texture(None);
                gl.quad_gl.draw_mode(DrawMode::Triangles);
                gl.quad_gl.geometry(&vertices, &[0u16, 1, 2, 0, 2, 3]);
                get_internal_gl().flush();
            }
            gl_use_default_material();
            return;
        }
    }

    res.note_buffer.borrow_mut().push((-1, tex), vertices);
}

/// 「强制定位」：每个按键周围那个长方形。以音符局部坐标 `(0, y)` 为中心，
/// 半宽 = `block_area_force_width × 屏幕高度 / 10`（基准与判定范围一致，默认 1.9 ⇒ 和默认的
/// 判定范围一样宽），全高 = `block_area_force_height × 屏幕高度`（默认 0.2 ⇒ 20% 屏高）。
///
/// 它有两个作用：
/// - **恒**把四个角点（chart 世界坐标，`world_to_screen` 只过模型栈、和 `draw_judge_range`
///   同一套）推入 `Resource::force_position_quads`，噪域那层据此在掩码里把这块**完全挖掉**
///   （覆盖度 1.0，不像判定范围那样按 alpha 打折）；
/// - 打开「矩形着色」(`block_area_force_show`) 时，再用「判定范围颜色」把矩形本身半透明画
///   出来 —— 噪域压在最高层、矩形画在音符层，所以只有在被挖掉的那个洞里才看得见它，
///   正好用来核对洞的位置与大小。默认关闭 ⇒ 只挖洞、不着色。
///
/// `tint` 再决定这一段要不要着色：判定之后「延后视窗」里的洞是**在音符本身已经不再绘制**
/// 时补出去的（见 `Note::render` 开头那段），那时再着色会凭空多出一个方块，所以传 `false`。
///
/// `noise` 表明这个按键是「纯配置谱面」里被噪域盖住的那一类：矩形的大小改由
/// `pure_config_chart_noise_width` / `_height` 决定 —— 那一层只画在这些按键上，与「强制定位」
/// 满屏铺噪域的场合需要各自独立的大小。
fn draw_force_position_hole(res: &Resource, y: f32, tint: bool, noise: bool) {
    let screen_height = 2. / res.aspect_ratio;
    let (width, height) = if noise {
        (res.config.pure_config_chart_noise_width, res.config.pure_config_chart_noise_height)
    } else {
        (res.config.block_area_force_width, res.config.block_area_force_height)
    };
    let hw = width * screen_height / 10.;
    let hh = height * screen_height * 0.5;
    if hw <= 0. || hh <= 0. {
        return;
    }
    let p = [
        Point::new(-hw, y - hh),
        Point::new(hw, y - hh),
        Point::new(hw, y + hh),
        Point::new(-hw, y + hh),
    ];
    let p = p.map(|it| res.world_to_screen(it));
    res.force_position_quads
        .borrow_mut()
        .push(([[p[0].x, p[0].y], [p[1].x, p[1].y], [p[2].x, p[2].y], [p[3].x, p[3].y]], 1.));
    if !tint || !res.config.block_area_force_show {
        return;
    }
    let c = res.config.judge_range_color;
    let color = Color::new(c[0], c[1], c[2], res.config.judge_range_alpha);
    #[rustfmt::skip]
    let vertices = [
        Vertex::new(p[0].x, p[0].y, 0., 0., 0., color),
        Vertex::new(p[1].x, p[1].y, 0., 1., 0., color),
        Vertex::new(p[2].x, p[2].y, 0., 1., 1., color),
        Vertex::new(p[3].x, p[3].y, 0., 0., 1., color),
    ];
    let tex = WHITE_TEXTURE.raw_miniquad_texture_handle().gl_internal_id();
    res.note_buffer.borrow_mut().push((-1, tex), vertices);
}

impl Note {
    /// 打击特效的颜色：「Rr」演出的调试覆盖优先，其次是全局「打击特效颜色」（只管 Perfect），
    /// 都没有才用资源包里 `colorPerfectFx` / `colorGoodFx` 的颜色。
    pub(crate) fn hit_fx_color(res: &crate::core::resource::Resource, perfect: bool) -> Color {
        let fallback = if perfect { res.res_pack.info.fx_perfect() } else { res.res_pack.info.fx_good() };
        let hex = if res.config.rr_sp && !res.config.rr_hit_fx_color.is_empty() {
            &res.config.rr_hit_fx_color
        } else if perfect {
            &res.config.hitcolor
        } else {
            return fallback;
        };
        Self::hex_to_color(hex, fallback)
    }

    fn hex_to_color(hex: &str, fallback: Color) -> Color {
        let hex = hex.strip_prefix('#').unwrap_or(hex);
        if hex.len() != 6 {
            return fallback;
        }
        match u8::from_str_radix(&hex[0..2], 16) {
            Ok(r) => {
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(fallback.g as u8 * 255);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(fallback.b as u8 * 255);
                Color::new(r as f32 / 255., g as f32 / 255., b as f32 / 255., 1.0)
            }
            Err(_) => fallback,
        }
    }

    pub fn rotation(&self, line: &JudgeLine) -> f32 {
        line.object.rotation.now() + if self.above { 0. } else { 180. }
    }

    /// 判定范围这一帧该用的透明度；返回 None 表示不绘制。
    /// 只在判定前 `judge_range_time`（显现时间，配置项）秒内显示，其余时刻完全不画
    ///（也就是「只有按键判定前若干秒才开始画」）。
    /// 判定前 judge_range_time → judge_range_fade_time 之间做过渡：
    /// dt = judge_range_time 时全透明(alpha 0)，到 dt = judge_range_fade_time 时升到
    /// judge_range_alpha，之后一直保持 judge_range_alpha。
    /// `min_dt` 是允许的最早时间差（相对判定时刻）：普通按键传 0.，判定后就不再画；
    /// Hold 传 f64::NEG_INFINITY，长条在按住期间一直显示，直到 hold 结束。
    fn judge_range_alpha(&self, res: &Resource, min_dt: f64) -> Option<f32> {
        // 总开关：少了这个判断，判定范围会「始终启用」，界面上的开关形同虚设。
        if !res.config.render_judge_range {
            return None;
        }
        let dt = self.time - res.time;
        let show = res.config.judge_range_time;
        let fade = res.config.judge_range_fade_time;
        if dt > show || dt < min_dt {
            return None;
        }
        let base = res.config.judge_range_alpha;
        let alpha = if fade < show && dt > fade {
            let t = ((show - dt) / (show - fade)) as f32;
            base * t
        } else {
            base
        };
        Some(alpha)
    }

    pub fn update(
        &mut self,
        res: &mut Resource,
        parent_rot: f32,
        parent_tr: &Matrix,
        ctrl_obj: &mut CtrlObject,
        line_height: f64,
        bpm_list: &mut BpmList,
        index: usize,
    ) {
        crate::core::water::update(self, res, parent_tr, ctrl_obj, line_height);
        if self.time < res.config.play_start_time || res.disable_hit_fx {
            return;
        }
        // 颜色顺序：按键自带 → 调试覆盖 → 全局「打击特效颜色」→ 资源包里的 AP 色
        self.object.set_time(res.time);
        let color = if let JudgeStatus::Hold(perfect, ref mut at, ..) = self.judge {
            if res.time >= *at {
                let beat = 30. / bpm_list.now_bpm(if bpm_list.per_line_bpm_storage { index as f64 } else { self.time });
                //println!("{} {} {}", index, bpm_list.now_bpm(index as f32), beat);
                *at = res.time + beat * res.info.hold_particle_interval_ratio as f64 / res.config.speed as f64; //HOLD_PARTICLE_INTERVAL
                Some(if let Some(color) = self.hit_fx_color.now_opt() {
                    color
                } else {
                    Self::hit_fx_color(res, perfect && !res.config.all_good && !res.config.all_bad)
                })
            } else {
                None
            }
        } else {
            None
        };

        if let Some(color) = color {
            self.init_ctrl_obj(ctrl_obj, line_height);
            let rotation = if self.above { 0. } else { 180. };
            res.with_model(parent_tr * self.now_transform(res, ctrl_obj, 0., 0., false, false), |res| {
                res.emit_at_origin(parent_rot + rotation, color)
            });
        }
    }

    pub fn dead(&self) -> bool {
        if self.water.death > 0. {
            // 「屏幕进水」：死亡动画播完之前不许消失
            return false;
        }
        if self.water.death_total > 0. {
            // 「屏幕进水」：溺水死亡的动画播完就**直接消失** —— 不然会接着走原版「判定后长条
            // 淡化」那条路，在屏幕上继续画一根发白的半透明长条（要去掉长条死亡后的
            // 淡色效果）。`water_death_total` 只在溺水死亡触发时置为 `water_death_time`，
            // 之后不再清零 ⇒ 它 > 0 且 `death == 0` 就表示「动画已播完」。
            return true;
        }
        (!matches!(self.kind, NoteKind::Hold { .. }) || matches!(self.judge, JudgeStatus::Judged)) && self.object.dead()
        // && self.ctrl_obj.dead()
    }

    pub(crate) fn init_ctrl_obj(&self, ctrl_obj: &mut CtrlObject, line_height: f64) {
        ctrl_obj.set_height((self.height - line_height + self.object.translation.1.now() as f64 / self.speed) * RPE_HEIGHT as f64 / 2.);
    }

    pub fn now_transform(&self, res: &Resource, ctrl_obj: &CtrlObject, base: f32, incline_sin: f32, can_scale_x: bool, can_scale_y: bool) -> Matrix {
        let incline_val = 1. - incline_sin * (base * res.aspect_ratio + self.object.translation.1.now()) * RPE_HEIGHT / 2. / 360.;
        let mut tr = self.object.now_translation(res);
        tr.x *= incline_val * ctrl_obj.pos.now_opt().unwrap_or(1.);
        tr.y += base;
        let mut scale = self.object.scale.now_with_def(1.0, 1.0);
        if !can_scale_x {
            scale.x = 1.0;
        };
        scale.x *= ctrl_obj.size.now_opt().unwrap_or(1.0);
        if !res.info.note_uniform_scale || !can_scale_y {
            scale.y = 1.0;
        };
        scale.y *= ctrl_obj.size.now_opt().unwrap_or(1.0);
        let mut rot = self.object.now_rotation();
        if self.water.death > 0. {
            // 「屏幕进水」死亡：随进度倾斜
            let total = self.water.death_total.max(1e-3);
            let progress = 1. - (self.water.death / total).clamp(0., 1.);
            rot = rot * Matrix::new_rotation(crate::core::water::WATER_DEATH_TILT * progress);
        }
        rot.append_nonuniform_scaling(&scale).append_translation(&tr)
    }

    /// 「强制定位」的**延后视窗**（`block_area_force_delay`，默认 0.3 秒）：判定完的按键周围
    /// 那个洞还要再留一段时间才被噪域重新盖住。
    ///
    /// 为什么单独开一个方法：判定掉的按键（非 Hold）在 `Line::render` 的循环里会被
    /// `continue` 跳过 —— `Note::render` 根本不会被调用，写在里面的推洞逻辑自然也不会跑，
    /// 洞会跟着音符一起立刻消失（实测过：延后视窗完全无效）。所以那个循环在 `continue`
    /// 之前显式调用本方法，只把洞推出去、**不着色**（音符本身已经没了，着色会凭空多出一个
    /// 方块）；洞的几何与 `render` 里那段完全一致。
    ///
    /// Hold 的判定到长条尾端才结束：按住期间 `end_time > res.time`、差值为负 ⇒ 这里不触发，
    /// 正被按住的 Hold 由 `render` 里那段负责（Hold 不会被那个 `continue` 跳过）。
    /// 这一帧要不要围绕这个按键推出「强制定位」的四边形。
    ///
    /// 「纯配置谱面」下同一个四边形换了个用途：它是按键噪域的几何，只有被 [`Note::pure_config_noise`]
    /// 标中的按键才推，此时**不看** `block_area_force_position`。强制定位要铺一整层全屏噪域再由按键挖洞，
    /// 而纯配置模式把那一层关掉了、只把四边形本身当噪域画（见 `Chart::render_block_area`），两个开关叠在
    /// 一起就会让每个按键周围都糊上一块噪域。
    fn force_position_drawn(&self, res: &Resource) -> bool {
        // 「定轨谱面」不画噪域，四边形也跟着不推（`Chart::render_block_area` 会把已经攒下的清掉）。
        if res.config.fixed_lane_chart {
            return false;
        }
        if res.config.pure_config_chart {
            self.pure_config_noise
        } else {
            res.config.block_area_force_position
        }
    }

    pub fn render_force_linger(&self, res: &mut Resource, config: &mut RenderConfig) {
        // 「纯配置谱面」里被标了噪域的按键也走这条通路 —— 那些按键周围要单独长出噪域矩形，而全屏噪域
        // 那一层是关着的（见 `Chart::render_block_area`）。
        if !self.force_position_drawn(res) || self.fake {
            return;
        }
        let delay = if self.pure_config_noise {
            res.config.pure_config_chart_noise_delay
        } else {
            res.config.block_area_force_delay
        } as f64;
        if delay <= 0. {
            return;
        }
        let force_end = match self.kind {
            NoteKind::Hold { end_time, .. } => end_time,
            _ => self.time,
        };
        let after = res.time - force_end;
        if !(after > 0. && after <= delay) {
            return;
        }
        let aspect_ratio = res.aspect_ratio as f64;
        let ctrl_obj = &mut config.ctrl_obj;
        self.init_ctrl_obj(ctrl_obj, config.line_height);
        let spd = self.speed * ctrl_obj.y.now_opt().unwrap_or(1.) as f64;
        let line_height = config.line_height / aspect_ratio * spd;
        let height = self.height / aspect_ratio * spd;
        let base = height - line_height;
        match self.kind {
            NoteKind::Hold { .. } => {
                let h = if self.time <= res.time { line_height } else { height };
                let bottom = (h - line_height) as f32;
                res.with_model(self.now_transform(res, ctrl_obj, 0., 0., true, false), |res| {
                    draw_force_position_hole(res, bottom, false, self.pure_config_noise)
                });
            }
            _ => {
                res.with_model(self.now_transform(res, ctrl_obj, base as f32, config.incline_sin, true, true), |res| {
                    draw_force_position_hole(res, 0., false, self.pure_config_noise)
                });
            }
        }
    }

    pub fn render(
        &self,
        ui: &mut Ui,
        res: &mut Resource,
        config: &mut RenderConfig,
        bpm_list: &mut BpmList,
        line_set_debug_alpha: bool,
        line_id: usize,
        height_above: f64,
        height_below: f64,
    ) {
        let ctrl_obj = &mut config.ctrl_obj;
        self.init_ctrl_obj(ctrl_obj, config.line_height);

        // 「待判按键」染色那一趟（`Chart::render_pending_tint`）：整趟只画判定时刻最靠前的
        // 那个按键，别的一个都不画；而且会把判定范围 / 强制定位挖洞这些**副作用**全部跳过
        //（本体那一趟已经做过一遍了，再做一遍就会重复）。
        let tint = res.pending_tint.get();
        if tint && (self.fake || self.time != res.pending_note_time.get()) {
            return;
        }

        // 「待判按键」：记下**判定时刻最靠前**的那个还没判定的按键（每个按键顺手收敛一次最小值，
        // 比在 `Chart` 里再遍历一遍全谱便宜，也早于本函数所有提前返回，不会漏掉最近的那个）。
        // 真正染色的是 `Game::render` 里噪域之后那一趟 —— 于是染色压在噪域之上；
        // 而且那一趟是让按键用**自己的**绘制代码再画一遍，
        // 旋转 / 移动时间 / 长条头尾 / 双押贴图全都天然一致。
        if !tint && res.config.render_pending_note && !self.fake && self.time >= res.time && self.time < res.pending_note_time.get() {
            res.pending_note_time.set(self.time);
        }

        let aspect_ratio = res.aspect_ratio as f64;
        let spd = self.speed * ctrl_obj.y.now_opt().unwrap_or(1.) as f64;
        let line_height = config.line_height / aspect_ratio * spd;
        let height = self.height / aspect_ratio * spd;
        let base = height - line_height;

        // 判定范围只看判定时刻，不吃音符自身的出现 / 淡出规则：appear_before 往往不到 1 秒，
        // 跟着音符一起画的话 judgeRangeTime → judgeRangeFadeTime 的淡入根本看不到。
        // 所以这里画在下面所有提前返回之前（fake 音符本来就不判定，不给它画）。
        if !self.fake && !tint {
            match self.kind {
                NoteKind::Hold { .. } => {
                    if let Some(alpha) = self.judge_range_alpha(res, f64::NEG_INFINITY) {
                        let h = if self.time <= res.time { line_height } else { height };
                        let bottom = (h - line_height) as f32; //StartY
                        res.with_model(self.now_transform(res, ctrl_obj, 0., 0., true, false), |res| draw_judge_range(res, &self.kind, bottom, alpha));
                    }
                }
                _ => {
                    if let Some(alpha) = self.judge_range_alpha(res, 0.) {
                        res.with_model(self.now_transform(res, ctrl_obj, base as f32, config.incline_sin, true, true), |res| {
                            draw_judge_range(res, &self.kind, 0., alpha)
                        });
                    }
                }
            }
        }

        if config.appear_before.is_finite() && self.water.death <= 0. {
            //if config.appear_before.is_finite() && !matches!(self.kind, NoteKind::Hold { .. }) {
            let beat = bpm_list.beat(self.time);
            let time = bpm_list.time_beats(beat - config.appear_before);
            if time > res.time {
                return;
            }
        }

        let hex_str = &res.config.notecolor;
        let global_default_color = Self::hex_to_color(hex_str, WHITE);
        let mut color = self.color.now_opt().unwrap_or(global_default_color);

        let alpha = self.object.now_alpha().max(0.);
        color.a = parse_alpha(color.a * alpha, 1.0, 0.2, res.config.chart_debug_note > 0.);

        if config.invisible_time.is_finite() && self.time - config.invisible_time < res.time && self.water.death <= 0. {
            if res.config.chart_debug_note > 0. {
                color.a *= 0.2;
            } else {
                return;
            }
        }

        // 「待判按键」染色那一趟：只把颜色换成配置里的染色颜色，alpha 照旧走上面的淡入淡出，
        // 于是「淡出 / 溺水 / fade」这些规则和本体完全一致，旋转和移动时间也一模一样。
        if tint {
            let c = res.config.pending_note_color;
            color.r = c[0];
            color.g = c[1];
            color.b = c[2];
        }

        let cover_base = if !res.info.hold_partial_cover {
            height + self.object.translation.1.now() as f64 / aspect_ratio - line_height
        } else {
            match self.kind {
                NoteKind::Hold {
                    end_time: _,
                    end_height,
                    end_speed: _,
                } => {
                    let end_height = end_height / aspect_ratio;
                    end_height + self.object.translation.1.now() as f64 / aspect_ratio - line_height
                }
                _ => height + self.object.translation.1.now() as f64 / aspect_ratio - line_height,
            }
        };

        if res.config.alpha_tint {
            if color.a <= 0.5 {
                color.r *= 0.6;
                color.g *= 0.8;
                color.b *= 1.0;
            } else if color.a < 1.0 {
                color.r *= 1.0;
                color.g *= 0.7;
                color.b *= 0.9;
            }
            color.a = res.alpha;
        } else {
            color.a *= parse_alpha(ctrl_obj.alpha.now_opt().unwrap_or(1.), res.alpha, 0.2, res.config.chart_debug_note > 0.);
        }

        // 「屏幕进水」：受伤闪红 / 死亡常红 + 淡出。**必须放在最后**：上面那些透明度
        // （`object.now_alpha`、`res.alpha`、`ctrl_obj.alpha`）在判定之后会被算成 0，
        // 先染色再被乘掉的话，长条溺水就只剩一团看不见的东西。
        if res.config.water_screen {
            if self.water.death > 0. {
                let total = self.water.death_total.max(1e-3);
                color.r = color.r * 0.15 + 1. * 0.85;
                color.g *= 0.15;
                color.b *= 0.15;
                color.a = (self.water.death / total).clamp(0., 1.);
            } else if self.water.flash > 0. {
                let k = (self.water.flash / crate::core::water::WATER_FLASH_TIME).clamp(0., 1.) * 0.8;
                color.r = color.r * (1. - k) + 1. * k;
                color.g *= 1. - k;
                color.b *= 1. - k;
                color.a = color.a.max(0.75);
            }
        }

        let is_covered = cover_base <= -0.001;
        // && ((res.time - FADEOUT_TIME >= self.time) || (self.fake && res.time >= self.time) || (self.time > res.time && base <= -1e-5))
        if !config.draw_below && self.water.death <= 0.
            && ((res.time - FADEOUT_TIME >= self.time && !matches!(self.kind, NoteKind::Hold { .. })) || (self.time > res.time && is_covered))
        // && self.speed != 0.
        {
            if res.config.chart_debug_note > 0. {
                color.a *= 0.2;
            } else {
                return;
            }
        }
        if line_set_debug_alpha {
            color.a *= 0.4;
        }
        if res.config.fade > 0. && self.water.death <= 0. {
            let base = base as f32;
            let over = res.config.fade * 0.8;
            if base > res.config.fade {
                return;
            } else if base > over {
                color.a *= (res.config.fade - base) / (res.config.fade - over);
            }
        } else if res.config.fade < 0. && self.water.death <= 0. {
            let base = base as f32;
            let fade_out = res.config.fade.abs();
            let over = fade_out * 0.8;
            if base < over {
                return;
            } else if base < fade_out {
                color.a *= (base - over) / (fade_out - over);
            }
        }

        // 「强制定位」：这个按键周围的长方形（挖洞 + 可选着色）。
        //
        // 位置选在「淡出 / 遮挡」这些提前返回**之后**：只有真的会画出来的按键才配一个洞，
        // 否则噪域里会在没有按键的地方空出一块背景，与「除了按键，哪里都是噪域」不符。
        // fake 音符不是按键（和判定范围一样不给它挖洞）。几何与判定范围完全同一套：
        // 局部 `(0, y)`、同一个模型栈、同样的屏幕尺寸基准；取点也照抄判定范围
        //（长条取头部那一端，普通按键取音符中心）。
        //
        // 必须写在下面 `&res.res_pack...`（`style`）之前：`with_model` 要 `&mut res`，
        // 而 `style` 借的是 `res`。
        //
        // 时间窗（`block_area_force_time`）：只有「判定时刻已经进入本值以内」的按键才挖洞
        //（`dt = self.time - res.time <= 本值`）。判定还早得很的按键不再挖洞，远端于是被
        // 噪域盖住。
        //
        // 延后视窗（`block_area_force_delay`，默认 0.3 秒）：判定**完成之后**洞还会再停留
        // 本值这么长时间，然后被噪域重新盖住（`res.time - 判定时刻 <= 本值`）。Hold 的判定
        // 到长条尾端才结束，所以那里取 `end_time` —— 整条按住期间差值恒为负，洞一直在；
        // 松手（尾端过去）之后再留 `block_area_force_delay` 秒。
        //
        // 「纯配置谱面」下同一个矩形换了个用途：被 `Note::pure_config_noise` 标中的按键，这里推出去的
        // 四边形会被 `block_area::native` 当成**噪域本体**（上游传空方块列表），于是按键周围自己长出一块
        // 噪域，而屏幕上别处没有 —— 时间窗与延后视窗改由这一层自己的
        // `pure_config_chart_noise_time` / `_delay` 决定，与「强制定位」那两项互不影响。
        let (force_time, force_delay) = if self.pure_config_noise {
            (res.config.pure_config_chart_noise_time, res.config.pure_config_chart_noise_delay)
        } else {
            (res.config.block_area_force_time, res.config.block_area_force_delay)
        };
        let force_dt = self.time - res.time;
        let force_end = match self.kind {
            NoteKind::Hold { end_time, .. } => end_time,
            _ => self.time,
        };
        let force_after = res.time - force_end;
        if !tint
            && self.force_position_drawn(res)
            && !self.fake
            && force_dt <= force_time as f64
            && force_after <= force_delay as f64
        {
            match self.kind {
                NoteKind::Hold { .. } => {
                    let h = if self.time <= res.time { line_height } else { height };
                    let bottom = (h - line_height) as f32;
                    res.with_model(self.now_transform(res, ctrl_obj, 0., 0., true, false), |res| {
                        draw_force_position_hole(res, bottom, true, self.pure_config_noise)
                    });
                }
                _ => {
                    res.with_model(self.now_transform(res, ctrl_obj, base as f32, config.incline_sin, true, true), |res| {
                        draw_force_position_hole(res, 0., true, self.pure_config_noise)
                    });
                }
            }
        }
        let scale = (if res.config.render_double_hint && self.multiple_hint {
            res.res_pack.note_style_mh.click.width() / res.res_pack.note_style.click.width()
        } else {
            1.0
        }) * res.note_width;
        let order = self.kind.order();
        let style = if res.config.render_double_hint && self.multiple_hint {
            &res.res_pack.note_style_mh
        } else {
            &res.res_pack.note_style
        };
        let draw = |res: &mut Resource, tex: Texture2D| {
            let mut color = color;
            if !config.draw_below && self.water.death <= 0. {
                color.a *= ((self.time - res.time).min(0.) / FADEOUT_TIME + 1.) as f32;
            }
            // 「屏幕进水」：溺水死亡的按键不看自身的透明度（判定过的按键早就透明了、对象动画
            // 也可能把 alpha 收到 0），一律按死亡进度淡出，否则普通按键的溺水动画根本看不见。
            if self.water.death > 0. {
                color.a = (self.water.death / self.water.death_total.max(1e-3)).clamp(0., 1.);
            }
            res.with_model(self.now_transform(res, ctrl_obj, base as f32, config.incline_sin, true, true), |res| {
                // 「激进谱面」去重只在普通那一趟生效：染色那一趟再数一次的话，待判按键那个位置的
                // 计数会被多算一份，附近的按键会被提前剔掉。
                if res.config.aggressive_note && !tint {
                    let pt = res.world_to_screen(Point::default());
                    if pt.x.abs() > 1.15 * res.config.chart_ratio || pt.y.abs() * res.config.chart_ratio * res.aspect_ratio > 1.01 {
                        return;
                    }
                    let roughly_pos = ((pt.x * 200.0).round() as i32, (pt.y * 200.0).round() as i32);
                    let count = res.note_pos_map.entry(roughly_pos).or_insert(0);
                    if *count < 2 {
                        *count += 1;
                    } else {
                        return;
                    }
                }
                draw_center(res, tex, order, scale, color);
            });
        };
        match self.kind {
            NoteKind::Click => {
                if self.fake && res.time >= self.time {
                    return;
                };
                draw(res, *style.click);
            }
            NoteKind::Hold {
                end_time,
                end_height,
                end_speed,
            } => {
                if self.fake && res.time >= end_time {
                    return;
                };
                // 「屏幕进水」：溺水死亡的长条仍然画长条本体的红色死亡动画（整条变红 + 倾斜 +
                // 淡出），只是不再被判定后的 0.5 倍透明度压暗（见下面 `Judged` 的守卫）。
                res.with_model(self.now_transform(res, ctrl_obj, 0., 0., true, false), |res| {
                    if matches!(self.judge, JudgeStatus::Judged) && self.water.death <= 0. {
                        // miss
                        color.a *= 0.5;
                    }
                    if res.time >= end_time {
                        return;
                    }

                    let end_height = end_height / aspect_ratio * spd;
                    let time = if res.time >= self.time { res.time } else { self.time };

                    //let clip = !config.draw_below && config.settings.hold_partial_cover;
                    let clip = false;

                    let h = if self.time <= res.time { line_height } else { height };
                    let bottom = h - line_height; //StartY
                    let top = if let Some(end_spd) = end_speed {
                        let end_spd = end_spd * ctrl_obj.y.now_opt().unwrap_or(1.) as f64;
                        if end_spd == 0. {
                            if res.config.chart_debug_note > 0. {
                                color.a *= 0.2;
                            } else {
                                return;
                            }
                        }

                        let hold_height = end_height - height;
                        let hold_line_height = (time - self.time) * end_spd / aspect_ratio / HEIGHT_RATIO;
                        bottom + hold_height - hold_line_height
                    } else {
                        end_height - line_height
                    };

                    let style = if res.config.render_double_hint && self.multiple_hint {
                        &res.res_pack.note_style_mh
                    } else {
                        &res.res_pack.note_style
                    };

                    let tex = &style.hold;
                    let ratio = style.hold_ratio();
                    let is_negative_length = top - bottom < 0.;
                    let flip_y = res.info.negative_length_hold && (config.draw_below || !is_covered) && is_negative_length;
                    let real_h = if flip_y { bottom - top } else { top - bottom } as f32;
                    // 「定轨谱面」关掉「长条长度」时只留一小截身体：高度按头部（hold 的头那一片）
                    // 自身的显示高度来算，看上去就是个点按，判定时长不变。
                    let short = res.config.fixed_lane_chart && !res.config.fixed_lane_hold_length;
                    let head = style.hold_head_rect();
                    let head_h = head.h / head.w * scale * ratio * 2.;
                    let body_h = if short { real_h.min(head_h) } else { real_h };
                    let body_y = if flip_y { bottom as f32 - body_h } else { bottom as f32 };
                    // body：溺水死亡的动画期间叠 4 遍 —— 条身贴图本身带半透明（单画一遍时红色
                    // 看着发灰、像半透明），叠 4 遍后不透明度约 97%，是一条实心的红条；活着时只画一遍。
                    for _ in 0..(if self.water.death > 0. { 4 } else { 1 }) {
                        draw_tex(
                            res,
                            **(if res.res_pack.info.hold_repeat {
                                style.hold_body.as_ref().unwrap()
                            } else {
                                tex
                            }),
                            order,
                            -scale,
                            body_y,
                            color,
                            DrawTextureParams {
                                source: Some({
                                    if res.res_pack.info.hold_repeat {
                                        let hold_body = style.hold_body.as_ref().unwrap();
                                        let width = hold_body.width();
                                        let height = hold_body.height();
                                        Rect::new(0., 0., 1., body_h / scale / 2. * width / height)
                                    } else {
                                        style.hold_body_rect()
                                    }
                                }),
                                dest_size: Some(vec2(scale * 2., body_h)),
                                flip_y,
                                ..Default::default()
                            },
                            clip,
                        );
                    }
                    // head
                    if res.time < self.time || res.res_pack.info.hold_keep_head {
                        let r = style.hold_head_rect();
                        let hf = vec2(scale, r.h / r.w * scale * ratio);
                        let head_y = if flip_y { bottom as f32 + hf.y * 2. } else { bottom as f32 };
                        draw_tex(
                            res,
                            **tex,
                            order,
                            -scale,
                            head_y - if res.res_pack.info.hold_compact { hf.y } else { hf.y * 2. },
                            color,
                            DrawTextureParams {
                                source: Some(r),
                                dest_size: Some(hf * 2.),
                                flip_y,
                                ..Default::default()
                            },
                            clip,
                        );
                    }
                    // tail
                    if short || (!flip_y && is_negative_length) {
                        // only render a short body
                        return;
                    }
                    let r = style.hold_tail_rect();
                    let hf = vec2(scale, r.h / r.w * scale * ratio);
                    let tail_y = if flip_y { top as f32 - hf.y * 2. } else { top as f32 };
                    draw_tex(
                        res,
                        **tex,
                        order,
                        -scale,
                        tail_y - if res.res_pack.info.hold_compact { hf.y } else { 0. },
                        color,
                        DrawTextureParams {
                            source: Some(r),
                            dest_size: Some(hf * 2.),
                            flip_y,
                            ..Default::default()
                        },
                        clip,
                    );
                });
            }
            NoteKind::Flick => {
                if self.fake && res.time >= self.time {
                    return;
                };
                draw(res, *style.flick);
            }
            NoteKind::Drag => {
                if self.fake && res.time >= self.time {
                    return;
                };
                draw(res, *style.drag);
            }
        }
        if res.config.chart_debug_note > 0. {
            match self.kind {
                NoteKind::Hold { end_time, .. } => {
                    if cover_base > height_above || res.time >= end_time {
                        return;
                    }
                    let bottom = if self.time <= res.time { 0. } else { height - line_height };
                    let debug_text = debug_note_text(res, self, line_id, base, config.line_angle);
                    res.with_model(self.now_transform(res, ctrl_obj, bottom as f32, config.incline_sin, false, false), |res: &mut Resource| {
                        res.with_model(
                            Matrix::new_nonuniform_scaling(&Vector::new(1.0, if self.above { -1.0 } else { 1.0 })),
                            |res: &mut Resource| {
                                res.apply_model(|res| {
                                    ui.text(debug_text)
                                    .pos(
                                        0.,
                                        if self.above {
                                            res.config.chart_debug_note * 0.2
                                        } else {
                                            -res.config.chart_debug_note * 0.2
                                        },
                                    )
                                    .anchor(0.0, 0.)
                                    .size(res.config.chart_debug_note)
                                    .color(Color::new(1., 1., 1., color.a))
                                    .centered_multiline()
                                    .draw();
                                });
                            },
                        );
                    });
                }
                _ => {
                    if cover_base > height_above || cover_base < height_below || res.time >= self.time {
                        return;
                    }
                    let debug_text = debug_note_text(res, self, line_id, base, config.line_angle);
                    res.with_model(self.now_transform(res, ctrl_obj, base as f32, config.incline_sin, false, false), |res: &mut Resource| {
                        res.with_model(
                            Matrix::new_nonuniform_scaling(&Vector::new(1.0, if self.above { -1.0 } else { 1.0 })),
                            |res: &mut Resource| {
                                res.apply_model(|res| {
                                    ui.text(debug_text)
                                    .pos(0., res.config.chart_debug_note * 0.15)
                                    .anchor(0.0, 0.)
                                    .size(res.config.chart_debug_note)
                                    .color(Color::new(1., 1., 1., color.a))
                                    .centered_multiline()
                                    .draw();
                                });
                            },
                        );
                    });
                }
            }
        }
    }
}

pub struct BadNote {
    pub time: f64,
    pub kind: NoteKind,
    pub matrix: Matrix,
}

impl BadNote {
    pub fn render(&self, res: &mut Resource) -> bool {
        if res.time > self.time + BAD_TIME {
            return false;
        }
        res.with_model(self.matrix, |res| {
            let style = &res.res_pack.note_style;
            draw_center(
                res,
                match &self.kind {
                    NoteKind::Click => *style.click,
                    NoteKind::Drag => *style.drag,
                    NoteKind::Flick => *style.flick,
                    _ => unreachable!(),
                },
                self.kind.order(),
                res.note_width,
                Color::new(0.423529, 0.262745, 0.262745, ((self.time - res.time).max(-1.) / BAD_TIME + 1.) as f32),
            );
        });
        true
    }
}
