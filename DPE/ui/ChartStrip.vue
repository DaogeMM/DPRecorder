<script setup lang="ts">
/**
 * 已选谱面弹窗：DPE 这一侧的全局悬浮条。
 *
 * 只显示两件事：**选了几首**、**最后选的那首的曲绘**。不显示渲染进度 ——
 * 那条只属于 DPR（`TaskStrip.vue`）。
 *
 * 数据与列表页共用同一份 localStorage（`BatchView.ChartList@dpe`，见 `BatchView.vue` 的
 * `storageKey()`）；「最后选中的是哪首」由列表页在勾选时写进 `DPE.LastSelected`
 * （`BatchView.vue` 里那个 `watch(charts)`），这里只读。
 *
 * 拖动的范围限制、位置与宽度的存储走 `@/components/floatingStrip`（与 DPR 的渲染进度条共用一套）。
 */
import { computed } from 'vue';

import { useStorage } from '@vueuse/core';
import { useI18n } from 'vue-i18n';
import { useRouter } from 'vue-router';

import ChartThumb from '@/components/ChartThumb.vue';
import { useFloatingStrip } from '@/components/floatingStrip';
import type { RenderChart } from '@/model';
import { diffBadge, diffLvl, diffTier } from '@/tasks';

const { t } = useI18n();
const router = useRouter();

/** 与 `BatchView.vue` 的 `scope === 'dpe'` 那份列表是同一个键 */
const charts = useStorage<RenderChart[]>('BatchView.ChartList@dpe', []);

/** 最后勾选的那张的路径；它被取消 / 移出列表时回落到勾选里的第一张 */
const lastPath = useStorage<string>('DPE.LastSelected', '');

const selected = computed(() => charts.value.filter((chart) => chart.isSelect));

const last = computed(
  () => selected.value.find((chart) => chart.path === lastPath.value) ?? selected.value[0],
);

/** 文件名（谱面包名）：路径的最后一段 */
const lastFile = computed(() => last.value?.path.split(/[\\/]/).pop() ?? '');

/** 难度徽章：口径同渲染悬浮窗（`TaskStrip.vue`）与谱面列表行内那枚，见 `tasks.ts` */
const diffTierText = computed(() => diffTier(last.value?.chartInfo));
const diffLvlText = computed(() => diffLvl(last.value?.chartInfo));
const diffBadgeText = computed(() => diffBadge(last.value?.chartInfo));

const { widgetEl, bottomOffset, rightOffset, widthPct, onDragStart, onDragMove, onDragEnd, onClick } =
  useFloatingStrip({
    bottomKey: 'ChartStrip.bottom',
    rightKey: 'ChartStrip.right',
    widthKey: 'ChartStrip.width',
    defaultBottom: 16,
    defaultRight: 24,
    defaultWidth: 20,
    onClick: () => void router.push('/charts'),
  });
</script>

<template>
  <!-- 一张都没勾时整条不渲染 -->
  <transition name="fade-transition">
    <div
      v-if="last"
      ref="widgetEl"
      class="chart-strip"
      :style="{ bottom: `${bottomOffset}px`, right: `${rightOffset}px`, width: `${widthPct}%` }"
      :title="t('open-chart-list')"
      @pointerdown="onDragStart"
      @pointermove="onDragMove"
      @pointerup="onDragEnd"
      @pointercancel="onDragEnd"
      @click="onClick"
    >
      <div class="chart-strip__cover">
        <ChartThumb :path="last.path" :illustration="last.chartInfo.illustration"></ChartThumb>
      </div>
      <div class="chart-strip__text">
        <div class="chart-strip__title">
          <span class="chart-strip__name">{{ last.chartInfo.name }}</span>
          <!-- 难度：写法与渲染悬浮窗（`TaskStrip.vue`）一致 —— 第一行曲名 + 灰底难度徽章 -->
          <span
            v-if="diffBadgeText"
            class="chart-strip__diff"
            :title="last.chartInfo.difficulty || diffBadgeText"
          >
            <span class="chart-strip__diff-tier">{{ diffTierText || diffBadgeText }}</span>
            <span v-if="diffTierText && diffLvlText" class="chart-strip__diff-lvl">Lv.{{ diffLvlText }}</span>
          </span>
        </div>
        <span class="chart-strip__file">{{ lastFile }}</span>
      </div>
      <span v-if="selected.length >= 2" class="chart-strip__more">
        {{ t('chart-strip-more', { n: selected.length }) }}
      </span>
    </div>
  </transition>
</template>

<style scoped>
/* 与 `.task-strip` 同一套外观：`App.vue` 里 `.phi-frame` 的子元素，所以 absolute；
   位置与宽度完全由内联样式给（`useFloatingStrip`）。 */
.chart-strip {
  position: absolute;
  z-index: 10;
  display: flex;
  align-items: center;
  gap: var(--phi-space-3);
  height: 72px;
  padding: var(--phi-space-2) var(--phi-space-4);
  border: 0.5px solid var(--phi-border-l3);
  border-radius: var(--phi-radius-lg);
  background: rgb(21 21 23 / 88%);
  box-shadow: var(--phi-elevation-panel);
  touch-action: none;
  box-sizing: border-box;
  cursor: grab;
  user-select: none;
}

.chart-strip:active {
  cursor: grabbing;
}

/* 曲绘固定 72px：与渲染进度条里的曲绘同尺寸 */
.chart-strip__cover {
  flex: none;
  overflow: hidden;
  width: 72px;
  aspect-ratio: 2048 / 1080;
  border: 0.5px solid var(--phi-border-l3);
  background: var(--phi-bg-layer-3);
}

.chart-strip__cover :deep(img) {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.chart-strip__text {
  display: flex;
  overflow: hidden;
  flex: 1 1 auto;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

/* 第一行曲名 + 难度徽章：与渲染悬浮窗（`TaskStrip.vue` 的 `.task-strip__row`）同一套间距 */
.chart-strip__title {
  display: flex;
  align-items: center;
  gap: var(--phi-space-3);
  min-width: 0;
}

.chart-strip__name {
  overflow: hidden;
  flex: 0 1 auto;
  min-width: 0;
  font-size: 1rem;
  font-weight: 600;
  line-height: 1.2;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--phi-label-primary);
}

/* 难度徽章：与渲染悬浮窗 / ChartHero 那枚「灰底白字、无圆角」一致。改过难度的谱面，
   徽章里是整串「（A → B）」，整行放不下时先让曲名收缩，徽章自己再省略（完整文本看 title） */
.chart-strip__diff {
  display: inline-flex;
  overflow: hidden;
  flex: 0 1 auto;
  align-items: center;
  gap: 5px;
  min-width: 0;
  height: 20px;
  padding: 0 7px;
  border-radius: 0;
  background: var(--phi-static-neutral-600);
  color: var(--phi-static-bluish-00);
  font-size: 0.72rem;
  font-weight: 400;
  letter-spacing: 0.06em;
  white-space: nowrap;
  user-select: none;
}

.chart-strip__diff-tier {
  overflow: hidden;
  min-width: 0;
  text-overflow: ellipsis;
  text-transform: uppercase;
}

.chart-strip__diff-lvl {
  flex: none;
  font-variant-numeric: tabular-nums;
  letter-spacing: 0;
}

.chart-strip__file {
  overflow: hidden;
  font-size: var(--phi-font-size-caption);
  line-height: 1.2;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--phi-label-tertiary);
}

/* 「等共 N 张谱」贴在最右侧，不参与省略 */
.chart-strip__more {
  flex: none;
  font-size: var(--phi-font-size-caption);
  white-space: nowrap;
  color: var(--phi-label-secondary);
}
</style>
