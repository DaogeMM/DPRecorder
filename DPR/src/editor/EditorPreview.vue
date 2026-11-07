<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { useStorage } from '@vueuse/core';

import { useI18n } from 'vue-i18n';
const { t } = useI18n();

import { invoke } from '@tauri-apps/api/core';

import { toastError } from '@/common';

/**
 * 编辑窗口左上角的预览区：一块 16:9 的舞台，里面用 iframe 装网页版的引擎。
 *
 * 引擎编成了 wasm（`DPS/dps/src/bin/wasm_player.rs`，产物在 `public/player/`），页面里没有
 * 文件系统，所以三条轨道上的文件由这里读成字节、挂到 `window.__dprwPendingChart` 上，等
 * iframe 里的播放页起来时再交给它（`dps::fs::js_file` 从 `__dprw_files` 这张 Map 里查）。
 *
 * 播放控制交给宿主：这里把 iframe 里的 `DPRWPlayer`（`player-api.js` 暴露的薄壳）透出去，
 * 由 `EditorWindow` 驱动下面的时间轴（播放键、进度条都在那一条上）。
 *
 * 预览**默认暂停**：播放页的 `?paused=1` 一上来就把引擎的时钟拨回停止，等这里的播放键。
 */

const props = withDefaults(
  defineProps<{
    /** 谱面轨道上的文件；空着也能预览（引擎里塞一张空谱）。 */
    chartPath: string;
    /** 音乐轨道上的文件：预览的最低要求，没有它就不起。 */
    musicPath?: string;
    /** 曲绘轨道上的文件；可选，没有就是个黑底。 */
    coverPath?: string;
    /** 是否已经占满整窗（窗口全屏 + 编辑窗口只剩预览）。图标跟着它换，实际开关由父级做。 */
    fullscreen?: boolean;
  }>(),
  { musicPath: '', coverPath: '' },
);

const emit = defineEmits<{ 'toggle-fullscreen': [] }>();

const frame = ref<HTMLIFrameElement | null>(null);
/** 轨道上的文件读完了才挂 iframe：播放页一起来就会去读这些字节。 */
const playerSrc = ref('');
/** 「谱面加载中」：从读素材一直亮到播放页里的引擎报到第一帧。 */
const loading = ref(false);
/** 没有可用的音乐文件：引擎起不来，画面上直接说明。 */
const noMusic = ref(false);
/** 音乐长度（秒）。播放页只认它自己目录下的 `assets/info.txt`，这里自己解一遍更省事。 */
const trackDuration = ref(0);
/** 引擎当前是不是停着。停着的时候音乐音量压到 0（见 `restorePlayhead`）。 */
const paused = ref(true);

/** iframe 里的播放页暴露的接口（`public/player/player-api.js` 的 `window.DPRWPlayer`）。 */
type PlayerHandle = {
  ready: () => boolean;
  play: () => void;
  pause: () => void;
  seek: (seconds: number) => void;
  getTime: () => number;
  getPaused: () => boolean;
  getDuration: () => number;
  /** 音乐 / 音效音量，0~1；`public/player/wasm` 是重建前的旧产物时可能没有这一项。 */
  setVolume?: (volume: { music?: number; sfx?: number }) => void;
};

function player(): PlayerHandle | null {
  const win = frame.value?.contentWindow as (Window & { DPRWPlayer?: PlayerHandle }) | null;
  return win?.DPRWPlayer ?? null;
}

/** 从音乐文件字节里量出总时长，给时间轴当刻度上限。 */
async function probeDuration(bytes: Uint8Array): Promise<number> {
  const Ctx = window.AudioContext;
  if (!Ctx) {
    return 0;
  }
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

/** 路径最后一段（Windows 与 POSIX 分隔符都认）。 */
function baseName(path: string) {
  const cut = Math.max(path.lastIndexOf('\\'), path.lastIndexOf('/'));
  return cut >= 0 ? path.slice(cut + 1) : path;
}

/** 是谱面 / 是引擎认的 info 文件，按名字认（谱面包里可能带着自己的那份）。 */
const isChartName = (name: string) => /\.(json|pec)$/i.test(name);
const isInfoName = (name: string) => /(^|\/)(info\.(txt|ya?ml|csv)|:info)$/i.test(name);

/**
 * 没有谱面时塞给引擎的空谱（PGR 格式，字段与 `DPS/dps/src/parse/pgr.rs` 的 `PgrChart` 对应）。
 *
 * 引擎没有谱面就起不来（`GameScene::load_chart` 找不到谱面文件直接 `bail!`），而预览的最低要求
 * 只是一首歌，所以没有谱面时给它一张「一条判定线、没有音符」的谱：曲子多长，预览就放多长。
 */
const EMPTY_CHART_NAME = 'editor-empty.json';
const EMPTY_CHART = new TextEncoder().encode(
  JSON.stringify({
    formatVersion: 3,
    offset: 0,
    judgeLineList: [
      {
        bpm: 120,
        speedEvents: [],
        notesAbove: [],
        notesBelow: [],
        judgeLineMoveEvents: [],
        judgeLineRotateEvents: [],
        judgeLineDisappearEvents: [],
      },
    ],
  }),
);

/** 读一个素材文件；文件已经不在（被挪走 / 删掉）时返回 null，不把错误抛给调用方。 */
async function readMedia(path: string): Promise<Uint8Array | null> {
  try {
    const raw = await invoke<ArrayBuffer>('read_editor_media', { path });
    return new Uint8Array(raw);
  } catch {
    return null;
  }
}
/** 「谱面加载中」等第一帧用的轮询（见 `waitPlayerReady`）。 */
let readyTimer = 0;

function stopWait() {
  if (readyTimer) window.clearInterval(readyTimer);
  readyTimer = 0;
}

/**
 * 等播放页里的引擎报到第一帧，等到了就把「谱面加载中」撤掉。
 *
 * `seekTo` / `playing` 是重搭前记下的播放头与播放态：换轨道会整页重开，装好之后照原样落回去
 * —— 默认是停着，重搭完也不该自己播起来，更不该把播放头弹回曲首。
 */
function waitPlayerReady(seekTo = 0, playing = false) {
  stopWait();
  let ticks = 0;
  readyTimer = window.setInterval(() => {
    ticks += 1;
    if (player()?.ready() || ticks > 200) {
      loading.value = false;
      stopWait();
      restorePlayhead(seekTo, playing);
    }
  }, 100);
}

/**
 * 引擎报到第一帧之后把播放头与声音安置回去。
 *
 * 引擎放 BGM 不走「暂停中就整体静音」那条路：曲子开头那一帧它自己会 `music.play()` 一次
 * （见 `DPS/dps/src/scene/game.rs` 的 `State::BeforeMusic`），所以停在 0 秒时音量必须先压到 0，
 * 等宿主真的按下播放再由 `syncAudioState` 还原 —— 否则画面停着、声音却从头放到尾。
 */
function restorePlayhead(seekTo: number, playing: boolean) {
  const handle = player();
  if (!handle) return;
  const duration = trackDuration.value;
  const target = seekTo > 0 ? (duration > 0 ? Math.min(seekTo, duration) : seekTo) : 0;
  if (target > 0) handle.seek(target);
  if (!playing) handle.pause();
  paused.value = !playing;
  volumeKey = '';
  syncAudioState();
}

/**
 * 把三条轨道读成字节交给播放页（它自己再灌进 wasm 的内存文件系统）。
 *
 * 最低要求是一首歌：没有音乐、或者音乐文件已经不在了，就什么都不起，画面上说明。
 * 谱面与曲绘都可以没有 —— 没谱面时塞一张空谱，没曲绘时背景就是黑的。
 *
 * 文件名一律用轨道上那份文件自己的名字，再写一份只含 `Chart` / `Music` / `Image` 的
 * `info.txt`：引擎照这份文件里的名字去取文件，换过的音乐 / 曲绘才能顶替谱面包自带的那份；
 * 也只写这三行 —— 曲名、难度这些由草稿自己管，不往谱面里塞。
 */
async function preparePlayer() {
  // 换轨道会整页重开：先把播放头与播放态记下来，新页面就绪之后照原样落回去。
  // 播放页没起来（第一次装配、或者上一轮就没有音乐）时按「0 秒、停着」处理。
  const previous = player();
  const seekTo = previous?.ready() ? previous.getTime() || 0 : 0;
  const playing = previous?.ready() ? !previous.getPaused() : false;
  stopWait();
  loading.value = true;
  noMusic.value = false;
  playerSrc.value = '';
  trackDuration.value = 0;
  // 这一轮读完就按当前状态重搭了，轨道文件的快照留给下一轮轮询重新建立
  trackSnapshot = null;
  try {
    const files: Record<string, Uint8Array> = {};
    if (props.chartPath) {
      try {
        const names = await invoke<string[]>('list_editor_chart_files', {
          path: props.chartPath,
        });
        for (const name of names) {
          const raw = await invoke<ArrayBuffer>('read_editor_chart_file', {
            path: props.chartPath,
            name,
          });
          files[name] = new Uint8Array(raw);
        }
      } catch {
        // 谱面文件读不动（比如刚被删掉）：当没有谱面，光有音乐照样能预览
      }
    }
    const musicBytes = props.musicPath ? await readMedia(props.musicPath) : null;
    if (!musicBytes) {
      loading.value = false;
      noMusic.value = true;
      return;
    }
    const music = baseName(props.musicPath);
    files[music] = musicBytes;
    const coverBytes = props.coverPath ? await readMedia(props.coverPath) : null;
    const cover = coverBytes ? baseName(props.coverPath) : '';
    if (coverBytes) files[cover] = coverBytes;

    // 谱面轨道上那份文件优先：素材库把压缩包解成了文件夹，列出来的是整包的文件，
    // 包里可能不止一个 json（多难度、旧版本），照名字认回用户放到轨道上的那份
    const wanted = props.chartPath ? baseName(props.chartPath) : '';
    const chart =
      Object.keys(files).find((name) => name === wanted || name.endsWith(`/${wanted}`)) ??
      Object.keys(files).find((name) => isChartName(name)) ??
      EMPTY_CHART_NAME;
    if (chart === EMPTY_CHART_NAME) files[EMPTY_CHART_NAME] = EMPTY_CHART;
    // 谱面包里自带的那份 info 会把音乐 / 曲绘指到包里的文件名，整份丢掉，换成下面这份
    for (const name of Object.keys(files)) {
      if (isInfoName(name)) delete files[name];
    }
    const manifest = ['#', `Chart: ${chart}`, `Music: ${music}`];
    if (cover) manifest.push(`Image: ${cover}`);
    files['info.txt'] = new TextEncoder().encode(manifest.join('\n'));

    (window as unknown as { __dprwPendingChart?: Record<string, Uint8Array> }).__dprwPendingChart =
      files;
    trackDuration.value = await probeDuration(musicBytes);
    // `mute=1`：预览区只当画面用，播放页那条 BGM 桥不该出声（也不该被自动播放拦住）。
    // `paused=1`：打开就停在 0 秒。`r` 是每次装配的时间戳，点「重新载入」也会真的重载一次播放页。
    playerSrc.value = `/player/player.html?mute=1&paused=1&r=${Date.now()}`;
    waitPlayerReady(seekTo, playing);
  } catch (err) {
    loading.value = false;
    toastError(err);
  }
}

/**
 * 轨道上的文件被删掉（或者又放回来）时，播放页里那份还是旧的：预览是把字节读进引擎的内存
 * 文件系统的，源文件没了引擎不会知道，所以隔两秒问一次后端，发现变化就暂停再重搭。
 */
const watchedPaths = computed(() =>
  [props.chartPath, props.musicPath, props.coverPath].filter((path) => path !== ''),
);

/** 上一次看到的「还在 / 不在了」快照；`null` 表示刚从 `preparePlayer` 出来，还没比过。 */
let trackSnapshot: string | null = null;

async function checkTrackFiles() {
  const paths = watchedPaths.value;
  if (paths.length === 0) {
    trackSnapshot = null;
    return;
  }
  let gone: string[];
  try {
    gone = await invoke<string[]>('missing_editor_media', { paths });
  } catch {
    return;
  }
  const snapshot = paths.map((path) => (gone.includes(path) ? '0' : '1')).join('');
  if (trackSnapshot === null) {
    trackSnapshot = snapshot;
    return;
  }
  if (snapshot === trackSnapshot) return;
  trackSnapshot = snapshot;
  // 轨道文件没了：先把声音停住，再照当前草稿重搭一遍（缺音乐的那份会切到「未添加音乐」）
  player()?.pause();
  void preparePlayer();
}

/** 工具条左下角那颗刷新：把三条轨道重新读一遍、播放页重开。 */
function reloadPreview() {
  void preparePlayer();
}

watch(
  () => [props.chartPath, props.musicPath, props.coverPath] as const,
  () => void preparePlayer(),
  { immediate: true },
);

defineExpose({
  ready: () => player()?.ready() ?? false,
  // 播放 / 暂停时立刻把音量推过去：引擎那边暂停不会被自动播放策略拦住，声音一开一关都得跟手，
  // 不能等 200ms 的轮询（停着时音量是压到 0 的，见 `restorePlayhead`）。
  play: () => {
    pushVolume(false);
    player()?.play();
  },
  pause: () => {
    player()?.pause();
    pushVolume(true);
  },
  seek: (seconds: number) => player()?.seek(seconds),
  getTime: () => player()?.getTime() ?? 0,
  getPaused: () => player()?.getPaused() ?? true,
  // 以这里量的为准：播放页量的是它自己目录下那份 `assets/info.txt`，不是草稿里的歌
  getDuration: () => trackDuration.value || player()?.getDuration() || 0,
});

/**
 * 音乐 / 音效音量（0~100）。初值与播放页里 `dpr_preview_config()` 的 0.5 / 0.4 对齐，
 * 拖过之后记在本地（`useStorage`）：换谱面、重开窗口都还是这一份。
 */
const volumeMusic = useStorage('DPEditor.volumeMusic', 50);
const volumeSfx = useStorage('DPEditor.volumeSfx', 40);

const volumeIcon = computed(() => {
  const top = Math.max(volumeMusic.value, volumeSfx.value);
  if (top <= 0) return 'mdi-volume-off';
  return top <= 50 ? 'mdi-volume-low' : 'mdi-volume-high';
});

/**
 * 把音量推给播放页：引擎那边音乐换混音器放大器、音效改 `config.volume_sfx`，都是当场生效。
 *
 * 停着的时候音乐一律推 0（原因见 `restorePlayhead`）；播放中才用滑条上的值。
 * 播放页（重新）起来之前这份音量无处可推，所以 iframe 的 `load` 里再补一遍 ——
 * 页面自己会先把命令攒着，等引擎就绪后再执行。
 */
function applyVolume() {
  volumeKey = '';
  syncAudioState();
}

/** 上一次下发的音量状态，用来避开每 200ms 一次的重复下发。 */
let volumeKey = '';

function pushVolume(playing: boolean) {
  paused.value = !playing;
  volumeKey = `${paused.value}|${volumeMusic.value}|${volumeSfx.value}`;
  player()?.setVolume?.({
    music: playing ? volumeMusic.value / 100 : 0,
    sfx: volumeSfx.value / 100,
  });
}

function syncAudioState() {
  const handle = player();
  if (!handle?.ready()) return;
  paused.value = handle.getPaused();
  const key = `${paused.value}|${volumeMusic.value}|${volumeSfx.value}`;
  if (key === volumeKey) return;
  volumeKey = key;
  handle.setVolume?.({
    music: paused.value ? 0 : volumeMusic.value / 100,
    sfx: volumeSfx.value / 100,
  });
}

watch([volumeMusic, volumeSfx], applyVolume);

/**
 * 预览画面的比例。谱面本身是 16:9，换别的比例只是把画面框成那样看构图（引擎会把画面拉伸适配）。
 */
const RATIOS = ['16:9', '4:3', '1:1', '9:16', '3:4'];
const ratio = useStorage('DPEditor.previewRatio', '16:9');
const screenRatio = computed(() => {
  const [w, h] = ratio.value.split(':').map(Number);
  return (w ?? 0) > 0 && (h ?? 0) > 0 ? (w as number) / (h as number) : 16 / 9;
});

/**
 * 16:9 的屏幕按舞台的实际可用区域算：宽高哪一头先顶到就按哪一头收，另一头等比缩。
 *
 * 只靠 `aspect-ratio` 做不到「同时受宽高约束」——`width:100%` 配 `max-height:100%` 在舞台偏矮时
 * 是先把高度截住、宽度照样撑满，画面会被拉扁。窗口可以拖到任意大小，所以这里直接量。
 */
const stage = ref<HTMLElement | null>(null);
const screenStyle = ref<Record<string, string>>({});
let stageObserver: ResizeObserver | null = null;

function fitScreen() {
  const el = stage.value;
  if (!el) return;
  const style = getComputedStyle(el);
  const padX = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
  const padY = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
  const availW = el.clientWidth - padX;
  const availH = el.clientHeight - padY;
  if (availW <= 0 || availH <= 0) return;
  const width = Math.floor(Math.min(availW, availH * screenRatio.value));
  screenStyle.value = { width: `${width}px`, height: `${Math.floor(width / screenRatio.value)}px` };
}

/** 隔两秒查一次轨道文件还在不在（见 `checkTrackFiles`）。 */
let trackTimer = 0;
/** 隔 200ms 跟一次播放态：按下播放 / 暂停之后，声音要立刻跟着开关。 */
let audioTimer = 0;

onMounted(() => {
  fitScreen();
  if (stage.value && typeof ResizeObserver !== 'undefined') {
    stageObserver = new ResizeObserver(fitScreen);
    stageObserver.observe(stage.value);
  }
  trackTimer = window.setInterval(() => void checkTrackFiles(), 2000);
  audioTimer = window.setInterval(syncAudioState, 200);
});

onUnmounted(() => {
  window.clearInterval(trackTimer);
  window.clearInterval(audioTimer);
  trackTimer = 0;
  audioTimer = 0;
  stopWait();
  stageObserver?.disconnect();
  stageObserver = null;
});

// 换比例就重新量一次：屏幕的宽高由脚本算，光靠 CSS 的 `aspect-ratio` 换不了。
watch(ratio, () => fitScreen());
</script>

<template>
  <section class="preview">
    <!-- 舞台：16:9 的屏幕，里面是网页版引擎的 canvas（iframe） -->
    <div ref="stage" class="preview__stage">
      <div class="preview__screen" :style="screenStyle">
        <iframe
          v-if="playerSrc"
          ref="frame"
          class="preview__frame"
          :src="playerSrc"
          title="preview"
          tabindex="-1"
          @load="applyVolume"
        ></iframe>
        <!-- 引擎起来之前这块保持空白：以前的占位卡片（模糊曲绘 + 曲名 + 中央播放键）
             会和引擎画面对在一起，分不清哪张才是真的预览。 -->
        <div v-else class="preview__empty">
          <!-- 音乐是预览的最低要求：没有它引擎起不来，直接说明，不要留一整块黑 -->
          <template v-if="noMusic">
            <i class="mdi mdi-file-music-outline preview__empty-icon"></i>
            <p class="preview__empty-text">{{ t('editor-no-music') }}</p>
          </template>
        </div>
        <!-- 「谱面加载中」压在画面上：引擎要先读到谱面、拉起场景才有第一帧，这中间是空白的 -->
        <div v-if="loading" class="preview__loading">{{ t('editor-preview-loading') }}</div>
      </div>
    </div>

    <footer class="preview__bar">
      <!-- 刷新：把草稿当前的三条轨道重新读一遍（改完信息页的曲名/难度后再点一下） -->
      <v-btn
        variant="text"
        size="small"
        icon="mdi-refresh"
        :title="t('editor-preview-refresh')"
        :disabled="!musicPath"
        @click="reloadPreview"
      ></v-btn>
      <v-spacer></v-spacer>
      <!-- 音量：一个按钮点开两条滑杆（音乐 / 音效分开调），拖动当场生效 -->
      <v-menu :close-on-content-click="false" location="top end" offset="6">
        <template #activator="{ props: menu }">
          <v-btn
            v-bind="menu"
            variant="text"
            size="small"
            :icon="volumeIcon"
            :title="t('editor-volume')"
          ></v-btn>
        </template>
        <!-- 面板用 v-sheet 而不是裸 div：全局给 `.v-menu > .v-overlay__content > .v-sheet`
             补了底与描边（App.vue），裸 div 会变成"滑杆直接盖在画面上"。 -->
        <v-sheet class="preview__volume">
          <div class="preview__volume-row">
            <i class="mdi mdi-music"></i>
            <span>{{ t('editor-volume-music') }}</span>
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
            <span>{{ t('editor-volume-sfx') }}</span>
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
      <!-- 画面比例：一个窄下拉框。箭头自己画在框里（`menu-icon=""` 关掉 Vuetify 那个），
           这样箭头不占布局，框里的比例文字才能真居中。 -->
      <div class="preview__ratio" :title="t('editor-preview-ratio')">
        <v-select
          v-model="ratio"
          :items="RATIOS"
          variant="outlined"
          density="compact"
          hide-details
          menu-icon=""
          :menu-props="{ location: 'top end', offset: 6 }"
        ></v-select>
        <i class="mdi mdi-menu-down preview__ratio-arrow"></i>
      </div>
      <v-btn
        variant="text"
        size="small"
        :icon="fullscreen ? 'mdi-fullscreen-exit' : 'mdi-fullscreen'"
        :title="fullscreen ? t('editor-fullscreen-exit') : t('editor-fullscreen')"
        @click="emit('toggle-fullscreen')"
      ></v-btn>
    </footer>
  </section>
</template>

<style scoped>
.preview {
  display: flex;
  flex-direction: column;
  min-height: 0;
  overflow: hidden;
  background: var(--phi-bg-layer-1);
  border: 0.5px solid var(--phi-border-l2);
  border-radius: var(--phi-radius-md);
}

/* 音量弹窗：两条滑杆，标题行末尾跟当前百分比 */
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

/* 画面比例：工具条右边一个小下拉框 —— 细边框 + 深底 + 右侧箭头，框里的比例文字居中 */
.preview__ratio {
  position: relative;
  flex: none;
  width: 104px;
  font-size: 0.72rem;
  color: var(--phi-label-secondary);
}

.preview__ratio :deep(.v-field) {
  min-height: 24px;
  border-radius: var(--phi-radius-sm);
  background: var(--phi-bg-layer-2);
}

.preview__ratio :deep(.v-field__outline) {
  --v-field-border-width: 0.5px;
  color: var(--phi-border-l3);
}

.preview__ratio :deep(.v-field__input) {
  justify-content: center;
  min-height: 24px;
  padding: 0;
}

.preview__ratio :deep(.v-select__selection) {
  margin-inline: 0;
  white-space: nowrap;
}

/* 箭头不占布局，压在框的右端：文字居中才不会被它挤偏 */
.preview__ratio-arrow {
  position: absolute;
  top: 50%;
  right: 5px;
  font-size: 14px;
  color: var(--phi-label-caption);
  pointer-events: none;
  transform: translateY(-50%);
}

.preview__stage {
  display: flex;
  flex: 1 1 auto;
  align-items: center;
  justify-content: center;
  min-height: 0;
  padding: var(--phi-space-3);
  background: var(--phi-bg-base);
  user-select: none;
}

/* 16:9 的舞台。里面的元素全是绝对定位，所以必须给这块自己一个宽度，
   否则它会被算成 0×0，舞台就只剩黑底（父容器是 flex，居中不管尺寸）。
   实际宽高由脚本按舞台可用区域算好写到行内样式上（见 `fitScreen`），这里的
   `aspect-ratio` 只是挂载前那一帧的兜底。 */
.preview__screen {
  position: relative;
  aspect-ratio: 16 / 9;
  overflow: hidden;
  background: #000;
  border-radius: var(--phi-radius-sm);
  box-shadow: 0 0 0 0.5px var(--phi-border-l3);
}

/* 引擎画面：铺满 16:9 的屏幕，自身不留边框与滚动条。
   预览只是看的：指针事件不往 iframe 里送，免得点一下就被引擎当成暂停、拖一下当成打歌操作。 */
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
  align-items: center;
  justify-content: center;
  gap: var(--phi-space-2);
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

/* 「谱面加载中」：压在画面上，不拦指针（底下的工具条还要能点） */
.preview__loading {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--phi-label-caption);
  font-size: 0.78rem;
  background: rgb(0 0 0 / 28%);
  pointer-events: none;
}

.preview__bar {
  display: flex;
  flex: none;
  align-items: center;
  gap: var(--phi-space-1);
  height: 40px;
  padding: 0 var(--phi-space-2);
  border-top: 0.5px solid var(--phi-border-l1);
}
</style>
