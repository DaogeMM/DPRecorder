<script setup lang="ts">
/**
 * 渲染弹窗：当前正在渲染的谱面。
 *
 * 长条型，显示曲绘 / 曲名 / 难度 / 进度条：圆角、第一行曲名与难度，第二行进度条。
 * 它在应用里全局显示：挂在 `App.vue` 的 `.phi-center` 里，任何页面都能看到；
 * 列表与轮询走 `../tasks.ts`。
 *
 * 只有这一条长条，一直显示展开态（没有折叠态）。
 * 宽度走 localStorage `TaskStrip.width`，设置页那根滑杆说了算，这里的内联 `width` 直接吃这个值。
 */
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';

import { convertFileSrc } from '@tauri-apps/api/core';
import { useStorage } from '@vueuse/core';
import { useI18n } from 'vue-i18n';
import { useRouter } from 'vue-router';

import { diffBadge, diffLvl, diffTier, pickActiveTask, useTasks } from '../tasks';

const tasks = useTasks();
const { t } = useI18n();
const router = useRouter();

/** 一轮里可能同时有好几条在跑，这里只认 `pickActiveTask` 选出的那一条 */
const activeTask = computed(() => pickActiveTask(tasks.value));

/** 曲绘：任务早就把曲绘落盘成临时文件了（`TaskView.cover`），直接转 asset URL，不用再读谱面包 */
const activeCover = computed(() => {
  const cover = activeTask.value?.cover;
  return cover ? convertFileSrc(cover) : '';
});

/** 曲绘读不出来时退回占位图标；换任务（URL 变了）时重置 */
const stripCoverBroken = ref(false);

function onStripCoverError() {
  stripCoverBroken.value = true;
}

watch(activeCover, () => {
  stripCoverBroken.value = false;
});

/** 进度（0–100）。加载 / 混音准备阶段没有百分比，走不确定态 */
const activeProgress = computed(() => {
  const status = activeTask.value?.status;
  if (status && 'progress' in status) return status.progress * 100;
  return 0;
});

const activeIndeterminate = computed(() => {
  const type = activeTask.value?.status.type;
  return type === 'loading' || type === 'mixing';
});

// 难度徽章与任务列表的折叠行共用同一套拆解（见 `../tasks.ts`）
const activeDiffTier = computed(() => diffTier(activeTask.value?.info));
const activeDiffLvl = computed(() => diffLvl(activeTask.value?.info));
const activeDiffBadge = computed(() => diffBadge(activeTask.value?.info));

/**
 * 上下左右拖动。
 * 存的是「距框架底边 / 右边的距离」而不是屏幕坐标：窗口尺寸变了位置依然合理。
 * 内容区不给渲染弹窗留空，
 * 所以它压住渲染页 / 批量页底部操作条时，把这里拖上去即可。
 *
 * 范围限制：两个方向都跟手拖，但整条必须留在右侧内容区
 * `.phi-center` 之内（渲染 / 批量 / 任务页就是它里面的 `router-view`），
 * 四边各留 `MIN_EDGE`。它是 `App.vue` 里 `.phi-center` 的**兄弟**（`position: absolute`、
 * 定位父级是 `.phi-frame`），`.phi-center` 的 `overflow: hidden` 拦不住它，所以范围
 * 只能靠 `clampToFrame()` 自己算：拿 `.phi-frame` 与 `.phi-center` 两个
 * `getBoundingClientRect()` 一减，换算成「离底边 / 右边多少」再夹一次。
 */
const DRAG_STORAGE_KEY = 'TaskStrip.bottom';
const DEFAULT_BOTTOM = 16; // ＝ --phi-space-4
const H_DRAG_STORAGE_KEY = 'TaskStrip.right';
const DEFAULT_RIGHT = 24; // ＝ --phi-space-6
const MIN_EDGE = 8; // 四边至少留这么多，条不会被标题栏/侧栏压住
const STRIP_HEIGHT = 72; // 与 `.task-strip` 的 height 一致（量不到元素时的兜底）
const MIN_WIDTH = 15;
const MAX_WIDTH = 100;
const DEFAULT_WIDTH = 15; // 默认就是最窄那格（设置页那根滑杆的最小值也是 15）

/**
 * 渲染弹窗宽度（百分比），放到设置里调。
 *
 * 和设置页共用同一个 localStorage 键 `TaskStrip.width`（`useStorage` 同一个页面里
 * 多个实例之间会同步），所以设置页一拉滑杆这里立刻跟着变；
 * 位置（`TaskStrip.bottom` / `TaskStrip.right`）本来就是 localStorage，保持一致。
 */
const stripWidth = useStorage<number>('TaskStrip.width', DEFAULT_WIDTH);

/** 夹到合法值域后的宽度，直接当内联 `width` 用，也供 `clampToFrame()` 兜底估宽 */
const stripWidthPct = computed(() => {
  const value = Number(stripWidth.value);
  if (!Number.isFinite(value)) return DEFAULT_WIDTH;
  return Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, value));
});

function readBottom(): number {
  const stored = Number(localStorage.getItem(DRAG_STORAGE_KEY));
  // 只认正数：万一存进去过越界（负数）的位置，回落到默认值
  return Number.isFinite(stored) && stored > 0 ? stored : DEFAULT_BOTTOM;
}

function readRight(): number {
  const stored = Number(localStorage.getItem(H_DRAG_STORAGE_KEY));
  return Number.isFinite(stored) && stored > 0 ? stored : DEFAULT_RIGHT;
}

const widgetEl = ref<HTMLElement | null>(null);
const bottomOffset = ref(readBottom());
const rightOffset = ref(readRight());

/** 渲染弹窗的定位父级：`App.vue` 的 `.phi-frame`（`position: relative`），偏移量以它为准 */
function frameEl(): HTMLElement | null {
  return widgetEl.value?.closest<HTMLElement>('.phi-frame') ?? null;
}

/**
 * 右侧内容区：渲染 / 批量 / 任务页所在的 `<main class="phi-center">`。
 *
 * 注意它是渲染弹窗的**兄弟**不是父级（`App.vue` 里 `<TaskStrip />` 紧跟在 `</main>` 后面），
 * 而且 `.task-strip` 是 `position: absolute`、定位父级是 `.phi-frame` ——
 * `.phi-center` 的 `overflow: hidden` 管不到它，所以「拖出内容区」只能靠这里算边界拦。
 */
function centerEl(): HTMLElement | null {
  return frameEl()?.querySelector<HTMLElement>('.phi-center') ?? null;
}

/** 把当前位置夹回右侧内容区：两个方向都拖得动，但整条不会跑出可视范围 */
function clampToFrame() {
  const outer = frameEl();
  const strip = widgetEl.value;
  if (!outer || !strip) return;
  const stripH = strip.offsetHeight || STRIP_HEIGHT;
  const stripW = strip.offsetWidth || (outer.clientWidth * stripWidthPct.value) / 100;
  const outerBox = outer.getBoundingClientRect();
  const centerBox = centerEl()?.getBoundingClientRect();
  // 偏移量是「离 `outer` 底边 / 右边的距离」，所以把内容区的边也换算成同一种度量
  const bottomFloor = (centerBox ? outerBox.bottom - centerBox.bottom : 0) + MIN_EDGE;
  const bottomCeil = (centerBox ? outerBox.bottom - centerBox.top : outerBox.height) - stripH - MIN_EDGE;
  const rightFloor = (centerBox ? outerBox.right - centerBox.right : 0) + MIN_EDGE;
  const rightCeil = (centerBox ? outerBox.right - centerBox.left : outerBox.width) - stripW - MIN_EDGE;
  bottomOffset.value = Math.min(Math.max(bottomOffset.value, bottomFloor), Math.max(bottomFloor, bottomCeil));
  rightOffset.value = Math.min(Math.max(rightOffset.value, rightFloor), Math.max(rightFloor, rightCeil));
}

/** 本次手势是否真的拖动过：3px 以内当误触，不写 localStorage */
let dragged = false;
let dragging = false;
let dragStartY = 0;
let dragStartX = 0;
let dragStartBottom = DEFAULT_BOTTOM;
let dragStartRight = DEFAULT_RIGHT;

function onDragStart(event: PointerEvent) {
  if (event.button !== 0) return;
  dragging = true;
  dragged = false;
  dragStartY = event.clientY;
  dragStartX = event.clientX;
  dragStartBottom = bottomOffset.value;
  dragStartRight = rightOffset.value;
  (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
}

function onDragMove(event: PointerEvent) {
  if (!dragging) return;
  const dy = dragStartY - event.clientY; // 往上拖为正
  const dx = event.clientX - dragStartX; // 往右拖为正
  if (!dragged && Math.abs(dy) < 3 && Math.abs(dx) < 3) return; // 3px 以内还是当点击
  dragged = true;
  // 上下左右都跟手，但夹在内容区内。
  // 往右拖 ⇒ 离右边更近 ⇒ 存的距离变小。
  bottomOffset.value = dragStartBottom + dy;
  rightOffset.value = dragStartRight - dx;
  clampToFrame();
}

/**
 * 单击（没有拖动）⇒ 跳到任务页。
 *
 * 刚拖完的那次 `click` 不算：`dragged` 要等下一次按下（`onDragStart`）才复位，
 * 所以拖完松手时它会挡住这里 —— 不用额外挂「吞掉下一个 click」的定时器。
 */
function onStripClick() {
  if (dragged) return;
  void router.push('/tasks');
}

function onDragEnd() {
  if (!dragging) return;
  dragging = false;
  if (!dragged) return;
  // 松手前再夹一次：兜住「拖拽中途量不到内容区」之类没夹到的情况
  clampToFrame();
  localStorage.setItem(DRAG_STORAGE_KEY, String(Math.round(bottomOffset.value)));
  localStorage.setItem(H_DRAG_STORAGE_KEY, String(Math.round(rightOffset.value)));
}

// 内容区尺寸变了（窗口缩放、左侧栏拖动调宽）、或者渲染弹窗刚出现（宽度要靠它自己量）时，
// 都把位置夹回内容区。用 ResizeObserver 盯着 `.phi-frame` / `.phi-center` 本身，
// 比只听 window.resize 准。
let resizeObserver: ResizeObserver | null = null;
onMounted(() => {
  void nextTick(() => {
    clampToFrame();
    if (typeof ResizeObserver === 'undefined') return;
    resizeObserver = new ResizeObserver(() => clampToFrame());
    for (const el of [frameEl(), centerEl()]) {
      if (el) resizeObserver.observe(el);
    }
  });
});
onUnmounted(() => {
  resizeObserver?.disconnect();
  resizeObserver = null;
});
watch(activeTask, () => void nextTick(clampToFrame));
// 宽度变了（设置页拉滑杆）条会往左长，可能顶出内容区，跟着重新夹一次
watch(stripWidthPct, () => void nextTick(clampToFrame));
// 任何原因把位置写到界外（启动时读到的旧值、条变高变宽、窗口被拉小）都立刻夹回来。
// Vue 对「写入同一个值」不再触发 watcher，所以夹完一轮就收敛，不会来回抖。
watch([bottomOffset, rightOffset], () => clampToFrame());
</script>

<template>
  <!-- 空闲时整个渲染弹窗都不渲染 -->
  <transition name="fade-transition">
    <!-- 长条型渲染弹窗（一直显示展开的，宽度 20%），
         上下左右都能拖。 -->
    <div
      v-if="activeTask"
      ref="widgetEl"
      class="task-strip"
      :style="{ bottom: `${bottomOffset}px`, right: `${rightOffset}px`, width: `${stripWidthPct}%` }"
      :title="t('open-tasks')"
      @pointerdown="onDragStart"
      @pointermove="onDragMove"
      @pointerup="onDragEnd"
      @pointercancel="onDragEnd"
      @click="onStripClick"
    >
      <div class="task-strip__cover">
        <img
          v-if="activeCover && !stripCoverBroken"
          :src="activeCover"
          alt=""
          @error="onStripCoverError"
        />
        <v-icon v-else size="22">mdi-image-outline</v-icon>
      </div>
      <!-- 第一行曲名 + 难度，第二行进度条铺满 -->
      <div class="task-strip__main">
        <div class="task-strip__row">
          <span class="task-strip__name">{{ activeTask.name }}</span>
          <span v-if="activeDiffBadge" class="task-strip__diff">
            <span class="task-strip__diff-tier">{{ activeDiffTier || activeDiffBadge }}</span>
            <span v-if="activeDiffTier && activeDiffLvl" class="task-strip__diff-lvl">Lv.{{ activeDiffLvl }}</span>
          </span>
        </div>
        <v-progress-linear
          class="task-strip__bar"
          :class="{ 'glow-spinner': activeIndeterminate }"
          :model-value="activeProgress"
          :indeterminate="activeIndeterminate"
          height="6"
          rounded
        ></v-progress-linear>
      </div>
    </div>
  </transition>
</template>

<style scoped>
/* ---------- 渲染弹窗：当前渲染的谱面 ----------
   长条型，放曲绘 / 曲名 / 难度 / 进度条四样东西；
   曲绘在左，右侧两行 —— 第一行曲名 + 难度，第二行进度条。
   它是 `App.vue` 里 `.phi-frame`（框架层，position: relative）的子元素，所以用
   absolute 而不是 fixed：fixed 会按视口定位，被左侧导航栏和标题栏挤歪。
   宽度与位置：右下角对齐；宽度由内联样式给（设置页的 `TaskStrip.width`），
   下面这条 `width` 只剩「内联没来之前」的静态默认值。
   高度固定 72px（box-sizing 写死，免得被全局样式影响）。
   它直接浮在内容之上：内容区不再为它留 padding-bottom
   （`App.vue` 里 `.phi-center` 只保留 position: relative）。
   可以上下左右拖动，但限制在右侧区域内 ⇒ 位置完全由内联 `bottom` / `right` 决定（那段会按
   `.phi-center` 实时尺寸夹回内容区内），这里的两条 `right`/`bottom` 只是静态默认值。 */
.task-strip {
  position: absolute;
  z-index: 10;
  right: var(--phi-space-6);
  bottom: var(--phi-space-4);
  width: 20%;
  touch-action: none;
  user-select: none;
  cursor: grab;
  box-sizing: border-box;
  display: flex;
  align-items: center;
  gap: var(--phi-space-4);
  height: 72px;
  padding: var(--phi-space-2) var(--phi-space-4);
  border: 0.5px solid var(--phi-border-l3);
  /* 和 `.phi-config-dialog` 同一层「浮起来」的感觉 */
  background: rgb(21 21 23 / 88%);
  box-shadow: var(--phi-elevation-panel);
  /* 渲染弹窗加个圆角 */
  border-radius: var(--phi-radius-lg);
}

.task-strip:active {
  cursor: grabbing;
}

/* 曲绘固定 72px：条本身窄，再宽曲名只剩几个字就被截断。 */
.task-strip__cover {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 72px;
  aspect-ratio: 2048 / 1080;
  overflow: hidden;
  border: 0.5px solid var(--phi-border-l3);
  background: var(--phi-bg-layer-3);
  color: var(--phi-label-caption);
}

.task-strip__cover img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.task-strip__name {
  overflow: hidden;
  flex: 0 1 auto;
  min-width: 0;
  font-size: 1rem;
  font-weight: 600;
  line-height: 1.25;
  text-overflow: ellipsis;
  white-space: nowrap;
  user-select: none;
  color: var(--phi-label-primary);
}

/* 难度徽章：与 ChartHero 那枚「灰底白字、无圆角」一致 */
.task-strip__diff {
  display: inline-flex;
  flex: none;
  align-items: center;
  gap: 5px;
  height: 20px;
  padding: 0 7px;
  border-radius: 0;
  background: var(--phi-static-neutral-600);
  color: var(--phi-static-bluish-00);
  font-size: 0.72rem;
  font-weight: 400;
  letter-spacing: 0.06em;
  user-select: none;
}

.task-strip__diff-tier {
  text-transform: uppercase;
}

.task-strip__diff-lvl {
  font-variant-numeric: tabular-nums;
  letter-spacing: 0;
}

/* 右侧两行：第一行曲名 + 难度徽章，第二行进度条铺满整行 */
.task-strip__main {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  gap: var(--phi-space-2);
  min-width: 0;
}

.task-strip__row {
  display: flex;
  align-items: center;
  gap: var(--phi-space-3);
  min-width: 0;
}

.task-strip__bar {
  width: 100%;
}
</style>
