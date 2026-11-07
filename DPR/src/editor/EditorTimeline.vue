<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';

import { useI18n } from 'vue-i18n';
const { t } = useI18n();

import type { DraftMedia, DraftMediaKind, EditorDraft } from '@/model';
import { dragLane, dragMedia } from './media-drag';

/**
 * 编辑窗口底部：进度条 + 时间轴。
 *
 * 剪映底部是「工具栏 + 标尺 + 一条条轨道」：这里三条固定轨道 —— 谱面 / 音乐 / 曲绘，
 * 对应草稿里的三条轨道。素材从右上角处理区的素材库拖进来，落在哪条轨道上就放到谱面的哪一项；
 * 拖到别的轨道上也不出错（按素材自己的种类归位，见 `EditorWindow.assignMedia`）。
 */

const props = defineProps<{
  draft: EditorDraft | null;
  playing: boolean;
  time: number;
  duration: number;
}>();

const emit = defineEmits<{
  'toggle-play': [];
  seek: [seconds: number];
  assign: [media: DraftMedia];
  clear: [kind: DraftMediaKind];
}>();

/** 播放头 / 进度条填充的百分比（时长按 0 处理也要给出合法值）。 */
const percent = computed(() => {
  const total = props.duration > 0 ? props.duration : 1;
  return `${Math.min(100, Math.max(0, (props.time / total) * 100))}%`;
});

function clock(seconds: number) {
  const total = Math.max(0, Math.floor(seconds));
  return `${String(Math.floor(total / 60)).padStart(2, '0')}:${String(total % 60).padStart(2, '0')}`;
}

const timeText = computed(() => clock(props.time));
const durationText = computed(() => clock(props.duration));

/**
 * 横向缩放：1 倍 = 整段谱面正好铺满可视区；再往上放大就超出可视区、改走横向滚动条。
 * 倍数按档位取，工具条上那两个放大镜各走一档；最大 16 倍（三分钟的歌一屏也就十来秒）。
 */
const ZOOM_STEPS = [1, 1.5, 2, 3, 4, 6, 8, 12, 16];

const zoomIndex = ref(0);
const zoom = computed(() => ZOOM_STEPS[zoomIndex.value] ?? 1);
const canZoomOut = computed(() => zoomIndex.value > 0);
const canZoomIn = computed(() => zoomIndex.value < ZOOM_STEPS.length - 1);

/** 轨道区（横向滚动容器）本身，以及它当前的可视宽度 —— 刻度按它算疏密。 */
const lanes = ref<HTMLElement | null>(null);
const viewportWidth = ref(0);

/** 标尺刻度的秒数档位：挑第一个能让相邻刻度拉开约 80px 的。 */
const TICK_STEPS = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800];

/**
 * 标尺刻度：按当前缩放与可视宽度挑一个「好看的」秒数，让相邻标签不会被挤在一起；
 * 末尾补一个整段终点的刻度。
 */
const ticks = computed(() => {
  const duration = props.duration;
  if (duration <= 0) return [];
  const pxPerSecond = (zoom.value * Math.max(viewportWidth.value, 1)) / duration;
  const step = TICK_STEPS.find((it) => it * pxPerSecond >= 80) ?? 1800;

  const out: { left: string; label: string; last: boolean }[] = [];
  for (let second = 0; second <= duration; second += step) {
    out.push({
      left: `${(second / duration) * 100}%`,
      label: clock(second),
      last: false,
    });
  }
  const last = out[out.length - 1];
  if (last && last.left !== '100%') {
    out.push({ left: '100%', label: clock(duration), last: true });
  } else if (last) {
    last.last = true;
  }
  return out;
});

/** 在标尺或进度条上点/拖：按点击位置换算成秒数。 */
function seekFromEvent(event: MouseEvent) {
  const el = event.currentTarget as HTMLElement;
  const rect = el.getBoundingClientRect();
  if (rect.width <= 0) return;
  const ratio = Math.min(Math.max((event.clientX - rect.left) / rect.width, 0), 1);
  emit('seek', ratio * props.duration);
}

/** 拖动条给的是档位（0 … ZOOM_STEPS.length - 1），和两个放大镜走同一套换档逻辑。 */
function onZoomInput(value: number | number[]) {
  const index = Array.isArray(value) ? (value[0] ?? 0) : value;
  void setZoom(index);
}

/** 换一档缩放：把「视口中心那一刻」留在原地，放大 / 缩小后画面不会跳走。
 * 宽度是百分比写的，得等这一帧把新宽度写进 DOM 再改 `scrollLeft`。
 */
async function setZoom(next: number) {
  const clamped = Math.min(Math.max(next, 0), ZOOM_STEPS.length - 1);
  if (clamped === zoomIndex.value) return;

  const el = lanes.value;
  const width = el?.clientWidth ?? 0;
  const center = el && width > 0 ? (el.scrollLeft + width / 2) / (zoom.value * width) : 0.5;

  zoomIndex.value = clamped;
  await nextTick();

  if (!el || width <= 0) return;
  el.scrollLeft = Math.max(0, center * zoom.value * el.clientWidth - el.clientWidth / 2);
}

/** 播放头跑出可视区（或被缩放挤出画面）就把它拉回来，滚到正中。 */
function revealPlayhead() {
  const el = lanes.value;
  if (!el || props.duration <= 0 || el.clientWidth <= 0) return;

  const x = (props.time / props.duration) * zoom.value * el.clientWidth;
  const margin = 24;
  if (x < el.scrollLeft + margin || x > el.scrollLeft + el.clientWidth - margin) {
    el.scrollLeft = Math.max(0, x - el.clientWidth / 2);
  }
}

watch(() => props.time, revealPlayhead);

/**
 * 选中的轨道对象与它上面的右键菜单。
 *
 * 轨道上每样东西都是一个对象（剪映里叫片段）：点一下选中它（画一圈高亮），
 * 右键弹出菜单做移除这类操作，选中时按 Delete / Backspace 也能移除。
 */
const selected = ref<DraftMediaKind | null>(null);
const menu = ref<{ kind: DraftMediaKind; x: number; y: number } | null>(null);

function selectClip(kind: DraftMediaKind, event: MouseEvent) {
  selected.value = kind;
  seekFromEvent(event);
}

function openMenu(kind: DraftMediaKind, event: MouseEvent) {
  event.preventDefault();
  selected.value = kind;
  // 菜单用 fixed 定位：时间轴里横向滚动，跟着内容走会被裁掉
  menu.value = { kind, x: event.clientX, y: event.clientY };
}

function closeMenu() {
  menu.value = null;
}

function removeSelected() {
  const kind = selected.value;
  if (!kind) return;
  emit('clear', kind);
  selected.value = null;
}

function removeFromMenu() {
  const kind = menu.value?.kind;
  closeMenu();
  if (!kind) return;
  selected.value = null;
  emit('clear', kind);
}

/** Delete / Backspace 删掉选中的对象；正在输入框里打字时不动它。 */
function onKeydown(event: KeyboardEvent) {
  if (event.key !== 'Delete' && event.key !== 'Backspace') return;
  const target = event.target as HTMLElement | null;
  if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)) {
    return;
  }
  if (!selected.value) return;
  event.preventDefault();
  removeSelected();
}

// 换草稿时选中的对象跟着换，不清会指到上一份草稿的轨道上
watch(() => props.draft?.id, () => {
  selected.value = null;
  closeMenu();
});

/** 可视宽度变了（窗口拉伸、时间轴拖高）就重算刻度疏密。 */
let resizeObserver: ResizeObserver | null = null;

onMounted(() => {
  const el = lanes.value;
  if (!el) return;
  viewportWidth.value = el.clientWidth;
  resizeObserver = new ResizeObserver(() => {
    viewportWidth.value = el.clientWidth;
  });
  resizeObserver.observe(el);
  window.addEventListener('keydown', onKeydown);
  // 点到别处就把菜单收起来（下次右键再开）
  window.addEventListener('pointerdown', closeMenu);
});

onBeforeUnmount(() => {
  resizeObserver?.disconnect();
  resizeObserver = null;
  window.removeEventListener('keydown', onKeydown);
  window.removeEventListener('pointerdown', closeMenu);
});

/**
 * 轨道表：音乐 / 谱面 / 曲绘。
 *
 * 三条都是随草稿一起存在的主轨道（剪映里这类轨道固定在顶部、不能增删），
 * 没放东西的轨道显示一句「把xx拖到这里」。
 */
const tracks = computed(() => [
  {
    id: 'music' as const,
    icon: 'mdi-music-note',
    label: 'editor-track-music',
    drop: 'editor-drop-music',
    media: props.draft?.music ?? null,
  },
  {
    id: 'chart' as const,
    icon: 'mdi-file-document-outline',
    label: 'editor-track-chart',
    drop: 'editor-drop-chart',
    media: props.draft?.chart ?? null,
  },
  {
    id: 'cover' as const,
    icon: 'mdi-image-outline',
    label: 'editor-track-cover',
    drop: 'editor-drop-cover',
    media: props.draft?.cover ?? null,
  },
]);

/* 素材是 pointer 拖进来的（见 `media-drag.ts`）：这里只留落点属性 `data-track-kind`，
   指针压在哪条轨道上就点亮哪条（`dragLane` 由指针位置算，不按素材类型）。 */

/**
 * 指针正压着这条轨道、而这条轨道上已经有别的东西 —— 松手就是替换。
 * 用来把块上的文件名换成「松开替换」，免得以为又叠了一层。
 */
function replacing(track: { id: DraftMediaKind; media: DraftMedia | null }) {
  return (
    dragLane.value === track.id &&
    !!track.media &&
    !!dragMedia.value &&
    dragMedia.value.path !== track.media.path
  );
}
</script>

<template>
  <footer class="timeline">
    <div class="timeline__bar">
      <v-btn
        variant="text"
        size="small"
        :icon="playing ? 'mdi-pause' : 'mdi-play'"
        :title="playing ? t('editor-pause') : t('editor-play')"
        @click="emit('toggle-play')"
      ></v-btn>
      <span class="timeline__time">{{ timeText }}</span>
      <span class="timeline__sep">/</span>
      <span class="timeline__total">{{ durationText }}</span>

      <v-spacer></v-spacer>

      <!-- 缩放拖动条：直接拖到想要的倍率（档位和两个放大镜共用一套） -->
      <div class="timeline__zoom">
        <v-slider
          :model-value="zoomIndex"
          :min="0"
          :max="ZOOM_STEPS.length - 1"
          :step="1"
          :thumb-size="10"
          density="compact"
          hide-details
          color="primary"
          @update:model-value="onZoomInput"
        ></v-slider>
      </div>
      <v-btn
        variant="text"
        size="small"
        icon="mdi-magnify-minus-outline"
        :title="t('editor-zoom-out')"
        :disabled="!canZoomOut"
        @click="setZoom(zoomIndex - 1)"
      ></v-btn>
      <v-btn
        variant="text"
        size="small"
        icon="mdi-magnify-plus-outline"
        :title="t('editor-zoom-in')"
        :disabled="!canZoomIn"
        @click="setZoom(zoomIndex + 1)"
      ></v-btn>
    </div>

    <!-- 进度条：和下面标尺、轨道共用一条时间轴（左边让出轨道名那一列），
         所以拖动/点击换算出的秒数与播放头落在同一处 -->
    <div class="timeline__scrub">
      <div class="timeline__scrub-gutter"></div>
      <div
        class="timeline__progress"
        role="slider"
        :aria-valuenow="Math.round(time)"
        :aria-valuemin="0"
        :aria-valuemax="Math.round(duration)"
        tabindex="0"
        @click="seekFromEvent"
      >
        <div class="timeline__progress-fill" :style="{ width: percent }"></div>
        <div class="timeline__progress-thumb" :style="{ left: percent }"></div>
      </div>
    </div>

    <div class="timeline__body">
      <div class="timeline__heads">
        <div class="timeline__head-spacer"></div>
        <div v-for="track in tracks" :key="track.id" class="timeline__head">
          <v-icon :icon="track.icon" size="14"></v-icon>
          <span>{{ t(track.label) }}</span>
        </div>
      </div>

      <!-- 轨道区是横向滚动容器，里面这条「画布」按缩放倍数撑宽：
           标尺、轨道块、播放头都挂在画布上，所以它们永远共用同一条时间轴 -->
      <div ref="lanes" class="timeline__lanes">
        <div class="timeline__canvas" :style="{ width: `${zoom * 100}%` }">
          <!-- 标尺 -->
          <div class="timeline__ruler" @click="seekFromEvent">
            <div
              v-for="tick in ticks"
              :key="tick.left"
              class="timeline__tick"
              :class="{ 'timeline__tick--last': tick.last }"
              :style="{ left: tick.left }"
            >
              <span class="timeline__tick-line"></span>
              <span class="timeline__tick-label">{{ tick.label }}</span>
            </div>
          </div>

          <div
            v-for="track in tracks"
            :key="track.id"
            class="timeline__lane"
            :class="{ 'timeline__lane--over': dragLane === track.id }"
            :data-track-kind="track.id"
          >
            <!-- 轨道上有东西：铺一条整段的块（对象），点一下选中、右键出菜单 -->
            <div
              v-if="track.media"
              class="timeline__clip"
              :class="{
                'timeline__clip--active': selected === track.id,
                'timeline__clip--replace': replacing(track),
              }"
              @click="selectClip(track.id, $event)"
              @contextmenu="openMenu(track.id, $event)"
            >
              <i class="mdi timeline__clip-icon" :class="track.icon"></i>
              <span class="timeline__clip-text" :title="track.media.path">
                {{ replacing(track) ? t('editor-track-replace') : track.media.name }}
              </span>
            </div>
            <!-- 空轨道：虚线框 + 一句「把xx拖到这里」，说明这一条放什么 -->
            <div v-else class="timeline__clip timeline__clip--empty" @click="selected = null">
              <v-icon icon="mdi-plus" size="14"></v-icon>
              <span>{{ t(track.drop) }}</span>
            </div>
          </div>

          <!-- 播放头贯穿标尺与各条轨道 -->
          <div class="timeline__playhead" :style="{ left: percent }">
            <span class="timeline__playhead-grip"></span>
          </div>
        </div>
      </div>
    </div>

    <!-- 轨道对象的右键菜单：fixed 定位在鼠标那一点，点别处就收 -->
    <div
      v-if="menu"
      class="timeline__menu"
      :style="{ left: `${menu.x}px`, top: `${menu.y}px` }"
      @pointerdown.stop
    >
      <button class="timeline__menu-item" type="button" @click="removeFromMenu">
        <i class="mdi mdi-trash-can-outline"></i>
        <span>{{ t('editor-track-clear') }}</span>
      </button>
    </div>
  </footer>
</template>

<style scoped>
.timeline {
  --timeline-heads: 112px;
  /* 单条轨道（以及左边那列轨道名）的高度是固定的：窗口里把时间轴拖高时，
     多出来的是留白，不是把几条轨道抻开——以后加的新项目也按这个高度往上排。 */
  --timeline-lane: 40px;
  display: flex;
  flex-direction: column;
  flex: none;
  height: 214px;
  margin-top: var(--phi-space-3);
  overflow: hidden;
  background: var(--phi-bg-layer-1);
  border-top: 0.5px solid var(--phi-border-l2);
}

.timeline__bar {
  display: flex;
  flex: none;
  align-items: center;
  gap: var(--phi-space-1);
  height: 36px;
  padding: 0 var(--phi-space-3);
  border-bottom: 0.5px solid var(--phi-border-l1);
}

.timeline__time {
  font-size: 0.78rem;
  color: var(--phi-label-primary);
  font-variant-numeric: tabular-nums;
}

.timeline__sep,
.timeline__total {
  font-size: 0.74rem;
  color: var(--phi-label-caption);
  font-variant-numeric: tabular-nums;
}

/* 缩放拖动条：夹在两个放大镜左边的一条窄滑杆（Vuetify 自己带 8px 左右外边距） */
.timeline__zoom {
  flex: none;
  width: 112px;
}

.timeline__scrub {
  display: flex;
  flex: none;
  margin-top: 6px;
}

/* 和 `.timeline__heads` 同宽，让进度条与标尺、轨道从同一条竖线开始 */
.timeline__scrub-gutter {
  flex: none;
  width: var(--timeline-heads);
}

.timeline__progress {
  position: relative;
  flex: 1 1 auto;
  min-width: 0;
  height: 10px;
  cursor: pointer;
  background: var(--phi-bg-layer-3);
  border-radius: var(--phi-radius-pill);
}

.timeline__progress-fill {
  height: 100%;
  background: var(--phi-accent);
  border-radius: var(--phi-radius-pill);
}

.timeline__progress-thumb {
  position: absolute;
  top: 50%;
  width: 12px;
  height: 12px;
  margin-left: -6px;
  background: #fff;
  border-radius: 50%;
  transform: translateY(-50%);
  box-shadow: 0 0 0 2px var(--phi-accent);
}

.timeline__body {
  display: flex;
  flex: 1 1 auto;
  min-height: 0;
  overflow: hidden;
}

.timeline__heads {
  display: flex;
  flex: none;
  flex-direction: column;
  width: var(--timeline-heads);
  border-right: 0.5px solid var(--phi-border-l1);
}

/* 轨道名那列要和标尺错开，所以顶部留一块与标尺等高的空位 */
.timeline__head-spacer {
  flex: none;
  height: 24px;
}

.timeline__head {
  display: flex;
  flex: none;
  align-items: center;
  gap: 6px;
  height: var(--timeline-lane);
  padding: 0 var(--phi-space-2);
  font-size: 0.72rem;
  color: var(--phi-label-secondary);
  border-top: 0.5px solid var(--phi-border-l1);
}

.timeline__lanes {
  position: relative;
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  min-width: 0;
  /* 放大后画布比可视区宽，横向滚动；纵向永远不滚（轨道高度是定死的） */
  overflow-x: auto;
  overflow-y: hidden;
}

/* 横向滚动条压细一点：它一高就把轨道挤出视野 */
.timeline__lanes::-webkit-scrollbar {
  height: 6px;
}

.timeline__lanes::-webkit-scrollbar-thumb {
  background: var(--phi-border-l4);
  border-radius: var(--phi-radius-pill);
}

/* 画布：宽度 = 可视宽度 × 缩放倍数，标尺 / 轨道 / 播放头都挂在它上面 */
.timeline__canvas {
  position: relative;
  display: flex;
  flex: none;
  flex-direction: column;
  min-width: 100%;
}

.timeline__ruler {
  position: relative;
  flex: none;
  height: 24px;
  cursor: pointer;
  border-bottom: 0.5px solid var(--phi-border-l1);
}

.timeline__tick {
  position: absolute;
  top: 0;
  bottom: 0;
}

.timeline__tick-line {
  position: absolute;
  top: 14px;
  left: 0;
  width: 1px;
  height: 10px;
  background: var(--phi-border-l4);
}

.timeline__tick-label {
  position: absolute;
  top: 1px;
  left: 3px;
  font-size: 0.64rem;
  color: var(--phi-label-caption);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

/* 末尾那刻度贴着右边缘，标签改往左边排，不然会顶出容器 */
.timeline__tick--last .timeline__tick-label {
  left: auto;
  right: 3px;
}

.timeline__lane {
  position: relative;
  flex: none;
  height: var(--timeline-lane);
  /* 左右不留白：轨道块从 0% 铺到 100%，与标尺、播放头、进度条同一条轴 */
  padding: 2px 0;
  border-top: 0.5px solid var(--phi-border-l1);
}

.timeline__clip {
  display: flex;
  gap: 5px;
  align-items: center;
  height: 100%;
  overflow: hidden;
  padding: 0 var(--phi-space-2);
  font-size: 0.7rem;
  color: var(--phi-label-primary);
  cursor: pointer;
  background: #ffffff14;
  border: 0.5px solid var(--phi-border-l3);
  border-radius: var(--phi-radius-xs);
}

.timeline__clip-icon {
  flex: none;
  font-size: 13px;
  color: var(--phi-label-caption);
}

.timeline__clip--empty {
  gap: 4px;
  color: var(--phi-label-caption);
  cursor: default;
  background: none;
  border-style: dashed;
}

/* 拖素材经过时整条轨道亮起来 */
.timeline__lane--over .timeline__clip--empty {
  color: var(--phi-accent);
  background: color-mix(in srgb, var(--phi-accent) 10%, transparent);
  border-color: var(--phi-accent);
}

/* 已经有素材的轨道：拖动时整条亮一层，表示换掉它也行 */
.timeline__lane--over .timeline__clip {
  border-color: var(--phi-accent);
  box-shadow: inset 0 0 0 0.5px var(--phi-accent);
}

/* 轨道上已经有东西、又拖了一份过来：块上改说「松开替换」，别再以为是叠上去 */
.timeline__clip--replace .timeline__clip-text {
  color: var(--phi-accent);
  font-weight: 600;
}

.timeline__clip-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 选中的对象：一圈亮边，跟素材库里的高亮一致 */
.timeline__clip--active {
  border-color: var(--phi-accent);
  box-shadow: inset 0 0 0 0.5px var(--phi-accent);
}

/* 右键菜单 */
.timeline__menu {
  position: fixed;
  z-index: 40;
  min-width: 128px;
  padding: 4px;
  background: var(--phi-bg-layer-2);
  border: 0.5px solid var(--phi-border-l3);
  border-radius: var(--phi-radius-sm);
  box-shadow: 0 6px 18px rgb(0 0 0 / 35%);
}

.timeline__menu-item {
  display: flex;
  align-items: center;
  gap: var(--phi-space-1);
  width: 100%;
  padding: 5px 8px;
  font-family: inherit;
  font-size: 0.76rem;
  color: var(--phi-label-primary);
  text-align: left;
  cursor: pointer;
  background: transparent;
  border: none;
  border-radius: var(--phi-radius-xs);
}

.timeline__menu-item:hover {
  background: var(--phi-interactive-hover);
}

.timeline__playhead {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 1px;
  pointer-events: none;
  background: var(--phi-accent);
}

.timeline__playhead-grip {
  position: absolute;
  top: 0;
  left: -4px;
  width: 9px;
  height: 9px;
  background: var(--phi-accent);
  border-radius: 2px;
}
</style>
