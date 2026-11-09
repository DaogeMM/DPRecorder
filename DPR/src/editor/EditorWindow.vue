<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';

import { useI18n } from 'vue-i18n';
import { VSonner } from 'vuetify-sonner';
const { t } = useI18n();

import { useStorage } from '@vueuse/core';

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { getCurrentWindow } from '@tauri-apps/api/window';

import type {
  ChartInfo,
  DraftClip,
  DraftFolder,
  DraftInfo,
  DraftLane,
  DraftMedia,
  DraftMediaKind,
  DraftTrackKind,
  EditorDraft,
  FileDropEvent,
  MediaInDir,
  TextNoiseEffect,
} from '@/model';
import { MAX_FOLDER_DEPTH, DEFAULT_NOISE_MATERIAL } from '@/model';
import { toast, toastError } from '@/common';
import EditorPreview from './EditorPreview.vue';
import EditorInspector from './EditorInspector.vue';
import EditorEditBar from './EditorEditBar.vue';
import EditorTimeline from './EditorTimeline.vue';
import { snapTime, timelineSpan } from './timeline';
import { blockAreasOf, clipsOf, defaultTextNoise } from './text-noise';
import { lyricClips, type LyricLine } from './lyrics';

/**
 * DPE「编辑窗口」的根组件 —— 独立窗口里只渲染它，不挂主界面的外壳（见 `main.ts`）。
 *
 * 这一版照剪映那样**按草稿**来：打开窗口面对的是一个草稿（`dpe/editor/drafts/<id>/draft.json`），
 * 不是某一张谱面。草稿里记三条轨道上放了什么（谱面 / 音乐 / 曲绘）和一份素材库，
 * 素材本身**只记原文件路径**、不复制，所以草稿只有几 KB，用户自己的文件保持原样。
 *
 * 版式照剪映那类剪辑器：左边是功能栏（信息 / 素材库 / 效果），中间是谱面预览，右边是编辑栏
 * （时间轴上选中哪条轨道，就在这里看那份素材、调它的效果），底部是时间轴；
 * 把素材从功能栏拖到时间轴对应的轨道上，就成了这张自定义谱面。
 *
 * 窗口没有系统标题栏（后端建窗时 `decorations(false)`）：顶上那条 `.editor-titlebar`
 * 就是标题栏 —— 左边 DPEditor、接着是草稿名（点开是草稿菜单）、右边是路径与导出，
 * 最右是三个窗口按钮，整条同时是拖拽区。窗口标题只留给任务栏（`DPEditor`）。
 *
 * 打开哪份草稿不写进 URL：后端记在进程状态里，这里用 `editor_context` 取；主窗口再点另一张
 * 谱面时后端发 `editor-draft-changed`，这个窗口不另开、直接换草稿。
 */

const draft = ref<EditorDraft | null>(null);
/** 草稿菜单里那份列表（最近改过的排前面）。 */
const drafts = ref<EditorDraft[]>([]);
const chartInfo = ref<ChartInfo | null>(null);
/** 曲绘的预览地址：草稿里指定了曲绘时用它的字节现做一个 Blob URL。 */
const coverUrl = ref('');
const loading = ref(true);

/** 谱面轨道上的那份谱面（目录或包）；空着就是还没放。 */
const chartPath = computed(() => draft.value?.chart?.path ?? '');

/** 特效轨铺出来的噪域方块：右栏的预览与中间的引擎预览都从这里取。 */
const blockAreas = computed(() => blockAreasOf(draft.value?.lanes ?? [], undefined, draft.value?.noise ?? null));

/** 播放状态、播放头与时长都由预览区里的播放器报回来（`DPRWPlayer` 的 `getTime` 等）。 */
const playing = ref(false);
const time = ref(0);
/** 时长：谱面本身不带总时长，播放器给不出来时先用一个占位值，进度条照样能画。 */
const duration = ref(180);

const preview = ref<InstanceType<typeof EditorPreview> | null>(null);
/** 左边的功能栏：编辑栏点「换素材」时要让它翻到素材库那一页。 */
const inspector = ref<InstanceType<typeof EditorInspector> | null>(null);
/** 底部时间轴：Ctrl+= / Ctrl+- 直接调它的缩放档位，和工具条的放大镜一致。 */
const timeline = ref<InstanceType<typeof EditorTimeline> | null>(null);

/**
 * 时间轴上选中的那条轨道。选中状态放在这里而不是时间轴里 —— 右边的编辑栏也要看它，
 * 两处各存一份迟早对不上；时间轴点选、拖素材上轨道都只发事件，由这里改。
 *
 * 特效轨一行可以放好几段，所以另记选中片段的 id；其余轨道的 `selectedClip` 恒为空。
 */
const selectedTrack = ref<string | null>(null);
const selectedClip = ref<string | null>(null);

/**
 * 批量选中的特效片段（Ctrl / Cmd 点时间轴上的片段）。
 *
 * 只收特效片段：右栏那些参数是噪域专有的，混进别的轨道没有可改的字段。
 * 主选中的那一段同时记在 `selectedTrack / selectedClip` 里，右栏按它取默认值。
 */
const multiClips = ref<{ laneId: string; clipId: string }[]>([]);

/** 选中轨道（以及特效轨上的某一段），同时把批量选择收成这一段。 */
function selectTrack(track: string | null, clip: string | null = null) {
  selectedTrack.value = track;
  selectedClip.value = track ? clip : null;
  multiClips.value = track && clip ? [{ laneId: track, clipId: clip }] : [];
}

/* ---- 撤销 / 重做 ----
 *
 * 整份草稿做快照：一次改动的所有字段（轨道、片段、素材、信息）一起回退，
 * 不按字段各记一套 —— 那样撤销顺序会跟改动的先后对不上。
 */
const undoStack = ref<string[]>([]);
const redoStack = ref<string[]>([]);
const HISTORY_LIMIT = 100;
/** 上一次落盘后的样子：撤销栈里存的是它之前的状态。 */
let historyBaseline = '';
let historyAt = 0;
let restoring = false;

function draftSnapshot(): string {
  return JSON.stringify(draft.value);
}

/** 换草稿后从头记历史（旧草稿的步骤不能撤到新草稿上）。 */
function resetHistory() {
  undoStack.value = [];
  redoStack.value = [];
  historyBaseline = draftSnapshot();
  historyAt = 0;
}

/**
 * 记一步历史。连续拖动滑杆会连着存好几次，同一个 500ms 突发只记第一步 ——
 * 否则撤销一次只退一小格，得按几十下才回到拖动前。
 */
function recordHistory() {
  if (restoring || !draft.value) return;
  const current = draftSnapshot();
  if (current === historyBaseline) return;
  const now = performance.now();
  if (now - historyAt > 500) {
    undoStack.value.push(historyBaseline);
    if (undoStack.value.length > HISTORY_LIMIT) undoStack.value.shift();
    redoStack.value = [];
    historyAt = now;
  }
  historyBaseline = current;
}

/** 回放一份快照：直接替换草稿再落盘，落盘这一下不再记历史。 */
async function applySnapshot(snapshot: string) {
  const keepTrack = selectedTrack.value;
  const keepClip = selectedClip.value;
  draft.value = JSON.parse(snapshot) as EditorDraft;
  historyBaseline = snapshot;
  restoring = true;
  try {
    await saveDraft();
  } finally {
    restoring = false;
  }
  // 撤销/重做不该顺手把选中态清掉：快照里还在的那段继续选中，没了才退回「什么都没选」
  const lane = draft.value.lanes.find((it) => it.id === keepTrack);
  if (lane && (!keepClip || findClip(lane, keepClip))) selectTrack(keepTrack, keepClip);
  else selectTrack(null);
}

function undo() {
  const target = undoStack.value.pop();
  if (!target) return;
  redoStack.value.push(draftSnapshot());
  void applySnapshot(target);
}

function redo() {
  const target = redoStack.value.pop();
  if (!target) return;
  undoStack.value.push(draftSnapshot());
  void applySnapshot(target);
}

/** 时间轴发来的选中：普通的换选中，按住 Ctrl 的加/减进批量选择。 */
function selectClipEvent(track: string | null, clip: string | null = null, additive = false) {
  if (!additive || !track || !clip) {
    selectTrack(track, clip);
    return;
  }
  const index = multiClips.value.findIndex((it) => it.laneId === track && it.clipId === clip);
  if (index < 0) {
    multiClips.value = [...multiClips.value, { laneId: track, clipId: clip }];
    selectedTrack.value = track;
    selectedClip.value = clip;
    return;
  }
  multiClips.value = multiClips.value.filter((_, i) => i !== index);
  if (multiClips.value.length === 0) {
    selectedTrack.value = null;
    selectedClip.value = null;
    return;
  }
  if (selectedTrack.value === track && selectedClip.value === clip) {
    const last = multiClips.value[multiClips.value.length - 1];
    if (last) {
      selectedTrack.value = last.laneId;
      selectedClip.value = last.clipId;
    }
  }
}

/** 时间轴上的框选：矩形碰到的特效片段一次全选上；全空就清掉选中。 */
function selectManyClips(items: { laneId: string; clipId: string }[]) {
  if (items.length === 0) {
    selectTrack(null);
    return;
  }
  multiClips.value = items.map((it) => ({ ...it }));
  const last = items[items.length - 1];
  if (last) {
    selectedTrack.value = last.laneId;
    selectedClip.value = last.clipId;
  }
}

/** 换草稿时把选中清掉，别指着上一份草稿的轨道。 */
watch(
  () => draft.value?.id,
  () => {
    selectTrack(null);
  },
);

/** 编辑栏的「换素材 / 去素材库挑」：翻到左边的素材库，素材还是照老办法拖到轨道上。 */
function pickMediaForTrack() {
  inspector.value?.showMedia();
}

/**
 * 两侧栏的宽度与时间轴高度都可以拖分隔条改，记在本地（`useStorage`）：关掉窗口再开还是上次调好的尺寸。
 *
 * 上限都按窗口算而不是写死像素：预览区是 16:9 的，功能栏 / 编辑栏被拖到太宽、时间轴被拖得太高，
 * 预览就只剩一条缝。
 */
const inspectorWidth = useStorage('DPEditor.inspectorWidth', 320);
const editWidth = useStorage('DPEditor.editWidth', 300);
const timelineHeight = useStorage('DPEditor.timelineHeight', 214);

const INSPECTOR_MIN = 260;
/** 功能栏最多占窗口宽度的四成：素材卡宽一点更好用，但也别把预览挤没。 */
const INSPECTOR_MAX_RATIO = 0.4;
const EDIT_MIN = 220;
/** 编辑栏里主要是属性行和几个按钮，窄一点也够，上限同样按四成算。 */
const EDIT_MAX_RATIO = 0.4;
const TIMELINE_MIN = 140;
/** 时间轴最多占窗口高度的六成。 */
const TIMELINE_MAX_RATIO = 0.6;

/** 拖的是哪条竖分隔条：功能栏在左边（往右拖变宽），编辑栏在右边（往左拖变宽）。 */
type DragPanel = 'inspector' | 'edit';

let dragging: { axis: 'x' | 'y'; panel: DragPanel | null; from: number; start: number } | null = null;

/**
 * 按下分隔条开始拖：用 pointer capture 把后续 move 都收在这个元素上，
 * 这样指针滑出那条细线也不会断（松手在窗口外也能收到 up）。
 */
function startDrag(axis: 'x' | 'y', event: PointerEvent, panel: DragPanel | null = null) {
  // 合成事件（自动化里按坐标点）没有真实指针，捕获会抛 NotFoundError，忽略即可。
  try {
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  } catch {
    /* 没有指针可捕获：不影响拖动本身 */
  }
  dragging = {
    axis,
    panel,
    from: axis === 'x' ? event.clientX : event.clientY,
    start: panel === 'edit' ? editWidth.value : axis === 'x' ? inspectorWidth.value : timelineHeight.value,
  };
  event.preventDefault();
}

/**
 * 功能栏在预览左边、编辑栏在右边、时间轴在底部，所以三个方向都是「往回拖变大」：
 * 左边的栏用起点加位移，右边和下面的栏用起点减位移。
 */
function dragTo(event: PointerEvent) {
  if (!dragging) return;
  const { axis, panel, from, start } = dragging;
  if (axis === 'x') {
    const delta = event.clientX - from;
    const max = Math.round(window.innerWidth * (panel === 'edit' ? EDIT_MAX_RATIO : INSPECTOR_MAX_RATIO));
    const next = panel === 'edit' ? start - delta : start + delta;
    const min = panel === 'edit' ? EDIT_MIN : INSPECTOR_MIN;
    const value = Math.round(Math.min(max, Math.max(min, next)));
    if (panel === 'edit') editWidth.value = value;
    else inspectorWidth.value = value;
  } else {
    const next = start - (event.clientY - from);
    const max = Math.round(window.innerHeight * TIMELINE_MAX_RATIO);
    timelineHeight.value = Math.round(Math.min(max, Math.max(TIMELINE_MIN, next)));
  }
}

function endDrag(event: PointerEvent) {
  if (!dragging) return;
  const target = event.currentTarget as HTMLElement;
  if (target.hasPointerCapture?.(event.pointerId)) target.releasePointerCapture(event.pointerId);
  dragging = null;
}

/** 草稿菜单里显示的时间（`08-12 21:03`）。 */
function formatUpdated(ms: number) {
  if (!ms) return '';
  const date = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** 草稿菜单那份列表：最近改过的排前面。 */
async function refreshDrafts() {
  try {
    drafts.value = (await invoke('list_editor_drafts')) as EditorDraft[];
  } catch (e) {
    toastError(e);
  }
}

/** 读谱面轨道上那份谱面的信息（曲名、难度、曲师……）；没放谱面就清空。 */
async function loadChartInfo() {
  chartInfo.value = null;
  if (!chartPath.value) return;
  try {
    chartInfo.value = (await invoke('parse_chart', { path: chartPath.value })) as ChartInfo;
  } catch (e) {
    toastError(e);
    return;
  }
  await seedInfoFromChart();
}

/**
 * 谱面自带的信息补进「信息」页里空着的字段（曲线 / 曲师 / 谱师也是）。
 *
 * 只填空位：用户自己填过的一律不覆盖，谱面读不出来或本身没写就什么都不做。
 * 预览里跑的就是这张谱面，信息页跟着它走才对得上。
 */
async function seedInfoFromChart() {
  const info = chartInfo.value;
  const current = draft.value;
  if (!info || !current) return;
  const filled: DraftInfo = { ...current.info };
  const pairs: [keyof DraftInfo, string][] = [
    ['name', info.name],
    ['difficulty', info.difficulty],
    ['charter', info.charter],
    ['composer', info.composer],
  ];
  let changed = false;
  for (const [key, value] of pairs) {
    const next = (value ?? '').trim();
    if (next && !filled[key].trim()) {
      filled[key] = next;
      changed = true;
    }
  }
  if (!changed) return;
  draft.value = { ...current, info: filled };
  await saveDraft();
}

/** 曲绘的 Blob URL：换草稿 / 换曲绘前要先 release 掉旧的，否则每换一次漏一份内存。 */
let coverObjectUrl = '';

async function loadCover() {
  if (coverObjectUrl) {
    URL.revokeObjectURL(coverObjectUrl);
    coverObjectUrl = '';
  }
  coverUrl.value = '';
  const path = draft.value?.cover?.path;
  if (!path) return;
  try {
    const bytes = await invoke<ArrayBuffer>('read_editor_media', { path });
    coverObjectUrl = URL.createObjectURL(new Blob([bytes]));
    coverUrl.value = coverObjectUrl;
  } catch (e) {
    toastError(e);
  }
}

/** 压缩包谱面的扩展名：素材库里它们应该已经展开成文件夹，不该再以文件形式出现。 */
const ARCHIVE_RE = /\.(pez|zip)$/i;

/**
 * 旧草稿里还留在素材库的 `.pez` / `.zip`：就地展开成文件夹，压缩包本身从素材库里去掉。
 *
 * 展开的路径固定在展开缓存里，所以这里只在第一次打开那种草稿时干活；谱面轨道上挂的还是包本身时，
 * 顺手换成包里的那份谱面文件 —— 素材库里已经没有这个包了，轨道上却还显示它会更让人费解。
 * 包已经不在了就原样留着（素材行会照常标出文件丢失）。
 */
async function unpackLibraryArchives(next: EditorDraft): Promise<EditorDraft | null> {
  const archives = next.media.filter((it) => ARCHIVE_RE.test(it.path));
  if (archives.length === 0) return null;
  const paths = new Set(archives.map((it) => it.path));
  let found: MediaInDir[];
  try {
    found = (await invoke('import_editor_media', { paths: [...paths] })) as MediaInDir[];
  } catch {
    return null;
  }
  const { folders, items } = importTree(found, null);
  const migrated: EditorDraft = {
    ...next,
    media: [...next.media.filter((it) => !paths.has(it.path)), ...items],
    folders: [...next.folders, ...folders],
  };
  const chartFile = items.find((it) => it.kind === 'chart');
  if (migrated.chart && paths.has(migrated.chart.path) && chartFile) migrated.chart = chartFile;
  return migrated;
}

/** 换到某份草稿：预览从头开始，谱面信息与曲绘按新草稿重读，再让后端记住「当前草稿」。 */
async function applyDraft(next: EditorDraft) {
  // 旧草稿还没落盘的改动不能再写回去（否则会盖掉正在加载的这份）
  cancelPendingSave();
  draft.value = next;
  playing.value = false;
  time.value = 0;
  const migrated = await unpackLibraryArchives(next);
  if (migrated) {
    draft.value = migrated;
    await saveDraft();
  }
  await invoke('set_editor_draft', { id: next.id }).catch(() => {
    /* 记不住也不影响这次编辑：主窗口下次点谱面会另开一个草稿 */
  });
  await loadChartInfo();
  await loadCover();
  await refreshDrafts();
  resetHistory();
}

async function loadDraft(id: string) {
  loading.value = true;
  try {
    await applyDraft((await invoke('load_editor_draft', { id })) as EditorDraft);
  } catch (e) {
    toastError(e);
    draft.value = null;
    resetHistory();
  } finally {
    loading.value = false;
  }
}

/**
 * 保存当前草稿（轨道上放了什么、名字改没改都在里面）。
 *
 * 拖动滑杆时一秒能改几十次，每次都写盘不但慢，还会把草稿菜单刷得乱跳：
 * 这里先攒 250ms，一次改动攒成一次写入。返回值在真正落盘后才 resolve，
 * 需要「改完立刻要文件」的地方先 `await flushSave()`。
 */
const SAVE_DELAY = 250;
let saveTimer: number | null = null;
let saveWaiters: (() => void)[] = [];
/** 每次要求存盘加一：存盘期间又改了，就别用旧结果盖回新改动。 */
let saveSeq = 0;
let saving: Promise<void> | null = null;

function saveDraft(): Promise<void> {
  if (!draft.value) return Promise.resolve();
  saveSeq += 1;
  return new Promise((resolve) => {
    saveWaiters.push(resolve);
    if (saveTimer !== null) window.clearTimeout(saveTimer);
    saveTimer = window.setTimeout(() => {
      saveTimer = null;
      void flushSave();
    }, SAVE_DELAY);
  });
}

/** 丢掉还没落盘的那一次（换草稿时用：旧草稿的改动不该盖到新草稿上）。 */
function cancelPendingSave() {
  if (saveTimer !== null) {
    window.clearTimeout(saveTimer);
    saveTimer = null;
  }
  saveSeq += 1;
  const waiters = saveWaiters;
  saveWaiters = [];
  for (const resolve of waiters) resolve();
}

/** 立刻落盘。切草稿、关窗口前必须先调它。 */
async function flushSave(): Promise<void> {
  if (saveTimer !== null) {
    window.clearTimeout(saveTimer);
    saveTimer = null;
  }
  if (saving) {
    await saving;
    if (saveWaiters.length > 0) await flushSave();
    return;
  }
  const draftNow = draft.value;
  if (!draftNow) {
    const waiters = saveWaiters;
    saveWaiters = [];
    for (const resolve of waiters) resolve();
    return;
  }
  const seq = saveSeq;
  const waiters = saveWaiters;
  saveWaiters = [];
  recordHistory();
  saving = (async () => {
    try {
      const saved = (await invoke('save_editor_draft', { draft: draftNow })) as EditorDraft;
      // 存盘这段时间又改了：保留新改动，下一轮再写
      if (seq === saveSeq && draft.value) {
        draft.value = saved;
        historyBaseline = draftSnapshot();
      }
      await refreshDraftsThrottled();
    } catch (e) {
      toastError(e);
    } finally {
      for (const resolve of waiters) resolve();
      saving = null;
    }
  })();
  await saving;
  if (saveWaiters.length > 0) await flushSave();
}

/** 草稿菜单最多每 1.5 秒刷一次：菜单只显示名字与时间，没必要每次存盘都重列。 */
let draftsAt = 0;
async function refreshDraftsThrottled() {
  const now = performance.now();
  if (now - draftsAt < 1500) return;
  draftsAt = now;
  await refreshDrafts();
}

async function createDraft() {
  await flushSave();
  try {
    await applyDraft((await invoke('create_editor_draft', { name: null })) as EditorDraft);
  } catch (e) {
    toastError(e);
  }
}

/**
 * 能不能导出：音乐与曲名是硬条件。
 *
 * 播放器没有音乐就起不来（谱面长度取音乐长度），没有曲名的包在任何列表里都只能显示成文件名；
 * 谱面与曲绘空着不挡 —— 谱面空着导出时塞一张空谱面，曲绘空着就是黑屏。
 */
const canExport = computed(() => !!draft.value?.music && !!draft.value.info.name.trim());
const exporting = ref(false);

/** 导出成 `<dpe>/editor/output/<曲名>_<草稿名>.pez`，落地的路径由后端回给这里。 */
async function exportDraft() {
  if (!draft.value || !canExport.value || exporting.value) return;
  exporting.value = true;
  try {
    // 特效轨的噪域要写进导出的谱面里：编辑器这边算好方块，后端只负责并进谱面字节
    const saved = (await invoke('export_editor_draft', {
      draft: draft.value,
      blockAreas: blockAreas.value,
    })) as string;
    toast(t('editor-export-done', { path: saved }), 'success');
  } catch (e) {
    toastError(e);
  } finally {
    exporting.value = false;
  }
}

async function switchDraft(id: string) {
  if (id === draft.value?.id) return;
  await flushSave();
  await loadDraft(id);
}

/**
 * 把一项素材放到某条轨道上（从素材库拖过来，或点素材库上的「放到轨道」）。
 *
 * `track` 是松手时指针压着的那条轨道 id：做
 * - 主轨道（`music` / `chart` / `cover`）只收同种素材，拖错类型不落上去，也不弹提示；
 * - 自己加的轨道（`draft.lanes` 里的 id）什么素材都收，长短由这条轨道自己定。
 *
 * 只放这一项：谱面包里同目录的音乐与曲绘不跟着上轨道，用哪首 / 哪张由用户自己挑。
 */
async function assignMedia(media: DraftMedia, track?: string) {
  if (!draft.value) return;
  // 歌词没有固定轨道：压在特效轨上就并进那条，否则按它的时间新铺一条文字噪域轨。
  // 素材库双击 / 点「放到轨道」、从资源管理器整份拖进来都走这一条。
  if (media.kind === 'lyric') {
    await addLyrics(media, track && !isMainTrack(track) ? track : null);
    return;
  }
  const lane = track && !isMainTrack(track) ? draft.value.lanes.find((it) => it.id === track) : null;
  if (lane) {
    // 自己加的轨道按种类收货：往上加的画面轨只收图片，往下加的音频轨只收音频；
    // 文字轨与特效轨没有素材文件，拖什么上来都不接
    if (lane.kind === 'text' || lane.kind === 'effect') return;
    if (lane.kind === 'image' && media.kind !== 'cover') return;
    if (lane.kind === 'audio' && media.kind !== 'music') return;
    await updateLane(track as string, { media });
    return;
  }
  if (track && track !== media.kind) return;
  const next: EditorDraft = { ...draft.value };
  if (media.kind === 'chart') next.chart = media;
  else if (media.kind === 'music') next.music = media;
  else next.cover = media;
  if (!next.media.some((it) => it.path === media.path && it.kind === media.kind)) {
    next.media = [...next.media, media];
  }
  draft.value = next;
  await saveDraft();
  if (media.kind === 'chart') await loadChartInfo();
  if (media.kind === 'cover') await loadCover();
}

/** 把某项从轨道上拿下来（素材还留在素材库里）。`track` 是轨道 id，自己加的轨道也一样。 */
async function clearTrack(track: string) {
  if (!draft.value) return;
  if (!isMainTrack(track)) {
    await updateLane(track, { media: null });
    return;
  }
  const next: EditorDraft = { ...draft.value };
  if (track === 'chart') next.chart = null;
  else if (track === 'music') next.music = null;
  else next.cover = null;
  draft.value = next;
  await saveDraft();
  if (track === 'chart') await loadChartInfo();
  if (track === 'cover') await loadCover();
}

/** 自己加的轨道 id 一律不是这三个，用来把「主轨道」和自己加的轨道分开。 */
function isMainTrack(track: string): track is DraftTrackKind {
  return track === 'chart' || track === 'music' || track === 'cover';
}

/** 改自己加的轨道上的片段（起点 / 长度 / 名字 / 素材），改完照常落盘。 */
async function updateLane(id: string, patch: Partial<DraftLane>) {
  if (!draft.value) return;
  const next: EditorDraft = {
    ...draft.value,
    lanes: draft.value.lanes.map((it) => (it.id === id ? { ...it, ...patch } : it)),
  };
  draft.value = next;
  await saveDraft();
}

/** 特效轨上某一段；轨道没有片段或 id 对不上时返回 null。 */
function findClip(lane: DraftLane | null | undefined, clipId: string | null): DraftClip | null {
  if (!lane || !clipId) return null;
  return (lane.clips ?? []).find((it) => it.id === clipId) ?? null;
}

/** 改特效轨上的某一段（起止或文字参数），改完照常落盘。 */
async function updateClip(laneId: string, clipId: string, patch: Partial<DraftClip>) {
  if (!draft.value) return;
  draft.value = {
    ...draft.value,
    lanes: draft.value.lanes.map((lane) =>
      lane.id === laneId ? { ...lane, clips: (lane.clips ?? []).map((clip) => (clip.id === clipId ? { ...clip, ...patch } : clip)) } : lane,
    ),
  };
  await saveDraft();
}

/**
 * 批量改选中的那几段：只动它们的 `effect`，别的字段（起止、id）各留各的。
 *
 * 右栏在「多个值」状态下改一下滑杆就走这里 —— 一次把同一项写到每一段上，
 * 于是改完它们在这一项上就一致了。
 */
async function updateClips(patch: Partial<TextNoiseEffect>) {
  if (!draft.value || multiClips.value.length === 0) return;
  const targets = new Set(multiClips.value.map((it) => `${it.laneId}/${it.clipId}`));
  draft.value = {
    ...draft.value,
    lanes: draft.value.lanes.map((lane) => ({
      ...lane,
      clips: (lane.clips ?? []).map((clip) =>
        targets.has(`${lane.id}/${clip.id}`) && clip.effect ? { ...clip, effect: { ...clip.effect, ...patch } } : clip,
      ),
    })),
  };
  await saveDraft();
}

/** 批量改起止：同一项写到每一段上（想让几段对齐时用）。 */
async function updateClipsTime(patch: Partial<Pick<DraftClip, 'start' | 'length'>>) {
  if (!draft.value || multiClips.value.length === 0) return;
  const targets = new Set(multiClips.value.map((it) => `${it.laneId}/${it.clipId}`));
  draft.value = {
    ...draft.value,
    lanes: draft.value.lanes.map((lane) => ({
      ...lane,
      clips: (lane.clips ?? []).map((clip) => (targets.has(`${lane.id}/${clip.id}`) ? { ...clip, ...patch } : clip)),
    })),
  };
  await saveDraft();
}

/** 时间轴上拉片段的两端（或整段平移）松手：把新的起点与长度写回那一段。 */
async function resizeClip(laneId: string, clipId: string, start: number, length: number) {
  await updateClip(laneId, clipId, { start, length });
}

/**
 * 把一段拖到别的特效轨上：换层级，起点按松手位置重新定。
 *
 * 源轨道因此空了就顺手删掉 —— 没有片段的特效轨只剩一行空壳，留着只会碍事。
 * 拖回同一条轨道时只改起点。
 */
async function moveClip(laneId: string, clipId: string, toLaneId: string, start: number) {
  if (!draft.value) return;
  const lane = draft.value.lanes.find((it) => it.id === laneId);
  const clip = findClip(lane, clipId);
  const target = draft.value.lanes.find((it) => it.id === toLaneId && it.kind === 'effect');
  if (!lane || !clip || !target) return;
  if (laneId === toLaneId) {
    await updateClip(laneId, clipId, { start });
    return;
  }
  const moved: DraftClip = { ...clip, start };
  const lanes = draft.value.lanes
    .map((it) => {
      if (it.id === laneId) return { ...it, clips: (it.clips ?? []).filter((one) => one.id !== clipId) };
      if (it.id === toLaneId) return { ...it, clips: [...(it.clips ?? []), moved] };
      return it;
    })
    .filter((it) => !(it.kind === 'effect' && (it.clips ?? []).length === 0));
  draft.value = { ...draft.value, lanes };
  selectTrack(toLaneId, clipId);
  await saveDraft();
}

/** 删掉特效轨上的一段；这段是轨道上最后一段时，整条轨道一起走。 */
async function removeClip(laneId: string, clipId: string) {
  if (!draft.value) return;
  const lanes = draft.value.lanes
    .map((it) => (it.id === laneId ? { ...it, clips: (it.clips ?? []).filter((one) => one.id !== clipId) } : it))
    .filter((it) => !(it.kind === 'effect' && (it.clips ?? []).length === 0));
  draft.value = { ...draft.value, lanes };
  if (selectedTrack.value === laneId && selectedClip.value === clipId) selectTrack(null);
  await saveDraft();
}

/** 在已有特效轨上再加一段（时间轴上双击空白处）：参数照这一段新起一份，材质沿用上一条。 */
async function addClip(laneId: string, seconds: number) {
  if (!draft.value) return;
  const lane = draft.value.lanes.find((it) => it.id === laneId && it.kind === 'effect');
  if (!lane) return;
  const clips = lane.clips ?? [];
  const used = new Set(clips.map((it) => it.id));
  let index = clips.length + 1;
  let id = `${laneId}-c${index}`;
  while (used.has(id)) id = `${laneId}-c${++index}`;
  const material = clips[0]?.effect?.material ?? draft.value.noise ?? DEFAULT_NOISE_MATERIAL;
  const clip: DraftClip = {
    id,
    start: Math.max(0, seconds),
    length: 10,
    effect: { ...defaultTextNoise(), material },
  };
  draft.value = {
    ...draft.value,
    lanes: draft.value.lanes.map((it) => (it.id === laneId ? { ...it, clips: [...(it.clips ?? []), clip] } : it)),
  };
  selectTrack(laneId, id);
  await saveDraft();
}

/**
 * 加一条自己的轨道：`side` 决定摆在主轨道上面还是下面，`start` 是片段起点（特效卡拖到时间轴上时按落点给）。
 *
 * - `'up'`：画面轨，只收图片（以及文字轨上写的字）；
 * - `'down'`：音频轨，只收音乐；
 * - `'effect'`：特效轨，目前只有「文字噪域」，参数摆在右边编辑栏，片段先给 10 秒。
 *
 * `before` 是时间轴发来的落点：特效卡松在两条轨道之间时，那条「下面」的轨道 id。
 * 只有自己加的轨道能按位置插进去；命中主轨道（谱面 / 曲绘 / 音乐）时插到自加轨道那一侧的末尾。
 */
async function addLane(side: 'up' | 'down' | 'effect', start = 0, before: string | null = null) {
  if (!draft.value) return;
  const id = `l${Date.now().toString(16)}${Math.random().toString(16).slice(2, 6)}`;
  const startAt = Math.max(0, start);
  const clip: DraftClip | null =
    side === 'effect'
      ? {
          id: `${id}-c1`,
          start: startAt,
          length: 10,
          effect: { ...defaultTextNoise(), material: draft.value.noise ?? DEFAULT_NOISE_MATERIAL },
        }
      : null;
  const lane: DraftLane = {
    id,
    name: side === 'effect' ? t('editor-effect-text-noise') : '',
    media: null,
    start: startAt,
    length: 10,
    kind: side === 'effect' ? 'effect' : side === 'up' ? 'image' : 'audio',
    text: '',
    effect: null,
    clips: clip ? [clip] : [],
  };
  const lanes = [...draft.value.lanes];
  const target = before ? lanes.findIndex((it) => it.id === before && it.kind !== 'audio') : -1;
  if (target >= 0) lanes.splice(target, 0, lane);
  else lanes.push(lane);
  draft.value = { ...draft.value, lanes };
  selectTrack(lane.id, clip?.id ?? null);
  await saveDraft();
}

/** 效果卡拖到时间轴上松手：那一刻是新特效轨的起点，落点决定它插在哪两条轨道之间。 */
async function addEffectAt(seconds: number, before: string | null = null) {
  await addLane('effect', seconds, before);
}

/**
 * 把一份歌词铺成文字噪域：压在已有的特效轨上就并进那条轨，否则新建一条特效轨。
 *
 * 一句一个片段，每段的起止全来自文件（见 `lyrics.ts`），所以指针位置只有两种用法：
 * 指着一条特效轨（`laneId`）= 片段追加进那条轨，已有的片段按起点跟新片段排在一起；
 * 指着别的轨道或空白处（`before`）= 新建一条，插在落点这一侧。
 *
 * 新建的那条跟手加的文字噪域轨没有区别：不挂素材文件、名字留空（时间轴自己编「轨道 N」），
 * 歌词文件只是素材库里的一项。解析失败或一句带时间的句子都没有时只弹提示，不动草稿。
 */
async function addLyrics(media: DraftMedia, laneId: string | null = null, before: string | null = null) {
  if (!draft.value) return;
  let lines: LyricLine[];
  try {
    lines = (await invoke('parse_editor_lyrics', { path: media.path })) as LyricLine[];
  } catch (e) {
    toastError(e);
    return;
  }
  const material = draft.value.noise ?? DEFAULT_NOISE_MATERIAL;
  const join = laneId ? draft.value.lanes.find((it) => it.id === laneId && it.kind === 'effect') : null;
  if (join) {
    const added = lyricClips(join.id, lines, material);
    const first = added[0];
    if (!first) {
      toast(t('editor-lyrics-none'), 'warning');
      return;
    }
    // 轨道自身的起止只用来让时间轴总长盖住所有片段（`laneEnd` 看的是 start + length）
    const merged = [...clipsOf(join), ...added].sort((a, b) => a.start - b.start);
    const start = Math.min(...merged.map((it) => it.start));
    const end = merged.reduce((max, it) => Math.max(max, it.start + it.length), start);
    await updateLane(join.id, { clips: merged, start, length: end - start });
    selectTrack(join.id, first.id);
    toast(t('editor-lyrics-added', { n: added.length }), 'success');
    return;
  }
  const id = `l${Date.now().toString(16)}${Math.random().toString(16).slice(2, 6)}`;
  const clips = lyricClips(id, lines, material);
  const first = clips[0];
  if (!first) {
    toast(t('editor-lyrics-none'), 'warning');
    return;
  }
  const start = first.start;
  const end = clips.reduce((max, clip) => Math.max(max, clip.start + clip.length), start);
  const lane: DraftLane = {
    id,
    name: '',
    media: null,
    start,
    length: end - start,
    kind: 'effect',
    text: '',
    effect: null,
    clips,
  };
  const lanes = [...draft.value.lanes];
  const target = before ? lanes.findIndex((it) => it.id === before && it.kind !== 'audio') : -1;
  if (target >= 0) lanes.splice(target, 0, lane);
  else lanes.push(lane);
  draft.value = { ...draft.value, lanes };
  if (!draft.value.media.some((it) => it.path === media.path && it.kind === media.kind)) {
    draft.value = { ...draft.value, media: [...draft.value.media, media] };
  }
  selectTrack(lane.id, first.id);
  await saveDraft();
  toast(t('editor-lyrics-added', { n: clips.length }), 'success');
}

/** 时间轴上拖片段 / 拉两边松手：把新的起点与长度写回草稿。 */
async function resizeLane(id: string, start: number, length: number) {
  await updateLane(id, { start, length });
}

/** 删掉一条自己加的轨道（主轨道不能删）。 */
async function removeLane(id: string) {
  if (!draft.value) return;
  if (selectedTrack.value === id) selectTrack(null);
  draft.value = { ...draft.value, lanes: draft.value.lanes.filter((it) => it.id !== id) };
  await saveDraft();
}

/**
 * 信息页那四个字段：先在本地改（敲一个字就刷新界面），停下 400ms 再写盘，
 * 免得每敲一个字母都写一次 draft.json。
 */
let infoTimer = 0;

function onUpdateInfo(next: DraftInfo) {
  if (!draft.value) return;
  draft.value = { ...draft.value, info: next };
  window.clearTimeout(infoTimer);
  infoTimer = window.setTimeout(() => void saveDraft(), 400);
}

/**
 * 从资源管理器直接把文件拖进窗口：按文件自己的种类放到对应轨道，
 * 认不出的（比如 txt）后端会跳过，只在素材库里出现能认出来的那些。
 *
 * 拖进来的是文件夹或 `.pez` / `.zip` 时只进素材库（按真实层级建文件夹），**不自动占轨道**：
 * 解出来的那份东西有好几样，放哪首当音乐、哪张当曲绘由用户自己挑。
 */
async function dropFiles(paths: string[]) {
  if (!draft.value || paths.length === 0) return;
  try {
    const found = (await invoke('import_editor_media', { paths })) as MediaInDir[];
    const { folders, items } = importTree(found, null);
    if (items.length > 0) {
      mergeMedia(items, folders);
      await saveDraft();
    }
    for (const at of found) {
      if (at.dir !== '') continue;
      await assignMedia(at.media);
    }
  } catch (e) {
    toastError(e);
  }
}

/** 文件选择框的过滤器：后端按扩展名认种类，这里把认得的都列上，免得多选时被挡。 */
const IMPORT_FILTERS = [
  {
    name: '谱面 / 音频 / 曲绘 / 歌词',
    extensions: [
      'pez', 'zip', 'mc', 'pec', 'json', 'yaml', 'yml',
      'mp3', 'ogg', 'wav', 'flac', 'm4a', 'aac', 'opus',
      'png', 'jpg', 'jpeg', 'webp', 'bmp', 'gif',
      'srt', 'lrc', 'ass', 'ssa', 'vtt',
    ],
  },
  { name: '所有文件', extensions: ['*'] },
];

/** 导入素材（选文件）：只记路径、不复制文件；混进认不出的文件时后端会跳过它们。 */
async function importMedia() {
  try {
    const picked = await openDialog({
      multiple: true,
      title: t('editor-import-file'),
      filters: IMPORT_FILTERS,
    });
    if (!picked) return;
    await addMedia(Array.isArray(picked) ? picked : [picked]);
  } catch (e) {
    toastError(e);
  }
}

/**
 * 导入素材（选文件夹）：文件选择框挑不到目录，所以下拉里单独一条。
 *
 * 素材库照这个目录的**真实层级**建文件夹 —— 选中的目录是第一级，它里面的子目录再往下套，
 * 最多 [`MAX_FOLDER_DEPTH`] 级，更深的摊在最深那一级里。里面的素材（最多往下三十一层）都收进库，
 * 里面的 `.pez` / `.zip` 解开成一层文件夹（包名当名字），不按压缩包文件出现；
 * 但**都不往轨道上放**：哪首当音乐、哪张当曲绘由用户自己挑。
 */
async function importFolder() {
  try {
    const picked = await openDialog({
      directory: true,
      multiple: false,
      title: t('editor-import-folder'),
    });
    if (!picked || Array.isArray(picked)) return;
    const found = (await invoke('import_editor_media_dir', { dir: picked })) as MediaInDir[];
    if (found.length === 0) {
      toastError(t('editor-add-folder-empty'));
      return;
    }
    const root = newFolder(folderNameOf(picked), null);
    const { folders, items } = importTree(found, root);
    // 整个文件夹都已经在库里了：什么都没变，不用多建一层同名文件夹
    if (items.length === 0) return;
    mergeMedia(items, folders);
    await saveDraft();
  } catch (e) {
    toastError(e);
  }
}

/**
 * 把导入结果摊成素材库的文件夹树：相对目录的每一段建一层，最多 [`MAX_FOLDER_DEPTH`] 级，
 * 更深的摊在最深那一级里。
 *
 * `root` 是选文件夹时先按目录名建的那一层（选文件、拖文件时没有：`.pez` / `.zip` 解出来的
 * 第一段目录直接从根上建）。已经在库里的素材跳过，连带为它要建的文件夹也不建。
 */
function importTree(found: MediaInDir[], root: DraftFolder | null) {
  const folders: DraftFolder[] = root ? [root] : [];
  // 相对目录 → 建出来的那一层；根那层用空串
  const byPath = new Map<string, { folder: DraftFolder; depth: number }>();
  if (root) byPath.set('', { folder: root, depth: 1 });
  const known = new Set((draft.value?.media ?? []).map((it) => `${it.kind}:${it.path}`));
  const items: DraftMedia[] = [];
  for (const at of found) {
    if (known.has(`${at.media.kind}:${at.media.path}`)) continue;
    let here = byPath.get('') ?? null;
    let rel = '';
    for (const segment of at.dir.split('/').filter(Boolean)) {
      rel = rel ? `${rel}/${segment}` : segment;
      const seen = byPath.get(rel);
      if (seen) {
        here = seen;
        continue;
      }
      // 到顶了就不再往下建，更深的目录摊在这一级里
      if (here && here.depth >= MAX_FOLDER_DEPTH) break;
      const folder = newFolder(segment, here ? here.folder.id : null);
      here = { folder, depth: here ? here.depth + 1 : 1 };
      byPath.set(rel, here);
      folders.push(folder);
    }
    items.push({ ...at.media, folder: here ? here.folder.id : null });
  }
  return { folders, items };
}

/** 路径最后一段当文件夹名（素材库按包名建文件夹时用）。 */
function folderNameOf(path: string) {
  const clean = path.replace(/[\\/]+$/, '');
  const cut = Math.max(clean.lastIndexOf('\\'), clean.lastIndexOf('/'));
  return cut >= 0 ? clean.slice(cut + 1) : clean;
}

function newFolder(name: string, parent: string | null): DraftFolder {
  return { id: `f${Date.now().toString(16)}${Math.random().toString(16).slice(2, 6)}`, name, parent };
}

/** 空文件夹的默认名字按同一层里的已有数量编号（「文件夹 2」这种），建完可以在素材库里改名。 */
function nextFolderName(parent: string | null) {
  const base = t('editor-folder-name');
  const count = draft.value?.folders.filter((it) => (it.parent ?? null) === parent).length ?? 0;
  return `${base} ${count + 1}`;
}

/** 素材库的「新建文件夹」：在当前这一层建，先问名字，再建一个空的，拖素材行进来就归它管。 */
function createFolder(parent: string | null) {
  if (!draft.value) return;
  folderParent.value = parent;
  folderName.value = nextFolderName(parent);
  folderOpen.value = true;
}

async function confirmFolder() {
  if (!draft.value) return;
  const parent = folderParent.value;
  const name = folderName.value.trim() || nextFolderName(parent);
  folderOpen.value = false;
  const folder = newFolder(name, parent);
  draft.value = { ...draft.value, folders: [...draft.value.folders, folder] };
  await saveDraft();
}

/** 素材行上的路径是个链接：点它到资源管理器里定位**原文件**（草稿只记路径，不存副本）。 */
async function revealMedia(media: DraftMedia) {
  try {
    await revealItemInDir(media.path);
  } catch (e) {
    toastError(e);
  }
}

/** 把一行素材挪进某个文件夹（`null` 是挪回根上）。文件夹只是分组，不动磁盘。 */
async function moveMedia(media: DraftMedia, folder: string | null) {
  if (!draft.value) return;
  draft.value = {
    ...draft.value,
    media: draft.value.media.map((it) =>
      it.kind === media.kind && it.path === media.path ? { ...it, folder } : it,
    ),
  };
  await saveDraft();
}

async function renameFolder(folder: DraftFolder, name: string) {
  if (!draft.value) return;
  draft.value = {
    ...draft.value,
    folders: draft.value.folders.map((it) => (it.id === folder.id ? { ...it, name } : it)),
  };
  await saveDraft();
}

/**
 * 删文件夹连同它下面的子文件夹一起删：里面的素材条目也从素材库里去掉（磁盘上的原文件不动），
 * 轨道上正好用了被删掉的那份时，那条轨道也空出来。
 */
async function deleteFolder(folder: DraftFolder) {
  if (!draft.value) return;
  // 先把这一整枝的文件夹 id 收全（层级有上限，几轮就到底了）
  const ids = new Set([folder.id]);
  for (let grew = true; grew; ) {
    grew = false;
    for (const it of draft.value.folders) {
      if (!ids.has(it.id) && it.parent && ids.has(it.parent)) {
        ids.add(it.id);
        grew = true;
      }
    }
  }
  const gone = new Set(
    draft.value.media
      .filter((it) => it.folder && ids.has(it.folder))
      .map((it) => `${it.kind}:${it.path}`),
  );
  const keep = (it: DraftMedia | null) => !!it && !gone.has(`${it.kind}:${it.path}`);
  draft.value = {
    ...draft.value,
    folders: draft.value.folders.filter((it) => !ids.has(it.id)),
    media: draft.value.media.filter((it) => !it.folder || !ids.has(it.folder)),
    chart: keep(draft.value.chart) ? draft.value.chart : null,
    music: keep(draft.value.music) ? draft.value.music : null,
    cover: keep(draft.value.cover) ? draft.value.cover : null,
  };
  await saveDraft();
  if (!draft.value.chart) await loadChartInfo();
  if (!draft.value.cover) await loadCover();
}

async function addMedia(paths: string[]) {
  if (!draft.value || paths.length === 0) return;
  try {
    const found = (await invoke('import_editor_media', { paths })) as MediaInDir[];
    const { folders, items } = importTree(found, null);
    if (items.length === 0) return;
    mergeMedia(items, folders);
    await saveDraft();
  } catch (e) {
    toastError(e);
  }
}

/** 把新素材并进素材库，已经有的（种类 + 路径都一样）不重复收。 */
function mergeMedia(items: DraftMedia[], folders: DraftFolder[] = []) {
  if (!draft.value) return;
  const known = new Set(draft.value.media.map((it) => `${it.kind}:${it.path}`));
  const fresh = items.filter((it) => !known.has(`${it.kind}:${it.path}`));
  const knownFolders = new Set(draft.value.folders.map((it) => it.id));
  draft.value = {
    ...draft.value,
    media: [...draft.value.media, ...fresh],
    folders: [...draft.value.folders, ...folders.filter((it) => !knownFolders.has(it.id))],
  };
}

/**
 * 从素材库里删掉一项：轨道上要正好是它才一起摘下来（别的素材还占着轨道就只出库）。
 * 只是从草稿里去掉引用，磁盘上的文件一个字都不动。
 */
async function removeMedia(media: DraftMedia) {
  if (!draft.value) return;
  const next: EditorDraft = { ...draft.value };
  next.media = next.media.filter((it) => !(it.kind === media.kind && it.path === media.path));
  if (next.chart?.path === media.path && media.kind === 'chart') next.chart = null;
  if (next.music?.path === media.path && media.kind === 'music') next.music = null;
  if (next.cover?.path === media.path && media.kind === 'cover') next.cover = null;
  draft.value = next;
  await saveDraft();
  if (media.kind === 'chart') await loadChartInfo();
  if (media.kind === 'cover') await loadCover();
}

const renameOpen = ref(false);
const renameText = ref('');

function openRename() {
  renameText.value = draft.value?.name ?? '';
  renameOpen.value = true;
}

async function confirmRename() {
  if (!draft.value) return;
  const name = renameText.value.trim();
  if (!name) return;
  renameOpen.value = false;
  draft.value = { ...draft.value, name };
  await saveDraft();
}

const deleteOpen = ref(false);

/** 新建素材夹：先弹个输入框问名字，免得建出来一堆「文件夹 N」。 */
const folderOpen = ref(false);
const folderName = ref('');
/** 新文件夹建在哪一层（`null` 是「全部素材」底下）；由素材库当前所在的那层给。 */
const folderParent = ref<string | null>(null);

async function confirmDelete() {
  const id = draft.value?.id;
  if (!id) return;
  // 先停掉待写的那一次：草稿目录都要删了，再落盘会把它重新写出来
  cancelPendingSave();
  try {
    await invoke('delete_editor_draft', { id });
    deleteOpen.value = false;
    const rest = drafts.value.filter((it) => it.id !== id);
    if (rest.length > 0) await loadDraft(rest[0]!.id);
    else await createDraft();
  } catch (e) {
    toastError(e);
  }
}

async function openDraftsFolder() {
  try {
    await invoke('open_editor_drafts_folder');
  } catch (e) {
    toastError(e);
  }
}

/** 窗口起来时问后端「这次开哪份草稿」，没有就新建一份（正常由主窗口先建好）。 */
async function boot() {
  loading.value = true;
  try {
    const id = (await invoke('editor_context')) as string | null;
    if (id) await applyDraft((await invoke('load_editor_draft', { id })) as EditorDraft);
    else await createDraft();
  } catch (e) {
    toastError(e);
    draft.value = null;
  } finally {
    loading.value = false;
  }
}

const unlisteners: UnlistenFn[] = [];

/**
 * 播放头由引擎报时间：每 100ms 问一次预览区里的播放器（`DPRWPlayer.getTime()`），
 * 播放状态与总时长也从它那里同步过来。谱面时间本身由引擎的虚拟时钟推进。
 */
let ticker = 0;

function syncFromPlayer() {
  const player = preview.value;
  if (!player || !player.ready()) return;
  const current = player.getTime();
  if (Number.isFinite(current)) time.value = Math.min(Math.max(current, 0), duration.value);
  playing.value = !player.getPaused();
  const total = player.getDuration();
  if (Number.isFinite(total) && total > 0) duration.value = total;
}

onMounted(async () => {
  await boot();
  unlisteners.push(
    (await listen('editor-draft-changed', (event) => void loadDraft(event.payload as string))) as UnlistenFn,
  );
  // 系统拖动进窗口的文件（Tauri 会把它拦在自己手里，页面上拿不到 HTML5 的 drop）
  unlisteners.push(
    (await listen<FileDropEvent>('tauri://drag-drop', (event) => void dropFiles(event.payload.paths))) as UnlistenFn,
  );
  ticker = window.setInterval(syncFromPlayer, 100);
  window.addEventListener('keydown', onKeydown);
});

onUnmounted(() => {
  window.clearInterval(ticker);
  window.clearTimeout(infoTimer);
  window.removeEventListener('keydown', onKeydown);
  for (const unlisten of unlisteners) unlisten();
  unlisteners.length = 0;
});

/** 播放键：预览只要起来了就能走（谱面、曲绘都可以没有，音乐在就行）。 */
function togglePlay() {
  if (!preview.value?.ready()) return;
  playing.value = !playing.value;
  if (playing.value) preview.value?.play();
  else preview.value?.pause();
}

/** 时间轴上点一下 = 把播放头挪过去（拖动条也走这里）。 */
/** 时间轴总长（和时间轴、编辑栏共用 `timeline.ts` 那一份算法）。 */
const span = computed(() => timelineSpan(duration.value, draft.value?.lanes));

function seek(seconds: number) {
  time.value = Math.min(Math.max(seconds, 0), span.value);
  // 引擎只认音乐那一段：拖到曲子结束之后时，画面停在最后一帧
  preview.value?.seek(Math.min(time.value, duration.value));
}

/** 这个窗口没有系统标题栏（见后端建窗时的 `decorations(false)`），
 *  最小化 / 最大化 / 关闭由标题栏右边那三个按钮走这里。 */
const editorWindow = getCurrentWindow();

// 窗口标题跟着草稿走：任务栏 / Alt+Tab 里能看出正在编哪一份。
watch(
  () => draft.value,
  (next) => {
    const name = next?.info.name || next?.name || '';
    void editorWindow.setTitle(name ? `DPEditor — ${name}` : 'DPEditor');
  },
  { immediate: true },
);

function minimizeWindow() {
  void editorWindow.minimize();
}

function toggleMaximizeWindow() {
  void editorWindow.toggleMaximize();
}

async function closeWindow() {
  // 关之前把攒着的那次改动写掉，别让它随窗口一起丢
  await flushSave();
  void editorWindow.close();
}

/* ---- 键盘热键 ----
 *
 * 只在这个窗口里生效。文本输入框 / 下拉框里按下的键一律放行（该打字的打字、该撤文本的撤文本），
 * 其余按下面这张表走：播放、播放头微调、删片段、复制粘贴、撤销、对齐、缩放。
 */

/** 焦点是否在可输入控件里（这类控件自己处理键盘）。 */
function typingInField(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el) return false;
  return el.isContentEditable || el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT';
}

/** 选中的那段特效片段（批量选择时取主选中的那段）。 */
function currentClip(): { lane: DraftLane; clip: DraftClip } | null {
  const lane = draft.value?.lanes.find((it) => it.id === selectedTrack.value);
  const clip = findClip(lane, selectedClip.value);
  return lane && clip ? { lane, clip } : null;
}

/** 特效轨上还没用过的最小片段 id（`c1`、`c2`…）。 */
function nextClipId(lane: DraftLane): string {
  const used = new Set((lane.clips ?? []).map((it) => it.id));
  let index = used.size + 1;
  let id = `${lane.id}-c${index}`;
  while (used.has(id)) id = `${lane.id}-c${++index}`;
  return id;
}

/** 往一条特效轨上追加一段，选中它并落盘。 */
async function insertClip(lane: DraftLane, clip: DraftClip) {
  if (!draft.value) return;
  draft.value = {
    ...draft.value,
    lanes: draft.value.lanes.map((it) => (it.id === lane.id ? { ...it, clips: [...(it.clips ?? []), clip] } : it)),
  };
  selectTrack(lane.id, clip.id);
  await saveDraft();
}

/** 片段剪贴板：只存在这个窗口里（不借系统剪贴板，免得把谱面数据塞给别的程序）。 */
const clipClipboard = ref<{ length: number; effect: TextNoiseEffect } | null>(null);

function cloneEffect(effect: TextNoiseEffect): TextNoiseEffect {
  return JSON.parse(JSON.stringify(effect)) as TextNoiseEffect;
}

function copyClip() {
  const hit = currentClip();
  if (!hit?.clip.effect) return;
  clipClipboard.value = { length: hit.clip.length, effect: cloneEffect(hit.clip.effect) };
}

/** Ctrl+D：复制选中的那段到它后面，接着可以继续按（一段接一段排下去）。 */
async function duplicateClip() {
  const hit = currentClip();
  if (!hit?.clip.effect) return;
  await insertClip(hit.lane, {
    id: nextClipId(hit.lane),
    start: hit.clip.start + hit.clip.length,
    length: hit.clip.length,
    effect: cloneEffect(hit.clip.effect),
  });
}

/**
 * Ctrl+V：粘到播放头那一刻。
 *
 * 落点按「选中的特效轨 → 播放头下正放着的特效轨 → 最上面那条特效轨」挑：先选后粘是常规操作，
 * 没选时粘回播放头下那条上，才符合「接着当前画面往下做」的直觉。
 */
async function pasteClip() {
  const data = clipClipboard.value;
  if (!draft.value) return;
  const lanes = draft.value.lanes.filter((it) => it.kind === 'effect');
  const at = time.value;
  const lane =
    lanes.find((it) => it.id === selectedTrack.value) ??
    lanes.find((it) => clipsOf(it).some((clip) => at >= clip.start && at < clip.start + clip.length)) ??
    lanes[0];
  if (!lane) return;
  await insertClip(lane, {
    id: nextClipId(lane),
    start: Math.max(0, snapTime(time.value)),
    length: data?.length ?? 10,
    effect: cloneEffect(data?.effect ?? defaultTextNoise()),
  });
}

/** Delete / Backspace：有选中的片段就删片段，否则删选中的自加轨道。 */
async function deleteSelected() {
  if (selectedClip.value && selectedTrack.value) {
    await removeClip(selectedTrack.value, selectedClip.value);
    return;
  }
  if (selectedTrack.value && !isMainTrack(selectedTrack.value)) {
    await removeLane(selectedTrack.value);
  }
}

/** `[`：把选中片段的起点对到播放头（长度不变）。 */
async function alignClipStart() {
  const hit = currentClip();
  if (!hit) return;
  await updateClip(hit.lane.id, hit.clip.id, { start: Math.max(0, snapTime(time.value)) });
}

/** `]`：把选中片段的终点对到播放头（至少留 0.1 秒）。 */
async function alignClipEnd() {
  const hit = currentClip();
  if (!hit) return;
  const length = Math.max(0.1, snapTime(time.value) - hit.clip.start);
  await updateClip(hit.lane.id, hit.clip.id, { length });
}

/** 热键总表：输入框放行先处理，其余按键按组合分发。 */
function onKeydown(event: KeyboardEvent) {
  if (typingInField(event.target)) return;

  const mod = event.ctrlKey || event.metaKey;
  const key = event.key.toLowerCase();

  if (mod && key === 'z') {
    event.preventDefault();
    if (event.shiftKey) redo();
    else undo();
    return;
  }
  if (mod && key === 'y') {
    event.preventDefault();
    redo();
    return;
  }
  if (mod && key === 'c') {
    copyClip();
    return;
  }
  if (mod && key === 'v') {
    event.preventDefault();
    void pasteClip();
    return;
  }
  if (mod && key === 'd') {
    event.preventDefault();
    void duplicateClip();
    return;
  }
  if (mod && (event.key === '=' || event.key === '+')) {
    event.preventDefault();
    timeline.value?.zoomIn();
    return;
  }
  if (mod && event.key === '-') {
    event.preventDefault();
    timeline.value?.zoomOut();
    return;
  }

  if (event.key === ' ') {
    event.preventDefault();
    togglePlay();
    return;
  }
  if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
    event.preventDefault();
    const delta = (event.shiftKey ? 1 : 0.1) * (event.key === 'ArrowLeft' ? -1 : 1);
    seek(snapTime(time.value + delta));
    return;
  }
  if (event.key === 'Delete' || event.key === 'Backspace') {
    event.preventDefault();
    void deleteSelected();
    return;
  }
  if (event.key === '[') {
    void alignClipStart();
    return;
  }
  if (event.key === ']') {
    void alignClipEnd();
  }
}
</script>

<template>
  <div class="editor-window">
    <!-- 标题栏：窗口没有系统标题栏（和主窗口一样由页面自己画），这条同时是拖拽区。
         左边是 DPEditor，接着是草稿名（点开是草稿菜单），右边依次是谱面路径、导出与窗口按钮。 -->
    <header class="editor-titlebar" data-tauri-drag-region>
      <div class="editor-titlebar__brand" data-tauri-drag-region>
        <img src="../assets/favicon.png" alt="icon" class="editor-titlebar__logo" />
        <span class="editor-titlebar__name">DPEditor</span>
      </div>

      <!-- 草稿名 = 这个窗口的「项目名」，点开换草稿 / 新建 / 改名 / 删 / 开草稿夹 -->
      <v-menu location="bottom start" offset="6">
        <template #activator="{ props }">
          <button class="editor-draft" type="button" v-bind="props" :title="t('editor-drafts')">
            <span class="editor-draft__name">{{ draft?.name ?? t('editor-no-draft') }}</span>
            <i class="mdi mdi-menu-down editor-draft__arrow"></i>
          </button>
        </template>
        <v-sheet class="draft-menu">
          <div class="draft-menu__head">
            <span class="draft-menu__title">{{ t('editor-drafts') }}</span>
            <button class="draft-menu__action" type="button" @click="createDraft">
              <i class="mdi mdi-plus"></i>
              {{ t('editor-draft-new') }}
            </button>
          </div>
          <div class="draft-menu__list">
            <button
              v-for="item in drafts"
              :key="item.id"
              class="draft-menu__item"
              :class="{ 'draft-menu__item--active': item.id === draft?.id }"
              type="button"
              @click="switchDraft(item.id)"
            >
              <span class="draft-menu__name">{{ item.name }}</span>
              <span class="draft-menu__time">{{ formatUpdated(item.updated) }}</span>
            </button>
            <p v-if="drafts.length === 0" class="draft-menu__empty">{{ t('editor-draft-empty') }}</p>
          </div>
          <div class="draft-menu__foot">
            <button class="draft-menu__action" type="button" @click="openRename">
              <i class="mdi mdi-rename-outline"></i>
              {{ t('editor-draft-rename') }}
            </button>
            <button class="draft-menu__action" type="button" @click="openDraftsFolder">
              <i class="mdi mdi-folder-open-outline"></i>
              {{ t('editor-open-drafts-folder') }}
            </button>
            <button
              class="draft-menu__action draft-menu__action--danger"
              type="button"
              @click="deleteOpen = true"
            >
              <i class="mdi mdi-trash-can-outline"></i>
              {{ t('editor-draft-delete') }}
            </button>
          </div>
        </v-sheet>
      </v-menu>

      <div class="editor-titlebar__spacer" data-tauri-drag-region></div>
      <div class="editor-head__path" :title="draft?.dir">{{ draft?.dir }}</div>
      <div class="editor-head__actions">
        <!-- 草稿随改随存，没有「保存」这一步；导出要音乐和曲名都齐了才点得动 -->
        <v-btn
          variant="text"
          size="small"
          prepend-icon="mdi-export-variant"
          :loading="exporting"
          :disabled="!canExport"
          :title="canExport ? t('editor-export-tip') : t('editor-export-blocked')"
          @click="exportDraft"
        >
          {{ t('editor-export') }}
        </v-btn>
      </div>
      <div class="editor-titlebar__controls">
        <button class="editor-winbtn" type="button" :title="t('minimize')" @click="minimizeWindow">
          <i class="mdi mdi-window-minimize"></i>
        </button>
        <button class="editor-winbtn" type="button" :title="t('maximize')" @click="toggleMaximizeWindow">
          <i class="mdi mdi-window-maximize"></i>
        </button>
        <button class="editor-winbtn editor-winbtn--close" type="button" :title="t('close')" @click="closeWindow">
          <i class="mdi mdi-window-close"></i>
        </button>
      </div>
    </header>

    <div class="editor-body">
      <!-- 左边：功能栏（信息 / 素材库 / 效果）。宽度由它右边那条分隔条拖 -->
      <EditorInspector
        ref="inspector"
        class="editor-body__inspector"
        :style="{ width: `${inspectorWidth}px` }"
        :draft="draft"
        :info="chartInfo"
        :cover-url="coverUrl"
        @import="importMedia"
        @import-folder="importFolder"
        @add-folder="createFolder"
        @assign="assignMedia"
        @remove="removeMedia"
        @clear="clearTrack"
        @update-info="onUpdateInfo"
        @move-media="moveMedia"
        @rename-folder="renameFolder"
        @delete-folder="deleteFolder"
        @reveal="revealMedia"
        @add-effect="addLane('effect')"
      />
      <div
        class="editor-splitter editor-splitter--v"
        @pointerdown="startDrag('x', $event, 'inspector')"
        @pointermove="dragTo"
        @pointerup="endDrag"
        @pointercancel="endDrag"
      ></div>

      <!-- 中间：实时谱面预览 -->
      <EditorPreview
        ref="preview"
        class="editor-body__preview"
        :chart-path="chartPath"
        :music-path="draft?.music?.path ?? ''"
        :cover-path="draft?.cover?.path ?? ''"
        :info="draft?.info ?? null"
        :block-areas="blockAreas"
      />

      <!-- 右边：编辑栏 —— 时间轴上选中哪条轨道，就在这里编辑那份素材 -->
      <div
        class="editor-splitter editor-splitter--v"
        @pointerdown="startDrag('x', $event, 'edit')"
        @pointermove="dragTo"
        @pointerup="endDrag"
        @pointercancel="endDrag"
      ></div>
      <EditorEditBar
        class="editor-body__editbar"
        :style="{ width: `${editWidth}px` }"
        :draft="draft"
        :track-id="selectedTrack"
        :clip-id="selectedClip"
        :multi="multiClips"
        :duration="duration"
        :cover-url="coverUrl"
        @reveal="revealMedia"
        @clear="clearTrack"
        @pick-media="pickMediaForTrack"
        @update-lane="updateLane"
        @update-clip="updateClip(selectedTrack ?? '', selectedClip ?? '', $event)"
        @update-clips="updateClips"
        @update-clips-time="updateClipsTime"
      />
    </div>

    <!-- 主体与时间轴之间的分隔条：上下拖改时间轴高度 -->
    <div
      class="editor-splitter editor-splitter--h"
      @pointerdown="startDrag('y', $event)"
      @pointermove="dragTo"
      @pointerup="endDrag"
      @pointercancel="endDrag"
    ></div>

    <!-- 底部：进度条 + 时间轴（素材从功能栏拖到这里的轨道上） -->
    <EditorTimeline
      ref="timeline"
      class="editor-footer"
      :style="{ height: `${timelineHeight}px` }"
      :draft="draft"
      :playing="playing"
      :time="time"
      :duration="duration"
      :selected="selectedTrack"
      :selected-clip="selectedClip"
      :multi="multiClips"
      @toggle-play="togglePlay"
      @seek="seek"
      @assign="assignMedia"
      @clear="clearTrack"
      @select="selectClipEvent"
      @select-many="selectManyClips"
      @remove-lane="removeLane"
      @drop-effect="addEffectAt"
      @drop-lyrics="addLyrics"
      @resize-lane="resizeLane"
      @resize-clip="resizeClip"
      @move-clip="moveClip"
      @remove-clip="removeClip"
      @add-clip="addClip"
    />

    <!-- 重命名草稿 -->
    <v-dialog v-model="renameOpen" width="auto" min-width="400px" class="log-card-bg">
      <v-card class="log-card-only-window">
        <v-card-title>{{ t('editor-draft-rename') }}</v-card-title>
        <v-card-text>
          <v-text-field
            v-model="renameText"
            density="compact"
            variant="outlined"
            hide-details
            autofocus
            @keydown.enter="confirmRename"
          />
        </v-card-text>
        <v-card-actions class="justify-end">
          <v-btn class="hover-scale" variant="text" @click="renameOpen = false">{{ t('cancel') }}</v-btn>
          <v-btn class="hover-scale" variant="text" color="btn" @click="confirmRename">
            {{ t('confirm') }}
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>

    <!-- 删草稿：只删草稿记录，素材原文件不动 -->
    <v-dialog v-model="deleteOpen" width="auto" min-width="400px" class="log-card-bg">
      <v-card class="log-card-only-window">
        <v-card-title>{{ t('editor-draft-delete') }}</v-card-title>
        <v-card-text>
          <div class="editor-dialog__text">{{ t('editor-draft-delete-tip') }}</div>
        </v-card-text>
        <v-card-actions class="justify-end">
          <v-btn class="hover-scale" variant="text" @click="deleteOpen = false">{{ t('cancel') }}</v-btn>
          <v-btn class="hover-scale" variant="text" @click="confirmDelete">
            {{ t('editor-draft-delete') }}
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>

    <!-- 新建素材夹：先问名字 -->
    <v-dialog v-model="folderOpen" width="auto" min-width="400px" class="log-card-bg">
      <v-card class="log-card-only-window">
        <v-card-title>{{ t('editor-add-folder') }}</v-card-title>
        <v-card-text>
          <v-text-field
            v-model="folderName"
            density="compact"
            variant="outlined"
            hide-details
            autofocus
            :placeholder="t('editor-folder-name')"
            @keydown.enter="confirmFolder"
          />
        </v-card-text>
        <v-card-actions class="justify-end">
          <v-btn class="hover-scale" variant="text" @click="folderOpen = false">{{ t('cancel') }}</v-btn>
          <v-btn class="hover-scale" variant="text" color="btn" @click="confirmFolder">
            {{ t('confirm') }}
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>

    <!-- 提示浮层：编辑窗口是独立入口，主界面的 `<v-sonner>` 在这里不存在 -->
    <v-sonner position="top-center" />
  </div>
</template>

<style scoped>
/* 整窗就是一条纵向的三段：顶栏 / 主体 / 时间轴。主体自己再横着切成预览与处理区。 */
.editor-window {
  display: flex;
  flex-direction: column;
  height: 100vh;
  overflow: hidden;
  background: var(--phi-bg-page);
  color: var(--phi-label-primary);
}

/* 标题栏：尺寸、底色、字号都照主窗口那条 `.phi-titlebar`，两个窗口看起来才是一套。
   整条可拖拽（`data-tauri-drag-region`），里面的按钮不算拖拽区。 */
.editor-titlebar {
  display: flex;
  flex: none;
  align-items: center;
  gap: var(--phi-space-3);
  height: var(--phi-titlebar-height);
  padding: 0 var(--phi-space-2) 0 var(--phi-space-4);
  background: var(--phi-nav-fill);
}

*[data-tauri-drag-region] {
  app-region: drag;
}

/* 同上：拖拽区里的草稿下拉与窗口按钮要标 no-drag，否则点上去是抓窗口 */
.editor-draft,
.editor-titlebar__controls {
  app-region: no-drag;
}

.editor-titlebar__brand {
  display: flex;
  flex: none;
  gap: var(--phi-space-2);
  align-items: center;
}

.editor-titlebar__logo {
  width: 18px;
  height: 18px;
}

.editor-titlebar__name {
  font-size: var(--phi-font-size-secondary);
  font-weight: 600;
  color: var(--phi-label-primary);
  letter-spacing: 0.02em;
}

/* 草稿名：标题栏上的一个下拉入口，和品牌名之间用一条淡竖线分开 */
.editor-draft {
  display: flex;
  flex: none;
  gap: var(--phi-space-2);
  align-items: center;
  max-width: 260px;
  padding: 2px var(--phi-space-2);
  margin-left: var(--phi-space-1);
  font-size: var(--phi-font-size-secondary);
  color: var(--phi-label-primary);
  cursor: pointer;
  background: transparent;
  border: 0.5px solid transparent;
  border-radius: var(--phi-radius-xs);
  transition:
    background-color var(--phi-duration) var(--phi-ease),
    border-color var(--phi-duration) var(--phi-ease);
}

.editor-draft:hover {
  background: var(--phi-interactive-hover);
  border-color: var(--phi-border-l3);
}

.editor-draft__name {
  overflow: hidden;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.editor-draft__arrow {
  flex: none;
  font-size: 14px;
  color: var(--phi-label-caption);
}

/* 草稿菜单：列表和底部三个动作 */
.draft-menu {
  display: flex;
  flex-direction: column;
  width: 264px;
  padding: var(--phi-space-1) 0;
  border: 0.5px solid var(--phi-border-l3);
}

.draft-menu__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 2px var(--phi-space-3) var(--phi-space-1);
}

.draft-menu__title {
  font-size: 0.7rem;
  font-weight: 600;
  color: var(--phi-label-caption);
  text-transform: uppercase;
  letter-spacing: 0.06em;
}

.draft-menu__list {
  max-height: 260px;
  overflow-y: auto;
}

.draft-menu__item {
  display: flex;
  gap: var(--phi-space-2);
  align-items: center;
  width: 100%;
  padding: 5px var(--phi-space-3);
  font-size: 0.78rem;
  color: var(--phi-label-secondary);
  text-align: left;
  cursor: pointer;
  background: transparent;
  border: none;
}

.draft-menu__item:hover {
  color: var(--phi-label-primary);
  background: var(--phi-interactive-hover);
}

.draft-menu__item--active {
  color: var(--phi-label-primary);
  background: color-mix(in srgb, var(--phi-accent) 14%, transparent);
}

.draft-menu__name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.draft-menu__time {
  flex: none;
  font-size: 0.68rem;
  color: var(--phi-label-caption);
  font-variant-numeric: tabular-nums;
}

.draft-menu__empty {
  padding: var(--phi-space-2) var(--phi-space-3);
  font-size: 0.72rem;
  color: var(--phi-label-caption);
}

.draft-menu__foot {
  display: flex;
  flex-direction: column;
  padding-top: var(--phi-space-1);
  margin-top: var(--phi-space-1);
  border-top: 0.5px solid var(--phi-border-l3);
}

.draft-menu__action {
  display: flex;
  gap: var(--phi-space-2);
  align-items: center;
  padding: 5px var(--phi-space-3);
  font-size: 0.76rem;
  color: var(--phi-label-secondary);
  cursor: pointer;
  background: transparent;
  border: none;
}

.draft-menu__action:hover {
  color: var(--phi-label-primary);
  background: var(--phi-interactive-hover);
}

.draft-menu__action i {
  font-size: 15px;
  color: var(--phi-label-caption);
}

.draft-menu__action--danger:hover {
  color: #fff;
  background: var(--phi-danger);
}

.draft-menu__action--danger:hover i {
  color: #fff;
}

/* 删除对话框里的那句说明：弹窗卡片本身用全局那套 `.log-card-only-window` */
.editor-dialog__text {
  font-size: 0.78rem;
  color: var(--phi-label-secondary);
}

.editor-titlebar__spacer {
  flex: 1;
  min-width: var(--phi-space-2);
  height: 100%;
}

.editor-titlebar__controls {
  display: flex;
  flex: none;
  gap: var(--phi-space-1);
  align-items: center;
}

.editor-winbtn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: var(--phi-nav-icon-button);
  height: var(--phi-nav-icon-button);
  font-size: 16px;
  color: var(--phi-label-secondary);
  cursor: pointer;
  background: transparent;
  border: none;
  border-radius: 0;
  transition:
    background-color var(--phi-duration) var(--phi-ease),
    color var(--phi-duration) var(--phi-ease);
}

.editor-winbtn:hover {
  color: var(--phi-label-primary);
  background: var(--phi-interactive-hover);
}

.editor-winbtn--close:hover {
  color: #fff;
  background: var(--phi-danger);
}

.editor-head__name {
  overflow: hidden;
  font-size: 0.9rem;
  font-weight: 600;
  color: var(--phi-label-primary);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.editor-head__diff {
  flex: none;
  padding: 1px 6px;
  font-size: 0.7rem;
  color: var(--phi-label-secondary);
  background: var(--phi-bg-layer-3);
  border-radius: var(--phi-radius-xs);
}

/* 路径是给人对位置用的，不抢标题的位置：占满中间、自己省略 */
.editor-head__path {
  flex: 0 1 auto;
  min-width: 0;
  overflow: hidden;
  font-size: 0.72rem;
  color: var(--phi-label-caption);
  text-align: right;
  text-overflow: ellipsis;
  white-space: nowrap;
  user-select: text;
}

.editor-head__actions {
  display: flex;
  flex: none;
  align-items: center;
  gap: var(--phi-space-1);
}

.editor-body {
  display: flex;
  flex: 1 1 auto;
  gap: 0;
  min-height: 0;
  padding: var(--phi-space-3);
  padding-bottom: 0;
}

.editor-body__preview {
  flex: 1 1 auto;
  min-width: 0;
}

/* 分隔条：视觉上只是一条细线，但整条 13px 都能抓（`touch-action: none` 免得触屏上被滚动手势吃掉）。 */
.editor-splitter {
  position: relative;
  flex: none;
  background: transparent;
  touch-action: none;
}

.editor-splitter::after {
  position: absolute;
  content: '';
  background: var(--phi-border-l3);
  border-radius: var(--phi-radius-pill);
  transition:
    background-color var(--phi-duration) var(--phi-ease),
    width var(--phi-duration) var(--phi-ease),
    height var(--phi-duration) var(--phi-ease);
}

.editor-splitter--v {
  width: 13px;
  cursor: col-resize;
}

.editor-splitter--v::after {
  top: var(--phi-space-2);
  bottom: var(--phi-space-2);
  left: 50%;
  width: 1px;
  transform: translateX(-50%);
}

.editor-splitter--h {
  height: 13px;
  cursor: row-resize;
}

.editor-splitter--h::after {
  top: 50%;
  right: var(--phi-space-3);
  left: var(--phi-space-3);
  height: 1px;
  transform: translateY(-50%);
}

.editor-splitter:hover::after {
  background: var(--phi-accent);
}

.editor-splitter--v:hover::after {
  width: 2px;
}

.editor-splitter--h:hover::after {
  height: 2px;
}

/* 两侧栏的宽度由 JS 写进 style，这里只负责「别被预览挤扁」 */
.editor-body__inspector,
.editor-body__editbar {
  flex: none;
}

.editor-footer {
  flex: none;
}
</style>

<style>
/* 提示浮层：编辑窗口是独立入口（根节点是 `.editor-window`，不是主界面那个 `#dpr`），
   主界面那套落在 `#dpr` 下的浮层样式选不到这里，所以单独摆一次。

   位置由 `<v-sonner position="top-center">` 定：浮层贴窗口顶部水平居中，单条提示从上方
   滑下来。这里只改宽度 —— 默认浮层整条按固定 `--width` 排，长路径会把提示撑成一行；
   改成宽度随内容走、最多到窗口的 95%，短提示就不会拖出一条长条。`translate` 单独用，
   不碰 sonner 拿 `transform: var(--y)` 做的进出场动画。 */
.editor-window [data-sonner-toaster] {
  left: 50% !important;
  right: auto !important;
  width: 95vw !important;
  transform: translateX(-50%);
}

.editor-window [data-sonner-toast] {
  left: 50% !important;
  right: auto !important;
  width: fit-content !important;
  max-width: 95vw;
  translate: -50% 0;
}
</style>
