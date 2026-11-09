import type { DraftClip, DraftLane, NoiseMaterial, TextNoiseEffect } from '@/model';
import { DEFAULT_NOISE_MATERIAL } from '@/model';

/**
 * 文字噪域：把一段文字光栅化成方格，再把方格合并成一块块噪域矩形。
 *
 * 引擎的噪域是一串矩形（谱面顶层 `blockAreaList` 里的原生条目，字段见
 * `DPS/dps/src/core/block_area/mod.rs` 的 `RawBlockArea`），坐标是屏幕百分比：
 * `(0,0)` 是屏幕左下角、`(1,1)` 是右上角（再按画面比例折成谱面坐标）。
 * 文字在画布上量出来时 `y` 朝下，所以换算时纵坐标要翻一下。
 *
 * 编辑窗口里那块黑底画布与交给引擎的那份谱面都从这同一份结果出：两边形状必须一致，
 * 所以光栅化与合并只留这一个来源。
 */

/** 画面比例：引擎预览把画面钉在 16:9，格子按这个比例算才是正方形。 */
export const NOISE_ASPECT = 16 / 9;

/** 最多分多少行：颗粒调得再细也不超这个数，免得预览一次画十万个方块。 */
const MAX_ROWS = 320;

/** 超采样倍数：先在放大 4 倍的画布上画字，再按格子取平均，边缘才不会一格一跳。 */
const SUPERSAMPLE = 4;

/**
 * 引擎原生噪域红：`DPS/dps/src/config.rs` 的 `block_area_color = [0.9, 0.1, 0.1]`。
 *
 * 引擎把等于这个颜色的方块当作「没上色」（走原生材质，省掉一张颜色平面），所以
 * 谱面里不必写它，界面上的默认色也用这一个。
 */
export const NATIVE_NOISE_COLOR = '#E61A1A';

/** 默认字体：仓库根 `assets/font.ttf`，界面里注册成 `Phigros`，与引擎画面同一份。 */
export const NATIVE_FONT = 'Phigros';

/** 默认参数：半屏高的大字，格子是屏高的 1.2%。 */
export function defaultTextNoise(): TextNoiseEffect {
  return {
    text: 'Text',
    fontFamily: NATIVE_FONT,
    color: NATIVE_NOISE_COLOR,
    fontSize: 0.5,
    cell: 0.012,
    x: 0.5,
    y: 0.5,
    lineHeight: 1.25,
    letterSpacing: 0,
    rotation: 0,
    enable: true,
    invert: false,
    subtract: false,
    blend: false,
    material: { ...DEFAULT_NOISE_MATERIAL },
  };
}

/**
 * 把界面上的颜色收成大写 `#RRGGBB`：只认 3 / 6 位十六进制，认不出来就退回原生红。
 *
 * 引擎那边同一种写法也收（见 `RawBlockArea::color`），这里先收一道，免得手填的值
 * 让谱面解析在别人的播放器里报错。
 */
export function normalizeNoiseColor(color: string): string {
  const digits = color.trim().replace(/^#/, '');
  if (/^[0-9a-f]{6}$/i.test(digits)) return `#${digits.toUpperCase()}`;
  if (/^[0-9a-f]{3}$/i.test(digits)) {
    return `#${digits
      .toUpperCase()
      .split('')
      .map((it) => it + it)
      .join('')}`;
  }
  return NATIVE_NOISE_COLOR;
}

/** 颜色拆成 Canvas 用的 `[r, g, b]`（0~255）。 */
export function noiseRgb(color: string): [number, number, number] {
  const hex = normalizeNoiseColor(color).slice(1);
  return [0, 2, 4].map((at) => parseInt(hex.slice(at, at + 2), 16)) as [number, number, number];
}

/** 光栅化之后的方格表：`filled[i]` 为 1 表示第 `i` 格要铺噪域。 */
export interface NoiseGrid {
  cols: number;
  rows: number;
  /** 下标是 `row * cols + col`，行从上往下。 */
  filled: Uint8Array;
}

function clamp(value: number, low: number, high: number) {
  return Math.min(high, Math.max(low, Number.isFinite(value) ? value : low));
}

/** 字体名包成 CSS 认的样子：带空格的名字要加引号。 */
function quoteFamily(family: string) {
  const name = family.trim().replace(/"/g, '');
  return name ? `"${name}", sans-serif` : 'sans-serif';
}

/**
 * 栅格缓存：右栏预览、存盘时的 `blockAreas`、导出各算一遍，文字不变就没必要重画。
 *
 * 键里不带颜色、材质与起止时间 —— 它们不影响文字形状；只拖材质滑杆时这里直接命中。
 * 上限取得很小：一屏同时也就编辑一两条特效轨。
 */
const GRID_CACHE_LIMIT = 24;
const gridCache = new Map<string, NoiseGrid>();
const rectCache = new Map<string, GridRect[]>();

/** 文字几何的指纹（不含 invert：invert 只影响合并出来的矩形）。 */
function gridKey(effect: TextNoiseEffect, aspect: number): string {
  return [effect.text, effect.fontFamily, effect.fontSize, effect.lineHeight, effect.letterSpacing, effect.rotation, effect.x, effect.y, effect.cell, aspect].join('\u0001');
}

function remember<T>(cache: Map<string, T>, limit: number, key: string, value: T): T {
  cache.set(key, value);
  while (cache.size > limit) {
    const oldest = cache.keys().next().value;
    if (oldest === undefined) break;
    cache.delete(oldest);
  }
  return value;
}

/**
 * 把文字画一遍，量出每个格子铺不铺噪域；文字空着时是整张空表。
 *
 * 结果按上面的指纹缓存，调用方拿到的是同一份 `filled`，别就地改它。
 */
export function noiseGrid(effect: TextNoiseEffect, aspect = NOISE_ASPECT): NoiseGrid {
  const key = gridKey(effect, aspect);
  const hit = gridCache.get(key);
  if (hit) {
    // 命中就挪到队尾：长期不用的先被挤出去
    gridCache.delete(key);
    gridCache.set(key, hit);
    return hit;
  }
  return remember(gridCache, GRID_CACHE_LIMIT, key, rasterizeGrid(effect, aspect));
}

function rasterizeGrid(effect: TextNoiseEffect, aspect: number): NoiseGrid {
  const rows = Math.max(1, Math.min(MAX_ROWS, Math.round(1 / clamp(effect.cell, 0.002, 0.2))));
  const cols = Math.max(1, Math.round(aspect * rows));
  const filled = new Uint8Array(cols * rows);
  const lines = effect.text.split(/\r?\n/);
  if (!effect.text.trim()) return { cols, rows, filled };

  const canvas = document.createElement('canvas');
  canvas.width = cols * SUPERSAMPLE;
  canvas.height = rows * SUPERSAMPLE;
  const ctx = canvas.getContext('2d', { willReadFrequently: true });
  if (!ctx) return { cols, rows, filled };

  const fontPx = clamp(effect.fontSize, 0.01, 2) * rows * SUPERSAMPLE;
  const linePx = fontPx * clamp(effect.lineHeight, 0.6, 4);
  ctx.fillStyle = '#fff';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.font = `${fontPx}px ${quoteFamily(effect.fontFamily)}`;
  // 字间距在界面上按 em 存，换算成 px 才是 Canvas 认的单位；老内核没有 letterSpacing 就当 0
  const spacing = effect.letterSpacing * fontPx;
  if (spacing) {
    (ctx as CanvasRenderingContext2D & { letterSpacing?: string }).letterSpacing = `${spacing}px`;
  }
  const centerX = clamp(effect.x, 0, 1) * cols * SUPERSAMPLE;
  const centerY = clamp(effect.y, 0, 1) * rows * SUPERSAMPLE;
  const top = centerY - (linePx * lines.length) / 2;
  ctx.save();
  // 旋转绕文字块自己的中心：先挪到中心、转完再挪回去，落点仍然是 x/y
  if (effect.rotation) {
    ctx.translate(centerX, centerY);
    ctx.rotate((effect.rotation * Math.PI) / 180);
    ctx.translate(-centerX, -centerY);
  }
  lines.forEach((line, index) => ctx.fillText(line, centerX, top + linePx * (index + 0.5)));
  ctx.restore();

  const alpha = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
  const half = (255 * SUPERSAMPLE * SUPERSAMPLE) / 2;
  for (let row = 0; row < rows; row += 1) {
    for (let col = 0; col < cols; col += 1) {
      let sum = 0;
      for (let y = 0; y < SUPERSAMPLE; y += 1) {
        const base = ((row * SUPERSAMPLE + y) * canvas.width + col * SUPERSAMPLE) * 4 + 3;
        for (let x = 0; x < SUPERSAMPLE; x += 1) sum += alpha[base + x * 4] ?? 0;
      }
      if (sum > half) filled[row * cols + col] = 1;
    }
  }
  return { cols, rows, filled };
}

/** 方格坐标里的一个矩形：`x0/x1` 是列边界、`y0/y1` 是行边界（右开下开）。 */
export interface GridRect {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

/** 一行里要铺噪域的区段；开了反转就取反，把「没铺的格子」连成一整片。 */
function runsOf(filled: Uint8Array, row: number, cols: number, invert: boolean): [number, number][] {
  const runs: [number, number][] = [];
  let begin = -1;
  for (let col = 0; col < cols; col += 1) {
    const want = (filled[row * cols + col] === 1) !== invert;
    if (want && begin < 0) begin = col;
    else if (!want && begin >= 0) {
      runs.push([begin, col]);
      begin = -1;
    }
  }
  if (begin >= 0) runs.push([begin, cols]);
  return runs;
}

/**
 * 把方格合成矩形：一行里连续的格子先连成一段，上下两行同一段再接着往下长。
 *
 * 文字的一竖就是一整块，交给引擎的方块数从「笔画格数」降到十位数 / 百位数 ——
 * 引擎那边每个方块都要在 CPU 上栅格化成掩码，方块越少越省。
 */
export function gridRects(grid: NoiseGrid, invert: boolean): GridRect[] {
  const { cols, rows, filled } = grid;
  /** 正在往下长的那几段：键是「起点列:终点列」，值是它从哪一行开始。 */
  let growing = new Map<string, { start: number; x0: number; x1: number }>();
  const rects: GridRect[] = [];
  for (let row = 0; row <= rows; row += 1) {
    const runs = row < rows ? runsOf(filled, row, cols, invert) : [];
    const next = new Map<string, { start: number; x0: number; x1: number }>();
    for (const [x0, x1] of runs) {
      const key = `${x0}:${x1}`;
      const carried = growing.get(key);
      next.set(key, carried ?? { start: row, x0, x1 });
    }
    // 上一行还在长、这一行没人接手的，到上一行就封口
    for (const [key, it] of growing) {
      if (!next.has(key)) rects.push({ x0: it.x0, y0: it.start, x1: it.x1, y1: row });
    }
    growing = next;
  }
  return rects;
}

/** 一个效果铺出来的矩形（方格坐标），预览与写谱面都用它。 */
export function noiseRects(effect: TextNoiseEffect, aspect = NOISE_ASPECT): GridRect[] {
  const key = `${gridKey(effect, aspect)}\u0001${effect.invert ? 1 : 0}`;
  const hit = rectCache.get(key);
  if (hit) {
    rectCache.delete(key);
    rectCache.set(key, hit);
    return hit;
  }
  return remember(rectCache, GRID_CACHE_LIMIT, key, gridRects(noiseGrid(effect, aspect), effect.invert));
}

/** 谱面里的一块噪域，字段与引擎的原生噪域条目一致（`RawBlockArea`，camelCase）。 */
export interface BlockAreaEntry {
  bottomLeftPercentage: { x: number; y: number };
  topRightPercentage: { x: number; y: number };
  appearTime: number;
  enableTime: number;
  disableTime: number;
  disappearTime: number;
  isSubtract: boolean;
  /** 只有这一段没激活时才写 `false`：引擎缺这个字段就按激活处理，写 `true` 白占体积。 */
  enable?: boolean;
  /** 只有不是原生红时才写：引擎见到等于原生红的颜色也会退回无色材质，写上白占体积。 */
  color?: string;
  /** 这条特效轨自己的材质，只写与引擎默认值不同的键（见 `materialDelta`）。 */
  material?: Partial<NoiseMaterial>;
}

/**
 * 材质与引擎默认值的差：没改过的键不写。
 *
 * 谱面上每一条噪域都是独立的一段 JSON，整份材质原样抄进每一块会白白放大谱面；
 * 只带改动过的键，引擎那边缺的键就按自己的默认值走。
 */
function materialDelta(material: NoiseMaterial | null | undefined): Partial<NoiseMaterial> | undefined {
  if (!material) return undefined;
  const delta: Record<string, boolean | number> = {};
  for (const key of Object.keys(DEFAULT_NOISE_MATERIAL) as (keyof NoiseMaterial)[]) {
    if (material[key] !== DEFAULT_NOISE_MATERIAL[key]) delta[key] = material[key];
  }
  return Object.keys(delta).length > 0 ? (delta as Partial<NoiseMaterial>) : undefined;
}

/** 百分比留六位小数就够了，谱面 JSON 能小一截。 */
function round6(value: number) {
  return Math.round(value * 1e6) / 1e6;
}

/** 方格矩形折成引擎要的屏幕百分比（纵向翻一下：画布从上往下，噪域从左下角起算）。 */
function toEntry(
  rect: GridRect,
  cols: number,
  rows: number,
  start: number,
  end: number,
  color?: string,
  material?: Partial<NoiseMaterial>,
  subtract = false,
  enable = true,
): BlockAreaEntry {
  return {
    bottomLeftPercentage: { x: round6(rect.x0 / cols), y: round6(1 - rect.y1 / rows) },
    topRightPercentage: { x: round6(rect.x1 / cols), y: round6(1 - rect.y0 / rows) },
    appearTime: start,
    enableTime: start,
    disableTime: end,
    disappearTime: end,
    isSubtract: subtract,
    ...(enable ? {} : { enable: false }),
    ...(color ? { color } : {}),
    ...(material ? { material } : {}),
  };
}

/**
 * 一条特效轨上的全部片段。
 *
 * 新草稿存在 `lane.clips` 里（一行可以放好几段）；老草稿只有轨道自己的
 * `start` / `length` / `effect`，这里补成一个片段，界面不用分两套走。
 */
export function clipsOf(lane: DraftLane): DraftClip[] {
  if (lane.kind !== 'effect') return [];
  if (lane.clips && lane.clips.length > 0) return lane.clips;
  if (!lane.effect) return [];
  return [{ id: `${lane.id}-clip`, start: lane.start, length: lane.length, effect: lane.effect }];
}

/**
 * 一个片段铺出来的噪域方块。
 *
 * 时间取片段自己的起止，且**出现即点亮**：`appear` 与 `enable` 是同一个时刻，不走渐显，
 * 也不会在点亮前闪那三下（那三下是官方谱面噪域的「准备」提示）；到点整块收掉。
 */
export function clipBlockAreas(lane: DraftLane, clip: DraftClip, aspect = NOISE_ASPECT, fallback?: NoiseMaterial | null): BlockAreaEntry[] {
  const effect = clip.effect;
  if (!effect) return [];
  const grid = noiseGrid(effect, aspect);
  if (!effect.text.trim() && !effect.invert) return [];
  const start = Math.max(0, Number.isFinite(clip.start) ? clip.start : 0);
  const length = Math.max(0, Number.isFinite(clip.length) ? clip.length : 0);
  const end = Math.max(start + 0.05, start + length);
  const color = normalizeNoiseColor(effect.color);
  // 「融入原谱」时颜色与材质都不写，全部交给谱面自己的噪域渲染设置
  const tint = effect.blend || color === NATIVE_NOISE_COLOR ? undefined : color;
  // 老草稿的材质存在草稿上（`EditorDraft.noise`），新草稿一条轨道一份；取不到就用默认
  const material = effect.blend ? undefined : materialDelta(effect.material ?? fallback ?? DEFAULT_NOISE_MATERIAL);
  // 「反相」只把每块标成减算（`isSubtract`），几何不变；和「反转」互不影响，可以同时开着
  const subtract = effect.subtract;
  // 「激活」关掉的那一段照样写进谱面（`enable: false`），引擎读到就整段跳过；
  // 这样时间轴上与谱面里的条目一一对应，回头再打开也不用重新铺一遍
  const enable = effect.enable !== false;
  return noiseRects(effect, aspect).map((rect) => toEntry(rect, grid.cols, grid.rows, start, end, tint, material, subtract, enable));
}

/**
 * 全部特效轨的噪域方块，交给预览 / 导出叠到谱面自带的噪域后面。
 *
 * 特效轨可以有好几条，按**层级**叠加：时间轴上从上往下的顺序就是层级，最上面那条画在最上层。
 * 引擎是按顺序画的（后来的盖在先前上），所以这里把轨道倒过来排 —— 最下面那条先画、
 * 最上面那条最后画，跟时间轴上的先后顺序一致。改这里之前先想清楚：换个顺序，
 * 多条特效叠在一起时的上下关系就跟着换。同一条轨里的片段按数组顺序画。
 */
export function blockAreasOf(lanes: DraftLane[], aspect = NOISE_ASPECT, fallback?: NoiseMaterial | null): BlockAreaEntry[] {
  return lanes
    .filter((lane) => lane.kind === 'effect')
    .slice()
    .reverse()
    .flatMap((lane) => clipsOf(lane).flatMap((clip) => clipBlockAreas(lane, clip, aspect, fallback)));
}

/**
 * 把噪域方块塞进一份谱面的 JSON 字节（PGR 与 RPE 都读顶层 `blockAreaList`）。
 *
 * 只认 JSON 谱面：`.pec` 没有噪域这个字段，原样退回。解析不了（比如文件本来就不是谱面）
 * 也原样退回，别把用户的谱面改坏。
 */
export function withBlockAreas(bytes: Uint8Array, areas: BlockAreaEntry[], name: string): Uint8Array {
  if (areas.length === 0 || !/\.json$/i.test(name)) return bytes;
  let chart: unknown;
  try {
    chart = JSON.parse(new TextDecoder().decode(bytes));
  } catch {
    return bytes;
  }
  if (!chart || typeof chart !== 'object' || Array.isArray(chart)) return bytes;
  const target = chart as Record<string, unknown>;
  const list = Array.isArray(target.blockAreaList) ? target.blockAreaList : [];
  target.blockAreaList = [...list, ...areas];
  return new TextEncoder().encode(JSON.stringify(target));
}
