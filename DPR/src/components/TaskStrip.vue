<script setup lang="ts">
/**
 * 渲染弹窗：当前正在渲染的谱面。
 *
 * 长条型，显示曲绘 / 曲名 / 难度 / 进度条：圆角、第一行曲名与难度，第二行进度条。
 * 它在应用里全局显示：挂在 `App.vue` 里，只要当前是 DPR 部分就能看到；
 * 列表与轮询走 `../tasks.ts`。
 *
 * 只有这一条长条，一直显示展开态（没有折叠态）。
 * 拖动的范围限制、位置与宽度的存储都在 `./floatingStrip.ts`（DPE 的已选谱面条共用同一套）。
 */
import { computed, nextTick, ref, watch } from 'vue';

import { convertFileSrc } from '@tauri-apps/api/core';
import { useI18n } from 'vue-i18n';
import { useRouter } from 'vue-router';

import { diffBadge, diffLvl, diffTier, pickActiveTask, useTasks } from '../tasks';
import { useFloatingStrip } from './floatingStrip';

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
 * 位置与宽度：右下角起、可拖动，宽度百分比存 `TaskStrip.width`（设置页那根滑杆说了算）。
 * 单击（没拖动）= 跳到任务页。
 */
const { widgetEl, bottomOffset, rightOffset, widthPct, clamp, onDragStart, onDragMove, onDragEnd, onClick } =
  useFloatingStrip({
    bottomKey: 'TaskStrip.bottom',
    rightKey: 'TaskStrip.right',
    widthKey: 'TaskStrip.width',
    defaultBottom: 16, // ＝ --phi-space-4
    defaultRight: 24, // ＝ --phi-space-6
    defaultWidth: 15, // 默认就是最窄那格（设置页那根滑杆的最小值也是 15）
    onClick: () => void router.push('/tasks'),
  });

// 任务出现 / 换任务时条才刚有尺寸，量完夹一次
watch(activeTask, () => void nextTick(clamp));
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
      :style="{ bottom: `${bottomOffset}px`, right: `${rightOffset}px`, width: `${widthPct}%` }"
      :title="t('open-tasks')"
      @pointerdown="onDragStart"
      @pointermove="onDragMove"
      @pointerup="onDragEnd"
      @pointercancel="onDragEnd"
      @click="onClick"
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
