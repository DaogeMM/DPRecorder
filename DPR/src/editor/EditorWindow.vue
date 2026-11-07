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
  DraftFolder,
  DraftInfo,
  DraftMedia,
  DraftMediaKind,
  EditorDraft,
  FileDropEvent,
  MediaInDir,
} from '@/model';
import { MAX_FOLDER_DEPTH } from '@/model';
import { toast, toastError } from '@/common';
import EditorPreview from './EditorPreview.vue';
import EditorInspector from './EditorInspector.vue';
import EditorTimeline from './EditorTimeline.vue';

/**
 * DPE「编辑窗口」的根组件 —— 独立窗口里只渲染它，不挂主界面的外壳（见 `main.ts`）。
 *
 * 这一版照剪映那样**按草稿**来：打开窗口面对的是一个草稿（`dpe/editor/drafts/<id>/draft.json`），
 * 不是某一张谱面。草稿里记三条轨道上放了什么（谱面 / 音乐 / 曲绘）和一份素材库，
 * 素材本身**只记原文件路径**、不复制，所以草稿只有几 KB，用户自己的文件保持原样。
 *
 * 版式照剪映那类剪辑器：左上角是谱面预览、右上角是处理区（素材库在这里）、底部是时间轴；
 * 把素材从处理区拖到时间轴对应的轨道上，就成了这张自定义谱面。
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

/** 播放状态、播放头与时长都由预览区里的播放器报回来（`DPRWPlayer` 的 `getTime` 等）。 */
const playing = ref(false);
const time = ref(0);
/** 时长：谱面本身不带总时长，播放器给不出来时先用一个占位值，进度条照样能画。 */
const duration = ref(180);

const preview = ref<InstanceType<typeof EditorPreview> | null>(null);

/**
 * 处理区宽度与时间轴高度可以拖分隔条改，记在本地（`useStorage`）：关掉窗口再开还是上次调好的尺寸。
 *
 * 上限都按窗口算而不是写死像素：预览区是 16:9 的，处理区被拖到太宽、时间轴被拖得太高，预览就只剩一条缝。
 */
const inspectorWidth = useStorage('DPEditor.inspectorWidth', 320);
const timelineHeight = useStorage('DPEditor.timelineHeight', 214);

const INSPECTOR_MIN = 260;
/** 处理区最多占窗口宽度的一半：四列素材卡宽一点更好用，但也别把预览挤没。 */
const INSPECTOR_MAX_RATIO = 0.5;
const TIMELINE_MIN = 140;
/** 时间轴最多占窗口高度的六成。 */
const TIMELINE_MAX_RATIO = 0.6;

let dragging: { axis: 'x' | 'y'; from: number; start: number } | null = null;

/**
 * 按下分隔条开始拖：用 pointer capture 把后续 move 都收在这个元素上，
 * 这样指针滑出那条细线也不会断（松手在窗口外也能收到 up）。
 */
function startDrag(axis: 'x' | 'y', event: PointerEvent) {
  // 合成事件（自动化里按坐标点）没有真实指针，捕获会抛 NotFoundError，忽略即可。
  try {
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  } catch {
    /* 没有指针可捕获：不影响拖动本身 */
  }
  dragging = {
    axis,
    from: axis === 'x' ? event.clientX : event.clientY,
    start: axis === 'x' ? inspectorWidth.value : timelineHeight.value,
  };
  event.preventDefault();
}

/** 处理区在右侧、时间轴在底部，所以两个方向都是「往回拖变大」：用起点减位移。 */
function dragTo(event: PointerEvent) {
  if (!dragging) return;
  const { axis, from, start } = dragging;
  if (axis === 'x') {
    const next = start - (event.clientX - from);
    const max = Math.round(window.innerWidth * INSPECTOR_MAX_RATIO);
    inspectorWidth.value = Math.round(Math.min(max, Math.max(INSPECTOR_MIN, next)));
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
  }
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
}

async function loadDraft(id: string) {
  loading.value = true;
  try {
    await applyDraft((await invoke('load_editor_draft', { id })) as EditorDraft);
  } catch (e) {
    toastError(e);
    draft.value = null;
  } finally {
    loading.value = false;
  }
}

/** 保存当前草稿（轨道上放了什么、名字改没改都在里面）。 */
async function saveDraft() {
  if (!draft.value) return;
  try {
    draft.value = (await invoke('save_editor_draft', { draft: draft.value })) as EditorDraft;
    await refreshDrafts();
  } catch (e) {
    toastError(e);
  }
}

async function createDraft() {
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
    const saved = (await invoke('export_editor_draft', { draft: draft.value })) as string;
    toast(t('editor-export-done', { path: saved }), 'success');
  } catch (e) {
    toastError(e);
  } finally {
    exporting.value = false;
  }
}

async function switchDraft(id: string) {
  if (id === draft.value?.id) return;
  await loadDraft(id);
}

/** 把一项素材放到它那条轨道上（从素材库拖过来，或点素材库上的「放到轨道」）。
 *
 * `track` 是松手时指针压着的那条轨道：拖错类型（比如把歌拖到谱面轨道）不会落上去，
 * 也不弹提示 —— 拖的时候那条轨道本来就不亮，松手当没发生。
 *
 * 只放这一项：谱面包里同目录的音乐与曲绘不跟着上轨道，用哪首 / 哪张由用户自己挑。
 */
async function assignMedia(media: DraftMedia, track?: DraftMediaKind) {
  if (!draft.value) return;
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

/** 把某项从轨道上拿下来（素材还留在素材库里）。 */
async function clearTrack(kind: DraftMediaKind) {
  if (!draft.value) return;
  const next: EditorDraft = { ...draft.value };
  if (kind === 'chart') next.chart = null;
  else if (kind === 'music') next.music = null;
  else next.cover = null;
  draft.value = next;
  await saveDraft();
  if (kind === 'chart') await loadChartInfo();
  if (kind === 'cover') await loadCover();
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
    name: '谱面 / 音频 / 曲绘',
    extensions: [
      'pez', 'zip', 'mc', 'pec', 'json', 'yaml', 'yml',
      'mp3', 'ogg', 'wav', 'flac', 'm4a', 'aac', 'opus',
      'png', 'jpg', 'jpeg', 'webp', 'bmp', 'gif',
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
function seek(seconds: number) {
  time.value = Math.min(Math.max(seconds, 0), duration.value);
  preview.value?.seek(time.value);
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

function closeWindow() {
  void editorWindow.close();
}

/**
 * 预览全屏：窗口进系统全屏，编辑窗口只剩预览 —— 标题栏、处理区、时间轴一起收起来，
 * 退出按钮仍是预览工具条上那颗（`EditorPreview` 按 `fullscreen` 换图标）。
 *
 * 走窗口全屏而不是 DOM 的 `requestFullscreen()`：WebView2 里 DOM 全屏只是把元素撑满这个
 * webview，窗口本身还是原来那么大一块，看着不叫全屏。
 */
const previewFullscreen = ref(false);

async function togglePreviewFullscreen() {
  previewFullscreen.value = !previewFullscreen.value;
  try {
    await editorWindow.setFullscreen(previewFullscreen.value);
  } catch (err) {
    toastError(err);
  }
}

/** 窗口全屏不会自己退：全屏时按 Esc 退出（和别的播放器一个习惯）。 */
function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape' && previewFullscreen.value) {
    void togglePreviewFullscreen();
  }
}
</script>

<template>
  <div class="editor-window" :class="{ 'editor-window--focus': previewFullscreen }">
    <!-- 标题栏：窗口没有系统标题栏（和主窗口一样由页面自己画），这条同时是拖拽区。
         左边是 DPEditor，接着是草稿名（点开是草稿菜单），右边依次是谱面路径、导出与窗口按钮。 -->
    <header v-if="!previewFullscreen" class="editor-titlebar" data-tauri-drag-region>
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
      <!-- 左上角：实时谱面预览 -->
      <EditorPreview
        ref="preview"
        class="editor-body__preview"
        :chart-path="chartPath"
        :music-path="draft?.music?.path ?? ''"
        :cover-path="draft?.cover?.path ?? ''"
        :fullscreen="previewFullscreen"
        @toggle-fullscreen="togglePreviewFullscreen"
      />
      <!-- 右上角：处理区（宽度由左边那条分隔条拖）。全屏预览时连分隔条一起收起来 -->
      <template v-if="!previewFullscreen">
        <div
          class="editor-splitter editor-splitter--v"
          @pointerdown="startDrag('x', $event)"
          @pointermove="dragTo"
          @pointerup="endDrag"
          @pointercancel="endDrag"
        ></div>
        <EditorInspector
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
        />
      </template>
    </div>

    <template v-if="!previewFullscreen">
      <!-- 主体与时间轴之间的分隔条：上下拖改时间轴高度 -->
      <div
        class="editor-splitter editor-splitter--h"
        @pointerdown="startDrag('y', $event)"
        @pointermove="dragTo"
        @pointerup="endDrag"
        @pointercancel="endDrag"
      ></div>

      <!-- 底部：进度条 + 时间轴（素材从处理区拖到这里的轨道上） -->
      <EditorTimeline
        class="editor-footer"
        :style="{ height: `${timelineHeight}px` }"
        :draft="draft"
        :playing="playing"
        :time="time"
        :duration="duration"
        @toggle-play="togglePlay"
        @seek="seek"
        @assign="assignMedia"
        @clear="clearTrack"
      />
    </template>

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

.editor-body__inspector {
  flex: none;
}

.editor-footer {
  flex: none;
}

/* 预览全屏：编辑窗口只剩预览，内边距与描边一并去掉，画面贴着屏幕边 */
.editor-window--focus .editor-body {
  padding: 0;
}

.editor-window--focus .editor-body__preview {
  border: 0;
  border-radius: 0;
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
