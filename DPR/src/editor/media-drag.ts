import { ref } from 'vue';

import type { DraftMedia } from '@/model';

/**
 * 素材库 → 时间轴 / 文件夹的拖拽状态（跨组件共享）。
 *
 * 为什么不用 HTML5 的 `draggable`：窗口开着 `dragDropEnabled`（要接系统拖进来的文件），
 * WebView2 会把页面内的 HTML5 拖放一并吞掉 —— 真实鼠标拖拽时 `dragstart` / `drop`
 * 根本不会触发（用脚本派发合成事件倒是能过，所以别拿合成事件当验收）。这里改用
 * pointer 事件自己跟：素材卡按下记起点，移动超过几个像素才算开始拖，松手时用
 * `elementFromPoint` 找落点（轨道 / 文件夹 / 「全部素材」那行）。
 */
export const dragMedia = ref<DraftMedia | null>(null);

/**
 * 正在从效果页拖出来的效果 id（目前只有 `text-noise`）。
 *
 * 效果不是素材文件，落点也不是「哪条轨道」而是一个**时刻**：时间轴按指针位置
 * 换算出秒数，新加一条特效轨，片段就从那儿开始。
 */
export const dragEffect = ref<string | null>(null);

/** 指针位置（页面坐标），拖出来的小影子跟着它走。 */
export const dragPoint = ref({ x: 0, y: 0 });

/** 指针此刻压在哪个轨道上（`chart` / `music` / `cover`），没有就是 null。 */
export const dragLane = ref<string | null>(null);

/** 指针此刻是不是压在时间轴的画布上（拖效果卡 / 歌词时用，松手就在那儿落下）。 */
export const dragTimeline = ref(false);

/** 指针此刻压在哪个文件夹卡上，没有就是 null。 */
export const dragFolderId = ref<string | null>(null);

/** 指针是不是压在「全部素材」那一行（拖到这里 = 移出文件夹）。 */
export const dragRoot = ref(false);

/** 落点在不在某个元素里；用于松手时找轨道与文件夹。 */
export function dropAt(x: number, y: number): Element | null {
  return document.elementFromPoint(x, y);
}

/**
 * 按指针位置算出「现在压着什么」。
 *
 * 轨道只在自己收得下这项素材时才亮：谱面轨道只认谱面、音乐轨道只认音频、曲绘轨道只认图片，
 * 拖错了类型不会有落点提示（松手时也不放行），省得把一首歌放到谱面轨道上。
 * 歌词是例外：它铺出来的是特效片段，压在特效轨上就并进那条、压在别处就新铺一条，
 * 所以轨道亮不亮由时间轴自己按落点算，这里只负责亮画布。
 * 文件夹与「全部素材」那行跟类型无关，照常亮。
 */
export function updateDragTarget(x: number, y: number) {
  const hit = dropAt(x, y);
  const lane = hit?.closest('[data-track-kind]');
  const laneKind = lane?.getAttribute('data-track-kind') ?? null;
  dragLane.value = laneKind && laneKind === dragMedia.value?.kind ? laneKind : null;
  dragTimeline.value =
    (!!dragEffect.value || dragMedia.value?.kind === 'lyric') && !!hit?.closest('[data-timeline-drop]');
  const folder = hit?.closest('[data-folder-id]');
  dragFolderId.value = folder?.getAttribute('data-folder-id') ?? null;
  dragRoot.value = !folder && !!hit?.closest('[data-media-root]');
}

/** 拖拽收摊：状态清空，界面上的高亮一起撤掉。 */
export function clearDrag() {
  dragMedia.value = null;
  dragEffect.value = null;
  dragLane.value = null;
  dragTimeline.value = false;
  dragFolderId.value = null;
  dragRoot.value = false;
}
