<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue';

import { DEFAULT_NOISE_MATERIAL, type TextNoiseEffect } from '@/model';
import { NoisePreviewRenderer, type NoisePreviewFrame } from './noise-preview/renderer';
import { noiseGrid, noiseRgb, type NoiseGrid } from './text-noise';

/**
 * 右栏的黑底预览：按噪域画笔那套 WebGL 着色器画这条特效轨的样子。
 *
 * 形状来自 `noiseGrid`（和交给引擎的是同一份格子），外观用引擎同款片元着色器 +
 * 同三张贴图（`BlockNoise1` / `PointNoise` / `FD_Noise`）渲染，所以斑点、火花、
 * 扭曲、颗粒、颜色的样子和模拟器一致；`time` 每帧推进，流动速度也看得出来。
 * 这里不是真的跑引擎 —— 引擎要整页重开才换得了噪域，跟不上敲键盘。
 */

const props = defineProps<{ effect: TextNoiseEffect | null }>();

const canvas = ref<HTMLCanvasElement | null>(null);
let renderer: NoisePreviewRenderer | null = null;
let grid: NoiseGrid | null = null;
/** 最终占位（已按反转取反）：只有格子表或反转变了才重算，别每帧重拷一份。 */
let cells: Uint8Array | null = null;
let cellsSource: NoiseGrid | null = null;
let cellsInvert = false;
let observer: ResizeObserver | null = null;
let raf = 0;
let time = 0;
let last = 0;
let alive = false;

/** 把 `filled` 和「反转」合成最终占位：1 表示这一格有噪域。 */
function cellsOf(source: NoiseGrid, invert: boolean): Uint8Array {
  const cells = new Uint8Array(source.cols * source.rows);
  for (let i = 0; i < cells.length; i += 1) cells[i] = (source.filled[i] === 1) !== invert ? 1 : 0;
  return cells;
}

/** 效果或字体变了才重算格子：`noiseGrid` 要光栅化一遍文字，不能每帧跑。 */
function rebuild() {
  grid = props.effect ? noiseGrid(props.effect) : null;
  if (!props.effect || !grid) {
    cells = null;
    cellsSource = null;
    return;
  }
  // 预览只按几何画：「反相」只是把方块的 isSubtract 标打开，画出来的形状跟这块的
  // 反转状态走，引擎那边怎么吃这个标记由它决定
  const invert = props.effect.invert;
  if (cellsSource !== grid || cellsInvert !== invert) {
    cells = cellsOf(grid, invert);
    cellsSource = grid;
    cellsInvert = invert;
  }
}

function frameOf(current: NoiseGrid, effect: TextNoiseEffect): NoisePreviewFrame {
  const material = effect.material ?? DEFAULT_NOISE_MATERIAL;
  return {
    cells: cells ?? cellsOf(current, effect.invert),
    gw: current.cols,
    gh: current.rows,
    time,
    color: noiseRgb(effect.color),
    edge: material.blockAreaEdgeNoise,
    pixelScale: material.blockAreaPixelScale,
    displace: material.blockAreaDisplaceStrength,
    noiseRadius: material.blockAreaNoiseRadius,
    noiseSmoothness: material.blockAreaNoiseSmoothness,
    blotch: material.blockAreaNoiseBlotch,
    spark: material.blockAreaSpark,
    distort: material.blockAreaDistort,
  };
}

function paint() {
  const el = canvas.value;
  const effect = props.effect;
  if (!el) return;
  const ratio = window.devicePixelRatio || 1;
  const width = Math.max(1, Math.round(el.clientWidth * ratio));
  const height = Math.max(1, Math.round(el.clientHeight * ratio));
  if (el.width !== width || el.height !== height) {
    el.width = width;
    el.height = height;
  }
  if (!effect || !grid) {
    const ctx = el.getContext('2d');
    if (ctx) {
      ctx.fillStyle = '#000';
      ctx.fillRect(0, 0, el.width, el.height);
    }
    return;
  }
  if (renderer) {
    renderer.render(frameOf(grid, effect));
    return;
  }
  paintFallback(grid, effect);
}

/** 没有 WebGL 时退回平涂：形状对得上，材质只能欠着。 */
function paintFallback(source: NoiseGrid, effect: TextNoiseEffect) {
  const el = canvas.value;
  const ctx = el?.getContext('2d');
  if (!el || !ctx) return;
  const material = effect.material ?? DEFAULT_NOISE_MATERIAL;
  const [red, green, blue] = noiseRgb(effect.color);
  const filledCells = cells ?? cellsOf(source, effect.invert);
  ctx.fillStyle = '#000';
  ctx.fillRect(0, 0, el.width, el.height);
  ctx.fillStyle = `rgb(${red} ${green} ${blue} / ${Math.min(1, material.blockAreaAlpha * 2.6)})`;
  const cellWidth = el.width / source.cols;
  const cellHeight = el.height / source.rows;
  for (let row = 0; row < source.rows; row += 1) {
    for (let col = 0; col < source.cols; col += 1) {
      if (filledCells[row * source.cols + col] !== 1) continue;
      ctx.fillRect(col * cellWidth, row * cellHeight, cellWidth + 0.5, cellHeight + 0.5);
    }
  }
}

function loop(now: number) {
  if (!alive) return;
  const effect = props.effect;
  const material = effect?.material ?? DEFAULT_NOISE_MATERIAL;
  const delta = last === 0 ? 0 : Math.min(0.1, (now - last) / 1000);
  last = now;
  // 「流动速度」在引擎里是一组噪声推进速度的组合；这里换算成一个时间倍率，肉眼观感接近
  time += delta * Math.max(0.1, material.blockAreaDisplaceSpeed);
  paint();
  raf = requestAnimationFrame(loop);
}

function restart() {
  rebuild();
  paint();
}

watch(() => props.effect, restart, { deep: true });
watch(canvas, restart);

onMounted(async () => {
  alive = true;
  const el = canvas.value;
  if (el) {
    const engine = new NoisePreviewRenderer();
    if (await engine.attach(el)) renderer = engine;
  }
  restart();
  // 内置字体（`Phigros`）是异步加载的：字体到位后重算格子，免得拿回退字体算了一遍
  void document.fonts?.ready.then(() => restart());
  if (typeof ResizeObserver !== 'undefined' && el) {
    observer = new ResizeObserver(() => paint());
    observer.observe(el);
  }
  raf = requestAnimationFrame(loop);
});

onBeforeUnmount(() => {
  alive = false;
  cancelAnimationFrame(raf);
  observer?.disconnect();
  observer = null;
  renderer?.dispose();
  renderer = null;
});
</script>

<template>
  <canvas ref="canvas" class="noise-sample"></canvas>
</template>

<style scoped>
/* 16:9 的一块黑底：宽度跟着右栏走，高度自己算 */
.noise-sample {
  display: block;
  width: 100%;
  aspect-ratio: 16 / 9;
  background: #000;
  border: 0.5px solid var(--phi-border-l1);
  border-radius: var(--phi-radius-sm);
}
</style>
