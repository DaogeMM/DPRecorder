<script setup lang="ts">
/**
 * 批量页：一次丢进一批谱面，按同一份预设配置批量渲染。
 *
 * 版式 = 「任务页的卡片」+「渲染页的谱面信息」的融合：
 *   折叠行  曲绘 / 曲名 / 难度徽章 / 展开箭头（与任务页 `.task-row` 同一套尺寸与配色）
 *   展开后  上半是谱面横幅（`ChartHero`，与渲染页顶部同一份），下半是谱面信息表单
 *           （`ChartInfoForm`，字段与渲染页 `#chart-info` 逐条对齐）
 *   底排    筛选 · 排序 · 移除全部任务 · 三个开关；预设配置 …… 全选/反选/移除/取消/渲染
 *
 * 三条刻意为之：
 *   - **不显示渲染进度**：进度、状态、输出日志都在任务页看，这里只负责「选谱面 + 发任务」。
 *   - **曲绘管预览、曲名管进渲染页**：和任务页「点行=展开」分工不同，所以两个都是
 *     `@click.stop`，不再跟着整行切换展开。
 *   - **排序只按曲名 / 定数**，且放在底排：谱面列表的顺序是用户自己看着顺眼，
 *     不像任务页那样按队列顺序有意义。定数取 `ChartInfo.lvl`（解析不出来时从难度
 *     文本里抠数字），同一档里再按难度文本、曲名兜底。
 *
 * 两个坑：`ChartInfoForm` 自己持有 `v-form` 的 ref，渲染前必须逐张调 `validate()`
 * （卡片收起来时组件根本没挂载，也就没有要校验的东西）；谱面 id 用「当前最大值 + 1」，
 * 它是列表的 key，重复会让 Vue 复用错行。
 */
import { ref, nextTick, onUnmounted, computed, watch } from 'vue';
import { useStorage } from '@vueuse/core';

import { useI18n } from 'vue-i18n';

import { invoke } from '@tauri-apps/api/core';

import { toastError, toast } from './common';
import { formatLvlInText } from './lvl';
import { diffTier, diffLvl } from './tasks';
import { DEFAULT_RENDER_CONFIG, type ChartInfo, type RenderConfig, type RenderChart, type Preset, type FileDropEvent, type Task } from './model';
import { sameRenderConfig } from './config/useRenderConfig';
import { chartModMarker, chartSideOf } from './config/chartSide';
import { jumpToFirstError, type FormError } from './formError';
import { finishPageLoad } from './pageLoad';
import router from './router';

import ConfigView from './components/ConfigView.vue';
import CfgSwitch from './components/CfgSwitch.vue';
import ChartHero from './components/ChartHero.vue';
import ChartInfoForm from './components/ChartInfoForm.vue';
import ChartThumb from './components/ChartThumb.vue';

import { listen } from "@tauri-apps/api/event";

const { t } = useI18n();

const charts = useStorage<RenderChart[]>('BatchView.ChartList', []);

/** 新的谱面 id：列表 key 与「这一张的谱面信息表单」都靠它，必须唯一。 */
function nextChartId(): number {
  return charts.value.reduce((max, chart) => Math.max(max, chart.id), -1) + 1;
}

/** 展开了哪些卡片（只记 id，重新排序不影响谁开着） */
const expandedIds = ref<number[]>([]);

function toggleExpanded(id: number) {
  const index = expandedIds.value.indexOf(id);
  if (index >= 0) expandedIds.value.splice(index, 1);
  else expandedIds.value.push(id);
}

/**
 * 展开的卡片各有一份谱面信息表单 —— 渲染前要校验**这一张**的表单，
 * 所以按 id 记住组件实例（收起时 Vue 会以 null 回调，正好把记录删掉）。
 */
interface InfoFormApi {
  validate(): Promise<{ valid: boolean; errors: FormError[] } | undefined>;
}
const infoForms = new Map<number, InfoFormApi>();

function setInfoForm(id: number, instance: unknown) {
  if (instance) infoForms.set(id, instance as InfoFormApi);
  else infoForms.delete(id);
}

const DEFAULT_PRESET: Preset = {
  name: t('default-preset'),
  key: 'default',
  config: DEFAULT_RENDER_CONFIG,
};

async function getPresets() {
  let result = [DEFAULT_PRESET];
  let pairs = (await invoke('get_presets')) as Record<string, RenderConfig>;
  for (let key of Object.keys(pairs).sort()) {
    result.push({
      name: key,
      key,
      config: pairs[key]!,
    });
  }
  return result;
}
const presets = ref([DEFAULT_PRESET]);
const preset = useStorage<Preset>('BatchView.Preset', DEFAULT_PRESET);
async function updatePresets() {
  presets.value = await getPresets();
  if (preset.value.key !== 'temp') {
    preset.value = presets.value.find((x) => x.key === preset.value.key) || presets.value[0];
  }
}
// 预设列表到位了才揭开内容区那层转圈（见 `./pageLoad`）；读失败也要揭开，别让页面一直盖着
void updatePresets().finally(finishPageLoad);

const loadingParsingChart = ref(false);
const loadingPreview = ref(false);
const loadingPostRender = ref(false);

/**
 * 「预设配置」编辑弹窗的宽度 = **内容区宽度的 100%**：配置条目比谱面信息多得多，
 * 按内容区 60% 算（齿轮弹窗那个算法）两栏挤得看不清。弹窗被 teleport 到 body，
 * 写百分比会按视口算（把左侧导航也算进去），所以每次打开时量一次。
 */
const presetDialogWidth = ref(880);
function measurePresetDialogWidth() {
  const content = document.querySelector<HTMLElement>('.phi-center');
  const base = content?.clientWidth || window.innerWidth;
  presetDialogWidth.value = Math.max(480, base);
}
const disSelectStartRender = useStorage<boolean>('BatchView.disSelectStartRender', true);
const removeStartRender = useStorage<boolean>('BatchView.removeStartRender', false);
const removeAfterRender = useStorage<boolean>('BatchView.removeAfterRender', false);

async function loadCharts(files: string[]) {
  loadingParsingChart.value = true;
  for (let file of files) {
    try {
      let chartInfo: ChartInfo = (await invoke('parse_chart', { path: file }));
      charts.value.push({
        id: nextChartId(),
        path: file,
        isSelect: true,
        taskId: null,
        output: '',
        status: { type: 'null' },
        chartInfo: chartInfo,
      });
    } catch (e) {
      toastError(e);
    }
  }
  applySort();
  loadingParsingChart.value = false;
}

/** 关键词筛选：空格分隔多个关键词，全部命中才留下。 */
const chartFilter = useStorage<string | null>('BatchView.Filter', '');

const filterTerms = computed(() =>
  (chartFilter.value ?? '')
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter((term) => term.length > 0)
);

const filteredCharts = computed(() => {
  const terms = filterTerms.value;
  if (terms.length === 0) return charts.value;
  return charts.value.filter((chart) => {
    const haystack = [
      chart.chartInfo.name,
      chart.chartInfo.difficulty,
      chart.chartInfo.diff,
      chartLevel(chart),
      chart.chartInfo.charter,
      chart.chartInfo.composer,
      chart.path,
    ]
      .join(' ')
      .toLowerCase();
    return terms.every((term) => haystack.includes(term));
  });
});

const hasSelection = computed(() => charts.value.some((chart) => chart.isSelect));

/** 选中的张数：谱面融合至少要两张 */
const selectedCount = computed(() => charts.value.filter((chart) => chart.isSelect).length);

const fileHovering = ref(false);
let files: string[] = [];
listen('tauri://drag-over', () => {
  if (loadingParsingChart.value) return;
  fileHovering.value = true
});
listen('tauri://drag-leave', () => {
  fileHovering.value = false
});
listen('tauri://drag-drop', async (event) => {
  fileHovering.value = false
  const newFiles = (event.payload as FileDropEvent).paths;
  files.push(...newFiles);
  if (loadingParsingChart.value) return;
  await loadCharts(files);
  files = [];
});

async function buildParams(chart: RenderChart) {
  // 校验不过就返回 null，调用方（postRender / previewChart）会直接放弃。
  if (!(await checkInfo(chart))) return null;
  const config = preset.value.config;
  // 预览片段窗口不超过 15 秒：预览开始 / 结束时间现在是**配置项**，所以这条校验跟着配置走
  // （渲染页那边由 ConfigView.buildConfig() 保证；批量页直接读预设里那份 config）。
  if (config.previewEnd && config.previewEnd - config.previewStart > 15) {
    toast(t('error.preview-start-end-15s'), 'error');
    return null;
  }
  // 谱面侧的渲染参数（宽高比 / 强制宽高比 / 折叠式入场 / 判定线长度 / 总分 / 预览起止 / 偏移 /
  // 背景压暗 …）现在是 `RenderConfig` 的字段：**以配置为准**，所以展开在谱面信息**之后**
  // 覆盖同名键（与 RenderView.buildParams 同一套做法，见 ./config/chartSide.ts）。
  // 难度文本同样以配置为准：开着「谱面修改」时换成 `（功能名 → 功能名）` 的标记，
  // 融合过的谱面把「谱面融合」摆在最前面。
  return {
    path: chart.path,
    info: { ...chart.chartInfo, ...chartSideOf(config, chartModMarker(config, t, chart.chartInfo.difficulty)) },
    config,
  };
}

async function postRender(chart: RenderChart) {
  try {
    // 去掉自动调整宽高比 ⇒ 这里不再复制、也不再按谱面宽高比改分辨率，
    // 提交的就是预设里那份 config（`invoke` 会序列化一份给后端，前端这份不会被改脏）。
    // 顺带说明：早先这里曾用 `structuredClone(preset.value.config)`，而 `preset.value.config`
    // 是 Vue 的响应式代理，代理塞进 structuredClone 会抛 DataCloneError —— 要复制就用
    // `{ ...toRaw(...) }`（现在这段已经不需要了，留个记号免得以后再踩）。
    let params = await buildParams(chart);
    if (!params) return false;
    await invoke('post_render', { params });
    let tasks = await invoke<Task[]>('get_tasks');
    chart.taskId = tasks[0]!.id;
    chart.output = tasks[0]!.output;
  } catch (e) {
    toastError(e);
    return false;
  }
  if (disSelectStartRender.value) chart.isSelect = false;
  return true;
}

async function postSelectRender() {
  loadingPostRender.value = true;
  if (removeStartRender.value) {
    for (let i = charts.value.length - 1; i >= 0; i--) {
      if (!loadingPostRender.value) break;
      if (charts.value[i]!.isSelect) {
        await postRender(charts.value[i]!);
        charts.value.splice(i, 1);
      }
    }
  } else {
    for (let chart of charts.value) {
      if (!loadingPostRender.value) break;
      if (chart.isSelect) {
        await postRender(chart);
      }
    }
  }
  loadingPostRender.value = false;
}

function removeSelectChart() {
  for (let i = charts.value.length - 1; i >= 0; i--) {
    if (charts.value[i]!.isSelect) {
      charts.value.splice(i, 1)
    }
  }
}

async function previewChart(chart: RenderChart) {
  loadingPreview.value = true;
  try {
    let params = await buildParams(chart);
    if (!params) return false;
    await invoke('preview_chart', { params });
    return true;
  } catch (e) {
    toastError(e);
    return false;
  } finally {
    loadingPreview.value = false
  }
}

async function checkInfo(chart: RenderChart) {
  // 卡片收起来时表单根本没挂载（`infoForms` 里没有这一条）——那就只做收尾，不校验。
  const form = infoForms.get(chart.id);
  if (form) {
    const { valid, errors } = (await form.validate()) ?? { valid: true, errors: [] };
    if (!valid) {
      // 标红的小字在表单里，用户看不到：把出错的控件滚进视野再提示
      toast(jumpToFirstError(errors) ?? t('has-error'), 'error');
      return false;
    }
  }
  // 「预览开始 / 结束时间」与「偏移」现在是配置项（`RenderConfig`），
  // 15 秒窗口那条校验搬到了 `buildParams()`（跟着配置走，见那边的注释），
  // 这里只剩谱面信息自己的收尾：Tip 是空串就写 null。
  if (!chart.chartInfo.tip?.trim().length) chart.chartInfo.tip = null;

  return true;
}

function selectAll() {
  filteredCharts.value.forEach(chart => {
    chart.isSelect = true
  })
}

function selectInvert() {
  filteredCharts.value.forEach(chart => {
    chart.isSelect = !chart.isSelect
  })
}

const configView = ref<typeof ConfigView>();
const presetDialog = ref(false);
/** 这份临时配置是从哪个预设改出来的（`*` 后面跟的就是它的名字）。
 *  单独存一份是为了别叠成 `**(已编辑)`：临时配置自己的名字每次都由它重算。 */
const presetBaseName = useStorage<string>('BatchView.PresetBaseName', '');

/** 把界面上的配置收成「工作中的临时配置」：自动保存，而且**不写进任何命名预设** */
function setWorkingPreset(config: RenderConfig) {
  // 值没变就别动（编辑弹窗打开时 ConfigView 也会发一次 change，
  // 不加这道判断的话，光打开弹窗就会把预设名变成 `*原名`）。
  // 用 `sameRenderConfig` 而不是 `JSON.stringify` 比：预设里存的旧配置可能少键、键序也不同。
  if (sameRenderConfig(config, preset.value.config)) return;
  const base = presetBaseName.value || t('default-preset');
  presetBaseName.value = base;
  preset.value = { name: '*' + base, key: 'temp', config };
}

async function editPreset() {
  // 记住「从哪个预设改出来的」（已经是 `temp` 就保持原来记的那个名字）
  if (preset.value.key !== 'temp') presetBaseName.value = preset.value.name;
  measurePresetDialogWidth();
  presetDialog.value = true;
  await nextTick();
  await configView.value?.applyConfig(preset.value.config);
}

/** 列表只按这两种排：曲名、定数（`ChartInfo.lvl`，解析不出来时从难度文本里抠数字） */
type SortKey = 'name' | 'level';

const sortKey = useStorage<SortKey>('BatchView.Sort', 'name');
const sortAsc = useStorage<boolean>('BatchView.SortAsc', true);

// 旧版本存的是「按难度文本」那个值：认不出来就退回曲名，免得下拉框里一片空白
if (sortKey.value !== 'name' && sortKey.value !== 'level') sortKey.value = 'name';

const sortItems = computed(() => [
  { value: 'name' as SortKey, title: t('info.name') },
  { value: 'level' as SortKey, title: t('info.lvl') },
]);

/**
 * 难度定数。`ChartInfo.lvl` 是解析 `"IN Lv.15.3"` 得到的数字；
 * 谱面写的格式不标准时 `lvl` 会停在默认值，这时从难度文本里抠第一个数字兜底。
 */
function chartLevel(chart: RenderChart): number {
  let lvl: unknown = chart.chartInfo.lvl;
  if (typeof lvl === 'string') lvl = parseFloat(lvl);
  if (typeof lvl === 'number' && Number.isFinite(lvl) && lvl > 0) return lvl;
  const matched = /(\d+(?:\.\d+)?)/.exec(chart.chartInfo.difficulty ?? '');
  return matched ? parseFloat(matched[1]!) : 0;
}

function textCompare(a: string | null | undefined, b: string | null | undefined) {
  return (a ?? '').localeCompare(b ?? '', undefined, { numeric: true, sensitivity: 'case', caseFirst: 'upper' });
}

/** 同一档定数里再分级的次级键：谱面写了 `difficulty` 就用它（定数补成一位小数），否则拿渲染出的档位凑 */
function difficultyText(chart: RenderChart): string {
  const text = (chart.chartInfo.difficulty ?? '').trim();
  if (text) return formatLvlInText(text);
  const lvl = chartLevel(chart);
  const diff = (chart.chartInfo.diff ?? '').trim();
  if (diff && lvl > 0) return `${diff} Lv.${lvl.toFixed(1)}`;
  return diff || (lvl > 0 ? `Lv.${lvl.toFixed(1)}` : '');
}

/** 定数相同再按难度文本、曲名，免得同一档的谱面在两种方向下顺序看起来随机 */
function compareCharts(a: RenderChart, b: RenderChart): number {
  if (sortKey.value === 'level') {
    return (
      chartLevel(a) - chartLevel(b) ||
      textCompare(difficultyText(a), difficultyText(b)) ||
      textCompare(a.chartInfo.name, b.chartInfo.name)
    );
  }
  return textCompare(a.chartInfo.name, b.chartInfo.name);
}

function applySort() {
  const direction = sortAsc.value ? 1 : -1;
  charts.value.sort((a, b) => direction * compareCharts(a, b));
}

function sortReverse() {
  sortAsc.value = !sortAsc.value;
  applySort();
}

watch([sortKey, sortAsc], applySort);

function openInRender(chart: RenderChart) {
  router.push({
    name: 'render',
    query: {
      chart: chart.path,
      info: JSON.stringify(chart.chartInfo),
      config: JSON.stringify(preset.value.config),
    },
  });
}

const merging = ref(false);

/**
 * 「谱面融合」：把选中的同一首歌的几个难度拼成一张谱面，拼完直接进渲染页。
 *
 * 同一首歌的判据是「曲名 + 音乐文件」——两样都对得上才算，混进别的歌就别拼
 * （后端也会再判一次，这里的判断只为能说清是哪两张对不上）。
 */
async function mergeSelected() {
  const picked = charts.value.filter((chart) => chart.isSelect);
  if (picked.length < 2) {
    toastError(t('merge-need-two'));
    return;
  }
  const base = picked[0]!;
  const sameSong = (chart: RenderChart) =>
    (chart.chartInfo.name ?? '').trim() === (base.chartInfo.name ?? '').trim() &&
    (chart.chartInfo.music ?? '').trim() === (base.chartInfo.music ?? '').trim();
  if (!picked.every(sameSong)) {
    toastError(t('merge-not-same-song'));
    return;
  }

  merging.value = true;
  try {
    const path = await invoke<string>('merge_charts', { paths: picked.map((chart) => chart.path) });
    // 融合包是临时文件，渲染页自己 `parse_chart` 读它；预设配置照旧带过去。
    router.push({ name: 'render', query: { chart: path, config: JSON.stringify(preset.value.config) } });
  } catch (e) {
    toastError(e);
  } finally {
    merging.value = false;
  }
}

const tasks = ref<Task[]>();

async function updateList() {
  if (!charts.value) return;
  tasks.value = await invoke<Task[]>('get_tasks');
  // 只按 `taskId` 回填状态：卡片本身不显示进度（进度在任务页看），
  // 但「渲染完成后移除」这条要认「跑完了没有」。
  for (let i = charts.value.length - 1; i >= 0; i--) {
    let chart = charts.value[i]!;
    let task = tasks.value.find((x) => x.id === chart.taskId);
    if (task) {
      chart.status = task.status;
    }
    if (removeAfterRender.value && chart.status.type === 'done') {
      charts.value.splice(i, 1);
    }
  }
}

async function cancelSelectTask() {
  for (let chart of charts.value) {
    if (chart.isSelect && chart.taskId !== null) {
      try {
        await invoke('cancel_task', { id: chart.taskId });
      } catch (e) {
        toastError(e);
      }
    }
  }
}

async function showInFolder(path: string) {
  try {
    // 走后端：`explorer.exe /select,...` 由 shell 起进程。插件那版 `revealItemInDir`
    // 走 COM 的 `SHOpenFolderAndSelectItems`，在这个进程里实测没反应（详见 lib.rs）。
    await invoke('reveal_in_dir', { path });
  } catch (e) {
    toastError(e);
  }
}

async function clearTasks() {
  // 以前这里 `window.location.reload()` 整页重载 ——
  // 现在移除任务列表 + 批量列表就够了，不用把整个 webview 刷一遍。
  loadingPostRender.value = false;
  charts.value = [];
  expandedIds.value = [];
  try {
    await invoke('clear_tasks');
  } catch (e) {
    toastError(e);
  }
  await updateList();
}

applySort();
await updateList();

const updateTask = setInterval(updateList, 700);
onUnmounted(() => clearInterval(updateTask));
</script>

<template>
  <div class="batch-view">
    <!-- 谱面列表：卡片高度随展开变化，所以不用 v-virtual-scroll，整列自己滚 -->
    <div class="batch-view__list">
      <div v-if="charts.length === 0" class="batch-view__empty">
        <p class="batch-view__empty-title">{{ t('choose.can-also-drop') }}</p>
      </div>
      <div v-else-if="filteredCharts.length === 0" class="batch-view__empty">
        <p class="batch-view__empty-title">{{ t('no-match') }}</p>
      </div>

      <template v-else>
        <div v-for="item in filteredCharts" :key="item.id" class="batch-item">
          <!-- 勾选框在卡片左边框外面（同任务页）：放在整行里会把行高撑开 -->
          <span
            class="batch-item__check"
            :class="{ 'batch-item__check--on': item.isSelect }"
            @mousedown.prevent
            @click.stop="item.isSelect = !item.isSelect"
          >
            <v-checkbox-btn :model-value="item.isSelect" density="compact" :ripple="false"></v-checkbox-btn>
          </span>

          <v-card class="batch-card" :class="{ 'batch-card--open': expandedIds.includes(item.id) }">
            <!-- 整行点击 = 展开 / 收起；曲绘与曲名各自 stop 掉，它们另有去处 -->
            <div class="batch-row" @click="toggleExpanded(item.id)">
              <div class="batch-row__cover" :title="t('preview-cover-tip')" @click.stop="previewChart(item)">
                <ChartThumb :path="item.path" :illustration="item.chartInfo.illustration"></ChartThumb>
              </div>
              <div class="batch-row__text">
                <span
                  class="batch-row__name"
                  :title="`${item.chartInfo.name}\n${t('chart-open-tip')}`"
                  @click.stop="openInRender(item)"
                  @contextmenu="showInFolder(item.path)"
                >{{ item.chartInfo.name }}</span>
                <span v-if="diffTier(item.chartInfo)" class="batch-row__diff">
                  <span class="batch-row__diff-tier">{{ diffTier(item.chartInfo) }}</span>
                  <span v-if="diffLvl(item.chartInfo)" class="batch-row__diff-lvl">Lv.{{ diffLvl(item.chartInfo) }}</span>
                </span>
              </div>
              <v-icon class="batch-row__chevron" icon="mdi-chevron-down" size="20"></v-icon>
            </div>

            <v-expand-transition>
              <div v-if="expandedIds.includes(item.id)" class="batch-detail">
                <!-- 上半：渲染页顶部那份谱面横幅（点一下同样进预览） -->
                <div class="batch-hero" :title="t('preview-cover-tip')" @click="previewChart(item)">
                  <ChartHero :info="item.chartInfo" :path="item.path"></ChartHero>
                </div>
                <!-- 下半：渲染页那张谱面信息表单，可编辑；每张卡片各有一份 -->
                <div class="config-root batch-detail__form">
                  <ChartInfoForm :ref="(el) => setInfoForm(item.id, el)" :info="item.chartInfo"></ChartInfoForm>
                </div>
              </div>
            </v-expand-transition>
          </v-card>
        </div>
      </template>
    </div>

    <!-- 底部固定操作条：和渲染页同一条（`border-top` + 左右内边距），分两排：
           上排 = 筛选 · 排序 · 移除全部任务 ……… 三个开关
           下排 = 预设配置（最左）………… 全选 反选 移除 取消 渲染（右下角）
         高度：`--v-input-control-height` 从 Vuetify compact 的 40px 压到 28px（见样式）。 -->
    <div class="batch-view__actions">
      <div class="batch-view__bar batch-view__bar--head">
        <div class="batch-view__pickers batch-view__pickers--filter config-root">
          <span class="batch-view__picker-label">{{ t('filter') }}</span>
          <v-text-field
            class="batch-view__picker batch-view__picker--filter"
            density="compact"
            hide-details
            clearable
            prepend-inner-icon="mdi-magnify"
            :placeholder="t('filter')"
            :title="t('filter-tip')"
            v-model="chartFilter"
          ></v-text-field>
        </div>

        <div class="batch-view__pickers batch-view__pickers--sort config-root">
          <span class="batch-view__picker-label">{{ t('sort') }}</span>
          <v-select
            class="batch-view__picker batch-view__picker--sort"
            density="compact"
            hide-details
            :items="sortItems"
            item-title="title"
            item-value="value"
            :menu-props="{ contentClass: 'v-select__content batch-preset-menu' }"
            v-model="sortKey"
          ></v-select>
          <v-btn
            variant="text"
            size="x-small"
            :icon="sortAsc ? 'mdi-arrow-up' : 'mdi-arrow-down'"
            :title="t('sort-reverse-tip')"
            @click="sortReverse"
          ></v-btn>
        </div>

        <v-divider vertical class="mx-1" style="height: 24px;"></v-divider>

        <v-btn variant="text" size="small" prepend-icon="mdi-delete-sweep-outline" @click="clearTasks">{{ t('clear-tasks') }}</v-btn>

        <!-- 谱面融合与「移除全部任务」相邻：两者都作用在整份列表上，而右边那三个开关是「发起后的动作」 -->
        <v-btn
          variant="text"
          size="small"
          prepend-icon="mdi-merge"
          :loading="merging"
          :disabled="selectedCount < 2"
          :title="t('merge-tip')"
          @click="mergeSelected"
        >{{ t('merge-charts') }}</v-btn>

        <v-spacer />

        <CfgSwitch small :label="t('dis-select-start-render')" v-model="disSelectStartRender" />
        <CfgSwitch small :label="t('remove-start-render')" v-model="removeStartRender" />
        <CfgSwitch small :label="t('remove-after-render')" v-model="removeAfterRender" />
      </div>

      <div class="batch-view__bar batch-view__bar--foot">
        <div class="batch-view__pickers batch-view__pickers--preset config-root">
          <span class="batch-view__picker-label">{{ t('presets') }}</span>
          <v-combobox
            class="batch-view__picker batch-view__picker--preset"
            density="compact"
            hide-details
            :menu-props="{ contentClass: 'v-select__content batch-preset-menu' }"
            :items="presets"
            item-title="name"
            item-value="config"
            :title="t('edit-preset')"
            v-model="preset"
          ></v-combobox>
          <v-btn icon="mdi-pencil" variant="text" size="x-small" :title="t('edit-preset')" @click="editPreset"></v-btn>
        </div>

        <v-spacer />

        <v-btn variant="text" size="small" :disabled="filteredCharts.length === 0" @click="selectAll">{{ t('select-all') }}</v-btn>
        <v-btn variant="text" size="small" :disabled="filteredCharts.length === 0" @click="selectInvert">{{ t('select-invert') }}</v-btn>
        <v-btn variant="text" size="small" :disabled="!hasSelection" @click="removeSelectChart">{{ t('remove-select') }}</v-btn>
        <v-btn variant="text" size="small" :disabled="!hasSelection" @click="cancelSelectTask">{{ t('cancel-select') }}</v-btn>
        <!-- 右边五个按钮字体大小要一样 ⇒ 渲染按钮补 `size="small"`：
             前四个小按钮都是 `.75rem`，它没写 size 就是默认的 `.875rem`，字号比旁边大一号。 -->
        <v-btn variant="tonal" size="small" :loading="loadingPostRender" :disabled="!hasSelection" @click="postSelectRender" class="gradient-primary">{{ t('post-select-render') }}</v-btn>
      </div>
    </div>

    <!-- 预设配置弹窗：同样按齿轮配置的格式（预设配置的编辑弹窗模仿齿轮配置），
         宽度是**内容区的 100%**（`measurePresetDialogWidth`，配置条目多，60% 太挤）。
         改动是随改随存的（`ConfigView` 发 `change` → `setWorkingPreset`），所以和齿轮弹窗一样
         一个按钮都没有：点空白处 / Esc 关掉就行。 -->
    <v-dialog v-model="presetDialog" :width="presetDialogWidth" class="log-card-bg">
      <v-card class="log-card-only-window phi-config-dialog">
        <div class="phi-config-dialog__head">
          <span class="phi-config-dialog__title" v-t="'presets'"></span>
        </div>
        <v-card-text class="config-root phi-config-dialog__body">
          <ConfigView ref="configView" @change="setWorkingPreset"></ConfigView>
        </v-card-text>
      </v-card>
    </v-dialog>

    <v-overlay v-model="loadingParsingChart" class="align-center justify-center" persistent noClickAnimation :close-on-content-click="false">
      <v-progress-circular indeterminate style="filter: none;"> </v-progress-circular>
    </v-overlay>

    <v-overlay v-model="fileHovering" contained class="align-center justify-center drop-zone-overlay" persistent :close-on-content-click="false">
      <div class="drop-pulse">
        <h1 v-t="'choose.drop'"></h1>
      </div>
    </v-overlay>
  </div>
</template>

<style scoped>

/* 整页铺满内容区（和渲染页 `.render-view` 同一套外壳） */
.batch-view {
  position: relative;
  display: flex;
  flex: 1;
  flex-direction: column;
  min-height: 0;
  overflow: hidden;
}

/* 谱面列表：卡片自己滚。左内边距多留 42px，给卡片外面那个勾选框 */
.batch-view__list {
  display: flex;
  flex: 1;
  flex-direction: column;
  min-height: 0;
  overflow-y: auto;
  padding: var(--phi-space-3) var(--phi-space-6) 0 calc(var(--phi-space-6) + 42px);
  scrollbar-width: none;
}

/* 一张谱面 = 一张卡片（版式照任务页 `.task-item` + `.task-card`） */
.batch-item {
  position: relative;
  --batch-row-h: 58px;
}

.batch-item + .batch-item {
  margin-top: 6px;
}

/* 勾选框在卡片左边框外面，平时透明，hover / 选中 / 聚焦时才露出来 */
.batch-item__check {
  position: absolute;
  left: -38.4px;
  top: calc(5px + var(--batch-row-h) / 2);
  display: flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  transform: translateY(-50%);
  opacity: 0;
  transition: opacity 0.12s;
  user-select: none;
}

.batch-item:hover .batch-item__check,
.batch-item__check--on,
.batch-item__check:focus-within {
  opacity: 1;
}

.batch-item__check :deep(.v-selection-control) {
  min-height: 24px;
}

.batch-item__check :deep(.v-icon) {
  font-size: 22px;
}

.batch-card {
  margin: 5px 0;
  border: 0.5px solid var(--phi-border-l3);
  border-radius: var(--phi-radius-lg) var(--phi-radius-lg) 0 0 !important;
  background: var(--phi-bg-layer-1);
  box-shadow: var(--phi-elevation-soft);
}

.batch-card:hover {
  background: var(--phi-bg-layer-2);
}

/* 折叠行：曲绘 / 曲名 + 难度徽章 / 展开箭头 */
.batch-row {
  display: flex;
  align-items: center;
  gap: var(--phi-space-3);
  padding: var(--phi-space-3) var(--phi-space-4);
  box-sizing: border-box;
  min-height: var(--batch-row-h);
  cursor: pointer;
}

.batch-row__cover {
  display: flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 64px;
  aspect-ratio: 2048 / 1080;
  overflow: hidden;
  border: 0.5px solid var(--phi-border-l3);
  border-radius: var(--phi-radius-sm);
  background: var(--phi-bg-layer-3);
  color: var(--phi-label-caption);
  cursor: pointer;
}

.batch-row__cover:hover {
  border-color: var(--phi-accent);
}

.batch-row__cover :deep(img) {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.batch-row__text {
  display: flex;
  flex: 1 1 auto;
  align-items: center;
  gap: var(--phi-space-3);
  min-width: 0;
}

.batch-row__name {
  overflow: hidden;
  min-width: 0;
  font-size: 1rem;
  font-weight: 600;
  line-height: 1.25;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--phi-label-primary);
  cursor: pointer;
}

.batch-row__name:hover {
  color: var(--phi-accent);
  text-decoration: underline;
}

/* 难度徽章：灰底白字，档位大写 + 定数（同任务页 `.task-row__diff`） */
.batch-row__diff {
  display: inline-flex;
  flex: none;
  align-items: center;
  gap: 5px;
  height: 20px;
  padding: 0 7px;
  border-radius: 0;
  background: var(--phi-static-neutral-600);
  color: var(--phi-static-bluish-00);
  font-size: 0.72rem;
  font-weight: 400;
  letter-spacing: 0.06em;
}

.batch-row__diff-tier {
  text-transform: uppercase;
}

.batch-row__diff-lvl {
  font-variant-numeric: tabular-nums;
  letter-spacing: 0;
}

.batch-row__chevron {
  flex: none;
  color: var(--phi-label-caption);
  transition: transform 0.15s ease;
}

.batch-card--open .batch-row__chevron {
  transform: rotate(180deg);
}

/* 展开区：上半是谱面横幅，下半是谱面信息表单 */
.batch-detail {
  border-top: 0.5px solid var(--phi-border-l3);
  background: var(--phi-bg-layer-1);
}

.batch-hero {
  position: relative;
  cursor: pointer;
}

/* 横幅高度对齐任务页展开的那一屏（`ChartHero` 的 `.chart-hero__inner` 自带 z-index 与
   右侧 36px 的箭头留白，这里都要按卡片的情况改：箭头在折叠行里，横幅不需要留白） */
.batch-hero :deep(.chart-hero),
.batch-hero :deep(.chart-hero__inner) {
  height: 200px;
  min-height: 0;
}

.batch-hero :deep(.chart-hero__inner) {
  padding-right: var(--phi-space-5);
}

.batch-detail__form {
  padding-bottom: var(--phi-space-2);
}

/* 空列表：中间只有一行文字，不画图标 */
.batch-view__empty {
  display: flex;
  flex: 1;
  align-items: center;
  justify-content: center;
}

.batch-view__empty-title {
  margin: 0;
  font-size: 1.05rem;
  color: var(--phi-label-secondary);
}

/* 底部固定操作条（同渲染页 `.render-view__actions`） */
.batch-view__actions {
  display: flex;
  flex: none;
  flex-direction: column;
  gap: var(--phi-space-1);
  padding: var(--phi-space-2) var(--phi-space-6);
  border-top: 0.5px solid var(--phi-border-l3);
}

/* 操作条里的一排（一共两排，见模板注释）。强制只显示一行：窄窗口下
   开关的标签用省略号，不要让后面的东西掉到第二排去。 */
.batch-view__bar {
  display: flex;
  flex-wrap: nowrap;
  align-items: center;
  gap: var(--phi-space-2);
  min-width: 0;
}

/* 上排：筛选 / 排序两组按内容伸缩，中间的分隔线与按钮不参与伸缩，
   右边三个开关让出空间给标签省略号。 */
.batch-view__bar--head > .batch-view__pickers--filter {
  flex: 0 1 26%;
  min-width: 0;
}

.batch-view__bar--head > .batch-view__pickers--sort {
  flex: 0 1 20%;
  min-width: 0;
}

.batch-view__bar--head > .v-btn,
.batch-view__bar--head > .v-divider {
  flex: none;
}

.batch-view__bar--head :deep(.cfg-item) {
  flex: 0 1 auto;
  min-width: 0;
}

.batch-view__bar--head :deep(.cfg-item__label) {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 下排：「预设配置」那一组（标签 + 下拉框 + 铅笔按钮）占整排的 60% */
.batch-view__bar--foot > .batch-view__pickers {
  flex: 0 0 60%;
  min-width: 0;
}

/* 操作条里的「标签 + 控件」组：不再是 cfg-grid 的条目，改成条内的行内小控件；
   外壳仍套 `.config-root`，输入框才和配置页一样是直角方框 + 白字。 */
.batch-view__pickers {
  display: flex;
  align-items: center;
  gap: var(--phi-space-2);
}

.batch-view__picker-label {
  font-size: 0.8rem;
  color: var(--phi-label-secondary);
  white-space: nowrap;
}

/* 下拉框 / 输入框撑满自己那一组（组宽由上面的 flex 定） */
.batch-view__picker {
  flex: 1 1 auto;
  width: auto;
  min-width: 0;
}

/* 高度 -30%：Vuetify 的 compact 档把 `--v-input-control-height` 定成 40px，
   这里改到 28px，并把 `__input` 的上下内边距清零（否则 `min-height` 里的
   `1.5rem + padding` 会把高度顶回去）。 */
.batch-view__pickers :deep(.v-input) {
  --v-input-control-height: 28px;
}

.batch-view__pickers :deep(.v-field__input) {
  min-height: 28px;
  padding-top: 0;
  padding-bottom: 0;
}

/* 输入框里的字统一 0.85rem。**必须把 `.v-select__selection` 一起写上**：
   `design/config.css:147-166` 那份 `.config-root …` 清单把 `.v-field__input` / `.v-field input` /
   `.v-field textarea` / `.v-select__selection` 全定成了 1rem，而 combobox **没打开**时字段里显示的是
   `span.v-select__selection`（1rem）、**打开**之后才换成真正的 `input`（0.85rem）——
   只写后者就会出现「预设配置字体打开下拉栏后变小」：下拉栏一打开字号就掉下去。
   （`:deep()` 编译出来是 `.batch-view__pickers[data-v-x] …`，比 `.config-root …` 高一档，压得住。） */
.batch-view__pickers :deep(.v-field),
.batch-view__pickers :deep(.v-field input),
.batch-view__pickers :deep(.v-field textarea),
.batch-view__pickers :deep(.v-select__selection),
.batch-view__pickers :deep(.v-combobox__selection),
.batch-view__pickers :deep(.v-autocomplete__selection) {
  font-size: 0.85rem;
}

/* 操作条里的方形开关：不要「条目」那套行高与内边距，只留 名字 + 小开关 */
.batch-view__actions :deep(.cfg-item) {
  min-height: 0;
  padding: 0;
  gap: var(--phi-space-2);
}

.batch-view__actions :deep(.cfg-item__label) {
  max-width: none;
  font-size: 0.8rem;
}

.batch-view__actions :deep(.cfg-item__control) {
  flex: none;
  margin-left: 0;
}

/* 主操作按钮：用主题强调色（同渲染页的「渲染」） */
.gradient-primary {
  background: var(--phi-accent) !important;
  color: var(--phi-label-inverted) !important;
}

.v-text-field :deep(.v-field--focused) {
  border-color: var(--phi-accent) !important;
}

:deep(span) {
  user-select: text;
}

.drop-pulse {
  animation: pulse 2s infinite;
}

@keyframes pulse {
  0% { transform: scale(1); }
  50% { transform: scale(1.05); }
  100% { transform: scale(1); }
}

</style>

<!-- 下拉栏（Vuetify 的菜单）是 teleport 到 `.v-overlay-container` 下的，scoped 样式够不着 ⇒
     这里单独一段**非 scoped** 样式；用控件上的 `menu-props.contentClass` 限定作用范围，
     免得顺手改到设置页那个语言下拉。字号与字段里的输入框保持一致。 -->
<style>
.batch-preset-menu .v-list-item-title {
  font-size: 0.85rem;
}
</style>
