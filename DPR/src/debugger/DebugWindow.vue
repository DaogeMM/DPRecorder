<script setup lang="ts">
/**
 * DPDebugger 窗口：调「谱面演出动画」的参数。
 *
 * 左边是实时预览，右边是这一张谱面用得上的那组参数（与 `assets/chart-animation.toml` 的
 * 一张表一一对应）。改一个数字，左边立刻按新值重画，同时把整份配置回传给主窗口 ——
 * 窗口里调的就是渲染页那份配置，关掉之后预览 / 渲染直接用调好的值。
 *
 * 预览不是另开进程，而是把引擎编成的网页版（`public/player/`）装进 iframe：
 * 谱面字节由后端读出来后挂在 `window.__dprwPendingChart`，配置挂在 `__dprwPendingConfig`，
 * 播放页起来时自己取走（与 DPEditor 的预览同一套做法）。
 *
 * 参数名不放进 i18n：它们直接对应 `RenderConfig` 的字段与 TOML 的键，属于调试用的技术名。
 */
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { useStorage } from '@vueuse/core';

import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open as openDialog } from '@tauri-apps/plugin-dialog';

import { DEFAULT_RENDER_CONFIG, type RenderConfig } from '../model';
import { chartAnimationFamily } from '../appendant';
import { formatTimecode, parseTimecode, secondsFromText } from '../timecode';
import { FAMILIES, type FamilySpec, type FieldSpec } from './families';

const { t } = useI18n();

/** 后端 `debugger::DebugContext`：调试哪张谱面，以及主窗口那份配置。 */
interface DebugContext {
  path: string;
  config: RenderConfig;
}

/** 后端 `debugger::DebugChartFiles`：包内文件名 + 谱面信息里那几行字。 */
interface ChartFiles {
  chart: string;
  music: string;
  illustration: string;
  name: string;
  level: string;
  charter: string;
  composer: string;
}

/** iframe 里的播放页暴露的接口（`public/player/player-api.js` 的 `window.DPRWPlayer`）。 */
type PlayerHandle = {
  ready: () => boolean;
  play: () => void;
  pause: () => void;
  seek: (seconds: number) => void;
  getTime: () => number;
  getPaused: () => boolean;
  getDuration: () => number;
  setVolume?: (volume: { music?: number; sfx?: number }) => void;
  setConfig?: (json: string) => void;
};

/** 后端 `render::DebugPreviewPack`：一个要摊进网页版引擎的附带资源包。 */
interface PreviewPack {
  name: string;
  prefix: string;
  files: string[];
  present: boolean;
}

/** 后端 `render::DebugPreview`：引擎配置 JSON + 强开过族开关的配置 + 要摊的资源包。 */
interface PreviewConfig {
  config: string;
  render: Record<string, unknown>;
  packs: PreviewPack[];
}

// ── 窗口状态 ────────────────────────────────────────────────────────────────

const chartPath = ref<string | null>(null);
const loading = ref(true);
const failure = ref('');

/** 窗口里编辑的就是渲染页那份配置：起步是主窗口传过来的，改动实时回传回去。 */
const config = reactive<Record<string, unknown>>({ ...DEFAULT_RENDER_CONFIG });

const family = computed(() => chartAnimationFamily(chartPath.value));
const familySpec = computed<FamilySpec | null>(() => (family.value ? FAMILIES[family.value] : null));

const chartLabel = computed(() => {
  const path = chartPath.value;
  if (!path) return '';
  return path.replace(/\\/g, '/').split('/').pop() ?? '';
});

/** 谱面自带的难度文本（`info.txt` 的 `Level:`，例如 `SP Lv.?`）：标题栏的灰色胶囊显示它。 */
const chartDifficulty = ref('');

const frame = ref<HTMLIFrameElement | null>(null);
const playerSrc = ref('');
const ready = ref(false);
const playing = ref(false);
const time = ref(0);
const duration = ref(0);

// ── 预览画面（与 DPEditor 的 `EditorPreview` 同一套做法）────────────────────

/** 两栏画面都按 16:9 摆：谱面本身是这个比例，对比视频框成同一个形状才好逐帧看差别。 */
const SCREEN_RATIO = 16 / 9;

/**
 * 画面按容器的实际可用区域算：宽高哪一头先顶到就按哪一头收，另一头等比缩。
 *
 * 只靠 `aspect-ratio` 做不到「同时受宽高约束」——`width:100%` 配 `max-height:100%` 在容器偏矮时
 * 是先把高度截住、宽度照样撑满，画面会被拉扁。窗口可以拖到任意大小，所以这里直接量。
 */
const stage = ref<HTMLElement | null>(null);
const screenStyle = ref<Record<string, string>>({});
let stageObserver: ResizeObserver | null = null;

/** 量一块舞台能放下的 16:9 尺寸；量不出来（还没挂载 / 被压成 0 宽）就原样返回空。 */
function boxStyle(el: HTMLElement | null): Record<string, string> {
  if (!el) return {};
  const style = getComputedStyle(el);
  const padX = (parseFloat(style.paddingLeft) || 0) + (parseFloat(style.paddingRight) || 0);
  const padY = (parseFloat(style.paddingTop) || 0) + (parseFloat(style.paddingBottom) || 0);
  const availW = el.clientWidth - padX;
  const availH = el.clientHeight - padY;
  // `!(x > 0)` 同时挡住 0 与 NaN：算出 `NaNpx` 会让行内宽高整条失效，元素退回自身的固有尺寸
  // （视频就是源文件那一千多像素），直接撑破整栏。
  if (!(availW > 0) || !(availH > 0)) return {};
  const width = Math.floor(Math.min(availW, availH * SCREEN_RATIO));
  if (!(width > 0)) return {};
  return { width: `${width}px`, height: `${Math.floor(width / SCREEN_RATIO)}px` };
}

function fitScreen() {
  screenStyle.value = boxStyle(stage.value);
}

// ── 视频对比 ────────────────────────────────────────────────────────────────

/** 右上角那栏：打开一个本地视频，播放 / 暂停 / 拖动都跟着左边的预览走。 */
const videoStage = ref<HTMLElement | null>(null);
const videoEl = ref<HTMLVideoElement | null>(null);
const videoStyle = ref<Record<string, string>>({});
const videoPath = useStorage('DPDebugger.videoPath', '');
const videoSrc = ref('');
const videoError = ref('');
const videoMuted = useStorage('DPDebugger.videoMuted', true);
const videoVolume = useStorage('DPDebugger.videoVolume', 100);
/** 同步偏移（毫秒）：`视频时间 = 预览时间 + 偏移`。片头对不上时用它抹平。 */
const videoOffset = useStorage('DPDebugger.videoOffset', 0);

const VIDEO_FILTERS = [{ name: '视频', extensions: ['mp4', 'webm', 'mkv', 'mov', 'm4v', 'avi'] }];

const videoName = computed(() => videoPath.value.replace(/\\/g, '/').split('/').pop() ?? '');

function fitVideo() {
  videoStyle.value = boxStyle(videoStage.value);
}

/** 两栏一起重算：分隔条拖完、窗口换尺寸都走这里。 */
function fitAll() {
  fitScreen();
  fitVideo();
}

/**
 * 音量滑条（0~100）。打开窗口时跟渲染配置里的 `volume_music` / `volume_sfx` 起 ——
 * 配置写着 0%（成片不要按键音）时，预览也不该还在响。
 * 之后拖动的只是这份预览，不写回配置：为了听清某个细节去拉音量，不该改掉成片的输出音量。
 */
const volumeMusic = ref(50);
const volumeSfx = ref(40);

/** 把配置里的音量搬到滑条上；滑条之间的比例换算都在百分比这一层。 */
function adoptConfigVolume() {
  const music = Number(config.volumeMusic);
  const sfx = Number(config.volumeSfx);
  volumeMusic.value = Math.round((Number.isFinite(music) ? music : 0.5) * 100);
  volumeSfx.value = Math.round((Number.isFinite(sfx) ? sfx : 0.4) * 100);
}

const volumeIcon = computed(() => {
  const top = Math.max(volumeMusic.value, volumeSfx.value);
  if (top <= 0) return 'mdi-volume-off';
  return top <= 50 ? 'mdi-volume-low' : 'mdi-volume-high';
});

/**
 * 把音量推给播放页：音乐换混音器放大器、音效改 `config.volume_sfx`，都是当场生效。
 *
 * 音乐不按播放态归零：暂停时引擎自己把音乐停在原处，这里再补一次 0 会在「点播放那一帧
 * 引擎还没换档」时把音乐永久按在 0——表现为只剩按键音。
 */
function applyVolume() {
  player()?.setVolume?.({
    music: volumeMusic.value / 100,
    sfx: volumeSfx.value / 100,
  });
}

watch([volumeMusic, volumeSfx], applyVolume);

/** 播放页起好了：音量按滑条推一遍，顺手把方向键挂到子窗口上（点过画面之后焦点在里头）。 */
function onFrameLoad() {
  applyVolume();
  bindFrameKeys();
}

/** 左侧预览面板：分隔条算宽度、拖拽都挂在这块上。 */
const previewPanel = ref<HTMLElement | null>(null);

/** 点当前时间就地改：调间隔、淡出这类时间参数时要能反复对位。 */
const editingTime = ref(false);
const timeInput = ref<HTMLInputElement | null>(null);
const timeDraft = ref('');

async function beginTimeEdit() {
  if (!ready.value) return;
  timeDraft.value = formatTime(time.value);
  editingTime.value = true;
  await nextTick();
  const el = timeInput.value;
  if (!el) return;
  el.focus();
  // 光标落在末尾，不整段选中：点开就是刚才那串字，改成别的时间直接删着改
  el.setSelectionRange(el.value.length, el.value.length);
}

function commitTimeEdit() {
  if (!editingTime.value) return;
  const seconds = parseTimecode(timeDraft.value);
  editingTime.value = false;
  if (seconds == null) return;
  seekTo(Math.max(0, Math.min(duration.value > 0 ? duration.value : seconds, seconds)));
}

function cancelTimeEdit() {
  editingTime.value = false;
}

// ── 视频对比 ────────────────────────────────────────────────────────────────

async function openVideo() {
  try {
    const picked = await openDialog({
      multiple: false,
      directory: false,
      title: t('debug-video-open'),
      // 上次那个视频还在就开在它旁边：对比同一段素材时少翻一遍目录
      defaultPath: videoPath.value || undefined,
      filters: VIDEO_FILTERS,
    });
    if (!picked || Array.isArray(picked)) return;
    attachVideo(picked);
  } catch (err) {
    // 选择框打不开是权限 / 环境问题，原始错误写进控制台，界面上只提示一句人话
    console.error('open video dialog failed', err);
    videoError.value = t('debug-video-dialog-failed');
  }
}

/** 把视频挂上去：本地路径交给 asset 协议，WebView 才允许 `<video>` 读它。 */
function attachVideo(path: string) {
  videoPath.value = path;
  videoError.value = '';
  videoBlocked.value = false;
  videoStall = 0;
  videoSrc.value = convertFileSrc(path);
}

/** 浏览器挡掉了自动播放（视频没静音时常见）：等用户在画面上点一下，别每帧重试。 */
const videoBlocked = ref(false);

/** 挂上去之后连续多少帧连元数据都没读到；够多就认定「这个文件播不了」，在界面上说清楚。 */
let videoStall = 0;
const VIDEO_STALL_FRAMES = 180;

/** 用户点画面：被自动播放策略挡住时这一下只负责解禁，其余情况照旧切播放 / 暂停。 */
function onVideoClick() {
  if (videoBlocked.value) {
    resumeVideo();
    return;
  }
  togglePlay();
}

function resumeVideo() {
  const el = videoEl.value;
  if (!el) {
    videoBlocked.value = false;
    return;
  }
  void el
    .play()
    .then(() => {
      videoBlocked.value = false;
    })
    .catch((err) => {
      console.warn('video play failed', err);
    });
}

function onVideoLoaded() {
  videoError.value = '';
  videoStall = 0;
}

function removeVideo() {
  videoPath.value = '';
  videoSrc.value = '';
  videoError.value = '';
}

function onVideoError() {
  videoError.value = t('debug-video-broken');
}

function setVideoOffset(event: Event) {
  const value = Number((event.target as HTMLInputElement).value);
  videoOffset.value = Number.isFinite(value) ? value : 0;
}

let videoRaf = 0;

/** 已经挂过快捷键监听的那个子窗口，换地址时要先把旧的摘掉。 */
let frameKeyTarget: Window | null = null;

/** 视频比谱面短：越过它末尾的那一段没有画面，盖一块黑，别停在最后一帧骗人。 */
const videoOutOfRange = ref(false);

/**
 * 每帧把视频对齐到引擎时间（时间轴以左边的谱面为准，视频只是跟着走）。
 *
 * 暂停时贴得紧一点（0.05 秒，拖进度条要立刻看到那一帧），播放中只在漂出 0.2 秒时才硬拉 ——
 * 每帧都写 `currentTime` 会让解码器一直从关键帧重来，画面反而是卡的。
 */
function syncVideo() {
  videoRaf = requestAnimationFrame(syncVideo);
  const el = videoEl.value;
  const handle = player();
  if (!el || !videoSrc.value || videoError.value || !handle?.ready()) return;

  // 元数据还没读到：给一段时间，读不到就是文件根本播不了，别一帧一帧地干等
  if (el.readyState < 1) {
    if (videoStall++ > VIDEO_STALL_FRAMES) videoError.value = t('debug-video-load-failed');
    return;
  }
  videoStall = 0;

  const target = Math.max(0, handle.getTime() + videoOffset.value / 1000);
  if (!Number.isFinite(target)) return;

  // 谱面比视频长：末段画面已经没了，视频停在结尾并盖黑
  const total = el.duration;
  if (Number.isFinite(total) && total > 0 && target >= total - 0.02) {
    if (!videoOutOfRange.value) videoOutOfRange.value = true;
    if (!el.paused) el.pause();
    return;
  }
  if (videoOutOfRange.value) videoOutOfRange.value = false;

  if (handle.getPaused()) {
    if (!el.paused) el.pause();
    if (!el.seeking && Math.abs(el.currentTime - target) > 0.05) el.currentTime = target;
    return;
  }

  if (el.paused) {
    if (videoBlocked.value) return; // 等用户点一下画面，别每帧都撞一次自动播放策略
    void el
      .play()
      .then(() => {
        videoBlocked.value = false;
      })
      .catch((err) => {
        if (err instanceof DOMException && err.name === 'NotAllowedError') videoBlocked.value = true;
        else console.warn('video play failed', err);
      });
  }
  if (!el.seeking && Math.abs(el.currentTime - target) > 0.2) el.currentTime = target;
}

// ── 窗口按钮 ────────────────────────────────────────────────────────────────

/** 这个窗口没有系统标题栏（见后端 `decorations(false)`），
 *  最小化 / 最大化 / 关闭走标题栏右边那三个按钮 —— 与 DPEditor 同一套做法。 */
const debugWindow = getCurrentWindow();

/** 上排两栏的宽度比：分隔条拖它，两边都至少留四分之一。拖过的位置存下来，下次开窗口还是这个分法。 */
const previewShare = useStorage('DPDebugger.previewShare', 0.5);

/** 底部参数区的高度：分隔条拖它，太矮放不下输入框，太高会把上面的画面挤没。 */
const PANEL_MIN = 150;
const PANEL_RESERVED = 260;
const panelHeight = useStorage('DPDebugger.panelHeight', Math.round(window.innerHeight * 0.34));
const comparePanel = ref<HTMLElement | null>(null);

/**
 * 正文的排版全交给 grid（两行三列）：上排左预览、中间分隔条、右视频，下排参数区。
 *
 * 两处尺寸都来自分隔条 —— 上排两栏的 fr 比与参数区的高度，所以这里只算行内样式，
 * 不需要额外的包裹层。
 */
/**
 * 存下来的尺寸可能是坏值（`null`、`NaN`）：直接拼进 `grid-template-*` 会让整条声明变成非法值，
 * 浏览器丢掉模板后退回自动列宽 —— 表现为预览撑满整个窗口、视频栏被挤成右边一条缝、
 * 参数区被顶到窗口外面。所以每次用之前都过一遍数学校验。
 */
function finiteNumber(value: unknown, fallback: number): number {
  const n = Number(value);
  return Number.isFinite(n) ? n : fallback;
}

function shareValue(): number {
  return Math.min(0.75, Math.max(0.25, finiteNumber(previewShare.value, 0.5)));
}

function panelValue(): number {
  const max = Math.max(PANEL_MIN, window.innerHeight - PANEL_RESERVED);
  return Math.min(max, Math.max(PANEL_MIN, finiteNumber(panelHeight.value, Math.round(window.innerHeight * 0.34))));
}

const bodyStyle = computed(() => {
  const share = shareValue();
  return {
    gridTemplateColumns: `minmax(0, ${share}fr) 13px minmax(0, ${(1 - share).toFixed(3)}fr)`,
    gridTemplateRows: `minmax(0, 1fr) 13px ${Math.round(panelValue())}px`,
  };
});

function clampPanel() {
  panelHeight.value = panelValue();
}

/** 拖过的分法存在本地，换一台分辨率不同的机器打开时可能已经超出可拖范围，开窗前先收回区间里。 */
function clampShare() {
  previewShare.value = shareValue();
}

function minimizeWindow() {
  void debugWindow.minimize();
}

function toggleMaximizeWindow() {
  void debugWindow.toggleMaximize();
}

function closeWindow() {
  void debugWindow.close();
}

// ── 关窗确认 ────────────────────────────────────────────────────────────────

/** 载入那一刻的整份配置：关窗前跟它比一次，就知道这一窗里有没有调过参数。 */
let mountedSnapshot = '';

/** 调过参数才弹的确认框：问要不要把这次的演出参数写回 `assets/chart-animation.toml`。 */
const saveDialog = ref(false);
const saving = ref(false);
const saveError = ref('');

function configSnapshot(): string {
  return JSON.stringify(JSON.parse(JSON.stringify(config)) as RenderConfig);
}

function isDirty(): boolean {
  return mountedSnapshot !== '' && configSnapshot() !== mountedSnapshot;
}

/**
 * 真关窗。`close()` 会再发一次 CloseRequested（又被自己拦一回、弹窗关不掉），
 * 走 `destroy()` 直接销毁窗口。
 */
async function closeWindowForReal() {
  await debugWindow.destroy();
}

async function saveAndClose() {
  if (saving.value) return;
  saving.value = true;
  saveError.value = '';
  try {
    const snapshot = JSON.parse(JSON.stringify(config)) as RenderConfig;
    await invoke<string>('save_chart_animation', { config: snapshot });
    saveDialog.value = false;
    await closeWindowForReal();
  } catch (err) {
    saveError.value = err instanceof Error ? err.message : String(err);
  } finally {
    saving.value = false;
  }
}

async function closeWithoutSaving() {
  saveDialog.value = false;
  await closeWindowForReal();
}

// 窗口标题跟着谱面走：任务栏 / Alt+Tab 里能看出正在调哪一张。
watch(
  chartLabel,
  (name) => {
    void debugWindow.setTitle(name ? `DPDebugger — ${name}` : 'DPDebugger');
  },
  { immediate: true },
);

// ── 分隔条 ──────────────────────────────────────────────────────────────────

/** 拖的是哪一根：上排中间那根改两栏宽度，底部那根改参数区高度。 */
let dragKind: '' | 'preview' | 'panel' = '';
let dragFrom = 0;
let dragValue = 0;
let dragWidth = 0;

function startResizePreview(event: PointerEvent) {
  dragKind = 'preview';
  dragFrom = event.clientX;
  dragValue = shareValue();
  dragWidth = (previewPanel.value?.clientWidth ?? 0) + (comparePanel.value?.clientWidth ?? 0);
  (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
}

function startResizePanel(event: PointerEvent) {
  dragKind = 'panel';
  dragFrom = event.clientY;
  dragValue = panelHeight.value;
  (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
}

function moveResize(event: PointerEvent) {
  if (!dragKind) return;
  if (dragKind === 'preview') {
    if (dragWidth <= 0) return;
    const next = dragValue + (event.clientX - dragFrom) / dragWidth;
    previewShare.value = Math.min(0.75, Math.max(0.25, next));
    return;
  }
  panelHeight.value = dragValue - (event.clientY - dragFrom);
  clampPanel();
}

function endResize(event: PointerEvent) {
  dragKind = '';
  (event.currentTarget as HTMLElement).releasePointerCapture?.(event.pointerId);
}

/** 拿 iframe 里的播放页接口；还没起来时是 null。 */
function player(): PlayerHandle | null {
  const win = frame.value?.contentWindow as (Window & { DPRWPlayer?: PlayerHandle }) | null;
  return win?.DPRWPlayer ?? null;
}

// ── 装配预览 ────────────────────────────────────────────────────────────────

/** 谱面信息那几行字：预览页的界面读的是它自己那份 `info.txt`。 */
function buildInfoText(files: ChartFiles): Uint8Array {
  const lines = ['#', `Chart: ${files.chart}`, `Music: ${files.music}`];
  if (files.illustration) lines.push(`Image: ${files.illustration}`);
  if (files.name.trim()) lines.push(`Name: ${files.name.trim()}`);
  if (files.level.trim()) lines.push(`Level: ${files.level.trim()}`);
  if (files.charter.trim()) lines.push(`Charter: ${files.charter.trim()}`);
  if (files.composer.trim()) lines.push(`Composer: ${files.composer.trim()}`);
  return new TextEncoder().encode(lines.join('\n'));
}

/** 从音乐字节里量出总时长，给进度条当上限（与 `EditorPreview` 的 `probeDuration` 同一套）。 */
async function probeDuration(bytes: Uint8Array): Promise<number> {
  const Ctx = window.AudioContext;
  if (!Ctx) return 0;
  const ctx = new Ctx();
  try {
    // `decodeAudioData` 会拿走这段 buffer，所以传副本。
    const buffer = await ctx.decodeAudioData(bytes.slice().buffer);
    return buffer.duration;
  } catch {
    return 0;
  } finally {
    void ctx.close();
  }
}

/** 磁盘上缺的附带资源包：缺了那一族演出就没有贴图 / 皮肤，界面顶部提示一句。 */
const missingPacks = ref<string[]>([]);

/**
 * 把当前配置转成引擎那边认的 JSON（`RenderConfig` → `dps::config::Config`）。
 *
 * 给了 `path` 就让后端按谱面名强开那一族演出，并把要摊的资源包一并列出来；改参数时只换算、
 * 不再列包（族开关已经在本地配置里了），免得多读一遍磁盘。
 */
async function previewConfig(path: string | null): Promise<PreviewConfig> {
  return invoke<PreviewConfig>('debug_preview_config', {
    config: JSON.parse(JSON.stringify(config)),
    path,
  });
}

/**
 * 把演出要用的附带资源包摊进播放页的内存文件表。
 *
 * 网页版引擎读不到 exe 旁边的 `appendant/`：`appendant::entry` 与 `ResourcePack::from_path` 在
 * wasm 下查的是 `fs_from_assets` 那张表，键名就是 `__dprw_files` 的键（`<prefix>/<包名>/<包内路径>`）。
 * 读不出来只让那一族少素材，不中断预览 —— 参数照样能调。
 */
async function injectPacks(
  packs: PreviewPack[],
  files: Record<string, Uint8Array>,
): Promise<string[]> {
  const missing: string[] = [];
  for (const pack of packs) {
    if (!pack.present) {
      missing.push(pack.name);
      continue;
    }
    for (const file of pack.files) {
      try {
        const buffer = await invoke<ArrayBuffer>('debug_appendant_file', {
          name: pack.name,
          file,
        });
        files[`${pack.prefix}/${pack.name}/${file}`] = new Uint8Array(buffer);
      } catch {
        missing.push(pack.name);
        break;
      }
    }
  }
  return missing;
}

/**
 * 读谱面字节、挂给播放页、起 iframe。
 *
 * 换谱面才走这条；改参数不走（那条只调 `setConfig`，画面不重开）。
 */
async function mountPreview() {
  const path = chartPath.value;
  if (!path) {
    failure.value = t('debug-no-chart');
    loading.value = false;
    return;
  }

  loading.value = true;
  failure.value = '';
  ready.value = false;
  playerSrc.value = '';

  try {
    const filesInfo = await invoke<ChartFiles>('debug_chart_files', { path });
    chartDifficulty.value = filesInfo.level.trim();
    const files: Record<string, Uint8Array> = {};
    const read = async (name: string): Promise<Uint8Array | null> => {
      if (!name) return null;
      const cached = files[name];
      if (cached) return cached;
      const buffer = await invoke<ArrayBuffer>('read_chart_asset', { path, name });
      const bytes = new Uint8Array(buffer);
      files[name] = bytes;
      return bytes;
    };
    await read(filesInfo.chart);
    const musicBytes = await read(filesInfo.music);
    await read(filesInfo.illustration);
    // 谱面自带的那份 `info.txt` 原样带上：难度、曲名、谱师、宽高比、试听区间都在里面，
    // 自己拼一份只会漏字段（引擎拿不到 `Level:` 就只能画默认难度）。读不到才退回拼的。
    let bundledInfo: Uint8Array | null = null;
    try {
      bundledInfo = await read('info.txt');
    } catch (err) {
      bundledInfo = null;
    }
    if (bundledInfo && bundledInfo.length > 0) files['info.txt'] = bundledInfo;
    else files['info.txt'] = buildInfoText(filesInfo);
    // 播放页的 BGM 桥只在包内找得到音乐时才知道总长，这里自己量一份兜底：
    // 量不出来（解码失败）就等 `getDuration()`。
    duration.value = musicBytes ? await probeDuration(musicBytes) : 0;

    // 播放页在实例化之前会先读这两个全局量：配置早到一步，第一次建场景就是这套参数
    // （界面开关、资源包这类只读一次的项，晚到的 `setConfig` 补不上）。
    const preview = await previewConfig(path);
    // 后端按谱面名强开了这一族的演出开关（以及那只专属皮肤），先并回本地配置：
    // 面板上显示的就是实际画出来的那份，后面推参数也不会把它改回去。
    Object.assign(config, preview.render);
    adoptConfigVolume();
    // 并完族开关再记一份：之后面板上的每一次改动都算「这一窗调过参数」。
    mountedSnapshot = configSnapshot();
    missingPacks.value = await injectPacks(preview.packs, files);

    const host = window as unknown as {
      __dprwPendingChart?: Record<string, Uint8Array>;
      __dprwPendingConfig?: string;
    };
    host.__dprwPendingChart = files;
    host.__dprwPendingConfig = preview.config;

    // `paused=1`：起来就停着，等这里的播放键（声音不静音，演出动画要跟着音乐对）。
    playerSrc.value = `/player/player.html?paused=1&r=${Date.now()}`;
    waitPlayer();
  } catch (err) {
    loading.value = false;
    failure.value = err instanceof Error ? err.message : String(err);
  }
}

/** 等播放页里的引擎报到第一帧，然后把播放头挪到这段演出的起点。 */
function waitPlayer() {
  stopWait();
  let ticks = 0;
  readyTimer = window.setInterval(() => {
    const handle = player();
    if (handle?.ready()) {
      stopWait();
      ready.value = true;
      loading.value = false;
      jumpToStart();
      return;
    }
    if (++ticks > 200) {
      stopWait();
      loading.value = false;
      failure.value = t('debug-load-failed');
    }
  }, 100);
}

/**
 * 把播放头落到本族演出的起点。
 *
 * 播放页报到第一帧时谱面可能还在解析，那一刻的 `seek` 会被随后建好的场景冲回 0；所以
 * 这里不假设一次就成——位置没到就再发一次，落上了就停（最多 24 次，约 3 秒）。
 */
function jumpToStart() {
  stopSeek();
  const start = familySpec.value?.start ?? 0;
  time.value = start;
  const handle = player();
  if (!(start > 0)) {
    if (handle?.ready()) handle.seek(0);
    return;
  }
  let tries = 0;
  const target = start;
  seekTimer = window.setInterval(() => {
    const current = player();
    if (!current?.ready()) return;
    if (Math.abs(current.getTime() - target) < 0.35 || ++tries > 24) {
      stopSeek();
      return;
    }
    current.seek(target);
  }, 120);
}

function stopSeek() {
  if (!seekTimer) return;
  window.clearInterval(seekTimer);
  seekTimer = 0;
}

function stopWait() {
  if (!readyTimer) return;
  window.clearInterval(readyTimer);
  readyTimer = 0;
}

let readyTimer = 0;
let seekTimer = 0;
let pollTimer = 0;
let syncTimer = 0;

// ── 参数 ↔ 预览 / 主窗口 ────────────────────────────────────────────────────

/**
 * 把改动交给预览与主窗口：先换成引擎那边的配置 JSON 推给播放页（`setConfig` 只换配置、
 * 不重载谱面，播放头与播放态都不动），再把 `RenderConfig` 原样发给主窗口。
 *
 * 输入框每敲一个字符都会触发，所以外面套了防抖。
 */
async function syncConfig() {
  const snapshot = JSON.parse(JSON.stringify(config)) as RenderConfig;
  try {
    // 不带谱面路径：族开关已经并进本地配置了，这里只做换算（再去列一遍包是白读磁盘）。
    const preview = await previewConfig(null);
    player()?.setConfig?.(preview.config);
    // 整份配置里带着 `volume_music` / `volume_sfx`：推过去会把引擎那边的音量盖回配置值
    // （滑条上写 0% 时下一帧又开始响）。推完立刻按滑条重新送一遍。
    applyVolume();
  } catch (err) {
    console.warn('[debugger] 推送预览配置失败:', err);
  }
  try {
    await emit('debug-config', snapshot);
  } catch (err) {
    console.warn('[debugger] 回传配置失败:', err);
  }
}

function scheduleSync() {
  if (syncTimer) return;
  syncTimer = window.setTimeout(() => {
    syncTimer = 0;
    void syncConfig();
  }, 120);
}

watch(config, scheduleSync, { deep: true });

function setNumber(key: string, kind: FieldSpec['kind'], event: Event) {
  const raw = (event.target as HTMLInputElement).value.trim();
  // 输入到一半的 `-`、`1.` 不写回配置，否则数字框会被自己的格式化打断
  if (!raw || raw === '-' || raw.endsWith('.')) return;
  const value = Number(raw);
  if (!Number.isFinite(value)) return;
  config[key] = kind === 'int' ? Math.round(value) : value;
}

function setText(key: string, event: Event) {
  config[key] = (event.target as HTMLInputElement).value;
}

/**
 * 以秒为单位的字段：框里既可以写秒数，也可以粘贴 `1:53.12` 这种时间码，
 * 改完（回车 / 失焦）换算成秒写回配置，框里只剩秒数。
 * 认不出来就把原文还原回去，别把敲了一半的字吞掉。
 */
function setSeconds(key: string, event: Event) {
  const el = event.target as HTMLInputElement;
  const seconds = secondsFromText(el.value);
  if (seconds == null) {
    el.value = valueOf(key);
    return;
  }
  config[key] = seconds;
  el.value = String(seconds);
}

/** 秒数在参数表里一直用 `hint: '秒'` 标：这里认它，模板就不用再挑一遍字段。 */
function isSecondsField(field: FieldSpec): boolean {
  return field.hint === '秒';
}

/** 回车就想提交：先让输入框失焦，`change` 才会跑。 */
function blurOnEnter(event: KeyboardEvent) {
  const el = event.target as HTMLInputElement | null;
  el?.blur();
}

function setBool(key: string, event: Event) {
  config[key] = (event.target as HTMLInputElement).checked;
}

/**
 * 颜色字段：界面上就一个输入框，里面写 `#694D24`（与主界面的颜色框同一套写法）。
 * 存进配置的仍是 0~1 的三分量数组；打了一半 / 写错格式时不写回，免得框里的字被打断。
 */
function setHexText(key: string, event: Event) {
  const text = (event.target as HTMLInputElement).value.trim();
  const match = /^#?([0-9a-fA-F]{6})$/.exec(text);
  if (!match) return;
  const value = Number.parseInt(match[1]!, 16);
  config[key] = [((value >> 16) & 0xff) / 255, ((value >> 8) & 0xff) / 255, (value & 0xff) / 255];
}

function defaultOf(key: string): unknown {
  const value = (DEFAULT_RENDER_CONFIG as unknown as Record<string, unknown>)[key];
  return Array.isArray(value) ? [...value] : value;
}

/** 输入框要的值：模板里不能做类型断言，统一在这里转成字符串。 */
function valueOf(key: string): string {
  const value = config[key];
  if (value === undefined || value === null) return '';
  // f32 转成 JS 的 f64 会露出 `0.9900000095367432` 这种尾巴，显示时按 f32 的
  // 有效位数收回 7 位，写回配置的仍是完整值。
  if (typeof value === 'number') return String(Number(value.toPrecision(7)));
  return String(value);
}

/** 颜色字段的显示值：`#RRGGBB` 原样（大写），`[r, g, b]`（0~1）转成大写文本。 */
function hexText(key: string): string {
  const value = config[key];
  if (typeof value === 'string') return value;
  if (!Array.isArray(value) || value.length < 3) return '';
  return (
    '#' +
    value
      .slice(0, 3)
      .map((it) =>
        Math.round(Math.min(1, Math.max(0, Number(it) || 0)) * 255)
          .toString(16)
          .padStart(2, '0')
      )
      .join('')
      .toUpperCase()
  );
}

function isOn(key: string): boolean {
  return !!config[key];
}

function resetField(key: string) {
  config[key] = defaultOf(key);
}

/** 这一组参数全部回到 `assets/chart-animation.toml` 里的默认值。 */
function resetAll() {
  const spec = familySpec.value;
  if (!spec) return;
  for (const group of spec.groups) {
    for (const field of group.fields) config[field.key] = defaultOf(field.key);
  }
}

// ── 预览刷新 ────────────────────────────────────────────────────────────────

/** 刷新：把谱面与配置重新读一遍再起一次播放页（改参数不用刷新，那条走 `setConfig`）。 */
function reloadPreview() {
  void mountPreview();
}

// ── 播放控制 ────────────────────────────────────────────────────────────────

function togglePlay() {
  const handle = player();
  if (!handle?.ready()) return;
  const wasPaused = handle.getPaused();
  if (wasPaused) handle.play();
  else handle.pause();
  // 播放态等下一轮轮询再读：命令是排队的，这一帧 `getPaused()` 还没换过来。
  playing.value = wasPaused;
  applyVolume();
}

function seekTo(seconds: number) {
  const handle = player();
  if (!handle?.ready()) return;
  handle.seek(seconds);
  time.value = seconds;
}

/** 进度条填充与滑块的百分比（总长还不知道时停在 0）。 */
const percent = computed(() => {
  const total = duration.value;
  if (!(total > 0)) return '0%';
  const ratio = Math.min(1, Math.max(0, time.value / total));
  return `${(ratio * 100).toFixed(3)}%`;
});

let scrubbing = false;

/** 按指针在进度条上的位置换算秒数并跳过去（与 `EditorTimeline` 的换算同一套）。 */
function scrubTo(event: PointerEvent) {
  const el = event.currentTarget as HTMLElement | null;
  const total = duration.value;
  if (!el || !(total > 0)) return;
  const rect = el.getBoundingClientRect();
  if (rect.width <= 0) return;
  const ratio = Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width));
  seekTo(ratio * total);
}

function startScrub(event: PointerEvent) {
  if (!ready.value) return;
  scrubbing = true;
  (event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId);
  scrubTo(event);
}

function moveScrub(event: PointerEvent) {
  if (scrubbing) scrubTo(event);
}

function endScrub(event: PointerEvent) {
  scrubbing = false;
  (event.currentTarget as HTMLElement).releasePointerCapture?.(event.pointerId);
}

// ── 快捷键 ──────────────────────────────────────────────────────────────────

/** 一帧多少秒：按渲染帧率算（成片就按这个帧率抽帧），对位时一步一格。 */
function frameStep(): number {
  const fps = Math.round(Number(config.fps));
  return 1 / (Number.isFinite(fps) && fps > 0 ? fps : 60);
}

/**
 * 跳到 `seconds`：先暂停再跳，逐帧看的时候画面必须停住；
 * 越出首尾就截在两端，`seek` 到负数会让引擎把时间倒着走。
 */
function jumpTo(seconds: number) {
  const handle = player();
  if (!handle?.ready()) return;
  if (!handle.getPaused()) {
    handle.pause();
    playing.value = false;
  }
  const total = duration.value;
  const end = total > 0 ? total : seconds;
  seekTo(Math.min(end, Math.max(0, seconds)));
}

/** 相对当前播放头挪 `delta` 秒。 */
function nudge(delta: number) {
  const handle = player();
  if (!handle?.ready()) return;
  jumpTo(handle.getTime() + delta);
}

/** 焦点在输入框、下拉框或按钮上时不抢按键：那里方向键要改数值、空格要点按钮。 */
function isTypingTarget(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el) return false;
  const tag = el.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || el.isContentEditable;
}

/** 空格播放/暂停，左右逐帧，Shift + 左右一秒，Home/End 跳到头尾。 */
function onKeydown(event: KeyboardEvent) {
  if (event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey) return;
  const target = event.target as HTMLElement | null;
  if (isTypingTarget(target)) return;
  // 空格落在按钮上由按钮自己处理，这里再toggle 一次会互相抵消
  if (event.key === ' ' && target?.closest?.('button, [role="button"]')) return;
  switch (event.key) {
    case ' ':
      event.preventDefault();
      togglePlay();
      break;
    case 'ArrowLeft':
      event.preventDefault();
      nudge(event.shiftKey ? -1 : -frameStep());
      break;
    case 'ArrowRight':
      event.preventDefault();
      nudge(event.shiftKey ? 1 : frameStep());
      break;
    case 'Home':
      event.preventDefault();
      jumpTo(0);
      break;
    case 'End':
      event.preventDefault();
      if (duration.value > 0) jumpTo(duration.value);
      break;
    default:
      break;
  }
}

/**
 * 点过预览画面之后焦点落在 iframe 里，父窗口的 keydown 收不到；
 * 播放页与本窗口同源，直接把同一个处理函数挂到子窗口上。
 */
function bindFrameKeys() {
  const target = frame.value?.contentWindow;
  if (!target || target === frameKeyTarget) return;
  frameKeyTarget?.removeEventListener('keydown', onKeydown as unknown as EventListener, true);
  target.addEventListener('keydown', onKeydown as unknown as EventListener, true);
  frameKeyTarget = target;
}

/** 读数与 DPEditor 时间轴一致：`分:秒.毫秒`，分钟补零、毫秒三位（实现共用 `../timecode`）。 */
const formatTime = formatTimecode;

// ── 生命周期 ────────────────────────────────────────────────────────────────

let unlisten: UnlistenFn | null = null;
let unlistenClose: UnlistenFn | null = null;
let videoObserver: ResizeObserver | null = null;

function applyContext(context: DebugContext) {
  chartPath.value = context.path;
  if (context.config) Object.assign(config, context.config);
  void mountPreview();
}

onMounted(async () => {
  clampShare();
  clampPanel();
  fitAll();
  if (typeof ResizeObserver !== 'undefined') {
    if (stage.value) {
      stageObserver = new ResizeObserver(fitScreen);
      stageObserver.observe(stage.value);
    }
    if (videoStage.value) {
      videoObserver = new ResizeObserver(fitVideo);
      videoObserver.observe(videoStage.value);
    }
  }
  window.addEventListener('resize', onWindowResize);
  window.addEventListener('keydown', onKeydown, true);
  // 上次开过的那个视频还在：直接挂回去，接着对比
  if (videoPath.value) attachVideo(videoPath.value);
  videoRaf = requestAnimationFrame(syncVideo);

  unlisten = await listen<DebugContext>('debug-context', (event) => applyContext(event.payload));
  // 系统关窗（Alt+F4、标题栏按钮、`close()`）都会先问这里：调过参数就拦下来弹确认框
  unlistenClose = await debugWindow.onCloseRequested((event) => {
    if (!isDirty()) return;
    event.preventDefault();
    saveError.value = '';
    saveDialog.value = true;
  });

  pollTimer = window.setInterval(() => {
    const handle = player();
    if (!handle?.ready()) return;
    time.value = handle.getTime();
    const nextPlaying = !handle.getPaused();
    // 引擎换档之后再把音量推一次：点播放那一帧 `getPaused()` 还是旧值，当时推过去会把音乐按成 0。
    if (nextPlaying !== playing.value) {
      playing.value = nextPlaying;
      applyVolume();
    }
    const total = handle.getDuration();
    if (total > 0) duration.value = total;
  }, 200);

  try {
    const context = await invoke<DebugContext | null>('debug_context');
    if (!context) {
      failure.value = t('debug-no-chart');
      loading.value = false;
      return;
    }
    applyContext(context);
  } catch (err) {
    failure.value = err instanceof Error ? err.message : String(err);
    loading.value = false;
  }
});

/** 窗口换尺寸：参数区高度要跟着收，两栏画面也要重新量。 */
function onWindowResize() {
  clampPanel();
  fitAll();
}

onBeforeUnmount(() => {
  unlisten?.();
  unlistenClose?.();
  stopWait();
  stopSeek();
  if (pollTimer) window.clearInterval(pollTimer);
  if (syncTimer) window.clearTimeout(syncTimer);
  if (videoRaf) cancelAnimationFrame(videoRaf);
  videoRaf = 0;
  window.removeEventListener('resize', onWindowResize);
  window.removeEventListener('keydown', onKeydown, true);
  frameKeyTarget?.removeEventListener('keydown', onKeydown as unknown as EventListener, true);
  frameKeyTarget = null;
  stageObserver?.disconnect();
  stageObserver = null;
  videoObserver?.disconnect();
  videoObserver = null;
  const host = window as unknown as {
    __dprwPendingChart?: unknown;
    __dprwPendingConfig?: unknown;
  };
  delete host.__dprwPendingChart;
  delete host.__dprwPendingConfig;
});
</script>

<template>
  <div class="dbg">
    <!-- 标题栏：这个窗口没有系统标题栏（与 DPEditor、主窗口一致），这条同时是拖拽区。 -->
    <header class="dbg-titlebar" data-tauri-drag-region>
      <div class="dbg-titlebar__brand" data-tauri-drag-region>
        <img src="../assets/favicon.png" alt="icon" class="dbg-titlebar__logo" />
        <span class="dbg-titlebar__name">DPDebugger</span>
      </div>

      <span v-if="familySpec" class="dbg-chart" :title="chartLabel">{{ chartLabel }}</span>
      <span v-if="familySpec" class="dbg-chart__family" :title="familySpec.title">{{ chartDifficulty || familySpec.title }}</span>

      <div class="dbg-titlebar__spacer" data-tauri-drag-region></div>

      <div class="dbg-head__path" :title="chartPath ?? ''">{{ chartPath }}</div>

      <div class="dbg-titlebar__controls">
        <button class="dbg-winbtn" type="button" :title="t('minimize')" @click="minimizeWindow">
          <i class="mdi mdi-window-minimize"></i>
        </button>
        <button class="dbg-winbtn" type="button" :title="t('maximize')" @click="toggleMaximizeWindow">
          <i class="mdi mdi-window-maximize"></i>
        </button>
        <button class="dbg-winbtn dbg-winbtn--close" type="button" :title="t('close')" @click="closeWindow">
          <i class="mdi mdi-window-close"></i>
        </button>
      </div>
    </header>

    <main class="dbg__body" :style="bodyStyle">
      <!-- 左上：预览窗口（结构照 DPEditor 的 `EditorPreview`：舞台 + 播放控制 + 工具条） -->
      <section ref="previewPanel" class="preview">
        <div ref="stage" class="preview__stage">
          <div class="preview__screen" :style="screenStyle">
            <iframe
              v-if="playerSrc"
              ref="frame"
              class="preview__frame"
              :src="playerSrc"
              title="DPDebugger preview"
              tabindex="-1"
              @load="onFrameLoad"
            ></iframe>
            <div v-else class="preview__empty">
              <i class="mdi mdi-movie-open-outline preview__empty-icon"></i>
              <p class="preview__empty-text">{{ loading ? t('debug-loading') : failure || t('debug-no-chart') }}</p>
            </div>
          </div>
        </div>

        <!-- 播放控制 + 进度条：照 DPEditor 时间轴那条（播放 / 回到起点 / 时间 / 进度） -->
        <footer class="preview__transport" :title="t('debug-keys-hint')">
          <v-btn
            variant="text"
            size="small"
            :icon="playing ? 'mdi-pause' : 'mdi-play'"
            :disabled="!ready"
            :title="playing ? t('debug-pause') : t('debug-play')"
            @click="togglePlay"
          ></v-btn>
          <!-- 当前时间点一下就地改，回车跳过去：调时间参数要反复对位，单开一个输入框太占地方。
               宽度由一个看不见的 `00:00.000` 撑出来：数字怎么变这一格都不动，也不留多余的空档 -->
          <span class="preview__clock">
            <span class="preview__clock-sizer" aria-hidden="true">00:00.000</span>
            <input
              v-if="editingTime"
              ref="timeInput"
              v-model="timeDraft"
              class="preview__clock-input"
              type="text"
              size="1"
              spellcheck="false"
              @keydown.enter="commitTimeEdit"
              @keydown.esc="cancelTimeEdit"
              @blur="commitTimeEdit"
            />
            <button
              v-else
              class="preview__clock-button"
              type="button"
              :title="t('debug-jump-hint')"
              :disabled="!ready"
              @click="beginTimeEdit"
            >
              {{ formatTime(time) }}
            </button>
          </span>
          <span class="preview__sep">/</span>
          <span class="preview__clock-total">{{ formatTime(duration) }}</span>
          <div
            class="preview__progress"
            role="slider"
            :aria-valuenow="Math.round(time)"
            :aria-valuemin="0"
            :aria-valuemax="Math.round(duration)"
            :class="{ 'preview__progress--disabled': !ready || !(duration > 0) }"
            tabindex="0"
            @pointerdown="startScrub"
            @pointermove="moveScrub"
            @pointerup="endScrub"
            @pointercancel="endScrub"
          >
            <div class="preview__progress-fill" :style="{ width: percent }"></div>
            <div class="preview__progress-thumb" :style="{ left: percent }"></div>
          </div>
          <v-btn variant="text" size="small" icon="mdi-refresh" :title="t('debug-refresh')" @click="reloadPreview"></v-btn>
          <!-- 音量：一个按钮点开两条滑杆（音乐 / 音效分开调），拖动当场生效 -->
          <v-menu :close-on-content-click="false" location="top end" offset="6">
            <template #activator="{ props: menu }">
              <v-btn v-bind="menu" variant="text" size="small" :icon="volumeIcon" :title="t('debug-volume')"></v-btn>
            </template>
            <v-sheet class="preview__volume">
              <div class="preview__volume-row">
                <i class="mdi mdi-music"></i>
                <span>{{ t('debug-volume-music') }}</span>
                <span class="preview__volume-value">{{ Math.round(volumeMusic) }}%</span>
              </div>
              <v-slider
                v-model="volumeMusic"
                :min="0"
                :max="100"
                :step="1"
                :thumb-size="11"
                density="compact"
                hide-details
              ></v-slider>
              <div class="preview__volume-row">
                <i class="mdi mdi-volume-high"></i>
                <span>{{ t('debug-volume-sfx') }}</span>
                <span class="preview__volume-value">{{ Math.round(volumeSfx) }}%</span>
              </div>
              <v-slider
                v-model="volumeSfx"
                :min="0"
                :max="100"
                :step="1"
                :thumb-size="11"
                density="compact"
                hide-details
              ></v-slider>
            </v-sheet>
          </v-menu>
        </footer>
      </section>

      <div
        class="dbg-splitter dbg-splitter--v"
        @pointerdown="startResizePreview"
        @pointermove="moveResize"
        @pointerup="endResize"
        @pointercancel="endResize"
      ></div>

      <!-- 右上：对比视频（本地文件，播放 / 暂停 / 拖动都跟着左边的预览走） -->
      <section ref="comparePanel" class="compare">
        <div ref="videoStage" class="compare__stage">
          <video
            v-if="videoSrc"
            ref="videoEl"
            class="compare__video"
            :class="{ 'compare__video--out': videoOutOfRange }"
            :style="videoStyle"
            :src="videoSrc"
            :muted="videoMuted"
            :volume="videoVolume / 100"
            playsinline
            preload="auto"
            @loadeddata="onVideoLoaded"
            @error="onVideoError"
            @click="onVideoClick"
          ></video>
          <!-- 自动播放被挡住时铺一句提示：这一层不吃点击，点它等于点视频 -->
          <div v-if="videoBlocked" class="compare__hint">{{ t('debug-video-blocked') }}</div>
          <!-- 空态只属于「还没选视频」：`v-else-if` 接在 `videoBlocked` 那一条后面，
               条件必须再判一次 `videoSrc`，否则视频挂上去之后这句提示会一直盖在画面上。 -->
          <div v-else-if="!videoSrc" class="compare__empty">
            <i class="mdi mdi-movie-open-outline compare__empty-icon"></i>
            <p class="compare__empty-text">{{ videoError || t('debug-video-empty') }}</p>
            <v-btn
              variant="tonal"
              size="small"
              prepend-icon="mdi-movie-open"
              class="compare__empty-btn"
              @click="openVideo"
            >
              {{ t('debug-video-open') }}
            </v-btn>
          </div>
        </div>

        <!-- 视频这一栏自己的工具条：换视频 / 移除 / 同步偏移 / 音量（播放控制共用左边那条） -->
        <footer class="compare__bar">
          <v-btn
            variant="text"
            size="small"
            icon="mdi-movie-open"
            :title="videoSrc ? t('debug-video-change') : t('debug-video-open')"
            @click="openVideo"
          ></v-btn>
          <v-btn
            v-if="videoSrc"
            variant="text"
            size="small"
            icon="mdi-close"
            :title="t('debug-video-remove')"
            @click="removeVideo"
          ></v-btn>
          <span v-if="videoSrc" class="compare__name" :title="videoPath">{{ videoName }}</span>
          <v-spacer></v-spacer>
          <label class="compare__offset">
            <span class="compare__offset-label">{{ t('debug-video-offset') }}</span>
            <input
              class="compare__offset-input"
              type="number"
              step="10"
              :value="videoOffset"
              @input="setVideoOffset"
            />
            <span class="compare__offset-unit">ms</span>
          </label>
          <v-menu :close-on-content-click="false" location="top end" offset="6">
            <template #activator="{ props: menu }">
              <v-btn
                v-bind="menu"
                variant="text"
                size="small"
                :icon="videoMuted || videoVolume <= 0 ? 'mdi-volume-off' : 'mdi-volume-high'"
                :title="t('debug-video-volume')"
              ></v-btn>
            </template>
            <v-sheet class="preview__volume">
              <div class="preview__volume-row">
                <i class="mdi mdi-movie-open"></i>
                <span>{{ t('debug-video-volume') }}</span>
                <span class="preview__volume-value">{{ videoMuted ? 0 : Math.round(videoVolume) }}%</span>
              </div>
              <v-slider
                v-model="videoVolume"
                :min="0"
                :max="100"
                :step="1"
                :thumb-size="11"
                density="compact"
                hide-details
                @update:model-value="videoMuted = false"
              ></v-slider>
            </v-sheet>
          </v-menu>
        </footer>
      </section>

      <div
        class="dbg-splitter dbg-splitter--h"
        @pointerdown="startResizePanel"
        @pointermove="moveResize"
        @pointerup="endResize"
        @pointercancel="endResize"
      ></div>

      <!-- 下：参数栏（结构照 DPEditor 的 `EditorEditBar`：头部一行 + 分组） -->
      <aside class="editbar">
        <div class="editbar__head">
          <v-icon icon="mdi-tune-variant" size="15"></v-icon>
          <span class="editbar__title">{{ t('debug-params') }}</span>
          <span v-if="familySpec" class="editbar__track">{{ familySpec.title }}</span>
          <v-spacer></v-spacer>
          <v-btn
            variant="text"
            size="small"
            icon="mdi-restore"
            :disabled="!familySpec"
            :title="t('debug-reset-all')"
            @click="resetAll"
          ></v-btn>
        </div>

        <div class="editbar__body">
          <p v-if="missingPacks.length" class="editbar__notice">
            <v-icon icon="mdi-alert-circle-outline" size="14"></v-icon>
            <span>{{ t('debug-pack-missing', { names: missingPacks.join('、') }) }}</span>
          </p>
          <p v-if="!familySpec" class="editbar__empty">{{ t('debug-unsupported') }}</p>
          <template v-else-if="familySpec">
            <!-- 一组参数一张卡片：宽了就并成三栏，不用把一长串参数从头滚到尾 -->
            <section v-for="group in familySpec.groups" :key="group.title" class="editbar__group">
              <h3 class="editbar__group-title">{{ group.title }}</h3>
              <div class="editbar__rows">
                <div v-for="field in group.fields" :key="field.key" class="editbar__row">
                  <label class="editbar__row-label" :for="`f-${field.key}`">
                    {{ field.label }}
                    <small v-if="field.hint">{{ field.hint }}</small>
                  </label>

                  <div class="editbar__row-field">
                    <label v-if="field.kind === 'bool'" class="editbar__check">
                      <input
                        :id="`f-${field.key}`"
                        type="checkbox"
                        :checked="isOn(field.key)"
                        @change="setBool(field.key, $event)"
                      />
                      <span>{{ isOn(field.key) ? '开' : '关' }}</span>
                    </label>

                    <input
                      v-else-if="field.kind === 'color3'"
                      :id="`f-${field.key}`"
                      class="editbar__input"
                      type="text"
                      :value="hexText(field.key)"
                      @input="setHexText(field.key, $event)"
                    />

                    <input
                      v-else-if="field.kind === 'text' || field.kind === 'color'"
                      :id="`f-${field.key}`"
                      class="editbar__input"
                      type="text"
                      :value="valueOf(field.key)"
                      @input="setText(field.key, $event)"
                    />

                    <input
                      v-else-if="isSecondsField(field)"
                      :id="`f-${field.key}`"
                      class="editbar__input"
                      type="text"
                      :value="valueOf(field.key)"
                      :title="t('debug-seconds-hint')"
                      @change="setSeconds(field.key, $event)"
                      @keydown.enter="blurOnEnter"
                    />

                    <input
                      v-else
                      :id="`f-${field.key}`"
                      class="editbar__input"
                      type="number"
                      :step="field.step ?? 'any'"
                      :min="field.min"
                      :max="field.max"
                      :value="valueOf(field.key)"
                      @input="setNumber(field.key, field.kind, $event)"
                    />
                  </div>

                  <button
                    class="editbar__row-reset"
                    type="button"
                    :title="t('debug-reset-field')"
                    @click="resetField(field.key)"
                  >
                    <i class="mdi mdi-restore"></i>
                  </button>
                </div>
              </div>
            </section>
          </template>
        </div>
      </aside>
    </main>

    <!-- 调过参数才弹：问要不要把这次的演出参数写回 assets/chart-animation.toml（全局窗口样式） -->
    <v-dialog v-model="saveDialog" width="auto" min-width="360px" persistent>
      <v-card class="log-card-bg log-card-only-window">
        <v-card-title>{{ t('debug-save-title') }}</v-card-title>
        <v-card-text class="dbg-save__body">
          <p>{{ t('debug-save-desc') }}</p>
          <p v-if="saveError" class="dbg-save__error">
            {{ t('debug-save-failed', { error: saveError }) }}
          </p>
        </v-card-text>
        <v-card-actions class="justify-end">
          <v-btn variant="text" :disabled="saving" @click="saveDialog = false">
            {{ t('debug-save-cancel') }}
          </v-btn>
          <v-btn variant="text" :disabled="saving" @click="closeWithoutSaving">
            {{ t('debug-save-discard') }}
          </v-btn>
          <v-btn variant="text" :loading="saving" @click="saveAndClose">
            {{ t('debug-save-confirm') }}
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
  </div>
</template>

<style scoped>
.dbg {
  display: flex;
  flex-direction: column;
  height: 100vh;
  overflow: hidden;
  background: var(--phi-bg-base);
  color: var(--phi-label-primary);
  font-family: var(--phi-font-family);
  font-size: var(--phi-font-size);
}

/* 标题栏：尺寸、底色、字号都照 DPEditor 那条 `.editor-titlebar`（也照主窗口），三个窗口才是一套。
   整条可拖拽（`data-tauri-drag-region`），里面的按钮不算拖拽区。 */
.dbg-titlebar {
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

.dbg-titlebar__brand {
  display: flex;
  flex: none;
  gap: var(--phi-space-2);
  align-items: center;
}

.dbg-titlebar__logo {
  width: 18px;
  height: 18px;
}

.dbg-titlebar__name {
  font-size: var(--phi-font-size-secondary);
  font-weight: 600;
  color: var(--phi-label-primary);
  letter-spacing: 0.02em;
}

/* 谱面名：与编辑器标题栏上的草稿名同位（那里点开是草稿菜单，这里只是显示） */
.dbg-chart {
  flex: none;
  max-width: 320px;
  overflow: hidden;
  font-size: var(--phi-font-size-secondary);
  font-weight: 600;
  color: var(--phi-label-primary);
  text-overflow: ellipsis;
  white-space: nowrap;
  user-select: text;
}

.dbg-chart__family {
  flex: none;
  padding: 1px 6px;
  font-size: 0.7rem;
  color: var(--phi-label-secondary);
  background: var(--phi-bg-layer-3);
  border-radius: var(--phi-radius-xs);
}

.dbg-titlebar__spacer {
  flex: 1 1 auto;
  min-width: 0;
}

/* 路径是给人对位置用的，不抢标题的位置：占满中间、自己省略 */
.dbg-head__path {
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

/* 拖拽区里的按钮要标 no-drag，否则点上去是抓窗口 */
.dbg-titlebar__controls {
  display: flex;
  flex: none;
  gap: var(--phi-space-1);
  align-items: center;
  app-region: no-drag;
}

.dbg-winbtn {
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

.dbg-winbtn:hover {
  color: var(--phi-label-primary);
  background: var(--phi-interactive-hover);
}

.dbg-winbtn--close:hover {
  color: #fff;
  background: var(--phi-danger);
}

/* 正文：两行三列的 grid（上排 预览 | 分隔条 | 视频，下排参数区跨满整行）。
   行列尺寸由 `bodyStyle` 按分隔条拖出来的比例写到行内样式上。 */
.dbg__body {
  display: grid;
  flex: 1 1 auto;
  min-height: 0;
  padding: var(--phi-space-2);
  /* 行内样式（`bodyStyle`）没绑上、或者存下来的比例是坏值被丢掉时，退回这套默认排法，
     免得三栏全挤成一列。 */
  grid-template-columns: minmax(0, 1fr) 13px minmax(0, 1fr);
  grid-template-rows: minmax(0, 1fr) 13px 260px;
}

/* ── 左上预览：样式照 DPEditor 的 `.preview` ──────────────────────────────── */

.preview {
  display: flex;
  grid-row: 1;
  grid-column: 1;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  background: var(--phi-bg-layer-1);
  border: 0.5px solid var(--phi-border-l2);
  border-radius: var(--phi-radius-md);
}

.preview__stage {
  display: flex;
  flex: 1 1 auto;
  align-items: center;
  justify-content: center;
  min-width: 0;
  min-height: 0;
  padding: var(--phi-space-3);
  overflow: hidden;
  background: var(--phi-bg-base);
  user-select: none;
}

/* 16:9 的屏幕。宽高由脚本按舞台的可用区域算好写到行内样式上（见 `fitScreen`），
   这里的 `aspect-ratio` 只是挂载前那一帧的兜底。 */
.preview__screen {
  position: relative;
  aspect-ratio: 16 / 9;
  max-width: 100%;
  max-height: 100%;
  overflow: hidden;
  background: #000;
  border-radius: var(--phi-radius-sm);
  box-shadow: 0 0 0 0.5px var(--phi-border-l3);
}

/* 引擎画面铺满屏幕；指针事件不往 iframe 里送，免得点一下被引擎当成打歌操作 */
.preview__frame {
  display: block;
  width: 100%;
  height: 100%;
  border: 0;
  pointer-events: none;
  user-select: none;
}

.preview__empty {
  display: flex;
  flex-direction: column;
  gap: var(--phi-space-2);
  align-items: center;
  justify-content: center;
  height: 100%;
  padding: var(--phi-space-4);
  text-align: center;
}

.preview__empty-icon {
  font-size: 30px;
  color: var(--phi-label-caption);
}

.preview__empty-text {
  color: var(--phi-label-secondary);
  font-size: 0.8rem;
  line-height: 1.6;
}

/* 播放控制 + 工具条合成一条：播放 / 时间 / 进度 / 刷新 / 音量，省下一整行 */
.preview__transport {
  display: flex;
  flex: none;
  gap: var(--phi-space-1);
  align-items: center;
  height: 40px;
  padding: 0 var(--phi-space-2);
  border-top: 0.5px solid var(--phi-border-l1);
}

/* 进度条照 `EditorTimeline` 那条：10px 的胶囊底、accent 填充、白色圆钮 */
.preview__progress {
  position: relative;
  flex: 1 1 auto;
  min-width: 0;
  height: 10px;
  margin: 0 8px 0 10px;
  cursor: pointer;
  background: var(--phi-bg-layer-3);
  border-radius: var(--phi-radius-pill);
}

.preview__progress--disabled {
  cursor: default;
  opacity: 0.5;
}

.preview__progress-fill {
  height: 100%;
  background: var(--phi-accent);
  border-radius: var(--phi-radius-pill);
}

.preview__progress-thumb {
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

/* 当前时间那一格：宽度由里面那个看不见的 `00:00.000` 撑出来（同一格里叠着放），
   数字怎么变这一格都不动，也不会像写死 em 那样留下多余的空档。 */
.preview__clock {
  display: inline-grid;
  flex: none;
  align-items: center;
  height: 24px;
  font-size: 0.9rem;
  line-height: 1.2;
  color: var(--phi-label-primary);
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}

.preview__clock > * {
  grid-area: 1 / 1;
}

.preview__clock-sizer {
  visibility: hidden;
}

/* 斜杠与总长跟着同一个字号：字号不同字面高度就不一样，两串数字看着一高一矮 */
.preview__clock-total,
.preview__sep {
  color: var(--phi-label-caption);
  font-size: 0.9rem;
  line-height: 1.2;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}

/* 斜杠两边的留白要相等：这一格自己不带左右内边距，间距全交给斜杠的 margin，
   否则「数字—斜杠」之间会多出那一格的内边距，看着左宽右窄 */
.preview__sep {
  margin: 0 var(--phi-space-2);
}

/* 当前时间本身就是跳转入口：平时看着像文字，点下去变成输入框 */
.preview__clock-button {
  box-sizing: border-box;
  width: 100%;
  min-width: 0;
  padding: 0;
  text-align: left;
  cursor: text;
  background: none;
  border: 0;
  border-radius: var(--phi-radius-xs);
}

.preview__clock-button:hover:not(:disabled) {
  background: var(--phi-interactive-hover);
}

.preview__clock-button:disabled {
  color: var(--phi-label-tertiary);
  cursor: default;
}

/* 点开后在原地改：仍然是那串白字，只多一根光标，不画输入框的底与边 */
.preview__clock-input {
  box-sizing: border-box;
  width: 100%;
  min-width: 0;
  max-width: 100%;
  height: 24px;
  padding: 0;
  font: inherit;
  font-size: 0.9rem;
  font-variant-numeric: tabular-nums;
  color: var(--phi-label-primary);
  caret-color: var(--phi-accent);
  background: transparent;
  border: 0;
  outline: none;
}

/* 音量浮层：两条滑杆（音乐 / 音效）同一张卡片里分开调 */
.preview__volume {
  width: 200px;
  padding: var(--phi-space-2) var(--phi-space-3);
}

.preview__volume-row {
  display: flex;
  gap: var(--phi-space-1);
  align-items: center;
  font-size: 0.74rem;
  color: var(--phi-label-secondary);
}

.preview__volume-row i {
  font-size: 15px;
  color: var(--phi-label-caption);
}

.preview__volume-value {
  margin-left: auto;
  color: var(--phi-label-caption);
  font-variant-numeric: tabular-nums;
}

/* ── 右上视频对比：与左边预览同一套卡片样式 ──────────────────────────────── */

.compare {
  display: flex;
  grid-row: 1;
  grid-column: 3;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  background: var(--phi-bg-layer-1);
  border: 0.5px solid var(--phi-border-l2);
  border-radius: var(--phi-radius-md);
}

/* 文件名跟谱面路径同一个待遇：占满剩下的地方、自己省略 */
.compare__name {
  flex: 0 1 auto;
  min-width: 0;
  overflow: hidden;
  font-size: 0.74rem;
  color: var(--phi-label-caption);
  text-overflow: ellipsis;
  white-space: nowrap;
  user-select: text;
}

.compare__stage {
  position: relative;
  display: flex;
  flex: 1 1 auto;
  align-items: center;
  justify-content: center;
  min-width: 0;
  min-height: 0;
  padding: var(--phi-space-3);
  overflow: hidden;
  background: var(--phi-bg-base);
}

/* 视频框与预览同尺寸：宽高由脚本按舞台算（见 `fitVideo`），`aspect-ratio` 只是兜底；
   源视频不是 16:9 时自己留黑边（`object-fit: contain`），不拉伸。 */
.compare__video {
  aspect-ratio: 16 / 9;
  max-width: 100%;
  max-height: 100%;
  cursor: pointer;
  background: #000;
  border-radius: var(--phi-radius-sm);
  box-shadow: 0 0 0 0.5px var(--phi-border-l3);
  object-fit: contain;
}

/* 谱面比视频长：越过视频末尾之后压成一块黑，而不是停在最后一帧 */
.compare__video--out {
  filter: brightness(0);
}

/* 自动播放被挡住的提示：不吃点击，点它等于点视频 */
.compare__hint {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 0.8rem;
  color: #fff;
  text-align: center;
  pointer-events: none;
  background: rgb(0 0 0 / 35%);
}

.compare__empty {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  gap: var(--phi-space-2);
  align-items: center;
  justify-content: center;
  align-self: stretch;
  padding: var(--phi-space-4);
  text-align: center;
}

.compare__empty-icon {
  font-size: 30px;
  color: var(--phi-label-caption);
}

.compare__empty-text {
  max-width: 28em;
  color: var(--phi-label-secondary);
  font-size: 0.8rem;
  line-height: 1.6;
}

.compare__bar {
  display: flex;
  flex: none;
  gap: var(--phi-space-2);
  align-items: center;
  height: 40px;
  padding: 0 var(--phi-space-2);
  border-top: 0.5px solid var(--phi-border-l1);
}

/* 同步偏移：视频比预览慢就填正数（`视频时间 = 预览时间 + 偏移`） */
.compare__offset {
  display: flex;
  gap: 4px;
  align-items: center;
  font-size: 0.74rem;
  color: var(--phi-label-secondary);
}

.compare__offset-label {
  white-space: nowrap;
}

.compare__offset-input {
  width: 64px;
  height: 24px;
  padding: 0 5px;
  font-size: 0.74rem;
  font-variant-numeric: tabular-nums;
  color: #fff;
  background: var(--phi-bg-module);
  border: 0.5px solid var(--phi-border-l2);
  border-radius: var(--phi-radius-xs);
}

.compare__offset-input:focus {
  border-color: var(--phi-accent);
  outline: none;
}

.compare__offset-unit {
  color: var(--phi-label-caption);
}

/* 底部参数栏：与 DPEditor 的 `.editbar` 同规，占满第三行；高度由分隔条拖（`bodyStyle` 给行高） */
.editbar {
  display: flex;
  grid-row: 3;
  grid-column: 1 / -1;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  background: var(--phi-bg-layer-1);
  border: 0.5px solid var(--phi-border-l2);
  border-radius: var(--phi-radius-md);
}

/* 分隔条：视觉上只是一条细线，但整条 13px 都能抓（`touch-action: none` 免得触屏上被滚动手势吃掉） */
.dbg-splitter {
  position: relative;
  flex: none;
  background: transparent;
  touch-action: none;
}

.dbg-splitter::after {
  position: absolute;
  content: '';
  background: var(--phi-border-l3);
  border-radius: var(--phi-radius-pill);
  transition:
    background-color var(--phi-duration) var(--phi-ease),
    width var(--phi-duration) var(--phi-ease);
}

.dbg-splitter--v {
  grid-row: 1;
  grid-column: 2;
  width: 13px;
  cursor: col-resize;
}

/* 竖直分隔线只留在中间一段：两栏各自有描边，顶到天花板反而多出一条线 */
.dbg-splitter--v::after {
  top: 50%;
  left: 50%;
  width: 1px;
  height: 56px;
  transform: translate(-50%, -50%);
}

.dbg-splitter--h {
  grid-row: 2;
  grid-column: 1 / -1;
  height: 13px;
  cursor: row-resize;
}

/* 横向分隔线同理：只画中间一段，别把两边卡片的描边接长 */
.dbg-splitter--h::after {
  top: 50%;
  left: 50%;
  width: 56px;
  height: 1px;
  transform: translate(-50%, -50%);
}

/* ── 底部参数栏的零件：样式照 DPEditor 的 `.editbar__*` ──────────────────── */

.editbar__head {
  display: flex;
  flex: none;
  gap: var(--phi-space-2);
  align-items: center;
  height: 34px;
  padding: 0 var(--phi-space-2);
  font-size: 0.85rem;
  color: var(--phi-label-primary);
  border-bottom: 0.5px solid var(--phi-border-l1);
}

.editbar__title {
  font-weight: 600;
}

.editbar__track {
  padding: 1px 6px;
  font-size: 0.74rem;
  color: var(--phi-label-caption);
  background: var(--phi-bg-layer-3);
  border-radius: var(--phi-radius-pill);
}

/* 参数一屏排成多栏：够宽就并三栏（每栏至少 380px），窗口窄了自动收成一两栏 */
.editbar__body {
  display: grid;
  flex: 1 1 auto;
  grid-template-columns: repeat(auto-fit, minmax(380px, 1fr));
  gap: 2px 18px;
  align-content: start;
  min-height: 0;
  padding: 6px 10px 10px;
  overflow-x: hidden;
  overflow-y: auto;
  border-top: 0.5px solid var(--phi-border-l1);
}

/* 一组参数一张卡片：标题在上、参数行竖着排，卡片之间靠 grid 的列间距隔开 */
.editbar__group {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
  padding-bottom: 6px;
  border-bottom: 0.5px solid var(--phi-border-l1);
}

.editbar__rows {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.editbar__row {
  display: flex;
  gap: var(--phi-space-2);
  align-items: center;
  min-height: 40px;
}

/* 参数名一栏按比例分：面板拉宽时两边一起长，名字不会挤成竖排 */
.editbar__row-label {
  display: flex;
  flex: 0 0 44%;
  flex-direction: column;
  min-width: 0;
  font-size: 0.78rem;
  line-height: 1.25;
  color: #fff;
}

.editbar__row-label small {
  font-size: 0.69rem;
  color: #fff;
  opacity: 0.6;
}

.editbar__row-field {
  flex: 1 1 0;
  min-width: 0;
}

/* 输入框统一 28px 高、等宽数字：数字对齐才好扫一眼看出差多少 */
.editbar__input {
  width: 100%;
  height: 28px;
  padding: 0 6px;
  font-size: 0.78rem;
  font-variant-numeric: tabular-nums;
  color: #fff;
  background: var(--phi-bg-module);
  border: 0.5px solid var(--phi-border-l2);
  border-radius: var(--phi-radius-xs);
}

.editbar__input:focus {
  border-color: var(--phi-accent);
  outline: none;
}

.editbar__check {
  display: flex;
  gap: var(--phi-space-1);
  align-items: center;
  font-size: 0.78rem;
  color: #fff;
  cursor: pointer;
}

.editbar__check input {
  accent-color: var(--phi-accent);
}

.editbar__row-reset {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  padding: 0;
  font-size: 13px;
  line-height: 1;
  color: var(--phi-label-caption);
  cursor: pointer;
  background: transparent;
  border: none;
  border-radius: var(--phi-radius-xs);
}

.editbar__row-reset:hover {
  color: var(--phi-label-primary);
  background: var(--phi-interactive-hover);
}

.editbar__group-title {
  margin: 4px 0 2px;
  font-size: 0.85rem;
  font-weight: 600;
  color: #fff;
}

/* 提示与空态跨满整行：它们不是某一组参数，不该被 grid 挤进某一栏 */
.editbar__notice,
.editbar__empty {
  grid-column: 1 / -1;
}

/* 缺包提示：只是「这段演出少素材」，参数照样能调，所以用警示色而不是报错样式 */
.editbar__notice {
  display: flex;
  gap: var(--phi-space-2);
  align-items: flex-start;
  margin: 0 0 4px;
  padding: 6px 8px;
  border: 0.5px solid var(--phi-border-l1);
  border-radius: var(--phi-radius-2, 6px);
  font-size: 0.75rem;
  color: var(--phi-warning-label);
}

.editbar__empty {
  display: flex;
  flex-direction: column;
  gap: var(--phi-space-2);
  align-items: center;
  justify-content: center;
  padding: var(--phi-space-4) var(--phi-space-2);
  font-size: 0.8rem;
  color: var(--phi-label-caption);
  text-align: center;
}

/* 关窗确认：说明一段 + 写盘失败的报错（成功就直接关窗，不再多一步） */
.dbg-save__body {
  font-size: 0.8rem;
  line-height: 1.6;
  color: var(--phi-label-secondary, #dcdcdc);
}

.dbg-save__body p {
  margin: 0;
}

.dbg-save__error {
  margin-top: var(--phi-space-2);
  color: var(--phi-error-label, #ff6b6b);
}
</style>
