/**
 * 悬浮长条的拖动与定位（渲染进度条、DPE 的已选谱面条共用一套）。
 *
 * 条是 `.phi-frame` 的子元素（`position: absolute`，定位父级是它），
 * 存的是「距框架底边 / 右边的距离」而不是屏幕坐标：窗口尺寸变了位置依然合理。
 *
 * 范围限制：两个方向都跟手拖，但整条必须留在右侧内容区 `.phi-center` 之内，
 * 四边各留 `MIN_EDGE`。条是 `.phi-center` 的**兄弟**（`App.vue` 里紧跟在 `</main>` 后面），
 * `.phi-center` 的 `overflow: hidden` 拦不住它，所以边界只能靠 `clamp()`
 * 拿 `.phi-frame` 与 `.phi-center` 两个 `getBoundingClientRect()` 一减自己算。
 *
 * 宽度是百分比（内联 `width`），位置与宽度都写 localStorage：宽度键由设置页 / 调用方决定。
 */
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';

import { useStorage } from '@vueuse/core';

/** 四边至少留这么多，条不会被标题栏 / 侧栏压住 */
const MIN_EDGE = 8;
/** 与 `.task-strip` 的 height 一致（量不到元素时的兜底） */
const STRIP_HEIGHT = 72;
/** 手势移动超过这么多像素才算拖动，否则当点击 */
const CLICK_SLOP = 3;

export interface FloatingStripOptions {
  /** 存「距底边距离」的 localStorage 键 */
  bottomKey: string;
  /** 存「距右边距离」的 localStorage 键 */
  rightKey: string;
  /** 存宽度的 localStorage 键（百分比） */
  widthKey: string;
  defaultBottom?: number;
  defaultRight?: number;
  defaultWidth?: number;
  minWidth?: number;
  maxWidth?: number;
  /** 条本身的高度，量不到元素时兜底 */
  height?: number;
  /** 没有拖动过的单击 */
  onClick?: () => void;
}

export function useFloatingStrip(options: FloatingStripOptions) {
  const defaultBottom = options.defaultBottom ?? 16;
  const defaultRight = options.defaultRight ?? 24;
  const minWidth = options.minWidth ?? 15;
  const maxWidth = options.maxWidth ?? 100;
  const defaultWidth = options.defaultWidth ?? 15;
  const stripHeight = options.height ?? STRIP_HEIGHT;

  const widthStorage = useStorage<number>(options.widthKey, defaultWidth);

  /** 夹到合法值域后的宽度，直接当内联 `width` 用，也供 `clamp()` 兜底估宽 */
  const widthPct = computed(() => {
    const value = Number(widthStorage.value);
    if (!Number.isFinite(value)) return defaultWidth;
    return Math.min(maxWidth, Math.max(minWidth, value));
  });

  function readOffset(key: string, fallback: number): number {
    const stored = Number(localStorage.getItem(key));
    // 只认正数：万一存进去过越界（负数）的位置，回落到默认值
    return Number.isFinite(stored) && stored > 0 ? stored : fallback;
  }

  const widgetEl = ref<HTMLElement | null>(null);
  const bottomOffset = ref(readOffset(options.bottomKey, defaultBottom));
  const rightOffset = ref(readOffset(options.rightKey, defaultRight));

  /** 定位父级：`App.vue` 的 `.phi-frame`（`position: relative`），偏移量以它为准 */
  function frameEl(): HTMLElement | null {
    return widgetEl.value?.closest<HTMLElement>('.phi-frame') ?? null;
  }

  function centerEl(): HTMLElement | null {
    return frameEl()?.querySelector<HTMLElement>('.phi-center') ?? null;
  }

  /** 把当前位置夹回右侧内容区：两个方向都拖得动，但整条不会跑出可视范围 */
  function clamp() {
    const outer = frameEl();
    const strip = widgetEl.value;
    if (!outer || !strip) return;
    const stripH = strip.offsetHeight || stripHeight;
    const stripW = strip.offsetWidth || (outer.clientWidth * widthPct.value) / 100;
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

  /** 本次手势是否真的拖动过：`CLICK_SLOP` 以内当误触，不算拖动、不写 localStorage */
  let dragged = false;
  let dragging = false;
  let dragStartY = 0;
  let dragStartX = 0;
  let dragStartBottom = defaultBottom;
  let dragStartRight = defaultRight;

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
    if (!dragged && Math.abs(dy) < CLICK_SLOP && Math.abs(dx) < CLICK_SLOP) return;
    dragged = true;
    // 上下左右都跟手，但夹在内容区内。往右拖 ⇒ 离右边更近 ⇒ 存的距离变小。
    bottomOffset.value = dragStartBottom + dy;
    rightOffset.value = dragStartRight - dx;
    clamp();
  }

  function onDragEnd() {
    if (!dragging) return;
    dragging = false;
    if (!dragged) return;
    // 松手前再夹一次：兜住「拖拽中途量不到内容区」之类没夹到的情况
    clamp();
    localStorage.setItem(options.bottomKey, String(Math.round(bottomOffset.value)));
    localStorage.setItem(options.rightKey, String(Math.round(rightOffset.value)));
  }

  /**
   * 单击（没有拖动）才回调。
   *
   * 刚拖完的那次 `click` 不算：`dragged` 要等下一次按下（`onDragStart`）才复位，
   * 所以拖完松手时它会挡住这里 —— 不用额外挂「吞掉下一个 click」的定时器。
   */
  function onClick() {
    if (dragged) return;
    options.onClick?.();
  }

  // 内容区尺寸变了（窗口缩放、左侧栏拖动调宽）、或者条刚出现（宽度要靠它自己量）时，
  // 都把位置夹回内容区。用 ResizeObserver 盯着 `.phi-frame` / `.phi-center` 本身，
  // 比只听 window.resize 准。
  let resizeObserver: ResizeObserver | null = null;
  onMounted(() => {
    void nextTick(() => {
      clamp();
      if (typeof ResizeObserver === 'undefined') return;
      resizeObserver = new ResizeObserver(() => clamp());
      for (const el of [frameEl(), centerEl()]) {
        if (el) resizeObserver.observe(el);
      }
    });
  });
  onUnmounted(() => {
    resizeObserver?.disconnect();
    resizeObserver = null;
  });
  // 宽度变了条会往左长，可能顶出内容区，跟着重新夹一次
  watch(widthPct, () => void nextTick(clamp));
  // 任何原因把位置写到界外（启动时读到的旧值、条变高变宽、窗口被拉小）都立刻夹回来。
  // Vue 对「写入同一个值」不再触发 watcher，所以夹完一轮就收敛，不会来回抖。
  watch([bottomOffset, rightOffset], () => clamp());

  return { widgetEl, bottomOffset, rightOffset, widthPct, clamp, onDragStart, onDragMove, onDragEnd, onClick };
}
