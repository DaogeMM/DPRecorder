<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';

import { useI18n } from 'vue-i18n';
const { t } = useI18n();

import type { DraftClip, DraftLane, DraftMedia, EditorDraft } from '@/model';
import { clearDrag, dragEffect, dragLane, dragMedia, dragTimeline } from './media-drag';
import { clock, clipRange, DEFAULT_LANE_LENGTH, formatTimecode, snapTime, timelineSpan } from './timeline';

/**
 * 编辑窗口底部：进度条 + 时间轴。
 *
 * 剪映底部是「工具栏 + 标尺 + 一条条轨道」：这里三条主轨道 —— 音乐 / 谱面 / 曲绘，
 * 对应草稿里的三条轨道；主轨道下面还能自己加轨道（`draft.lanes`），那些轨道的片段长度自己定，
 * 可以比音乐还长。素材从左边功能栏的素材库拖进来，落在哪条轨道上就放到哪条轨道里。
 *
 * 选中的是轨道的 id（主轨道是 `music` / `chart` / `cover`，自己加的是 `lane.id`），
 * 选中状态放在父级，右边的编辑栏跟着它换内容。
 */

/** 没认出来的素材种类（自己加的轨道上可以放任何素材）。 */
const LANE_ICON = 'mdi-layers-outline';


const props = defineProps<{
  draft: EditorDraft | null;
  playing: boolean;
  time: number;
  /** 音乐时长（秒）；时间轴总长还会算上自己加的轨道里的片段。 */
  duration: number;
  /** 选中的轨道 id：由父级拿着（右边的编辑栏也要看它），这里只负责画高亮。 */
  selected: string | null;
  /** 特效轨上选中的那一段（一行可以放好几段，其余轨道恒为空）。 */
  selectedClip?: string | null;
  /** 批量选中的特效片段：Ctrl 点片段会加进来，右栏据此显示「多个值」。 */
  multi?: { laneId: string; clipId: string }[];
}>();

const emit = defineEmits<{
  'toggle-play': [];
  seek: [seconds: number];
  assign: [media: DraftMedia];
  clear: [track: string];
  /** `additive` 为真表示按住 Ctrl / Cmd 点的，加进批量选择而不是替换。 */
  select: [track: string | null, clip?: string | null, additive?: boolean];
  /** 框选结束：矩形碰到的所有特效片段（可能为空，表示点在空白处）。 */
  'select-many': [items: { laneId: string; clipId: string }[]];
  'remove-lane': [track: string];
  /** 效果卡拖到时间轴上松手：按那一刻、按落点的上下位置新加一条特效轨。 */
  'drop-effect': [seconds: number, before: string | null];
  /** 歌词素材拖到时间轴上松手：整份文件按它自己的时间铺成一条特效轨，指针只决定插在哪条轨道前面。 */
  'drop-lyrics': [media: DraftMedia, lane: string | null, before: string | null];
  /** 拖片段（改起点）或拉两边（改长度）松手：把新的起止时间交给父级存盘。 */
  'resize-lane': [id: string, start: number, length: number];
  /** 特效片段：改起止。 */
  'resize-clip': [laneId: string, clipId: string, start: number, length: number];
  /** 特效片段拖到另一条特效轨上（换层级）。 */
  'move-clip': [laneId: string, clipId: string, toLaneId: string, start: number];
  /** 删掉特效片段（右栏的删除、右键菜单、Delete 键都走它）。 */
  'remove-clip': [laneId: string, clipId: string];
  /** 在特效轨的空白处双击：按那一刻再加一段。 */
  'add-clip': [laneId: string, seconds: number];
}>();

/**
 * 时间轴总长：音乐长度与自己加的轨道里的片段末端取最大值，至少留一段最小长度。
 * 有了这条，「轨道比音乐长」才画得出来 —— 标尺、播放头、片段宽度都按它换算。
 */
const span = computed(() => timelineSpan(props.duration, props.draft?.lanes));

/** 播放头 / 进度条填充的百分比（时长按 0 处理也要给出合法值）。 */
const percent = computed(() => `${Math.min(100, Math.max(0, (props.time / span.value) * 100))}%`);


/** 播放头 / 总时长的读数：带毫秒，拖到 0.1 秒也能看出变化。 */
const timeText = computed(() => formatTimecode(props.time));
const durationText = computed(() => formatTimecode(span.value));

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
/** 左侧轨道名那一列的内容层：轨道区纵向滚动时靠它平移对齐。 */
const headsInner = ref<HTMLElement | null>(null);
/** 里面那条按缩放倍数撑宽的画布：拖片段、拖效果卡换算秒数都按它的实际宽度算。 */
const canvas = ref<HTMLElement | null>(null);
const viewportWidth = ref(0);

/** 标尺刻度的秒数档位：挑第一个能让相邻刻度拉开约 80px 的。 */
const TICK_STEPS = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800];

/**
 * 标尺刻度：按当前缩放与可视宽度挑一个「好看的」秒数，让相邻标签不会被挤在一起；
 * 末尾补一个整段终点的刻度。
 */
const ticks = computed(() => {
  const total = span.value;
  if (total <= 0) return [];
  const pxPerSecond = (zoom.value * Math.max(viewportWidth.value, 1)) / total;
  const step = TICK_STEPS.find((it) => it * pxPerSecond >= 80) ?? 1800;

  const out: { left: string; label: string; last: boolean }[] = [];
  for (let second = 0; second <= total; second += step) {
    out.push({
      left: `${(second / total) * 100}%`,
      label: clock(second),
      last: false,
    });
  }
  const last = out[out.length - 1];
  if (last && last.left !== '100%') {
    out.push({ left: '100%', label: clock(total), last: true });
  } else if (last) {
    last.last = true;
  }
  return out;
});

/* ---- 拖动播放头：吸附 + 时间气泡 ----
 *
 * 标尺与进度条按住就能拖着走，而不是只能单击。拖动期间把秒数吸到 0.1 秒，
 * 指针旁边浮一个带毫秒的读数 —— 不然高倍缩放下根本分不清拖到了 12.3 还是 12.4。
 */
const scrub = ref<{ left: number; top: number; time: number } | null>(null);
let scrubRect: DOMRect | null = null;

/** 按指针 x 算时间、发 seek，并把气泡摆到指针上方（坐标换算成轨道区内的绝对位置）。 */
function scrubTo(clientX: number) {
  const rect = scrubRect;
  if (!rect || rect.width <= 0) return;
  const ratio = Math.min(Math.max((clientX - rect.left) / rect.width, 0), 1);
  const seconds = snapTime(ratio * span.value);
  emit('seek', seconds);

  const host = lanes.value;
  if (!host) return;
  const hostRect = host.getBoundingClientRect();
  scrub.value = {
    left: clientX - hostRect.left + host.scrollLeft,
    top: Math.max(0, rect.top - hostRect.top + host.scrollTop - 26),
    time: seconds,
  };
}

function startScrub(event: PointerEvent) {
  if (event.button !== 0) return;
  const el = event.currentTarget as HTMLElement;
  const rect = el.getBoundingClientRect();
  if (rect.width <= 0) return;
  scrubRect = rect;
  scrubTo(event.clientX);
  window.addEventListener('pointermove', onScrubMove);
  window.addEventListener('pointerup', finishScrub);
  window.addEventListener('pointercancel', finishScrub);
  event.preventDefault();
}

function onScrubMove(event: PointerEvent) {
  scrubTo(event.clientX);
  event.preventDefault();
}

/** 松手：气泡收起来，监听解绑。 */
function finishScrub() {
  scrub.value = null;
  scrubRect = null;
  window.removeEventListener('pointermove', onScrubMove);
  window.removeEventListener('pointerup', finishScrub);
  window.removeEventListener('pointercancel', finishScrub);
}

onBeforeUnmount(finishScrub);

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

/** 工具条与 Ctrl+= / Ctrl+- 共用：各走一档。 */
function zoomIn() {
  void setZoom(zoomIndex.value + 1);
}

function zoomOut() {
  void setZoom(zoomIndex.value - 1);
}

/** 轨道区纵向滚动时，左侧轨道名跟着平移：两列共用一个 scrollTop，否则轨道一多就错位。 */
function syncHeadsScroll() {
  const inner = headsInner.value;
  if (!inner) return;
  inner.style.transform = `translateY(${-(lanes.value?.scrollTop ?? 0)}px)`;
}

/** 播放头跑出可视区（或被缩放挤出画面）就把它拉回来，滚到正中。 */
function revealPlayhead() {
  const el = lanes.value;
  if (!el || span.value <= 0 || el.clientWidth <= 0) return;

  const x = (props.time / span.value) * zoom.value * el.clientWidth;
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
 * 选中状态本身存在父级（`props.selected`），这里只发 `select` —— 右边的编辑栏跟着它换内容。
 */
const menu = ref<{ track: string; clip: string | null; x: number; y: number } | null>(null);

function selectClip(track: string, clip: string | null = null, event?: MouseEvent) {
  // 刚拖完 / 刚框选完：这一次 click 是收尾自带的，不能拿它改选中（会把多选冲掉）
  if (performance.now() - clipDragEndedAt < 250 || performance.now() - marqueeEndedAt < 250) return;
  emit('select', track, clip, !!event && (event.ctrlKey || event.metaKey));
}

function clipActive(trackId: string, clipId: string | null): boolean {
  if (!clipId) return props.selected === trackId && !props.selectedClip;
  if (props.multi && props.multi.length > 1) {
    return props.multi.some((it) => it.laneId === trackId && it.clipId === clipId);
  }
  return props.selected === trackId && props.selectedClip === clipId;
}

function openMenu(track: string, event: MouseEvent, clip: string | null = null) {
  event.preventDefault();
  emit('select', track, clip);
  // 菜单用 fixed 定位：时间轴里横向滚动，跟着内容走会被裁掉
  menu.value = { track, clip, x: event.clientX, y: event.clientY };
}

function closeMenu() {
  menu.value = null;
}

/** 右键 / Delete 要删的对象：特效片段删片段，其它轨道把素材从轨道上拿下来。 */
function removeSelected() {
  const track = props.selected;
  if (!track) return;
  if (props.selectedClip) {
    emit('remove-clip', track, props.selectedClip);
    emit('select', null, null);
    return;
  }
  emit('clear', track);
  emit('select', null, null);
}

function removeFromMenu() {
  const { track, clip } = menu.value ?? { track: null, clip: null };
  closeMenu();
  if (!track) return;
  emit('select', null, null);
  if (clip) emit('remove-clip', track, clip);
  else emit('clear', track);
}

/** Delete / Backspace 删掉选中的对象；正在输入框里打字时不动它。 */
function onKeydown(event: KeyboardEvent) {
  if (event.key !== 'Delete' && event.key !== 'Backspace') return;
  const target = event.target as HTMLElement | null;
  if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)) {
    return;
  }
  if (!props.selected) return;
  event.preventDefault();
  removeSelected();
}

// 换草稿时把右键菜单收掉；选中由父级在换草稿时清
watch(() => props.draft?.id, () => {
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

/** 时间轴上一行轨道要画的东西：主轨道与「自己加的轨道」在这里拼成同一个形状。 */
interface TrackRow {
  id: string;
  icon: string;
  /** 轨道名（已翻译好）：主轨道用文案，自己加的用它的名字 / 「轨道 n」。 */
  title: string;
  /** 空轨道里那句「把xx拖到这里」。 */
  drop: string;
  media: DraftMedia | null;
  /** 自己加的轨道才有：片段另有起点与长度。 */
  lane?: DraftLane;
  /** 文字轨上已经写好的文字（这条轨道没有素材文件）。 */
  text?: string;
}

/** 这条轨道上已经有东西可画：有素材，文字轨上写了字，或者特效轨上摆着效果。 */
function hasClip(track: TrackRow) {
  return !!track.media || !!track.text;
}

/** 自己加的轨道上放什么，图标就用那种东西的（没放就画一个通用轨道图标）。 */
function laneIcon(lane: DraftLane) {
  if (lane.kind === 'text') return 'mdi-format-text';
  if (lane.kind === 'effect') return 'mdi-auto-fix';
  if (lane.kind === 'audio') return 'mdi-music-note';
  return lane.media?.kind === 'cover' ? 'mdi-image-outline' : LANE_ICON;
}

/** 自己加的轨道空着时那句提示：往上加的收图片 / 文字 / 特效，往下加的收音频。 */
function laneDrop(lane: DraftLane) {
  if (lane.kind === 'text') return t('editor-drop-lane-text');
  if (lane.kind === 'effect') return t('editor-drop-lane-effect');
  if (lane.kind === 'audio') return t('editor-drop-lane-audio');
  return t('editor-drop-lane-image');
}

/**
 * 轨道表：主轨道上下都能再排自己加的轨道。
 *
 * 三种主轨道按「谱面 → 曲绘 → 音乐」排；往上加的（图片 / 文字）排在主轨道上面，
 * 往下加的（音频）排在主轨道下面。主轨道的长度由音乐定、不能增删，
 * 自己加的轨道片段长度自己定（能比音乐长）。
 */
const tracks = computed<TrackRow[]>(() => {
  const lanes = props.draft?.lanes ?? [];
  const rows: TrackRow[] = [];
  const laneRow = (lane: DraftLane, index: number): TrackRow => ({
    id: lane.id,
    icon: laneIcon(lane),
    title: lane.name.trim() || t('editor-lane-title', { n: index + 1 }),
    drop: laneDrop(lane),
    media: lane.media,
    lane,
    text:
      lane.kind === 'text'
        ? lane.text.trim()
        : lane.kind === 'effect'
          ? lane.effect?.text.trim() || t('editor-track-effect')
          : '',
  });
  lanes.forEach((lane, index) => {
    if (lane.kind !== 'audio') rows.push(laneRow(lane, index));
  });
  rows.push(
    {
      id: 'chart',
      icon: 'mdi-file-document-outline',
      title: t('editor-track-chart'),
      drop: t('editor-drop-chart'),
      media: props.draft?.chart ?? null,
    },
    {
      id: 'cover',
      icon: 'mdi-image-outline',
      title: t('editor-track-cover'),
      drop: t('editor-drop-cover'),
      media: props.draft?.cover ?? null,
    },
    {
      id: 'music',
      icon: 'mdi-music-note',
      title: t('editor-track-music'),
      drop: t('editor-drop-music'),
      media: props.draft?.music ?? null,
    },
  );
  lanes.forEach((lane, index) => {
    if (lane.kind === 'audio') rows.push(laneRow(lane, index));
  });
  return rows;
});

/* ---- 拖片段 / 拉两边 ----
 *
 * 自己加的轨道上的片段可以整块拖（改起点）也能拉左右两边（改长度）：
 * 按下时记下指针位置与画布宽度，拖动期间只改一个本地预览值，松手才发
 * `resize-lane` —— 每动一像素就写盘、重排时间轴会跟手变卡。
 * 起止时间都夹在 0…总长之间；片子最短 `MIN_CLIP` 秒，别拉成看不见的线。
 *
 * 特效轨上一行可以放好几段：每段单独拖，平移时还能拖到别的特效轨上（换层级）。
 */
type ClipMode = 'move' | 'left' | 'right';

interface ClipDrag {
  /** 源轨道（特效轨就是它自己的 id）。 */
  laneId: string;
  /** 被拖的特效片段；非特效轨道为 null。 */
  clipId: string | null;
  mode: ClipMode;
  /** 按下时的指针 x、画布宽度与总长：拖动期间就按这三样把像素换成秒。 */
  x: number;
  width: number;
  total: number;
  start0: number;
  length0: number;
  /** 拖动中的预览值（片段样式直接用它们）。 */
  start: number;
  length: number;
  /** 平移时指针压着的特效轨（null = 没压在任何特效轨上）。 */
  over: string | null;
}

const clipDrag = ref<ClipDrag | null>(null);

/**
 * 这一下拖动有没有真的挪过位置。
 *
 * 松手后浏览器还会补一个 `click`：如果它在拖动之后又把选中切成单段，
 * Ctrl 多选就会在拖完一段后散掉 —— 所以拖动过的这一段要在 `selectClip` 里跳过。
 */
let clipMoved = false;
/** 上一次拖动结束的时刻（毫秒），用来忽略紧跟其后的 click。 */
let clipDragEndedAt = 0;

/* ---- 框选 ----
 *
 * 在轨道的空白处按下并拖出一个矩形，松手时把矩形碰到的特效片段都选上。
 * 片段自己的 pointerdown 会 stopPropagation，所以从片段上按下去仍然是拖片段，
 * 不会误起框选。
 */
interface Marquee {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
  /** 挪过 3px 才算框选，否则当成一次普通点击。 */
  active: boolean;
}

const marquee = ref<Marquee | null>(null);
/** 起框选时 `.timeline__lanes` 的位置：滚动容器里的绝对定位要减去它。 */
let marqueeHost = { left: 0, top: 0, scrollLeft: 0, scrollTop: 0 };
/** 框选刚结束的时刻：忽略它带出来的那次 click，别让选中立刻被改回单段。 */
let marqueeEndedAt = 0;

const marqueeStyle = computed(() => {
  const box = marquee.value;
  if (!box || !box.active) return {};
  const left = Math.min(box.x0, box.x1) + marqueeHost.scrollLeft - marqueeHost.left;
  const top = Math.min(box.y0, box.y1) + marqueeHost.scrollTop - marqueeHost.top;
  return {
    left: `${left}px`,
    top: `${top}px`,
    width: `${Math.abs(box.x1 - box.x0)}px`,
    height: `${Math.abs(box.y1 - box.y0)}px`,
  };
});

function startMarquee(event: PointerEvent) {
  if (event.button !== 0 || dragEffect.value) return;
  const target = event.target as HTMLElement | null;
  // 片段 / 把手 / 标尺 / 右侧删除按钮上按下都不是框选
  if (target?.closest('.timeline__clip, .timeline__grip, .timeline__ruler, .timeline__head-remove')) return;
  const host = lanes.value;
  if (!host) return;
  const rect = host.getBoundingClientRect();
  marqueeHost = { left: rect.left, top: rect.top, scrollLeft: host.scrollLeft, scrollTop: host.scrollTop };
  marquee.value = { x0: event.clientX, y0: event.clientY, x1: event.clientX, y1: event.clientY, active: false };
  window.addEventListener('pointermove', onMarqueeMove);
  window.addEventListener('pointerup', onMarqueeUp);
}

function onMarqueeMove(event: PointerEvent) {
  const box = marquee.value;
  if (!box) return;
  box.x1 = event.clientX;
  box.y1 = event.clientY;
  if (Math.abs(box.x1 - box.x0) > 3 || Math.abs(box.y1 - box.y0) > 3) box.active = true;
}

function onMarqueeUp() {
  const box = marquee.value;
  finishMarquee();
  if (!box || !box.active) return;
  const host = lanes.value;
  if (!host) return;
  const left = Math.min(box.x0, box.x1);
  const right = Math.max(box.x0, box.x1);
  const top = Math.min(box.y0, box.y1);
  const bottom = Math.max(box.y0, box.y1);
  const picked: { laneId: string; clipId: string }[] = [];
  for (const el of host.querySelectorAll<HTMLElement>('.timeline__clip[data-clip-id]')) {
    const rect = el.getBoundingClientRect();
    if (rect.right < left || rect.left > right || rect.bottom < top || rect.top > bottom) continue;
    const laneId = el.dataset.laneId;
    const clipId = el.dataset.clipId;
    if (laneId && clipId) picked.push({ laneId, clipId });
  }
  marqueeEndedAt = performance.now();
  emit('select-many', picked);
}

function finishMarquee() {
  marquee.value = null;
  window.removeEventListener('pointermove', onMarqueeMove);
  window.removeEventListener('pointerup', onMarqueeUp);
}

onBeforeUnmount(finishMarquee);

/** 片段最短多长（秒）。 */
const MIN_CLIP = 0.1;

/** 磁吸的吸附距离（像素）：片段边缘挨到别的边缘或播放头这么近就贴上去。 */
const SNAP_PX = 4;

function clampNum(value: number, low: number, high: number) {
  return Math.min(Math.max(value, low), high);
}

/** 磁吸候选：0、总长、播放头，以及其它片段的起点与终点。 */
function snapTargets(total: number, exclude: string | null): number[] {
  const targets = [0, total];
  if (props.time > 0 && props.time < total) targets.push(props.time);
  for (const lane of props.draft?.lanes ?? []) {
    if (lane.kind !== 'effect') continue;
    const clips = lane.clips && lane.clips.length > 0 ? lane.clips : [];
    for (const clip of clips) {
      if (clip.id === exclude) continue;
      targets.push(clip.start, clip.start + clip.length);
    }
    // 老草稿只有轨道自己的起止，没有片段列表
    if (clips.length === 0 && lane.effect) targets.push(lane.start, lane.start + lane.length);
  }
  return targets;
}

/** 返回离 `value` 最近、且落在 `threshold` 以内的候选；没有就原样返回。 */
function snapEdge(value: number, targets: number[], threshold: number): number {
  let best = value;
  let distance = threshold;
  for (const target of targets) {
    const gap = Math.abs(target - value);
    if (gap < distance) {
      distance = gap;
      best = target;
    }
  }
  return best;
}

/** 起止时间保留到 5 位小数：再细的差别在时间轴上也拖不出来。 */
function roundTime(value: number) {
  return Math.round(value * 1e5) / 1e5;
}

/** 一条特效轨上的片段（老草稿的单片段也补成一段）。 */
function clipsIn(track: TrackRow): DraftClip[] {
  const lane = track.lane;
  if (!lane || lane.kind !== 'effect') return [];
  if (lane.clips && lane.clips.length > 0) return lane.clips;
  if (!lane.effect) return [];
  return [{ id: `${lane.id}-clip`, start: lane.start, length: lane.length, effect: lane.effect }];
}

/** 片段的起止（夹在 0…总长里，短得看不见时给一段默认长度）。 */
function clipRangeOf(clip: DraftClip, total: number) {
  const start = clampNum(clip.start, 0, Math.max(0, total));
  const length = clampNum(clip.length, 0, Math.max(0, total - start));
  return { start, length: length > 0 ? length : Math.min(DEFAULT_LANE_LENGTH, Math.max(0, total - start)) };
}

function startClipDrag(event: PointerEvent, track: TrackRow, mode: ClipMode, clip?: DraftClip) {
  if (!track.lane || event.button !== 0) return;
  const el = canvas.value;
  if (!el) return;
  const rect = el.getBoundingClientRect();
  if (rect.width <= 0) return;
  const total = span.value > 0 ? span.value : 1;
  const range = clip ? clipRangeOf(clip, total) : clipRange(track.lane, total);
  clipDrag.value = {
    laneId: track.lane.id,
    clipId: clip?.id ?? null,
    mode,
    x: event.clientX,
    width: rect.width,
    total,
    start0: range.start,
    length0: range.length,
    start: range.start,
    length: range.length,
    over: null,
  };
  clipMoved = false;
  window.addEventListener('pointermove', onClipMove);
  window.addEventListener('pointerup', onClipUp);
  window.addEventListener('pointercancel', cancelClipDrag);
  event.preventDefault();
  event.stopPropagation();
}

/** 指针此刻压着哪条特效轨（跨行拖动换层用）。 */
function effectLaneAt(clientX: number, clientY: number): string | null {
  const hit = document.elementFromPoint(clientX, clientY) as HTMLElement | null;
  const id = hit?.closest<HTMLElement>('[data-track-kind]')?.dataset.trackKind ?? null;
  if (!id) return null;
  const lane = props.draft?.lanes.find((it) => it.id === id);
  return lane?.kind === 'effect' ? id : null;
}

function onClipMove(event: PointerEvent) {
  const drag = clipDrag.value;
  if (!drag) return;
  const dx = ((event.clientX - drag.x) / drag.width) * drag.total;
  if (dx !== 0) clipMoved = true;
  // 磁吸优先于 0.1 秒网格：挨上别的片段边缘 / 播放头时对齐到那条边上
  const targets = snapTargets(drag.total, drag.clipId);
  const threshold = (SNAP_PX / Math.max(1, drag.width)) * drag.total;
  if (drag.mode === 'move') {
    const max = Math.max(0, drag.total - drag.length0);
    const raw = clampNum(drag.start0 + dx, 0, max);
    // 头、尾哪个更贴近候选就按哪个吸，整段跟着走
    const head = snapEdge(raw, targets, threshold);
    const tail = snapEdge(raw + drag.length0, targets, threshold) - drag.length0;
    const picked = Math.abs(head - raw) <= Math.abs(tail - raw) ? head : tail;
    drag.start = picked !== raw ? clampNum(picked, 0, max) : snapTime(raw);
    drag.length = drag.length0;
    drag.over = drag.clipId ? effectLaneAt(event.clientX, event.clientY) : null;
  } else if (drag.mode === 'left') {
    const max = drag.start0 + drag.length0 - MIN_CLIP;
    const raw = clampNum(drag.start0 + dx, 0, max);
    const picked = snapEdge(raw, targets, threshold);
    drag.start = picked !== raw ? picked : snapTime(raw);
    drag.length = drag.start0 + drag.length0 - drag.start;
  } else {
    const raw = clampNum(drag.length0 + dx, MIN_CLIP, drag.total - drag.start0);
    const end = drag.start0 + raw;
    const picked = snapEdge(end, targets, threshold);
    drag.length = picked !== end ? Math.max(MIN_CLIP, picked - drag.start0) : snapTime(raw);
    drag.start = drag.start0;
  }
  event.preventDefault();
}

function onClipUp() {
  const drag = clipDrag.value;
  const moved = clipMoved;
  finishClipDrag();
  if (!drag) return;
  if (moved) clipDragEndedAt = performance.now();
  const start = roundTime(drag.start);
  const length = roundTime(drag.length);
  const changed = Math.abs(start - drag.start0) > 1e-5 || Math.abs(length - drag.length0) > 1e-5;
  // 平移到别的特效轨：换层级比改起止更要紧，先处理
  if (drag.clipId && drag.over && drag.over !== drag.laneId) {
    emit('move-clip', drag.laneId, drag.clipId, drag.over, start);
    return;
  }
  if (!changed) return;
  if (drag.clipId) {
    // 拖动这一下本身不算「点选」：只有它原本不在选中范围里时才补一次选中
    const known =
      (props.multi ?? []).some((it) => it.laneId === drag.laneId && it.clipId === drag.clipId) ||
      (props.selected === drag.laneId && (props.selectedClip === drag.clipId || !props.selectedClip));
    emit('resize-clip', drag.laneId, drag.clipId, start, length);
    if (!known) emit('select', drag.laneId, drag.clipId);
    return;
  }
  emit('resize-lane', drag.laneId, start, length);
}

function cancelClipDrag() {
  finishClipDrag();
}

/** 收摊：清预览值、解绑临时监听（松手、取消、组件卸载都走这里）。 */
function finishClipDrag() {
  clipDrag.value = null;
  window.removeEventListener('pointermove', onClipMove);
  window.removeEventListener('pointerup', onClipUp);
  window.removeEventListener('pointercancel', cancelClipDrag);
}

onBeforeUnmount(finishClipDrag);

/* ---- 效果卡落到时间轴 ----
 *
 * 效果卡从左边效果页拖出来（见 `media-drag.ts` 的 `dragEffect`），落到时间轴上时
 * 指针那一刻就是新特效轨的起点：这里把指针 x 换算成秒，松手交给父级加轨道。
 * 拖动期间在画布上画一条竖线提示落点，免得松手才发现位置不对。
 */
const dropTime = ref<number | null>(null);

/** 拖歌词时指针压着的那条特效轨：只用来点亮轨道行，落点等松手时再算一次。 */
const lyricTarget = ref<string | null>(null);

/** 指针 x 换算成时间轴上的秒数（按画布实际宽度算，横向滚动不会偏）。 */
function timeAt(clientX: number): number {
  const el = canvas.value;
  if (!el) return 0;
  const rect = el.getBoundingClientRect();
  if (rect.width <= 0) return 0;
  return snapTime(clampNum((clientX - rect.left) / rect.width, 0, 1) * span.value);
}

function onLanesPointerMove(event: PointerEvent) {
  // 拖歌词：压在特效轨上就点亮那条（并进去），压在别处什么都不亮（新铺一条）
  if (dragMedia.value?.kind === 'lyric') {
    lyricTarget.value = effectLaneAt(event.clientX, event.clientY);
  }
  if (!dragEffect.value) return;
  dropTime.value = timeAt(event.clientX);
}

function onLanesPointerLeave() {
  lyricTarget.value = null;
  dropTime.value = null;
}

/**
 * 落点在两条轨道之间时，插到下面那条轨道前面去。
 *
 * 取每条轨道的竖直中线：指针在中线以上就算落在它上方（插到它前面），一条都没命中
 * 就插在最后（自加轨道里最靠下）。主轨道（谱面 / 曲绘 / 音乐）固定排在中间，
 * 返回值是主轨道 id 时由父级把新轨道塞到自加轨道那一侧的末尾。
 */
function insertBeforeAt(clientY: number): string | null {
  const host = lanes.value;
  if (!host) return null;
  const rows = Array.from(host.querySelectorAll<HTMLElement>('.timeline__lane'));
  for (const row of rows) {
    const rect = row.getBoundingClientRect();
    if (clientY < rect.top + rect.height / 2) return row.dataset.trackKind ?? null;
  }
  return null;
}

/* ---- 素材拖到时间轴 ----
 *
 * 效果卡落在「指针那一刻」：新建一条特效轨，起点是指针位置（上面那条规则画落点竖线）。
 *
 * 歌词素材落的是**片段**，时间全在文件里，指针只决定落到哪儿：
 * 压在一条特效轨上 = 并进那条轨（追加片段），压在别的轨道 / 空白处 = 按 `insertBeforeAt`
 * 新铺一条。拖动期间不画落点竖线 —— 落点不是一个时刻，压在特效轨上时点亮那条轨就够了。
 */

/** 效果卡 / 歌词在这块画布上松手。 */
function onLanesPointerUp(event: PointerEvent) {
  if (dragMedia.value?.kind === 'lyric' && dragTimeline.value) {
    const media = dragMedia.value;
    const lane = effectLaneAt(event.clientX, event.clientY);
    const before = lane ? null : insertBeforeAt(event.clientY);
    lyricTarget.value = null;
    clearDrag();
    emit('drop-lyrics', media, lane, before);
    return;
  }
  if (!dragEffect.value) return;
  const seconds = timeAt(event.clientX);
  const before = insertBeforeAt(event.clientY);
  dropTime.value = null;
  clearDrag();
  emit('drop-effect', seconds, before);
}

watch(dragEffect, (value) => {
  if (!value) dropTime.value = null;
});

watch(dragMedia, (value) => {
  if (!value) lyricTarget.value = null;
});

/** 标尺、进度条与片段共用这条总长；主轨道铺满它，自己加的轨道按起止时间摆。
 * 还没设长度的空轨道给一段默认宽度，不然那块东西在画布上根本看不见。
 * 正在拖的片段画的是预览值，松手后才变成真正存下来的 start / length。 */
function clipStyle(track: TrackRow) {
  if (!track.lane) return {};
  const total = span.value > 0 ? span.value : 1;
  const drag = clipDrag.value?.laneId === track.lane.id && !clipDrag.value.clipId ? clipDrag.value : null;
  const { start, length } = drag ?? clipRange(track.lane, total);
  return { left: `${(start / total) * 100}%`, width: `${(length / total) * 100}%` };
}

/** 特效片段的样式：跟它自己那一段的起止走（拖动中画预览值）。 */
function clipStyleOf(clip: DraftClip) {
  const total = span.value > 0 ? span.value : 1;
  const drag = clipDrag.value?.clipId === clip.id ? clipDrag.value : null;
  const { start, length } = drag ?? clipRangeOf(clip, total);
  return { left: `${(start / total) * 100}%`, width: `${(length / total) * 100}%` };
}

/** 这条特效轨是不是正被拖过去的片段压着（换层时高亮一下）。 */
function dropTarget(track: TrackRow) {
  return !!clipDrag.value?.clipId && clipDrag.value.over === track.id && clipDrag.value.over !== clipDrag.value.laneId;
}

/** 特效轨空白处双击：在那一刻再加一段（同一行可以放好几段）。 */
function addClipAt(track: TrackRow, event: MouseEvent) {
  if (track.lane?.kind !== 'effect') return;
  emit('add-clip', track.lane.id, timeAt(event.clientX));
}

/* 素材是 pointer 拖进来的（见 `media-drag.ts`）：这里只留落点属性 `data-track-track`，
   指针压在哪条轨道上就点亮哪条（`dragLane` 由指针位置算，不按素材类型）。 */

/**
 * 指针正压着这条轨道、而这条轨道上已经有别的东西 —— 松手就是替换。
 * 用来把块上的文件名换成「松开替换」，免得以为又叠了一层。
 */
function replacing(track: TrackRow) {
  return (
    dragLane.value === track.id &&
    !!track.media &&
    !!dragMedia.value &&
    dragMedia.value.path !== track.media.path
  );
}

/** 键盘热键（Ctrl+= / Ctrl+-）从窗口那边调过来，和工具条的放大镜走同一套档位。 */
defineExpose({ zoomIn, zoomOut });
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

      <!-- 新建轨道不再走按钮：特效卡从左边拖到轨道之间就会在那里插一条；素材直接拖到轨道上。 -->

      <!-- 缩放拖动条：直接拖到想要的倍率（档位和两个放大镜共用一套），左边标出当前倍数 -->
      <span class="timeline__zoom-value">{{ Math.round(zoom * 100) }}%</span>
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
        :aria-valuemax="Math.round(span)"
        tabindex="0"
        @pointerdown="startScrub"
      >
        <div class="timeline__progress-fill" :style="{ width: percent }"></div>
        <div class="timeline__progress-thumb" :style="{ left: percent }"></div>
      </div>
    </div>

    <div class="timeline__body">
      <!-- 轨道名固定一列；轨道多到装不下时由右边轨道区纵向滚动，这一列跟着平移 -->
      <div ref="heads" class="timeline__heads">
        <div ref="headsInner" class="timeline__heads-inner">
          <div class="timeline__head-spacer"></div>
          <div v-for="track in tracks" :key="track.id" class="timeline__head">
            <v-icon :icon="track.icon" size="14"></v-icon>
            <span class="timeline__head-name">{{ track.title }}</span>
            <!-- 自己加的轨道可以整条删掉（主轨道不能） -->
            <button
              v-if="track.lane"
              class="timeline__head-remove"
              type="button"
              :title="t('editor-lane-remove')"
              @click.stop="emit('remove-lane', track.id)"
            >
              <i class="mdi mdi-close"></i>
            </button>
          </div>
        </div>
      </div>

      <!-- 轨道区是横向滚动容器，里面这条「画布」按缩放倍数撑宽：
           标尺、轨道块、播放头都挂在画布上，所以它们永远共用同一条时间轴。
           效果卡拖到这里松手就落在指针那一刻（`data-timeline-drop` 是落点标记）；
           歌词素材拖到这里松手就铺成一条新轨道，指针只决定插在哪条轨道前面 -->
      <div
        ref="lanes"
        class="timeline__lanes"
        :class="{ 'timeline__lanes--drop': dragEffect || dragMedia?.kind === 'lyric' }"
        data-timeline-drop
        @pointerdown="startMarquee"
        @pointermove="onLanesPointerMove"
        @pointerleave="onLanesPointerLeave"
        @pointerup="onLanesPointerUp"
        @scroll="syncHeadsScroll"
      >
        <div v-if="marquee && marquee.active" class="timeline__marquee" :style="marqueeStyle"></div>
        <!-- 拖播放头时跟着指针走的精确读数 -->
        <div v-if="scrub" class="timeline__scrub-tip" :style="{ left: `${scrub.left}px`, top: `${scrub.top}px` }">
          {{ formatTimecode(scrub.time) }}
        </div>
        <div ref="canvas" class="timeline__canvas" :style="{ width: `${zoom * 100}%` }">
          <!-- 标尺 -->
          <div class="timeline__ruler" @pointerdown="startScrub">
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
            :class="{ 'timeline__lane--over': dragLane === track.id, 'timeline__lane--drop': dropTarget(track) || lyricTarget === track.id }"
            :data-track-kind="track.id"
            @dblclick="addClipAt(track, $event)"
          >
            <!-- 特效轨：一行可以放好几段，每段各有起止与文字参数；平移到别的特效轨上就换层级 -->
            <template v-if="track.lane?.kind === 'effect'">
              <div
                v-for="clip in clipsIn(track)"
                :key="clip.id"
                class="timeline__clip timeline__clip--lane"
                :class="{ 'timeline__clip--active': clipActive(track.id, clip.id) }"
                :style="clipStyleOf(clip)"
                :data-lane-id="track.id"
                :data-clip-id="clip.id"
                @click.stop="selectClip(track.id, clip.id, $event)"
                @dblclick.stop
                @pointerdown="startClipDrag($event, track, 'move', clip)"
                @contextmenu="openMenu(track.id, $event, clip.id)"
              >
                <span
                  class="timeline__grip timeline__grip--left"
                  :title="t('editor-lane-start')"
                  @pointerdown.stop="startClipDrag($event, track, 'left', clip)"
                ></span>
                <i class="mdi timeline__clip-icon" :class="track.icon"></i>
                <span class="timeline__clip-text">{{ clip.effect?.text.trim() || t('editor-track-effect') }}</span>
                <span
                  class="timeline__grip timeline__grip--right"
                  :title="t('editor-lane-length')"
                  @pointerdown.stop="startClipDrag($event, track, 'right', clip)"
                ></span>
              </div>
              <div
                v-if="clipsIn(track).length === 0"
                class="timeline__clip timeline__clip--empty timeline__clip--lane"
                :class="{ 'timeline__clip--active': clipActive(track.id, null) }"
                :style="clipStyle(track)"
                @click="selectClip(track.id, null, $event)"
              >
                <v-icon icon="mdi-plus" size="14"></v-icon>
                <span>{{ track.drop }}</span>
              </div>
            </template>
            <!-- 轨道上有东西：铺一条整段的块（对象），点一下选中、右键出菜单。
                 点块只是选中，不改播放头 —— 改时间只走标尺和进度条 -->
            <div
              v-else-if="hasClip(track)"
              class="timeline__clip"
              :class="{
                'timeline__clip--active': clipActive(track.id, null),
                'timeline__clip--replace': replacing(track),
                'timeline__clip--lane': !!track.lane,
              }"
              :style="clipStyle(track)"
              @click="selectClip(track.id, null, $event)"
              @pointerdown="startClipDrag($event, track, 'move')"
              @contextmenu="openMenu(track.id, $event)"
            >
              <!-- 自己加的轨道片段：左右两边各一条把手，拉它改起点 / 长度 -->
              <span
                v-if="track.lane"
                class="timeline__grip timeline__grip--left"
                :title="t('editor-lane-start')"
                @pointerdown.stop="startClipDrag($event, track, 'left')"
              ></span>
              <i class="mdi timeline__clip-icon" :class="track.icon"></i>
              <span class="timeline__clip-text" :title="track.media?.path ?? track.text">
                {{ replacing(track) ? t('editor-track-replace') : (track.media?.name ?? track.text) }}
              </span>
              <span
                v-if="track.lane"
                class="timeline__grip timeline__grip--right"
                :title="t('editor-lane-length')"
                @pointerdown.stop="startClipDrag($event, track, 'right')"
              ></span>
            </div>
            <!-- 空轨道：虚线框 + 一句「把xx拖到这里」，说明这一条放什么；
                 点它也算选中这条轨道，右边编辑栏会给出「去素材库挑一个」 -->
            <div
              v-else
              class="timeline__clip timeline__clip--empty"
              :class="{ 'timeline__clip--lane': !!track.lane }"
              :style="clipStyle(track)"
              @click="selectClip(track.id, null, $event)"
            >
              <v-icon icon="mdi-plus" size="14"></v-icon>
              <span>{{ track.drop }}</span>
            </div>
          </div>

          <!-- 播放头贯穿标尺与各条轨道 -->
          <div class="timeline__playhead" :style="{ left: percent }">
            <span class="timeline__playhead-grip"></span>
          </div>

          <!-- 拖效果卡时的落点竖线：松手就在这一刻加一条特效轨 -->
          <div
            v-if="dropTime !== null"
            class="timeline__drop"
            :style="{ left: `${(dropTime / span) * 100}%` }"
          >
            <span class="timeline__drop-label">{{ clock(dropTime) }}</span>
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
  overflow: hidden;
  border-right: 0.5px solid var(--phi-border-l1);
}

/* 轨道多到超出高度时，这一层跟着轨道区的 scrollTop 往上挪 */
.timeline__heads-inner {
  display: flex;
  flex: none;
  flex-direction: column;
  will-change: transform;
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
  /* 放大后画布比可视区宽，横向滚动；轨道加得多了纵向也能滚（左边轨道名跟着平移） */
  overflow: auto;
}

/* 横向滚动条压细一点：它一高就把轨道挤出视野 */
.timeline__lanes::-webkit-scrollbar {
  height: 6px;
}

/* 框选矩形：跟着指针走，不接事件（否则会挡住下面的片段） */
.timeline__marquee {
  position: absolute;
  z-index: 30;
  pointer-events: none;
  background: color-mix(in srgb, var(--phi-accent) 12%, transparent);
  border: 1px solid var(--phi-accent);
}

.timeline__lanes::-webkit-scrollbar-thumb {
  background: var(--phi-border-l4);
  border-radius: var(--phi-radius-pill);
}

/* 拖播放头时的时间气泡：贴在指针上方，读数带毫秒 */
.timeline__scrub-tip {
  position: absolute;
  z-index: 40;
  transform: translateX(-50%);
  padding: 2px 6px;
  border-radius: var(--phi-radius-xs);
  background: var(--phi-accent);
  color: var(--phi-bg-layer-1);
  font-size: 0.66rem;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
  pointer-events: none;
  box-shadow: 0 2px 6px rgb(0 0 0 / 35%);
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

/* 自己加的轨道上的片段按起止时间绝对定位（主轨道的片段铺满整条轨道，不用这条） */
.timeline__clip--lane {
  position: absolute;
  top: 2px;
  bottom: 2px;
  height: auto;
  /* 淡橙色：一眼分清「自己加的轨道」与三条灰色主轨道 */
  background: rgb(255 168 76 / 18%);
  border-color: rgb(255 176 96 / 45%);
}

.timeline__clip--lane .timeline__clip-icon {
  color: rgb(255 197 136 / 90%);
}

/* 空的特效 / 文字轨只留虚线框，别在整条时间轴上铺一块橙底 */
.timeline__clip--lane.timeline__clip--empty {
  background: none;
  border-color: rgb(255 176 96 / 32%);
}

/* 左右两条把手：整块能拖起点，抓住两头拉长度 */
.timeline__grip {
  position: absolute;
  top: 0;
  bottom: 0;
  z-index: 1;
  width: 7px;
  cursor: ew-resize;
  background: rgb(255 176 96 / 55%);
}

.timeline__grip--left {
  left: 0;
  border-radius: var(--phi-radius-xs) 0 0 var(--phi-radius-xs);
}

.timeline__grip--right {
  right: 0;
  border-radius: 0 var(--phi-radius-xs) var(--phi-radius-xs) 0;
}

.timeline__grip:hover {
  background: var(--phi-accent);
}

/* 拖效果卡经过时间轴：整块画布泛一点主色，表示这里能落 */
.timeline__lanes--drop {
  background: color-mix(in srgb, var(--phi-accent) 6%, transparent);
}

/* 效果卡的落点竖线，带一个时刻标签 */
.timeline__drop {
  position: absolute;
  top: 0;
  bottom: 0;
  z-index: 20;
  width: 1px;
  pointer-events: none;
  background: var(--phi-accent);
}

.timeline__drop-label {
  position: absolute;
  top: 2px;
  left: 3px;
  padding: 1px 4px;
  font-size: 0.62rem;
  color: var(--phi-static-bluish-00);
  white-space: nowrap;
  background: var(--phi-accent);
  border-radius: var(--phi-radius-xs);
}

.timeline__zoom-value {
  flex: none;
  min-width: 3.2em;
  font-size: 0.72rem;
  color: var(--phi-label-secondary);
  text-align: right;
  font-variant-numeric: tabular-nums;
}

.timeline__head-name {
  overflow: hidden;
  flex: 1 1 auto;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.timeline__head-remove {
  display: flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  padding: 0;
  font-size: 12px;
  color: var(--phi-label-caption);
  cursor: pointer;
  background: none;
  border: 0;
  border-radius: var(--phi-radius-xs);
}

.timeline__head-remove:hover {
  color: var(--phi-label-primary);
  background: #ffffff14;
}

/* 「加轨道」的两项：往上加画面 / 文字，往下加音频 */
.timeline__lane-menu {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 132px;
  padding: 4px;
}

.timeline__lane-menu-item {
  display: flex;
  gap: var(--phi-space-2);
  align-items: center;
  padding: 6px var(--phi-space-2);
  font-size: 0.78rem;
  color: var(--phi-label-secondary);
  text-align: left;
  cursor: pointer;
  background: transparent;
  border: 0;
  border-radius: var(--phi-radius-sm);
}

.timeline__lane-menu-item:hover {
  color: var(--phi-static-bluish-00);
  background: #ffffff14;
}

.timeline__lane-menu-item i {
  font-size: 15px;
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

/* 拖着特效片段经过别的特效轨：目标轨道亮一层，示意松手就换到这一层 */
.timeline__lane--drop {
  background: color-mix(in srgb, var(--phi-accent) 7%, transparent);
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
