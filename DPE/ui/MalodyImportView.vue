<script setup lang="ts">
/**
 * Malody 转谱页：把一份 Malody 键位谱（`.mc` / `.mcz` / 谱面文件夹）转成 Phigros 谱面包（`.pez`）。
 *
 * 转换在 Rust 侧做（`DPE/src/malody.rs`，由 DPR 的命令层按路径引用），这一页只负责三件事：
 * 挑输入、选要转的那张谱面、给三个排版参数；产物落进 `<程序目录>/dpe/malody/`，
 * 与「谱面修改」的产物分开放，找东西时按页面就能定位。
 *
 * 版式与「噪域播放器」页一致：顶部一条横幅、中间分组参数、下面输出列表、操作条贴底。
 * 横幅直接用渲染页那块曲目信息（`ChartHero`），把谱面拖到它上面就导入 ——
 * 输入只收拖放，没有「选择文件」按钮；横幅下面（也就是它的 body）是这个谱面包里能转的谱面。
 */
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRouter } from 'vue-router';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useStorage } from '@vueuse/core';

import CfgField from '@/components/CfgField.vue';
import ChartHero from '@/components/ChartHero.vue';
import PhiGroup from '@/components/PhiGroup.vue';
import PhiSection from '@/components/PhiSection.vue';
import { toast, toastError } from '@/common';
import { DEFAULT_RENDER_CONFIG } from '@/model';
import type { ChartInfo } from '@/model';
import { finishPageLoad } from '@/pageLoad';

const { t } = useI18n();
const router = useRouter();

/**
 * 三个排版量：含义与 DPR「多K模式」的三个同名参数一致（`fixed_lane_line_y` /
 * `fixed_lane_gap` / `fixed_lane_speed`），取一组常用的定轨排布当默认值。
 */
const options = useStorage('MalodyImportView.Options', { lineY: 0.12, gap: 0.18, speed: 2.7 });

/** 包内的一张谱面（字段与 `DPE/src/malody.rs` 的 `Entry` 一一对应） */
interface MalodyEntry {
  index: number;
  file: string;
  title: string;
  artist: string;
  charter: string;
  version: string;
  column: number;
  notes: number;
  holds: number;
  duration: number;
  bpm: number;
  audio: string | null;
  audio_offset: number;
  illustration: string | null;
}

interface MalodyProbe {
  charts: MalodyEntry[];
  audio: string[];
  illustration: string[];
  /** 真正打开的那一层：拖进来的可能是曲包外层，后端会自动认下里面的谱面层 */
  root: string;
}

interface Converted {
  path: string;
  notes: number;
  name: string;
}

/** 输出目录里的一件产物（后端 `noise_player::ToolOutput`，那个命令只按目录列 `.pez`） */
interface ToolOutput {
  name: string;
  path: string;
  size: number;
  modified: number;
}

/* 上一次导入的谱面记在 localStorage 里，刷新页面接着用（重进时重新探一遍元数据，
   包要是被删了/搬走了就把记录清掉，页面回到空态）。 */
const inputPath = useStorage('MalodyImportView.Input', '');
const probe = ref<MalodyProbe | null>(null);
const probing = ref(false);
/** 选中包里第几张谱面（一个 Malody 包可能有好几张）：连同导入的包一起记住 */
const selected = useStorage('MalodyImportView.Selected', 0);
const converting = ref(false);
const previewingPath = ref('');
/** 拖放时给横幅加一圈虚线，告诉用户「这里能放」 */
const fileHovering = ref(false);
/** 产物列表，以及它所在的目录（目录由后端给，页面上不给改） */
const outputs = ref<ToolOutput[]>([]);
const outputDir = ref('');

const chart = computed(() => probe.value?.charts.find((it) => it.index === selected.value) ?? null);
const statusText = computed(() => (probing.value ? t('md-loading') : converting.value ? t('md-converting') : ''));
/**
 * 喂给横幅的谱面信息。Malody 的元数据只有曲名 / 曲师 / 谱师 / 难度串（形如 `4K HD Lv.22`），
 * 所以定数从难度串里拆、档位是去掉结尾 `Lv.x` 的那一段；曲绘优先用谱面自己指定的那张，
 * 没指定就退回包里找到的第一张图。
 */
const heroInfo = computed<Partial<ChartInfo> | undefined>(() => {
  const entry = chart.value;
  if (!entry) return undefined;
  const lvl = /lv\.?\s*([\d.]+)/i.exec(entry.version)?.[1];
  return {
    name: entry.title,
    composer: entry.artist,
    charter: entry.charter,
    difficulty: entry.version,
    diff: entry.version.replace(/\s*lv\.?\s*[\d.]+$/i, '').trim(),
    lvl: lvl ? Number(lvl) : 0,
    illustration: entry.illustration ?? probe.value?.illustration[0] ?? '',
  };
});

/**
 * 读曲绘的根：以后端真正打开的那一层为准（单张 `.mc` 不是压缩包，读不出包内文件，
 * 后端给的是它所在的文件夹；拖的是曲包外层时后端已经认下里面的谱面层）。
 */
const assetRoot = computed(() => probe.value?.root || inputPath.value);

async function probePath(path: string) {
  inputPath.value = path;
  probe.value = null;
  selected.value = 0;
  probing.value = true;
  try {
    probe.value = await invoke<MalodyProbe>('dpe_malody_probe', { path });
    if (!probe.value.charts.length) toast(t('md-no-charts'), 'warning');
  } catch (error) {
    toastError(error);
  } finally {
    probing.value = false;
  }
}

/** 能收的东西：`.mc` / `.mcz`，或者一个文件夹（文件夹没有后缀） */
function isMalodyPath(path: string) {
  const name = path.slice(folderOf(path).length + 1);
  return !name.includes('.') || /\.mcz?$/i.test(name);
}

async function dropPath(path: string) {
  if (!isMalodyPath(path)) {
    toast(t('md-not-malody'), 'warning');
    return;
  }
  await probePath(path);
}

async function convert() {
  if (!probe.value) return;
  converting.value = true;
  try {
    const result = await invoke<Converted>('dpe_malody_convert', {
      path: inputPath.value,
      chart: selected.value,
      audio: null,
      illustration: null,
      options: {
        line_y: Number(options.value.lineY),
        gap: Number(options.value.gap),
        speed: Number(options.value.speed),
      },
    });
    toast(t('md-done', { notes: result.notes }), 'success');
    await refreshOutputs();
  } catch (error) {
    toastError(error);
  } finally {
    converting.value = false;
  }
}

/** 路径里最后一个分隔符之前那一段（也就是它所在的文件夹） */
function folderOf(path: string): string {
  const at = Math.max(path.lastIndexOf('\\'), path.lastIndexOf('/'));
  return at > 0 ? path.slice(0, at) : path;
}

/** 列一遍输出目录里的谱面包；目录在第一次调用时问后端要 */
async function refreshOutputs() {
  try {
    if (!outputDir.value) outputDir.value = await invoke<string>('get_dpe_malody_dir_path');
    outputs.value = await invoke<ToolOutput[]>('noise_player_outputs', { dir: outputDir.value });
  } catch {
    // 列不出来（目录还不存在 / 权限）就当空的，不要把整页卡住
    outputs.value = [];
  }
}

function openOutputDir() {
  if (!outputDir.value) return;
  invoke('open_dir', { path: outputDir.value }).catch(toastError);
}

/** 点产物名字直接用渲染页打开它 */
function openOutput(row: ToolOutput) {
  router.push({ name: 'render', query: { chart: row.path } });
}

async function previewOutput(row: ToolOutput) {
  previewingPath.value = row.path;
  try {
    await invoke('dpe_preview', { path: row.path, config: DEFAULT_RENDER_CONFIG });
  } catch (error) {
    toastError(error);
  } finally {
    previewingPath.value = '';
  }
}

/** 产物列表里的「移除」：直接从磁盘删掉，不弹确认框 —— 删错了重新转一次就是。 */
async function deleteOutput(row: ToolOutput) {
  try {
    await invoke('delete_file', { path: row.path });
    await refreshOutputs();
  } catch (error) {
    toastError(error);
  }
}

/** 拖放的解绑函数：不解绑的话，离开这一页之后拖东西还会被这里抢走 */
const unlisteners: UnlistenFn[] = [];

/** 刷新后接着用上次导入的那个包：只重探，不弹错误（包没了就当没导入过）。 */
async function restorePath() {
  const path = inputPath.value;
  if (!path) return;
  probing.value = true;
  try {
    probe.value = await invoke<MalodyProbe>('dpe_malody_probe', { path });
    if (!probe.value.charts.length) toast(t('md-no-charts'), 'warning');
    // 记住的那张谱面可能已经不在包里了（包被换过），那就退回第一张
    if (!probe.value.charts.some((it) => it.index === selected.value)) {
      selected.value = probe.value.charts[0]?.index ?? 0;
    }
  } catch {
    probe.value = null;
    inputPath.value = '';
  } finally {
    probing.value = false;
  }
}

onMounted(async () => {
  unlisteners.push(await listen('tauri://drag-over', () => (fileHovering.value = true)));
  unlisteners.push(await listen('tauri://drag-leave', () => (fileHovering.value = false)));
  unlisteners.push(
    await listen<{ paths: string[] }>('tauri://drag-drop', (event) => {
      fileHovering.value = false;
      const path = event.payload.paths[0];
      if (path) void dropPath(path);
    }),
  );
  await restorePath();
  await refreshOutputs();
  finishPageLoad();
});

onUnmounted(() => {
  for (const unlisten of unlisteners) unlisten();
});
</script>

<template>
  <!-- 版式照「噪域播放器」页：顶部横幅 + 分组参数 + 输出列表，操作条贴底 -->
  <div class="render-view">
    <div class="render-view__scroll">
      <div class="config-root">
        <!-- 曲目信息横幅：与渲染页顶部那块同一个组件，拖放谱面到此处即可导入；
             它下面跟着这个谱面包里能转的谱面（导入之后才有内容，也才有折叠箭头） -->
        <PhiSection
          id="sec-md-chart"
          class="md-hero"
          :class="{ 'md-hero--drop': fileHovering }"
          variant="hero"
          :collapsible="!!chart"
        >
          <template #head>
            <ChartHero :info="heroInfo" :path="inputPath" :asset-path="assetRoot"></ChartHero>
          </template>
          <v-table v-if="probe?.charts.length" class="md-table" density="compact">
            <thead>
              <tr>
                <th class="md-col-version">{{ t('md-col-version') }}</th>
                <th class="md-col-num">{{ t('md-col-keys') }}</th>
                <th class="md-col-num">{{ t('md-col-notes') }}</th>
                <th class="md-col-num">{{ t('md-col-holds') }}</th>
                <th class="md-col-num">{{ t('md-col-duration') }}</th>
                <th class="md-col-num">{{ t('md-col-bpm') }}</th>
                <th>{{ t('md-col-file') }}</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="row in probe.charts"
                :key="row.file"
                class="md-row"
                :class="{ 'md-row--active': row.index === selected }"
                @click="selected = row.index"
              >
                <td class="md-col-version">{{ row.version || '-' }}</td>
                <td class="md-col-num">{{ row.column }}</td>
                <td class="md-col-num">{{ row.notes }}</td>
                <td class="md-col-num">{{ row.holds }}</td>
                <td class="md-col-num">{{ row.duration.toFixed(2) }} s</td>
                <td class="md-col-num">{{ row.bpm }}</td>
                <td class="md-path" :title="row.file">{{ row.file }}</td>
              </tr>
            </tbody>
          </v-table>
        </PhiSection>

        <PhiGroup id="grp-md-config" :title="t('md-config')">
          <PhiSection id="sec-md-layout" :title="t('md-layout')">
            <div class="cfg-grid">
              <CfgField :label="t('md-line-y')" :tip="t('md-line-y-tip')">
                <v-text-field
                  density="compact"
                  hide-details="auto"
                  type="number"
                  :min="0"
                  :max="1"
                  :step="0.01"
                  v-model.number="options.lineY"
                ></v-text-field>
              </CfgField>
              <CfgField :label="t('md-gap')" :tip="t('md-gap-tip')">
                <v-text-field
                  density="compact"
                  hide-details="auto"
                  type="number"
                  :min="0.02"
                  :max="0.5"
                  :step="0.01"
                  v-model.number="options.gap"
                ></v-text-field>
              </CfgField>
              <CfgField :label="t('md-speed')" :tip="t('md-speed-tip')">
                <v-text-field
                  density="compact"
                  hide-details="auto"
                  type="number"
                  :min="0.5"
                  :max="10"
                  :step="0.1"
                  v-model.number="options.speed"
                ></v-text-field>
              </CfgField>
            </div>
          </PhiSection>
        </PhiGroup>

        <!-- 输出列表：目录里已有的谱面包，名字点了直接用渲染页打开 -->
        <PhiGroup id="grp-md-list" :title="t('md-output')">
          <v-table v-if="outputs.length" class="md-table" density="compact">
            <thead>
              <tr>
                <th class="md-col-name">{{ t('md-col-name') }}</th>
                <th>{{ t('md-col-path') }}</th>
                <th class="md-col-act">{{ t('md-preview') }}</th>
                <th class="md-col-act">{{ t('md-col-delete') }}</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="row in outputs" :key="row.path">
                <td class="md-col-name">
                  <button type="button" class="md-link" :title="t('md-open-render')" @click="openOutput(row)">
                    {{ row.name }}
                  </button>
                </td>
                <td class="md-path" :title="row.path">{{ row.path }}</td>
                <td class="md-col-act">
                  <v-btn
                    size="small"
                    variant="text"
                    :loading="previewingPath === row.path"
                    @click="previewOutput(row)"
                    >{{ t('md-preview') }}</v-btn
                  >
                </td>
                <td class="md-col-act">
                  <v-btn size="small" variant="text" color="error" @click="deleteOutput(row)">
                    {{ t('md-col-delete') }}
                  </v-btn>
                </td>
              </tr>
            </tbody>
          </v-table>
        </PhiGroup>
      </div>
    </div>

    <!-- 贴底操作条：左边状态，右边只放「打开输出文件夹」与「转换」 -->
    <div class="render-view__actions">
      <span class="md-status">{{ statusText }}</span>
      <v-spacer></v-spacer>
      <v-btn variant="text" size="small" @click="openOutputDir">{{ t('md-open-dir') }}</v-btn>
      <v-btn
        variant="tonal"
        class="gradient-primary"
        :loading="converting"
        :disabled="!probe"
        @click="convert"
      >
        {{ t('md-convert') }}
      </v-btn>
    </div>
  </div>
</template>

<style scoped>
/* 三段式壳：与「噪域播放器」页同款（整页滚动 + 贴底操作条） */
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

/* 曲目信息横幅：下面接着这一组谱面，与渲染页「横幅 + 谱面信息」同一种排法 */
.render-view .md-hero {
  margin-bottom: var(--phi-space-4);
}

/* 拖着谱面进来时给横幅加一圈虚线，位置与大小都不变，不会让整页抖一下 */
.render-view .md-hero--drop {
  outline: 1px dashed var(--phi-accent);
  outline-offset: -1px;
}

/* 两张表同一种形状：只留行间一条细线，列宽按内容走 */
.md-table {
  width: 100%;
  table-layout: fixed;
  background: transparent;
  font-size: 0.85rem;
  font-variant-numeric: tabular-nums;
}

.md-table :deep(th) {
  height: 34px;
  font-size: 0.8rem;
  font-weight: 500;
  color: var(--phi-label-tertiary);
  border-bottom: 0.5px solid var(--phi-border-l3);
}

.md-table :deep(td) {
  height: 38px;
  padding-top: 0;
  padding-bottom: 0;
  border-bottom: 0.5px solid var(--phi-border-l3);
}

.md-table :deep(tbody tr:hover) {
  background: var(--phi-interactive-hover);
}

/* 谱面列表：难度一列宽一点，数值列靠右对齐 */
.md-col-version {
  width: 30%;
}

.md-col-num {
  width: 92px;
  text-align: right;
}

.md-table :deep(.md-col-num) {
  text-align: right;
}

/* 产物列表：名字一列、操作列定宽居中 */
.md-col-name {
  width: 26%;
}

.md-col-act {
  width: 110px;
  text-align: center;
}

.md-table :deep(.md-col-act) {
  text-align: center;
}

/* 路径列：单行、超出加省略号，完整值在 title 里 */
.md-path {
  overflow: hidden;
  max-width: 0;
  white-space: nowrap;
  text-overflow: ellipsis;
  color: var(--phi-label-caption);
  user-select: text;
}

.md-link {
  padding: 0;
  border: none;
  background: none;
  font: inherit;
  color: var(--phi-label-primary);
  cursor: pointer;
}

.md-link:hover {
  text-decoration: underline;
}

/* 难度选取：整行可点，hover 有底色，选中的那行再亮一档并在左边压一道强调条 */
.md-row {
  cursor: pointer;
  transition: background 0.12s ease;
}

.md-row--active {
  background: var(--phi-interactive-hover-accent);
}

.md-table :deep(.md-row--active td:first-child) {
  box-shadow: inset 2px 0 0 var(--phi-accent);
}

.md-table :deep(.md-col-version) {
  color: var(--phi-label-primary);
  font-weight: 500;
}

/* 操作条左边的状态字：探明 / 转换的进度没有百分比，只报一句在做什么 */
.md-status {
  font-size: 0.8rem;
  color: var(--phi-label-tertiary);
}

/* 「转换」用渲染页那颗按钮的蓝底。
   `.gradient-primary` 原本写在 `ConfigView.vue` 的 scoped 样式里，别的页面拿不到，这里补一份同样的规则。 */
.gradient-primary {
  background: var(--phi-accent) !important;
  color: var(--phi-label-inverted) !important;
}
</style>
