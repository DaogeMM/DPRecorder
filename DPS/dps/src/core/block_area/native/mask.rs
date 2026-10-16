//! 原生 `EdgeMask` / `GlowMask` / 六台掩码相机的 CPU 等价实现。
//!
//! 位行（bit rows）让九宫格膨胀很便宜，且**完全不需要离屏 FBO**：算完直接作为一张普通
//! 纹理上传。搬自 `Phira-Pro-main/prpr/src/core/block_mask.rs`，改动仅限于：
//! * `BlockNoise1.png` 的路径改成 `../BlockNoise1.png`（复用既有素材，不重复入库）；
//! * `Zone` 里的 `Vector` 类型名；
//! * 测试里的 `crate::core::Vector` 引用。
use super::Zone;
use once_cell::sync::Lazy;

static DISPLACE: Lazy<image::RgbImage> = Lazy::new(|| {
    let source = image::load_from_memory(include_bytes!("../BlockNoise1.png")).unwrap().to_rgb8();
    image::imageops::flip_vertical(&source)
});

/// 4096 个居中噪声样本（`value/255 - 0.5`，按 mediump 舍入）。
static CENTERED_NOISE: Lazy<[f32; 256]> = Lazy::new(|| std::array::from_fn(|value| medium(medium(value as f32 / 255.) - 0.5)));

#[derive(Default)]
pub(super) struct Masks {
    /// 送往着色器的 `uMasks`：R = 合成掩码（无符号，用阈值窗口处理减算），
    /// G = 描边、B = 辉光、A = disabled 合成。
    pub rgba: Vec<u8>,
    /// `uAuxMasks`：R = ready-only 普通，G = ready-only 减算后处理，B = disabled 合成，A = hover。
    pub aux_rgba: Vec<u8>,
    /// 点采样出来的原始相机输出（可用来对照 1/8 相机层）。
    pub sources_rgba: Vec<u8>,
    pub raw_disabled_green: Vec<u8>,
    pub width: usize,
    pub height: usize,
    /// 每次真正重算就 +1，供上传侧判断要不要 `update` 纹理。
    pub revision: u64,
    /// 掩码栅格化的格子边长（**屏幕**像素）。8 = 上游默认（原生相机的 1/8 降采样比），
    /// 由「噪域密度」写入。格子边长同时决定噪域边缘颗粒的大小、描边宽度（1 格）与
    /// 辉光半径（约 5 格），所以它就是「噪域边缘有多细」这一件事的旋钮。
    /// `0` 一律视作 8，这样 `derive(Default)` 的默认态就是官方行为。
    pub cell: usize,
    /// 「谱面缩放」带来的额外粒度系数（`chartRatio`）。**不要**改 `cell` 来实现缩小：
    /// 它是整数屏幕像素，`3 × 0.53 = 1.59` 会被 `round` 量化成 `2`（只缩到 0.67），
    /// 更小的比例干脆缩不动。这里改成放大掩码的**分辨率**：
    /// `bw = width / cell / cell_scale` ⇒ 每个纹素覆盖 `cell × cell_scale` 屏幕像素，
    /// 于是固定纹素半径的膨胀 / 噪声 / 描边 / 辉光在屏幕上按 `cell_scale` 连续缩小。
    /// `1.` 时严格等于原行为。
    pub cell_scale: f32,
    /// 「边缘破碎」强度：合成时每个纹素按 `DISPLACE` 噪声把采样坐标推开，幅度是
    /// `0.1 × edge_noise`（上游硬编码的 0.1）。这个位移就是**噪域边缘那圈锯齿破碎**
    /// 的来源 —— 置 0 边缘会变成干净的直边矩形。`1.` 时严格等于原行为（`0.1 * 1.0`
    /// 与字面量 `0.1` 逐位相同）。
    pub edge_noise: f32,
    /// 「强制定位」挖洞用的覆盖度（每掩码纹素 1 字节：0 = 不挖，255 = 完全挖掉）。
    ///
    /// 注意这里**不是**上游 `judge_range.glsl` 那条「判定范围让位」通路 —— 那条是判定范围
    /// **自己**按噪域的覆盖度淡出（见 `core/shaders/judge_range.glsl`，走 `judge_range_mat`，
    /// 判定范围的重叠部分被噪域抹去）。这张位图只服务「除了按键，哪里都是噪域」：每个按键
    /// 矩形按自己的覆盖度打进位图、重叠取 **max**（不是累加 —— 原生减算那种阈值窗口一重叠
    /// 就互相抵消），最后 `enabled_compose *= 1 - cover / 255`。
    judge_cover: Vec<u8>,
    active: Vec<u64>,
    ping: Vec<u64>,
    pong: Vec<u64>,
    last_dim: (usize, usize, f32),
    last_zones: Vec<Zone>,
    /// 上一帧的合成块（`judge_cover > 0` 的那批），**与 `last_zones` 分开缓存**。
    /// 「强制定位」的洞口跟着按键（音符）每帧都在动，若把它和方块混在一个缓存键里，
    /// 方块那半边（贵的那半边）就会每帧被重新栅格化一遍 —— 详见 `render_displaced_covers`。
    last_covers: Vec<Zone>,
    last_time: Option<f32>,
    layers: [Vec<u8>; 6],
    ready_green: Vec<u8>,
    gray_ping: Vec<u8>,
    gray_pong: Vec<u8>,
    warp_x: Vec<(f32, usize, usize)>,
    warp_y: Vec<(f32, usize)>,
    enabled_compose: Vec<u8>,
    point_aux: Vec<[u8; 3]>,
}

impl Masks {
    /// `width` / `height` 是**屏幕**尺寸：内部先按 `self.cell` 像素降采样（上游是固定的
    /// 1/8，即原生相机的降采样比），再横向 2× 上采样成 effect RT 尺寸（`bw*2 x bh*2`）。
    pub fn render_displaced(&mut self, width: usize, height: usize, aspect: f32, zones: &[Zone], time: f32) {
        // 单切片入口（测试 / 调试用）：按老语义把合成块筛出来。
        // 生产调用点走 `render_displaced_covers`，自己就把两者分开了，不会每帧多两次分配。
        let (covers, layers): (Vec<Zone>, Vec<Zone>) = zones.iter().cloned().partition(|z| z.judge_cover > 0);
        self.render_displaced_covers(width, height, aspect, &layers, &covers, time);
    }

    /// `width` / `height` 是**屏幕**尺寸：内部先按 `self.cell` 像素降采样（上游是固定的
    /// 1/8，即原生相机的降采样比），再横向 2× 上采样成 effect RT 尺寸（`bw*2 x bh*2`）。
    ///
    /// `zones` 是**会画出来的**方块，`covers` 是「强制定位」的合成块（只往 `judge_cover`
    /// 位图里打覆盖度，不进层、不绘制）。两者必须分开传，因为它们的
    /// 变化频率差了一个量级：
    ///
    /// - `zones` 那半边是**贵**的：每个方块按自己的面积逐掩码格栅格化，全屏方块一个就是
    ///   `bw * bh` 格（1080p、`cell = 3` 时约 23 万格），一张谱面几十个就是几百万格。
    /// - `covers` 每帧都在动：按键矩形跟着音符走，一个音符前后各一条四边形。
    ///
    /// 早先两者被拼成同一个切片再比较（`shader.rs` 的调用点），于是覆盖度每移动一次就
    /// 把**所有**方块的栅格化 + 6 张层全重算一遍。实测（`ハテ.rNFrums.0.AT`，0–30 s，
    /// 1080p60）：只有噪域 80.3 FPS → 再加一整套跟随音符的覆盖度（当时是判定范围）15.5 FPS。
    /// 分开缓存后方块那半边照旧命中缓存，只有位图与合成每帧重算（那一半在 `A` 组与 `C` 组里
    /// 都便宜到测不出来）。
    pub fn render_displaced_covers(
        &mut self,
        width: usize,
        height: usize,
        aspect: f32,
        zones: &[Zone],
        covers: &[Zone],
        time: f32,
    ) {
        let cell = if self.cell == 0 { 8 } else { self.cell };
        // 「谱面缩放」：`cell_scale < 1` 时提高掩码分辨率（而不是把整数 `cell` 乘小，
        // 那会被 `round` 量化掉）。固定纹素半径的膨胀 / 噪声 / 描边 / 辉光因此按
        // `cell_scale` 连续缩小，屏幕上每个纹素覆盖 `cell × cell_scale` 像素。
        let inv = if self.cell_scale.is_finite() && self.cell_scale > 0. { self.cell_scale as f64 } else { 1. };
        // 用**截断**而不是四舍五入：`inv == 1.` 时必须逐位复现原来的整数除法
        // （`1280 / 3 == 426`，而 `round` 会给出 427，掩码尺寸差一格就会让整张画面变样）。
        let (bw, bh) = (((width as f64 / cell as f64 / inv) as usize).max(1), ((height as f64 / cell as f64 / inv) as usize).max(1));
        let (ew, eh) = (bw * 2, bh * 2);
        let dim = (ew, eh, aspect);
        // 两个缓存键分开算：贵的那半边（逐方块栅格化 + 6 张层）只认 `zones`，
        // 便宜的那半边（覆盖度位图 + 合成）只认 `covers`。
        let layers_dirty = self.last_dim != dim || self.last_zones != zones;
        let covers_dirty = self.last_dim != dim || self.last_covers != covers;
        if !layers_dirty && !covers_dirty && self.last_time == Some(time) {
            return;
        }
        if layers_dirty {
            self.last_dim = dim;
            self.last_zones.clear();
            self.last_zones.extend_from_slice(zones);
            for layer in &mut self.layers {
                layer.resize(bw * bh, 0);
                layer.fill(0);
            }
            self.raw_disabled_green.resize(bw * bh, 0);
            self.raw_disabled_green.fill(0);
            self.ready_green.resize(bw * bh, 0);
            self.ready_green.fill(0);
            // 六台官方相机分别抓：enabled N/S、disabled N/S（含 ready）、ready-only N/S。
            // BlockSprite 的混合是 SrcAlpha, One；每个减算 sprite 的 alpha 是 0.1。
            // 只有会画出来的方块。合成块（`judge_cover > 0`）走下面 `covers` 那半边 —
            // 传进来的 `zones` 里不该有它们（生产调用点在 `shader.rs` 已经 partition 过）。
            debug_assert!(!zones.iter().any(|z| z.judge_cover > 0), "covers must go through the `covers` argument");
            for z in zones {
                let layer = if z.active { usize::from(z.invert) } else { 2 + usize::from(z.invert) };
                let opacity = if z.invert { 0.1 } else { z.opacity };
                raster_rows(bw, bh, aspect, z, |y, first, last| {
                    for x in first..last {
                        let i = y * bw + x;
                        self.layers[layer][i] = unorm(self.layers[layer][i] as f32 / 255. + opacity);
                        if z.invert && !z.active {
                            self.raw_disabled_green[i] = unorm(self.raw_disabled_green[i] as f32 / 255. + 0.1 * z.opacity);
                        }
                        if z.ready {
                            let ready = 4 + usize::from(z.invert);
                            self.layers[ready][i] = unorm(self.layers[ready][i] as f32 / 255. + opacity);
                            if z.invert {
                                self.ready_green[i] = unorm(self.ready_green[i] as f32 / 255. + 0.1 * z.opacity);
                            }
                        }
                    }
                });
            }
        }
        // 覆盖度位图 + 合成：判定范围每帧都在动，所以这一段基本每帧都会跑。它只依赖
        // `covers`、6 张层与 `judge_cover`，不碰方块几何（那是上面那半边的活），
        // 所以判定范围再密也不会把方块那半边的缓存打掉。
        if layers_dirty || covers_dirty {
            self.last_covers.clear();
            self.last_covers.extend_from_slice(covers);
            self.judge_cover.resize(bw * bh, 0);
            self.judge_cover.fill(0);
            for z in covers {
                raster_rows(bw, bh, aspect, z, |y, first, last| {
                    for x in first..last {
                        let i = y * bw + x;
                        if self.judge_cover[i] < z.judge_cover {
                            self.judge_cover[i] = z.judge_cover;
                        }
                    }
                });
            }
            self.sources_rgba.resize(bw * bh * 4, 0);
            self.enabled_compose.resize(bw * bh, 0);
            self.point_aux.resize(bw * bh, [0; 3]);
            for i in 0..bw * bh {
                for c in 0..4 {
                    self.sources_rgba[i * 4 + c] = self.layers[c][i];
                }
                // 「判定范围让位」：`alpha *= 1 - cover / 255`（上游 `judge_range.glsl` 的语义）。
                // `judge_cover` 只取 0 / 255（见 `mod.rs::push_quad_zones`），所以这里等于整块擦掉，
                // 掩码里不会留下中间灰度 —— 这是 `render_rings` 能走位图路径的前提。
                let composed = self.layers[0][i].abs_diff(if subtract_enabled(self.layers[1][i]) == 1. { 255 } else { 0 });
                let cover = self.judge_cover[i] as u32;
                self.enabled_compose[i] = if cover == 0 { composed } else { (composed as u32 * (255 - cover) / 255) as u8 };
                let disabled = if self.layers[2][i] == 0 && self.layers[3][i] == 0 && self.raw_disabled_green[i] == 0 {
                    0
                } else {
                    let (sr, sg) = subtract_disabled(self.layers[3][i], self.raw_disabled_green[i]);
                    unorm((sr * sg - self.layers[2][i] as f32 / 255.).abs())
                };
                let ready_s = if self.ready_green[i] == 0 {
                    0
                } else {
                    unorm(subtract_disabled(self.layers[5][i], self.ready_green[i]).1)
                };
                self.point_aux[i] = [self.layers[4][i], ready_s, disabled];
            }
        }
        self.width = ew;
        self.height = eh;
        self.rgba.resize(ew * eh * 4, 0);
        self.rgba.fill(0);
        self.aux_rgba.resize(ew * eh * 4, 0);
        self.aux_rgba.fill(0);
        let stride = ew.div_ceil(64);
        self.active.resize(stride * eh, 0);
        self.active.fill(0);
        // 两次原生位移采样共用同一个 Y：按行/列预先算好镜像后的纹理下标，
        // 而不是对每个掩码像素跑四遍浮点取模 + 图像查询。
        let texture = &*DISPLACE;
        let pixels = texture.as_raw();
        let texture_stride = texture.width() as usize * 3;
        let d = 0.70703125_f32;
        let dt = d * (time / 20. * 2.59);
        self.warp_x.clear();
        self.warp_y.clear();
        self.warp_x.extend((0..bw).map(|x| {
            let u = (x as f32 + 0.5) / bw as f32;
            (u, noise_index(dt + u * 2.13, texture.width()) as usize * 3, noise_index(-dt + u * 2.13, texture.width()) as usize * 3)
        }));
        self.warp_y.extend((0..bh).map(|y| {
            let v = (y as f32 + 0.5) / bh as f32;
            (v, noise_index(dt + v * 1.02, texture.height()) as usize * texture_stride)
        }));
        let centered = &*CENTERED_NOISE;
        for y in 0..bh {
            for x in 0..bw {
                let (u, xa, xb) = self.warp_x[x];
                let (v, row) = self.warp_y[y];
                let a = centered[pixels[row + xa] as usize];
                let b = centered[pixels[row + xb] as usize];
                // 「边缘破碎」：这个位移幅度就是边缘那圈锯齿的来源（见 `edge_noise` 字段）。
                let edge = 0.1 * self.edge_noise;
                let duv = [(d * a + b * -d) * edge + u, (d * a + b * d) * edge + v];
                let sx = (duv[0] * bw as f32).floor().clamp(0., (bw - 1) as f32) as usize;
                let sy = (duv[1] * bh as f32).floor().clamp(0., (bh - 1) as f32) as usize;
                let i = y * bw + x;
                let di = sy * bw + sx;
                let mask = self.enabled_compose[di];
                let [ready_n, ready_s, disabled] = self.point_aux[i];
                let rgba = [mask, 0, 0, disabled, mask, 0, 0, disabled];
                let aux = [ready_n, ready_s, disabled, 0, ready_n, ready_s, disabled, 0];
                for yy in [y * 2, y * 2 + 1] {
                    let dst = (yy * ew + x * 2) * 4;
                    self.rgba[dst..dst + 8].copy_from_slice(&rgba);
                    self.aux_rgba[dst..dst + 8].copy_from_slice(&aux);
                    if mask != 0 {
                        self.active[yy * stride + x * 2 / 64] |= 3 << ((x * 2) % 64);
                    }
                }
            }
        }
        self.render_rings();
        self.last_time = Some(time);
        self.revision = self.revision.wrapping_add(1);
    }

    fn render_rings(&mut self) {
        let (width, height) = (self.width, self.height);
        // 掩码里出现中间灰度（减算/半透明）时，位图膨胀会丢掉权重，改走灰度版。
        if self.rgba.chunks_exact(4).any(|p| p[0] != 0 && p[0] != 255) {
            self.render_gray_rings();
            return;
        }
        let stride = width.div_ceil(64);
        self.ping.clone_from(&self.active);
        self.pong.resize(self.active.len(), 0);
        for (pass, weight) in glow_weights().into_iter().enumerate() {
            if weight < 0.01 {
                break;
            }
            dilate(&self.ping, &mut self.pong, width, height);
            for y in 0..height {
                for word in 0..stride {
                    let i = y * stride + word;
                    let mut ring = self.pong[i] & !self.ping[i];
                    while ring != 0 {
                        let bit = ring.trailing_zeros() as usize;
                        let x = word * 64 + bit;
                        if x < width {
                            let dst = (y * width + x) * 4;
                            if pass == 0 {
                                self.rgba[dst + 1] = 255;
                            }
                            self.rgba[dst + 2] = unorm(weight);
                        }
                        ring &= ring - 1;
                    }
                }
            }
            std::mem::swap(&mut self.ping, &mut self.pong);
        }
    }

    fn render_gray_rings(&mut self) {
        let (w, h) = (self.width, self.height);
        self.gray_ping.resize(w * h, 0);
        self.gray_pong.resize(w * h, 0);
        for (i, p) in self.rgba.chunks_exact(4).enumerate() {
            self.gray_ping[i] = p[0];
        }
        for (pass, weight) in glow_weights().into_iter().enumerate() {
            if weight < 0.01 {
                break;
            }
            // 3×3 最大值拆成「先横后纵」两趟：最大值满足结合律，两趟都在边界处按同样的
            // 截断处理（用 `saturating_sub` / `min` 夹住下标），结果与逐像素九宫格完全一致。
            // 拆开是为了让内层没有范围迭代，1280×720 下一趟就是一条能向量化的直线循环 ——
            // 原版那个 `for yy in ...saturating_sub(1)..=(y+1).min(h-1)` 嵌套实测 79 ms/帧。
            for y in 0..h {
                let row = y * w;
                for x in 0..w {
                    let center = row + x;
                    let mut maximum = self.gray_ping[center];
                    if x > 0 && self.gray_ping[center - 1] > maximum {
                        maximum = self.gray_ping[center - 1];
                    }
                    if x + 1 < w && self.gray_ping[center + 1] > maximum {
                        maximum = self.gray_ping[center + 1];
                    }
                    self.gray_pong[center] = maximum;
                }
            }
            for y in 0..h {
                let row = y * w;
                let up = y.saturating_sub(1) * w;
                let down = (y + 1).min(h - 1) * w;
                for x in 0..w {
                    let i = row + x;
                    let mut maximum = self.gray_pong[i];
                    if self.gray_pong[up + x] > maximum {
                        maximum = self.gray_pong[up + x];
                    }
                    if self.gray_pong[down + x] > maximum {
                        maximum = self.gray_pong[down + x];
                    }
                    let source = self.gray_ping[i];
                    self.gray_ping[i] = maximum;
                    let delta = maximum - source;
                    if pass == 0 {
                        self.rgba[i * 4 + 1] = delta;
                    }
                    // 九宫格里没有更大的值 ⇒ 这一趟对它没有任何影响（`unorm(x / 255.0)`
                    // 对整数 x 会原样写回），跳过它与原版逐位相同，能省下大部分浮点计算：
                    // 噪声内部绝大多数像素都是这个情况，只有描边附近才真的有差。
                    if delta != 0 {
                        let outside = 1. - self.rgba[i * 4] as f32 / 255.;
                        self.rgba[i * 4 + 2] =
                            unorm(weight * (outside * (delta as f32 / 255.)) + self.rgba[i * 4 + 2] as f32 / 255.);
                    }
                }
            }
        }
    }
}

fn glow_weights() -> [f32; 6] {
    static WEIGHTS: Lazy<[f32; 6]> = Lazy::new(|| {
        let sum: f32 = (1..=6).map(|k| (k as f32).powf(2.65)).sum();
        std::array::from_fn(|pass| ((6 - pass) as f32).powf(2.65) / sum)
    });
    *WEIGHTS
}

fn unorm(v: f32) -> u8 {
    (v.clamp(0., 1.) * 255.).round() as u8
}

/// 原生 `SubtractBlockBlender` pass 0：一个阈值窗口，不是奇偶。
fn subtract_enabled(red: u8) -> f32 {
    let r = red as f32 / 255.;
    if (0.09..0.12).contains(&r) {
        1.
    } else {
        0.
    }
}

fn subtract_disabled(red: u8, green: u8) -> (f32, f32) {
    let g = green as f32 / 255.;
    let t = ((g - 0.2) * -10.).clamp(0., 1.);
    let r = subtract_enabled(red) + t * t * (3. - 2. * t);
    // 中间结果是 RG16，在 shader 输出后夹取并舍入。
    (unorm(r) as f32 / 255., unorm(g * r * 10.) as f32 / 255.)
}

/// `BlockCompose` pass 0：保留原生归一化方向与 ST。
///
/// 原生 `u_xlat16_*` 是 mediump（binary16）而不是全精度 CPU 浮点：导出的 GLES 着色器里，
/// 长度倒数、方向、采样噪声、居中噪声**各自**被舍入到 binary16。在这些步骤之后仍让 warp
/// 坐标保持 highp，对点采样边界是关键。
#[allow(dead_code)]
pub(super) fn compose_uv(uv: [f32; 2], time: f32) -> [f32; 2] {
    let d = medium(0.5 * medium((0.5_f32).sqrt().recip()));
    let t = time / 20. * 2.59;
    let st = [uv[0] * 2.13, uv[1] * 1.02];
    let a = medium(medium(noise([d * t + st[0], d * t + st[1]])) - 0.5);
    let b = medium(medium(noise([-d * t + st[0], d * t + st[1]])) - 0.5);
    [(d * a + b * -d) * 0.1 + uv[0], (d * a + b * d) * 0.1 + uv[1]]
}

fn medium(value: f32) -> f32 {
    // 归一化后的噪声/方向中间量都是 0 或正规 binary16。按 10 位小数、就近偶数舍入。
    let bits = value.to_bits();
    f32::from_bits((bits + 0xfff + ((bits >> 13) & 1)) & !0x1fff)
}

fn noise_index(v: f32, size: u32) -> u32 {
    let v = v.rem_euclid(2.);
    let v = if v > 1. { 2. - v } else { v };
    (v * size as f32).floor().clamp(0., (size - 1) as f32) as u32
}

#[allow(dead_code)]
fn noise(uv: [f32; 2]) -> f32 {
    // 与材质渲染器上传的「竖向翻转后的行」一致。
    DISPLACE.get_pixel(noise_index(uv[0], DISPLACE.width()), noise_index(uv[1], DISPLACE.height()))[0] as f32 / 255.
}

/// 按行求旋转矩形覆盖的 x 半开区间（`write(y, first, last)`）。
fn raster_rows(width: usize, height: usize, aspect: f32, zone: &Zone, mut write: impl FnMut(usize, usize, usize)) {
    if zone.half.x <= 0. || zone.half.y <= 0. {
        return;
    }
    let (s, c) = zone.angle.sin_cos();
    let y_half = s.abs() * zone.half.x + c.abs() * zone.half.y;
    let start = (((zone.center.y - y_half) * aspect + 1.) * height as f32 * 0.5 - 0.5)
        .ceil()
        .clamp(0., height as f32) as usize;
    let end = ((((zone.center.y + y_half) * aspect + 1.) * height as f32 * 0.5 - 0.5).floor() + 1.).clamp(0., height as f32) as usize;
    for y in start..end {
        let dy = ((y as f32 + 0.5) * 2. / height as f32 - 1.) / aspect - zone.center.y;
        let (mut lo, mut hi) = (-1.0_f32, 1.0_f32);
        for (a, b, half) in [(c, s * dy, zone.half.x), (-s, c * dy, zone.half.y)] {
            if a.abs() < 1e-7 {
                if b.abs() > half {
                    hi = lo - 1.;
                    break;
                }
            } else {
                let a0 = (-half - b) / a + zone.center.x;
                let a1 = (half - b) / a + zone.center.x;
                lo = lo.max(a0.min(a1));
                hi = hi.min(a0.max(a1));
            }
        }
        if lo > hi {
            continue;
        }
        let first = ((lo + 1.) * width as f32 * 0.5 - 0.5).ceil().clamp(0., width as f32) as usize;
        let last = (((hi + 1.) * width as f32 * 0.5 - 0.5).floor() + 1.).clamp(0., width as f32) as usize;
        write(y, first, last);
    }
}

fn dilate(source: &[u64], dest: &mut [u64], width: usize, height: usize) {
    let stride = width.div_ceil(64);
    let last_mask = u64::MAX >> ((64 - width % 64) % 64);
    for y in 0..height {
        for x in 0..stride {
            let mut value = 0;
            for row in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                let idx = row * stride + x;
                let mid = source[idx];
                let left = if x > 0 { source[idx - 1] >> 63 } else { 0 };
                let right = if x + 1 < stride { source[idx + 1] << 63 } else { 0 };
                value |= mid | (mid << 1) | left | (mid >> 1) | right;
            }
            dest[y * stride + x] = if x + 1 == stride { value & last_mask } else { value };
        }
    }
}

#[cfg(test)]
mod tests {

    /// 生产路径每帧都会把 `edge_noise` 从配置写进来，所以只有测试需要这个「官方默认值」的
    /// 起点 —— `Masks::default()` 的 `edge_noise` 是 `0.`（= 关闭边缘破碎），那会让
    /// 「位移随时间变化」这类断言失去意义。
    fn official_masks() -> Masks {
        let mut masks = Masks::default();
        masks.edge_noise = 1.;
        masks
    }
    use super::*;
    use crate::core::Vector;

    #[test]
    fn compose_uses_native_mediump_intermediate_rounding() {
        assert_eq!(medium(0.5 * medium((0.5_f32).sqrt().recip())), 0.70703125);
        assert_eq!(medium(medium(155. / 255.) - 0.5), 0.10791015625);
        assert_eq!(medium(1. + 1. / 2048.), 1., "ties round to even");
        assert_eq!(medium(1. + 3. / 2048.), 1.001953125);
    }

    fn zone(x: f32, y: f32, half_x: f32, half_y: f32, angle: f32, invert: bool) -> Zone {
        Zone {
            center: Vector::new(x, y),
            half: Vector::new(half_x, half_y),
            angle,
            invert,
            active: true,
            ready: false,
            before_enable: false,
            opacity: 1.,
            index: 0,
            judge_cover: 0,
        }
    }

    #[test]
    fn nine_tap_dilation_matches_pixel_reference_across_word_boundaries() {
        for width in [1_usize, 63, 64, 65, 129] {
            let height = 9;
            let stride = width.div_ceil(64);
            let mut source = vec![0_u64; stride * height];
            for y in 0..height {
                for x in 0..width {
                    if (x * 11 + y * 7) % 17 == 0 {
                        source[y * stride + x / 64] |= 1 << (x % 64);
                    }
                }
            }
            let mut dest = vec![0; source.len()];
            dilate(&source, &mut dest, width, height);
            for y in 0..height {
                for x in 0..width {
                    let mut expected = false;
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            let sx = (x as i32 + dx).clamp(0, width as i32 - 1) as usize;
                            let sy = (y as i32 + dy).clamp(0, height as i32 - 1) as usize;
                            expected |= source[sy * stride + sx / 64] & (1 << (sx % 64)) != 0;
                        }
                    }
                    assert_eq!(dest[y * stride + x / 64] & (1 << (x % 64)) != 0, expected, "{width}: ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn raw_camera_layers_match_resolved_rectangle_coverage() {
        let zones = [
            zone(0.1, -0.1, 0.45, 0.23, 0.57, false),
            zone(-0.5, 0.13, 0.33, 0.09, -1.17, false),
            zone(0.0, 0.0, 1.0, 0.6, 0.0, true),
            zone(0.15, 0.2, 0.12, 0.21, 1.32, true),
        ];
        let (width, height, aspect) = (129, 75, 16. / 9.);
        let mut masks = official_masks();
        masks.render_displaced(width * 8, height * 8, aspect, &zones, 0.);
        for y in 0..height {
            for x in 0..width {
                let p = Vector::new((x as f32 + 0.5) * 2. / width as f32 - 1., ((y as f32 + 0.5) * 2. / height as f32 - 1.) / aspect);
                let mut normal = false;
                let mut subtract_count = 0_usize;
                for z in &zones {
                    let local = nalgebra::Rotation2::new(-z.angle) * (p - z.center);
                    if local.x.abs() <= z.half.x && local.y.abs() <= z.half.y {
                        if z.invert {
                            subtract_count += 1;
                        } else {
                            normal = true;
                        }
                    }
                }
                assert_eq!(masks.sources_rgba[(y * width + x) * 4] != 0, normal, "normal ({x}, {y})");
                assert_eq!(masks.sources_rgba[(y * width + x) * 4 + 1], [0, 26, 52][subtract_count], "subtract ({x}, {y})");
            }
        }
    }

    #[test]
    fn exterior_rings_leave_fill_and_union_interiors_unchanged() {
        let mut masks = official_masks();
        masks.render_displaced(800, 800, 1., &[zone(-0.2, 0., 0.3, 0.3, 0., false), zone(0.2, 0., 0.3, 0.3, 0., false)], 1.);
        let p = (100 * masks.width + 100) * 4;
        assert_eq!(&masks.rgba[p..p + 4], &[255, 0, 0, 0]);
        for p in masks.rgba.chunks_exact(4) {
            if p[0] == 255 {
                assert_eq!(&p[1..3], &[0, 0]);
            }
            if p[1] == 255 {
                assert_eq!(p[0], 0);
                assert!(p[2] > 0);
            }
        }
        assert!(masks.rgba.chunks_exact(4).any(|p| p[1] == 255));
    }

    #[test]
    fn subtract_cancellation_has_no_phantom_edges_and_zone_count_is_unbounded() {
        let mut masks = official_masks();
        masks.render_displaced(800, 800, 1., &[zone(0., 0., 0.5, 0.3, 0., true), zone(0., 0., 0.5, 0.3, 0., true)], 1.);
        assert!(masks.rgba.iter().all(|&v| v == 0));
        let mut zones: Vec<_> = (0..159).map(|_| zone(2., 2., 0.1, 0.1, 0., false)).collect();
        zones.push(zone(0., 0., 0.2, 0.2, 0., false));
        masks.render_displaced(800, 800, 1., &zones, 1.);
        assert_eq!(masks.rgba[(100 * masks.width + 100) * 4], 255);
        let previous = masks.rgba.clone();
        masks.render_displaced(800, 800, 1., &zones, 1.);
        assert_eq!(masks.rgba, previous);
        masks.render_displaced(520, 560, 1., &zones, 1.);
        assert_eq!((masks.width, masks.height), (130, 140));
    }

    #[test]
    fn glow_weight_matches_native_material_and_skips_last_ring() {
        let weights = glow_weights();
        assert!((weights.iter().sum::<f32>() - 1.).abs() < 1e-6);
        assert!((weights[4] - 0.024947807).abs() < 1e-7);
        assert!(weights[5] < 0.01);
    }

    #[test]
    fn native_mask_and_effect_dimensions_match_start_and_point_upsampling() {
        let mut masks = official_masks();
        masks.render_displaced(960, 540, 16. / 9., &[zone(0., 0., 0.5, 0.25, 0., false)], 1.);
        assert_eq!((masks.width, masks.height), (240, 134));
        for y in (0..134).step_by(2) {
            for x in (0..240).step_by(2) {
                let sample = |px: usize, py: usize| masks.rgba[(py * 240 + px) * 4];
                assert_eq!(sample(x, y), sample(x + 1, y));
                assert_eq!(sample(x, y), sample(x, y + 1));
            }
        }
    }

    #[test]
    fn disabled_subtract_does_not_clip_enabled_region() {
        let active = zone(0., 0., 0.5, 0.25, 0., false);
        let disabled = Zone {
            active: false,
            invert: true,
            ..active.clone()
        };
        let mut original = official_masks();
        original.render_displaced(960, 540, 16. / 9., &[active.clone()], 1.);
        let mut mixed = official_masks();
        mixed.render_displaced(960, 540, 16. / 9., &[active, disabled], 1.);
        assert!(original
            .rgba
            .chunks_exact(4)
            .zip(mixed.rgba.chunks_exact(4))
            .all(|(a, b)| a[..3] == b[..3]));
        assert!(mixed.rgba.chunks_exact(4).any(|p| p[0] == 255 && p[3] == 255));
    }

    #[test]
    fn visual_subtract_uses_threshold_window_including_three_overlaps() {
        let sub = zone(0., 0., 1., 1., 0., true);
        for (count, value) in [(1, 255), (2, 0), (3, 0)] {
            let mut mask = official_masks();
            mask.render_displaced(128, 96, 4. / 3., &vec![sub.clone(); count], 2.);
            assert!(mask.rgba.chunks_exact(4).all(|p| p[0] == value));
        }
    }

    #[test]
    fn ready_inputs_use_ready_only_cameras_and_disabled_compose() {
        let disabled = Zone {
            active: false,
            ..zone(-0.5, 0., 0.2, 0.2, 0., false)
        };
        let ready = Zone {
            active: false,
            ready: true,
            ..zone(0.5, 0., 0.2, 0.2, 0., false)
        };
        let mut mask = official_masks();
        mask.render_displaced(128, 96, 4. / 3., &[disabled, ready], 1.);
        let pixel = |x: usize| &mask.aux_rgba[(12 * mask.width + x) * 4..(12 * mask.width + x) * 4 + 4];
        assert_eq!(&pixel(8)[..3], &[0, 0, 255]);
        assert_eq!(&pixel(24)[..3], &[255, 0, 255]);
    }

    #[test]
    fn compose_displacement_animates_only_enabled_masks_and_uses_red_channel() {
        let active = zone(0., 0., 0.5, 0.2, 0., false);
        let disabled = Zone {
            active: false,
            ..active.clone()
        };
        let mut mask = official_masks();
        mask.render_displaced(960, 540, 16. / 9., &[active.clone(), disabled.clone()], 1.);
        let before = mask.rgba.clone();
        mask.render_displaced(960, 540, 16. / 9., &[active, disabled], 2.);
        assert!(before.chunks_exact(4).zip(mask.rgba.chunks_exact(4)).any(|(a, b)| a[0] != b[0]));
        assert!(before.chunks_exact(4).zip(mask.rgba.chunks_exact(4)).all(|(a, b)| a[3] == b[3]));
        let pixel = DISPLACE.get_pixel(0, 0);
        assert_eq!(noise([0.0001, 0.0001]), pixel[0] as f32 / 255.);
        assert_ne!(pixel[0], pixel[1]);
    }

    #[test]
    fn initial_subtract_show_fades_green_while_alpha_stays_point_one() {
        let sub = Zone {
            active: false,
            opacity: 0.25,
            ..zone(0., 0., 1., 1., 0., true)
        };
        let mut mask = official_masks();
        mask.render_displaced(128, 96, 4. / 3., &[sub], 1.);
        assert_eq!(mask.sources_rgba[3], 26);
        assert_eq!(mask.raw_disabled_green[0], 6);
        assert_eq!(mask.rgba[3], 120);
    }

    /// 掩码里出现中间灰度时的退化路径（灰度勾边）改成分离式最大值之后必须**逐位一致**：
    /// 这里把原版那段「逐像素九宫格 + `saturating_sub` 范围迭代」照抄一份当参考 ——
    /// 换掉它的前提就是结果一个字节都不变（它慢的原因正是那个嵌套范围迭代）。
    #[test]
    fn gray_rings_match_pixel_reference_on_mixed_masks() {
        for (w, h) in [(9_usize, 7_usize), (64, 48), (65, 33)] {
            let mut rgba = vec![0_u8; w * h * 4];
            for i in 0..w * h {
                rgba[i * 4] = [0, 26, 128, 255][(i * 7 + i / w * 13) % 4];
            }
            let mut mask = Masks {
                width: w,
                height: h,
                rgba: rgba.clone(),
                ..Default::default()
            };
            mask.render_gray_rings();
            assert_eq!(mask.rgba, gray_rings_reference(&rgba, w, h), "{w}x{h}");
        }
    }

    /// 原版 `render_gray_rings`（照抄），只用于上面的逐位对照。
    fn gray_rings_reference(rgba: &[u8], w: usize, h: usize) -> Vec<u8> {
        let mut out = rgba.to_vec();
        let mut ping = vec![0_u8; w * h];
        let mut pong = vec![0_u8; w * h];
        for (i, p) in out.chunks_exact(4).enumerate() {
            ping[i] = p[0];
        }
        for (pass, weight) in glow_weights().into_iter().enumerate() {
            if weight < 0.01 {
                break;
            }
            for y in 0..h {
                for x in 0..w {
                    let i = y * w + x;
                    let mut maximum = ping[i];
                    for yy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                        for xx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                            maximum = maximum.max(ping[yy * w + xx]);
                        }
                    }
                    pong[i] = maximum;
                    let delta = (maximum - ping[i]) as f32 / 255.;
                    if pass == 0 {
                        out[i * 4 + 1] = maximum - ping[i];
                    }
                    let outside = 1. - out[i * 4] as f32 / 255.;
                    out[i * 4 + 2] = unorm(weight * (outside * delta) + out[i * 4 + 2] as f32 / 255.);
                }
            }
            std::mem::swap(&mut ping, &mut pong);
        }
        out
    }
}
