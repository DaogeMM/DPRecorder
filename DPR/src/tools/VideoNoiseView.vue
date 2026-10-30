<script setup lang="ts">
/**
 * 工具页 · 噪域播放器。
 *
 * 视频逐格取色后写成一张**全程只有噪域**的谱面，默认打包成一个 `.pez`
 * （`chart.json` + `info.yml` + `song.mp3` + `background.png` 平铺在包根上），
 * 在「渲染」页导入就能预览 / 渲染。抽帧、合成音频与打谱面都在 Rust 侧
 * （`src-tauri/src/tool_video.rs`），这一页只负责参数与输出列表。
 *
 * 版式与「渲染」页一致：整页滚动 + 贴底操作条（`.render-view` 那三件套），
 * 参数用 `PhiGroup` + `PhiSection` + `.cfg-grid` + `CfgField`，不另起一套卡片样式。
 * 项目之间的切换**只能**走左栏（`tools/projects.ts` + `App.vue` 的子导航），页内不放选项卡。
 */
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRouter } from 'vue-router';

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import * as dialog from '@tauri-apps/plugin-dialog';
import { useStorage } from '@vueuse/core';

import CfgField from '../components/CfgField.vue';
import CfgSlider from '../components/CfgSlider.vue';
import CfgSwitch from '../components/CfgSwitch.vue';
import PhiGroup from '../components/PhiGroup.vue';
import PhiSection from '../components/PhiSection.vue';
import { readChartConfig, withChartConfig } from '../chartConfig';
import { toast, toastError } from '../common';
import { chartModMarker, chartSideOf } from '../config/chartSide';
import { DEFAULT_RENDER_CONFIG } from '../model';
import type { ChartInfo, RenderConfig } from '../model';
import { finishPageLoad } from '../pageLoad';

const { t } = useI18n();
const router = useRouter();

/** `ffmpeg` 能解、这页也愿意收的容器；对话框过滤与拖放校验共用一份。 */
const VIDEO_EXTENSIONS = [
  'mp4', 'mov', 'mkv', 'avi', 'webm', 'flv', 'm4v', 'ts', 'mpg', 'mpeg', 'wmv', '3gp',
];

interface VideoMeta {
  duration: number;
  width: number;
  height: number;
  fps: number;
  hasAudio: boolean;
}

interface ChartReport {
  /** 生成的谱面包（`.pez`）。 */
  file: string;
  /** 谱面包所在的文件夹。 */
  dir: string;
  entries: number;
  cols: number;
  rows: number;
  frames: number;
  duration: number;
}

/** 输出列表的一行，和后端 `ToolOutput` 一一对应。 */
interface ToolOutput {
  name: string;
  path: string;
  size: number;
  modified: number;
}

/** 生成进度，对应后端 `ConversionProgress`。 */
interface ConversionProgress {
  running: boolean;
  /** `probe` / `audio` / `frames` / `chart` / `done` / `failed` */
  stage: string;
  frame: number;
  frames: number;
}

/* ----------------------------- 参数 -----------------------------
 * 全部存进 localStorage：写一张谱面要等一会儿，切页 / 重启后不该再从头调一遍。 */
const DEFAULTS = {
  /** 采样密度：相对「这段视频能取到的最细网格」的百分比，纵向格数按画面比例自动算。 */
  densityPct: 50,
  rate: 8,
  tolerance: 6,
  /** 输出宽高比，0 = 跟视频。 */
  aspect: 0,
  start: 0,
  end: 0,
  keepAudio: true,
  name: '',
  outputDir: '',
};

const params = useStorage('VideoNoiseView.Params', { ...DEFAULTS });

// 老版本存过形状不同的参数（那时横向格数叫 `cols`、比例还没有）：缺的键补回默认值，
// 否则采样密度滑块会停在最左边显示 0、视频比例下拉框是空的。
Object.entries(DEFAULTS).forEach(([key, value]) => {
  const field = key as keyof typeof DEFAULTS;
  if ((params.value as Record<keyof typeof DEFAULTS, unknown>)[field] === undefined) {
    (params.value as Record<keyof typeof DEFAULTS, unknown>)[field] = value;
  }
});

const videoPath = ref('');
const meta = ref<VideoMeta | null>(null);
const probing = ref(false);
const generating = ref(false);
const report = ref<ChartReport | null>(null);
const defaultOutputDir = ref('');
const appDir = ref('');
const fileHovering = ref(false);
const outputs = ref<ToolOutput[]>([]);
const loadingOutputs = ref(false);

/** 生成进度（左下角进度条用它）。 */
const progress = ref<ConversionProgress | null>(null);
/** 抽帧速度：帧/秒，由两次轮询的帧差算出来；其余阶段沿用上一次的值。 */
const speed = ref(0);
let progressTimer: number | undefined;
let lastSample: { frame: number; at: number } | null = null;

/** 百分比：只按抽帧的 `frame / frames` 算，抽完就是 100%。 */
const percent = computed(() => {
  const current = progress.value;
  if (!current || !current.frames) return 0;
  return Math.min(100, Math.round((current.frame / current.frames) * 100));
});

/** 进度条右边那行字：抽帧时给百分比和速度，其余阶段给一句当前动作。 */
const progressText = computed(() => {
  const current = progress.value;
  if (!current) return '';
  switch (current.stage) {
    case 'frames':
      return `${percent.value}% · ${t('vn-speed', { fps: speed.value.toFixed(1) })}`;
    case 'audio':
      return t('vn-stage-audio');
    case 'chart':
      return t('vn-stage-chart');
    // 打包：谱面 JSON 有几十 MB，deflate 这一趟要好几秒，按字节报百分比（`frames` 就是总字节）
    case 'save':
      return current.frames ? `${percent.value}% · ${t('vn-stage-save')}` : t('vn-stage-save');
    case 'done':
      return t('vn-done');
    default:
      return t('vn-stage-probe');
  }
});

/** 横幅左边只写文件名（带后缀；完整路径放在 title 里）。 */
const videoName = computed(() => videoPath.value.split(/[\\/]/).pop() ?? '');

/**
 * 采样密度的上限（横向格数）：一格至少占一个像素，所以不超过视频宽度；同时也受后端
 * 格子总数上限约束（`rows = cols × 高/宽`，格数 `cols × rows ≤ 65536`）。
 * 还没读视频时用默认的 160。
 */
const maxDensity = computed(() => {
  const current = meta.value;
  if (!current) return 160;
  const byCells = Math.floor(Math.sqrt((65536 * current.width) / current.height));
  return Math.max(8, Math.min(current.width, byCells));
});

/** 百分比对应的横向格数：100% 就是这段视频能取到的最细网格。 */
const densityCells = computed(() => {
  const max = maxDensity.value;
  const percent = Number(params.value.densityPct) || 0;
  return Math.max(8, Math.min(max, Math.round((max * percent) / 100)));
});

/** 噪域帧率上限：不超过原视频帧率（一秒钟内取不出比帧更多的颜色）。 */
const maxRate = computed(() => {
  const current = meta.value;
  if (!current) return 30;
  return Math.max(1, Math.min(60, Math.round(current.fps)));
});

/** 视频比例下拉：`0` = 跟视频自己。 */
const aspectOptions = computed(() => [
  { title: t('vn-aspect-original'), value: 0 },
  { title: '16:9', value: 16 / 9 },
  { title: '9:16', value: 9 / 16 },
  { title: '1:1', value: 1 },
  { title: '4:3', value: 4 / 3 },
  { title: '3:4', value: 3 / 4 },
]);

/** 默认输出目录在程序目录里面时，显示成 `.\tools\blockAreaPlayer` 这种相对写法。 */
const defaultOutputShown = computed(() => {
  const dir = defaultOutputDir.value;
  const base = appDir.value;
  if (!dir) return '';
  if (base && dir.toLowerCase().startsWith(base.toLowerCase())) {
    const rest = dir.slice(base.length).replace(/^[\\/]+/, '');
    return rest ? `.\\${rest}` : '.\\';
  }
  return dir;
});

/** 这一页真正会读写哪个目录：用户选过就是选的，没选就用默认。 */
const outputDir = computed(() => params.value.outputDir || defaultOutputDir.value);

/** 输出目录输入框里显示的文字（相对写法更短，完整路径在 `title` 里）。 */
const outputShown = computed(() => params.value.outputDir || defaultOutputShown.value);

/**
 * 后端错误串形如 `tool-video.no-ffmpeg` / `tool-video.decode-failed: <ffmpeg 的报错>`。
 * 前半段翻成本页文案（`vn-err-*`），冒号后的细节原样附在括号里 —— 否则 toast 上会直接
 * 显示一串 key，看不出是哪一步出的问题。
 */
function errorText(error: unknown) {
  const raw = error instanceof Error ? error.message : String(error);
  const match = /^tool-video\.([a-z-]+)(?:: ([^]*))?$/.exec(raw);
  if (!match) return raw;
  const message = t(`vn-err-${match[1]}`);
  return match[2] ? `${message}（${match[2].trim()}）` : message;
}

function fail(error: unknown) {
  toastError(errorText(error));
}

function isVideo(path: string) {
  const extension = path.split('.').pop()?.toLowerCase() ?? '';
  return VIDEO_EXTENSIONS.includes(extension);
}

/* ----------------------------- 视频 ----------------------------- */

async function loadVideo(path: string) {
  if (!isVideo(path)) {
    toast(t('vn-not-video'), 'error');
    return;
  }
  videoPath.value = path;
  meta.value = null;
  report.value = null;
  // 换了视频以后，上一段填的终点可能超出新视频的长度：清回「到结尾」，让用户自己决定
  params.value.start = 0;
  params.value.end = 0;
  probing.value = true;
  try {
    meta.value = await invoke<VideoMeta>('tool_video_probe', { path });
    // 上一段视频可能比这个长 / 快：帧率超出新上限就压回上限（密度是百分比，不用压）
    params.value.rate = Math.min(params.value.rate, maxRate.value);
  } catch (error) {
    fail(error);
  } finally {
    probing.value = false;
  }
}

async function pickOutputDir() {
  const picked = await dialog.open({ multiple: false, directory: true });
  if (typeof picked === 'string') params.value.outputDir = picked;
}

/* ----------------------------- 输出列表 ----------------------------- */

/** 重新列一遍输出目录里的 `.pez`。目录没建、或读失败都当空列表，不打扰用户。 */
async function refreshOutputs() {
  const dir = outputDir.value;
  if (!dir) return;
  loadingOutputs.value = true;
  try {
    outputs.value = await invoke<ToolOutput[]>('tool_video_outputs', { dir });
  } catch {
    outputs.value = [];
  } finally {
    loadingOutputs.value = false;
  }
}

/** 打开输出目录（还没生成过时目录可能不存在，让后端的报错兜底）。 */
async function openOutputDir() {
  const dir = outputDir.value;
  if (!dir) return;
  try {
    await invoke('tool_video_reveal', { dir });
  } catch (error) {
    fail(error);
  }
}

/** 带谱面包跳去渲染页（渲染页认 `query.chart`，见 `RenderView.vue`）。 */
function openOutput(row: ToolOutput) {
  void router.push({ name: 'render', query: { chart: row.path } });
}

/** 正在请求预览的那一行；请求发出后就清掉，只用来给按钮加转圈，不挡下一次点击。 */
const previewingPath = ref('');

/**
 * 渲染页存在 localStorage 里的工作配置（`RenderView.Config`，形如 `{ config, presetKey, … }`，
 * 老版本存的是裸 `RenderConfig`）。直接开预览要发和渲染页同一套参数，否则画面会和
 * 「在渲染页里点预览」不一样；没去过渲染页就用默认配置。
 */
function workingRenderConfig(): RenderConfig {
  let restored: unknown = null;
  try {
    restored = JSON.parse(localStorage.getItem('RenderView.Config') ?? 'null');
  } catch {
    // 存的东西坏了就当没存过：预览用默认配置也能跑，不值得拦住用户
  }
  const saved = restored as { config?: Partial<RenderConfig> } | Partial<RenderConfig> | null;
  const config = saved && 'config' in saved ? saved.config : saved;
  return { ...DEFAULT_RENDER_CONFIG, ...(config ?? {}) };
}

/**
 * 直接开预览窗口，**不跳渲染页**：谱面信息现场解析，渲染配置用渲染页那份工作配置，
 * 再盖上谱面自带的 `config.toml`（噪域谱面靠它把判定线 / 按键 / UI 全关掉）。
 * 预览子进程由后端 `preview_chart` 拉起（同一个 exe 带 `preview` 参数）。
 */
async function previewOutput(row: ToolOutput) {
  if (previewingPath.value) return;
  previewingPath.value = row.path;
  try {
    const parsed = await invoke<ChartInfo>('parse_chart', { path: row.path });
    let forced: Partial<RenderConfig> | null = null;
    try {
      forced = await readChartConfig(row.path);
    } catch (error) {
      // 谱面自带的配置读不出来不该挡住预览：按「没带这份配置」处理，原因照旧提示
      fail(error);
    }
    const config = withChartConfig(workingRenderConfig(), forced);
    const info = { ...parsed, ...chartSideOf(config, chartModMarker(config, t, parsed.difficulty)) };
    await invoke('preview_chart', { params: { path: row.path, info, config } });
  } catch (error) {
    fail(error);
  } finally {
    previewingPath.value = '';
  }
}

/** 待确认删除的那一行（`null` = 没有弹窗）。用应用自己的确认框，不弹系统框。 */
const deleteTarget = ref<ToolOutput | null>(null);

/** `v-dialog` 的开关：点遮罩或取消就清掉待删行。 */
const deleteDialog = computed({
  get: () => deleteTarget.value !== null,
  set: (open: boolean) => {
    if (!open) deleteTarget.value = null;
  },
});

function deleteOutput(row: ToolOutput) {
  deleteTarget.value = row;
}

/** 确认删除：删文件、刷新列表；关弹窗放在前面，删失败也有 toast 报原因。 */
async function confirmDelete() {
  const row = deleteTarget.value;
  deleteTarget.value = null;
  if (!row) return;
  try {
    await invoke('delete_file', { path: row.path });
    toast(t('vn-deleted'), 'success');
    await refreshOutputs();
  } catch (error) {
    fail(error);
  }
}

/* ----------------------------- 生成 ----------------------------- */

/** 开始轮询进度：250ms 一次，够画出平滑的进度条，也不至于把 IPC 打满。 */
function startProgress() {
  stopProgress();
  progress.value = { running: true, stage: 'probe', frame: 0, frames: 0 };
  speed.value = 0;
  lastSample = null;
  progressTimer = window.setInterval(async () => {
    try {
      const next = await invoke<ConversionProgress>('tool_video_progress');
      progress.value = next;
      if (next.stage !== 'frames' || !next.frame) return;
      const now = performance.now();
      if (lastSample && next.frame >= lastSample.frame) {
        const seconds = (now - lastSample.at) / 1000;
        if (seconds > 0) {
          const instant = (next.frame - lastSample.frame) / seconds;
          // 抽帧速度本身抖得厉害，用指数滑动平均压一压，读数才不跳
          speed.value = speed.value ? speed.value * 0.6 + instant * 0.4 : instant;
        }
      }
      lastSample = { frame: next.frame, at: now };
    } catch {
      // 轮询失败不该影响生成本身，等下一次
    }
  }, 250);
}

function stopProgress() {
  if (progressTimer !== undefined) {
    window.clearInterval(progressTimer);
    progressTimer = undefined;
  }
}

async function generate() {
  if (!videoPath.value || generating.value) return;
  generating.value = true;
  report.value = null;
  startProgress();
  try {
    report.value = await invoke<ChartReport>('tool_video_convert', {
      params: {
        video: videoPath.value,
        cols: densityCells.value,
        rate: params.value.rate,
        start: params.value.start,
        end: params.value.end,
        tolerance: params.value.tolerance,
        keepAudio: params.value.keepAudio,
        aspect: params.value.aspect,
        name: params.value.name,
        outputDir: params.value.outputDir,
      },
    });
    toast(t('vn-done'), 'success');
    await refreshOutputs();
  } catch (error) {
    fail(error);
  } finally {
    stopProgress();
    generating.value = false;
  }
}

/* ----------------------------- 页面生命周期 ----------------------------- */

const unlisteners: UnlistenFn[] = [];

// 换了输出目录就要换一批文件列表（`useStorage` 的值在别页也可能被改，所以走 watch）
watch(
  () => outputDir.value,
  () => void refreshOutputs(),
);

onMounted(async () => {
  invoke<string>('get_video_tool_dir_path')
    .then((dir) => {
      defaultOutputDir.value = dir;
      void refreshOutputs();
    })
    .catch(() => {});
  invoke<string>('get_app_dir')
    .then((dir) => (appDir.value = dir))
    .catch(() => {});

  // 拖进来的第一个视频直接当输入（与渲染页 / 批量页同一个事件源；那两页已改成离开即解绑，
  // 所以这里能安心收自己的 drop）
  unlisteners.push(await listen('tauri://drag-over', () => (fileHovering.value = true)));
  unlisteners.push(await listen('tauri://drag-leave', () => (fileHovering.value = false)));
  unlisteners.push(
    await listen('tauri://drag-drop', async (event) => {
      fileHovering.value = false;
      const path = (event.payload as { paths: string[] }).paths[0];
      if (path) await loadVideo(path);
    }),
  );

  // 上一次的转换可能还在跑（切走再回来），接着把它的进度显示出来
  try {
    const running = await invoke<ConversionProgress>('tool_video_progress');
    if (running.running) {
      generating.value = true;
      startProgress();
    }
  } catch {
    // 读不到进度不影响页面
  }

  // 揭开内容区那层转圈（见 `../pageLoad`）：这一页没有要等的异步数据
  finishPageLoad();
});

onUnmounted(() => {
  stopProgress();
  for (const unlisten of unlisteners) unlisten();
});
</script>

<template>
  <!-- 版式照「渲染」页：整页滚动 + 贴底操作条，中间是 PhiGroup + cfg-grid -->
  <div class="render-view">
    <div class="render-view__scroll">
      <div class="config-root">
        <!-- 视频横幅：位置与渲染页顶部的曲目信息一致（那里放曲绘，这里放视频信息）。
             左边两行是文件名与路径，四项信息靠右、在横幅里纵向居中。这里只收拖放，没有「选择视频」按钮。 -->
        <section class="vn-hero" :class="{ 'vn-hero--drop': fileHovering }">
          <template v-if="meta">
            <span class="vn-hero__name" :title="videoPath">{{ videoName }}</span>
            <span class="vn-hero__path" :title="videoPath">{{ videoPath }}</span>
            <span class="vn-hero__info">
              <span class="vn-hero__pair"><span class="vn-hero__k">{{ t('vn-duration') }}</span>{{ meta.duration.toFixed(2) }} s</span>
              <span class="vn-hero__pair"><span class="vn-hero__k">{{ t('vn-resolution') }}</span>{{ meta.width }}×{{ meta.height }}</span>
              <span class="vn-hero__pair"><span class="vn-hero__k">{{ t('vn-fps') }}</span>{{ meta.fps.toFixed(2) }}</span>
              <span class="vn-hero__pair"><span class="vn-hero__k">{{ t('vn-audio') }}</span>{{ meta.hasAudio ? t('vn-audio-yes') : t('vn-audio-no') }}</span>
            </span>
          </template>
          <span v-else class="vn-hero__empty">{{ t('vn-drop-video') }}</span>
        </section>

        <!-- 一、生成配置（采样与输出） -->
        <PhiGroup id="grp-vn-config" :title="t('vn-config')">
          <PhiSection id="sec-vn-grid" :title="t('vn-grid')">
            <div class="cfg-grid">
              <CfgSlider
                v-model="params.densityPct"
                :label="t('vn-density')"
                suffix="%"
                :tip="t('vn-density-tip')"
                :min="1"
                :max="100"
                :step="1"
              ></CfgSlider>
              <CfgSlider
                v-model="params.rate"
                :label="t('vn-frame-rate')"
                :tip="t('vn-frame-rate-tip')"
                :min="1"
                :max="maxRate"
                :step="1"
              ></CfgSlider>
              <CfgSlider
                v-model="params.tolerance"
                :label="t('vn-tolerance')"
                :tip="t('vn-tolerance-tip')"
                :min="0"
                :max="64"
                :step="1"
              ></CfgSlider>
            </div>
          </PhiSection>

          <PhiSection id="sec-vn-output" :title="t('vn-output-config')">
            <!-- 六项挤进同一个 cfg-grid：中途换 grid 会让 1/3、2/3 处的竖线断开 -->
            <div class="cfg-grid">
              <CfgField :label="t('vn-aspect')" :tip="t('vn-aspect-tip')">
                <v-select
                  density="compact"
                  hide-details="auto"
                  :items="aspectOptions"
                  item-title="title"
                  item-value="value"
                  v-model="params.aspect"
                ></v-select>
              </CfgField>
              <CfgField :label="t('vn-start')">
                <v-text-field density="compact" hide-details="auto" type="number" v-model.number="params.start"></v-text-field>
              </CfgField>
              <CfgField :label="t('vn-end')">
                <v-text-field density="compact" hide-details="auto" type="number" v-model.number="params.end"></v-text-field>
              </CfgField>
              <CfgField :label="t('vn-name')">
                <v-text-field density="compact" hide-details="auto" v-model="params.name"></v-text-field>
              </CfgField>
              <CfgSwitch :label="t('vn-keep-audio')" v-model="params.keepAudio"></CfgSwitch>
              <CfgField :label="t('vn-dir')" :tip="outputDir">
                <v-text-field
                  :class="{ 'vn-path--default': !params.outputDir }"
                  :model-value="outputShown"
                  readonly
                  density="compact"
                  hide-details="auto"
                  :title="outputDir"
                  append-inner-icon="mdi-folder-open"
                  @click:append-inner="pickOutputDir"
                ></v-text-field>
              </CfgField>
            </div>
          </PhiSection>
        </PhiGroup>

        <!-- 二、输出列表 -->
        <PhiGroup id="grp-vn-list" :title="t('vn-list')">
          <v-table v-if="outputs.length" class="vn-table" density="compact">
            <thead>
              <tr>
                <th class="vn-col-name">{{ t('vn-col-name') }}</th>
                <th>{{ t('vn-col-path') }}</th>
                <th class="vn-col-act">{{ t('vn-col-preview') }}</th>
                <th class="vn-col-act">{{ t('vn-col-delete') }}</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="row in outputs" :key="row.path">
                <td class="vn-col-name">
                  <button type="button" class="vn-link" :title="t('vn-open-render')" @click="openOutput(row)">
                    {{ row.name }}
                  </button>
                </td>
                <td class="vn-path" :title="row.path">{{ row.path }}</td>
                <td class="vn-col-act">
                  <v-btn
                    size="small"
                    variant="text"
                    :loading="previewingPath === row.path"
                    @click="previewOutput(row)"
                    >{{ t('vn-col-preview') }}</v-btn
                  >
                </td>
                <td class="vn-col-act">
                  <v-btn size="small" variant="text" color="error" @click="deleteOutput(row)">{{ t('vn-col-delete') }}</v-btn>
                </td>
              </tr>
            </tbody>
          </v-table>
        </PhiGroup>
      </div>
    </div>

    <!-- 贴底操作条：左边进度，右边只放「打开输出文件夹」与「生成谱面」（样式照渲染页/批量页） -->
    <div class="render-view__actions">
      <div class="vn-progress">
        <template v-if="generating">
          <span class="vn-progress__text">{{ progressText }}</span>
          <v-progress-linear
            :model-value="percent"
            :height="6"
            rounded
            color="primary"
            class="vn-progress__bar"
          ></v-progress-linear>
        </template>
      </div>
      <v-spacer></v-spacer>
      <v-btn variant="text" size="small" @click="openOutputDir">{{ t('vn-open-folder') }}</v-btn>
      <v-btn
        variant="tonal"
        class="gradient-primary"
        :loading="generating"
        :disabled="!videoPath"
        @click="generate"
      >
        {{ t('vn-generate') }}
      </v-btn>
    </div>

    <!-- 删除确认：用应用自己的确认框（与任务页「取消渲染」同一种），不调系统对话框 -->
    <v-dialog v-model="deleteDialog" width="auto" min-width="400px" class="log-card-bg">
      <v-card class="log-card-only-window">
        <v-card-title>{{ t('vn-delete-title') }}</v-card-title>
        <v-card-text>
          <div class="block whitespace-pre overflow-auto">
            {{ t('vn-delete-message', { name: deleteTarget?.name ?? '' }) }}
          </div>
        </v-card-text>
        <v-card-actions class="justify-end">
          <v-btn class="hover-scale" variant="text" @click="deleteDialog = false">{{ t('cancel') }}</v-btn>
          <v-btn class="hover-scale" variant="text" color="error" @click="confirmDelete">
            {{ t('vn-col-delete') }}
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
  </div>
</template>

<style scoped>
/* 三段式壳：与 `RenderView.vue` 同款（整页滚动 + 贴底操作条） */
.render-view {
  display: flex;
  flex: 1;
  flex-direction: column;
  min-height: 0;
  overflow: hidden;
}

.render-view__scroll {
  flex: 1;
  min-height: 0;
  overflow-x: hidden;
  overflow-y: auto;
  padding: var(--phi-space-6) var(--phi-space-6) 0;
}

.render-view__actions {
  display: flex;
  flex: none;
  align-items: center;
  gap: var(--phi-space-2);
  padding: var(--phi-space-2) var(--phi-space-6);
  border-top: 0.5px solid var(--phi-border-l3);
}

/* 左下角进度：一行文字 + 细条（粗细与任务页卡片的进度条一致），条占满剩下的宽度 */
.vn-progress {
  display: flex;
  flex: 0 1 380px;
  align-items: center;
  gap: var(--phi-space-3);
  min-width: 0;
}

.vn-progress__bar {
  flex: 1 1 auto;
  min-width: 120px;
}

.vn-progress__text {
  flex: none;
  font-size: 0.8rem;
  color: var(--phi-label-tertiary);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

/* 视频横幅：左边文件名 + 路径两行，四项信息靠右、纵向居中 */
.vn-hero {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  grid-template-rows: auto auto;
  column-gap: var(--phi-space-5);
  row-gap: 2px;
  align-items: center;
  min-height: 56px;
  margin-bottom: var(--phi-space-4);
  border-radius: var(--phi-radius-md);
  background: var(--phi-bg-layer-2);
  padding: var(--phi-space-3) var(--phi-space-5);
  overflow: hidden;
}

.vn-hero--drop {
  outline: 1px dashed var(--phi-accent);
}

/* 横幅左边只留文件名与路径 */
.vn-hero__name {
  grid-area: 1 / 1;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: 18px;
  font-weight: 600;
  line-height: 1.2;
  color: var(--phi-label-primary);
}

/* 右侧那列跨两行，纵向就自动居中了 */
.vn-hero__info {
  grid-area: 1 / 2 / span 2;
  display: flex;
  align-items: center;
  gap: var(--phi-space-5);
  white-space: nowrap;
}

.vn-hero__pair {
  display: inline-flex;
  flex: none;
  align-items: baseline;
  gap: var(--phi-space-2);
  font-size: 0.85rem;
  color: var(--phi-label-secondary);
}

.vn-hero__k {
  color: var(--phi-label-tertiary);
}

/* 路径：单行、超出加省略号，完整值在 title 里 */
.vn-hero__path {
  grid-area: 2 / 1;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: 0.7rem;
  line-height: 1.35;
  color: var(--phi-label-caption);
  user-select: text;
}

.vn-hero__empty {
  grid-area: 1 / 1 / span 2 / span 2;
  justify-self: center;
  font-size: 0.95rem;
  color: var(--phi-label-tertiary);
}

/* 输出目录框里写的是默认路径（用户没自己选过）时按灰字显示，跟设置页那几格一致 */
.vn-path--default :deep(.v-field__input),
.vn-path--default :deep(.v-field input) {
  color: var(--phi-label-tertiary);
  opacity: var(--v-disabled-opacity, 0.5);
}

/* 输出列表：表格线只留行间一条细线，列宽按内容走 */
.vn-table {
  width: 100%;
  table-layout: fixed;
  background: transparent;
  font-size: 0.85rem;
}

.vn-table :deep(th) {
  height: 34px;
  font-size: 0.8rem;
  font-weight: 500;
  color: var(--phi-label-tertiary);
  border-bottom: 0.5px solid var(--phi-border-l3);
}

.vn-table :deep(td) {
  height: 38px;
  padding-top: 0;
  padding-bottom: 0;
  border-bottom: 0.5px solid var(--phi-border-l3);
}

.vn-table :deep(tbody tr:hover) {
  background: var(--phi-bg-hover, transparent);
}

.vn-col-name {
  width: 26%;
}

.vn-col-act {
  width: 110px;
  text-align: center;
}

.vn-table :deep(.vn-col-act) {
  text-align: center;
}

.vn-link {
  padding: 0;
  border: none;
  background: none;
  font: inherit;
  /* 文件名保持普通文字色，不用强调色 —— 整张表里最跳的该是数据本身 */
  color: var(--phi-label-primary);
  cursor: pointer;
}

.vn-link:hover {
  text-decoration: underline;
}

/* 路径列：单行、超出加省略号，完整值在 title 里 */
.vn-path {
  overflow: hidden;
  max-width: 0;
  white-space: nowrap;
  text-overflow: ellipsis;
  color: var(--phi-label-caption);
  user-select: text;
}

/* 「生成谱面」用渲染页那颗按钮的蓝底。
   `.gradient-primary` 原本写在 `ConfigView.vue` 的 scoped 样式里，别的页面拿不到，这里补一份同样的规则。 */
.gradient-primary {
  background: var(--phi-accent) !important;
  color: var(--phi-label-inverted) !important;
}
</style>
