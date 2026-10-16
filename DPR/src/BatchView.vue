<script setup lang="ts">
import { ref, nextTick, onUnmounted, computed, type Ref } from 'vue';
import { useStorage } from '@vueuse/core';

import { useI18n } from 'vue-i18n';
const { t } = useI18n();

import { invoke } from '@tauri-apps/api/core';
import { openPath } from '@tauri-apps/plugin-opener';

import { toastError, RULES, toast } from './common';
import { formatLvl, formatLvlInText, useLvlField } from './lvl';
import { DEFAULT_RENDER_CONFIG, type ChartInfo, type RenderConfig, type RenderChart, type Preset, type FileDropEvent, type Task } from './model';
import { sameRenderConfig } from './config/useRenderConfig';
import { chartSideOf } from './config/chartSide';
import { finishPageLoad } from './pageLoad';
import router from './router';
import { AnsiUp } from 'ansi_up';
const ansi = new AnsiUp();

import { VForm } from 'vuetify/components';

import ConfigView from './components/ConfigView.vue';
import CfgField from './components/CfgField.vue';
import CfgSwitch from './components/CfgSwitch.vue';

import { listen } from "@tauri-apps/api/event";

/**
 * 批量渲染页。外壳按渲染页的格式重写过：
 *
 *   .batch-view              整页铺满内容区（不再套 v-card + sticky v-toolbar）
 *     .batch-view__list      谱面列表：表头 + v-virtual-scroll，表头与每行都是 .batch-grid 栅格
 *     .batch-view__actions   底部固定操作条，两排（上：筛选 / 输出文件夹 / 三个开关；下：预设配置 + 全选…渲染）
 *
 * 和渲染页对齐的几点：
 *   - 页面自己不再有卡片、内边距与工具栏，滚动交给列表内层；
 *   - 表单条目走 cfg-grid + CfgField / CfgSwitch，外层套 `.config-root`，
 *     输入框的方框、文字颜色、条目名字号才和配置页/渲染页一致；
 *   - 主操作按钮用 `.gradient-primary`（同渲染页的「渲染」按钮），次要操作 text / tonal；
 *   - 谱面信息、预设配置两个弹窗**模仿齿轮配置（三级菜单）弹窗的格式**：
 *     共用 `design/config.css` 的 `.phi-config-dialog*` 皮肤（内容区底色 + 88% 不透明、直角、
 *     标题居中 1.5rem、`__body` 无额外内边距），条目一行两个（`cfg-grid--two`），
 *     并且不再有底部按钮（点空白处 / Esc 关闭）。宽度：谱面信息是「内容区的 60%」
 *     （`measureConfigDialogWidth`），预设配置**吃满内容区**（`measurePresetDialogWidth`）——
 *     配置条目多，60% 太挤。
 *
 * 谱面**只能拖动导入**（和渲染页一样）：没有选择文件 / 文件夹的入口，也没有
 * `@tauri-apps/plugin-dialog` 的导入对话框；空列表中间只留一行文字，用的是
 * `render.json` 里现成的 `choose.can-also-drop` =「（可拖放谱面至此处）」。
 * 注意这个键是**嵌在 `choose` 里**的，漏掉前缀就会在界面上原样显示 `can-also-drop`。
 *
 * 操作条（`.batch-view__actions`）分**两排**（每排一个 `.batch-view__bar`）：
 *   上排 = 筛选 / 打开输出文件夹 · 移除全部任务 / 三个开关；
 *   下排 = 预设配置（最左）………… 全选 反选 移除 取消 渲染（右下角）。
 * 上排 `flex-wrap: nowrap` 不换行，筛选那组固定 1/5、后面五项（两个按钮 + 三个开关）
 * 均分剩下的 4/5；下排「预设配置」那一组占整排 60%，里面下拉框自己撑满。
 * 预设配置与筛选都不占页面顶部，改成操作条里的行内小控件
 * （`.batch-view__pickers`），高度按 Vuetify 的 `--v-input-control-height` 压到 28px。
 *
 * 没有「自动调整宽高比」开关：`postRender()` 直接把 `preset.value.config` 交给后端。
 * ⚠️ 注意 Vue 响应式代理塞进 `structuredClone` 会抛 DataCloneError。
 *
 * 两处必须留意的校验 / 编号问题（详见下面各处注释）：
 *   1. 谱面信息弹窗的 `<v-form>` 必须有 `ref="form"`，否则 `checkInfo()` 里
 *      `form.value!.validate()` 取到 undefined 会抛错 —— 批量「渲染」发不出去；
 *   2. 谱面 `id` 取当前最大值 +1，不能用 `charts.value.length`
 *      （删掉几条再导入会撞号，虚拟滚动的 item-key 和「正在编辑哪一条」都靠它）。
 */

const form = ref<VForm>();
const charts = useStorage<RenderChart[]>('BatchView.ChartList', []);

/** 新的谱面 id：虚拟滚动的 item-key 与「正在编辑哪一条」都靠它，必须唯一。 */
function nextChartId(): number {
  return charts.value.reduce((max, chart) => Math.max(max, chart.id), -1) + 1;
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

const chartInfoDialog = ref(false);
const chartInfoSelect = ref(0);
/**
 * 两个配置类弹窗（谱面信息 / 预设配置）的宽度 —— 与齿轮弹窗（`PhiSubMenu.vue` 的
 * `dialogWidth`）、「文件名格式」弹窗（`components/ConfigView.vue` 的
 * `fileNameFormatDialogWidth`）同一个算法：**内容区 `<main class="phi-center">` 的 60%**，
 * 下限 480px。弹窗被 teleport 到 body，写百分比会按视口算（把左侧导航也算进去），
 * 所以每次打开时量一次。
 * 预设配置、谱面信息两个编辑弹窗都模仿齿轮配置的格式。
 */
const configDialogWidth = ref(880);
function measureConfigDialogWidth() {
  const content = document.querySelector<HTMLElement>('.phi-center');
  const base = content?.clientWidth || window.innerWidth;
  configDialogWidth.value = Math.max(480, Math.round(base * 0.6));
}
/**
 * 「预设配置」编辑弹窗的宽度 = **内容区宽度的 100%**（它自己一条，不再和谱面信息共用）：
 * 配置条目比谱面信息多得多，60% 下两栏挤得看不清。
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

/**
 * 正在编辑的谱面。以前弹窗里到处写 `charts[chartInfoSelect]` —— 那是**数组下标**，
 * 而 `chartInfoSelect` 存的是 `id`；列表一排序两边就对不上了。现在统一按 id 找。
 */
const editingChart = computed(() => charts.value.find((chart) => chart.id === chartInfoSelect.value));

// 谱面信息弹窗里的「定数」输入框：显示一律补成一位小数（见 `./lvl.ts`），
// 输入过程仍然随手打，数值照旧写回选中的那条谱面。
const { text: lvlText, normalize: normalizeLvl } = useLvlField(
  () => editingChart.value?.chartInfo.lvl,
  (value) => {
    const chart = editingChart.value;
    if (chart) chart.chartInfo.lvl = value;
  },
);

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

async function buildParams(chartPath: string, chartInfo: ChartInfo, config: RenderConfig) {
  // 以前这里既没 await 也没看返回值，校验不过也照样提交；
  // 现在校验不过就返回 null，调用方（postRender / previewChart）会直接放弃。
  if (!(await checkInfo(chartInfo))) return null;
  // 预览片段窗口不超过 15 秒：预览开始 / 结束时间现在是**配置项**，所以这条校验跟着配置走
  // （渲染页那边由 ConfigView.buildConfig() 保证；批量页直接读预设里那份 config）。
  if (config.previewEnd && config.previewEnd - config.previewStart > 15) {
    toast(t('error.preview-start-end-15s'), 'error');
    return null;
  }
  // 谱面侧的渲染参数（宽高比 / 强制宽高比 / 折叠式入场 / 判定线长度 / 总分 / 预览起止 / 偏移 /
  // 背景压暗 …）现在是 `RenderConfig` 的字段：**以配置为准**，所以展开在谱面信息**之后**
  // 覆盖同名键（与 RenderView.buildParams 同一套做法，见 ./config/chartSide.ts）。
  return {
    path: chartPath,
    info: { ...chartInfo, ...chartSideOf(config) },
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
    let params = await buildParams(chart.path, chart.chartInfo, preset.value.config);
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
    let params = await buildParams(chart.path, chart.chartInfo, preset.value.config);
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

async function checkInfo(chartInfo: ChartInfo) {
  // 表单只在谱面信息弹窗里（弹窗没打开过时 form.value 是 undefined）——
  // 以前这里是 `form.value!.validate()`，弹窗没打开就直接抛错。
  if (form.value && !(await form.value.validate()).valid) {
    return false;
  }
  // 「预览开始 / 结束时间」与「偏移」现在是配置项（`RenderConfig`），
  // 15 秒窗口那条校验搬到了 `buildParams()`（跟着配置走，见那边的注释），
  // 这里只剩谱面信息自己的收尾：Tip 是空串就写 null。
  if (!chartInfo.tip?.trim().length) chartInfo.tip = null;

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

type SortKey = 'id' | 'name' | 'difficulty' | 'level' | 'charter' | 'path';

const sortKey = useStorage<SortKey>('BatchView.SortKey', 'id');
const sortAsc = useStorage<boolean>('BatchView.SortAsc', true);

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

/** 难度列显示文本：谱面没写 `difficulty` 时用解析出来的 `diff` + 定数拼一个。 */
function difficultyLabel(chart: RenderChart): string {
  // 不管走哪条分支，定数都补成一位小数
  if (chart.chartInfo.difficulty) return formatLvlInText(chart.chartInfo.difficulty);
  const lvl = formatLvl(chartLevel(chart));
  const diff = chart.chartInfo.diff;
  if (diff && lvl) return `${diff} Lv.${lvl}`;
  return diff || (lvl ? `Lv.${lvl}` : '');
}

/** 「难度」列按定数排（并列时再按难度名），其余列按各自字段排。 */
const COMPARATORS: Record<SortKey, (a: RenderChart, b: RenderChart) => number> = {
  id: (a, b) => a.id - b.id,
  name: (a, b) => textCompare(a.chartInfo.name, b.chartInfo.name),
  path: (a, b) => textCompare(a.path, b.path),
  difficulty: (a, b) => textCompare(a.chartInfo.diff, b.chartInfo.diff) || chartLevel(a) - chartLevel(b),
  level: (a, b) => chartLevel(a) - chartLevel(b) || textCompare(a.chartInfo.diff, b.chartInfo.diff),
  charter: (a, b) => textCompare(a.chartInfo.charter, b.chartInfo.charter),
};

function applySort() {
  const compare = COMPARATORS[sortKey.value] ?? COMPARATORS.id;
  const direction = sortAsc.value ? 1 : -1;
  charts.value.sort((a, b) => direction * compare(a, b));
}


function sortChartsByKey(key: SortKey) {
  // 点同一个表头换方向，换一个表头则重置为升序
  if (sortKey.value === key) {
    sortAsc.value = !sortAsc.value;
  } else {
    sortKey.value = key;
    sortAsc.value = true;
  }
  applySort();
}

function sortChartsReverse() {
  sortAsc.value = !sortAsc.value;
  applySort();
}

/** 当前列是否在排序中，返回要显示的箭头图标。 */
function sortIcon(key: SortKey): string | null {
  if (sortKey.value !== key) return null;
  return sortAsc.value ? 'mdi-arrow-up' : 'mdi-arrow-down';
}

/* ---------- 每一行的三列「状态位」 ----------
   `status.type === 'null'` 表示这条谱面还没进过任务队列：这三列显示 难度 / 谱师 / 路径；
   一旦发起渲染，三列改显示 状态 / 进度 / 输出（和重做前的显示逻辑一致，只是从
   一长串 v-if 收进了函数里，模板不用再为每种状态抄四遍同样的 v-col）。 */

/** 第三列（难度那一列）：没进队列时显示难度，进队列后显示状态。 */
function cellLevel(chart: RenderChart): string {
  switch (chart.status.type) {
    case 'null': return difficultyLabel(chart);
    case 'pending': return t('task.pending');
    case 'loading': return t('task.loading');
    case 'mixing': return t('task.mixing');
    case 'mixing_sfx': return t('task.mixing');
    case 'rendering': return `${(chart.status.progress * 100).toFixed(2)}%`;
    case 'paused': return t('task.paused');
    case 'done': return t('task.done');
    case 'canceled': return t('task.canceled');
    case 'failed': return t('task.failed');
  }
}

/** 第四列（谱师那一列）：没进队列时显示谱师，进队列后显示进度细节。 */
function cellDetail(chart: RenderChart): string {
  switch (chart.status.type) {
    case 'null': return chart.chartInfo.charter ?? '';
    case 'rendering': return `${chart.status.fps} FPS`;
    case 'done': return t('task.open-file');
    default: return '-';
  }
}

/** 第五列（路径那一列）：没进队列时显示路径，进队列后显示预计剩余 / 输出入口。 */
function cellExtra(chart: RenderChart): string {
  switch (chart.status.type) {
    case 'null': return chart.path;
    case 'rendering': return `${chart.status.estimate.toFixed(0)} s`;
    case 'done':
    case 'canceled':
    case 'failed': return t('task.show-output');
    default: return '-';
  }
}

function canOpenOutputFile(chart: RenderChart): boolean {
  return chart.status.type === 'done';
}

/** 「打开文件」：只有渲染完成那一行可以点。 */
function openChartFile(chart: RenderChart) {
  if (chart.status.type === 'done') openFile(chart.output);
}

/** 第五列的点击：没进队列时打开文件所在位置，跑完的行打开输出日志。 */
function activateExtraCell(chart: RenderChart) {
  if (chart.status.type === 'null') {
    showInFolder(chart.path);
  } else if (chart.status.type === 'done' || chart.status.type === 'canceled' || chart.status.type === 'failed') {
    openOutputDialog(chart.status.output);
  }
}

function openChartInfo(chart: RenderChart) {
  chartInfoSelect.value = chart.id;
  measureConfigDialogWidth();
  chartInfoDialog.value = true;
}

const tasks = ref<Task[]>();

async function updateList() {
  if (!charts.value) return;
  tasks.value = await invoke<Task[]>('get_tasks');
  // console.log(tasks.value);
  for (let i = charts.value.length - 1; i >= 0; i--) {
    let chart = charts.value[i]!;
    let task = tasks.value.find((x) => x.id === chart.taskId);
    if (task) {
      chart.status = task.status;
      // console.log(chart.status);
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

async function showOutputFolder() {
  try {
    await invoke('open_output_folder');
  } catch (e) {
    toastError(e);
  }
}

async function openFile(path: string) {
  try {
    await openPath(path);
  } catch (e) {
    toastError(e);
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

const outputDialog = ref(false);
const outputDialogMessage = ref('');

function openOutputDialog(output: string) {
  outputDialogMessage.value = ansi.ansi_to_html(output);
  filteredOutputDialogMessage.value = outputDialogMessage.value;
  filter.value = [];
  outputDialog.value = true;
}

const filteredOutputDialogMessage = ref('');

const filterItems: string[] = ["INFO", "DEBUG", "WARN", "ERROR", "! INFO", "! DEBUG", "! WARN", "! ERROR"];
const filter: Ref<string[]> = ref([]);

function filterText(
  rawText: string,
  filters: string[]
): string {
  const include: string[] = [];
  const exclude: string[] = [];

  for (let f of filters) {
    f = f.trim();
    if (!f) continue;
    if (f.startsWith('!')) {
      const kw = f.slice(1).trim();
      if (kw) exclude.push(kw);
    } else {
      include.push(f);
    }
  }

  if (include.length === 0 && exclude.length === 0) {
    return rawText;
  }

  const resultLines = rawText
    .split(/\r?\n/)
    .filter(line => {
      if (include.length > 0) {
        const hitInclude = include.some(kw => line.includes(kw));
        if (!hitInclude) return false;
      }
      const hitExclude = exclude.some(kw => line.includes(kw));
      if (hitExclude) return false;

      return true;
    });

  return resultLines.join('\n');
}

</script>

<template>
  <div class="batch-view">
    <!-- 谱面列表：表头固定，行交给 v-virtual-scroll 自己滚 -->
    <div class="batch-view__list">
      <div class="batch-grid batch-grid--head">
        <div class="batch-grid__cell batch-grid__cell--center" @click="sortChartsByKey('id')" @contextmenu="sortChartsReverse" :title="t('sort-tip')">
          ({{ filteredCharts.length }}<template v-if="filterTerms.length">/{{ charts.length }}</template>)
          <v-icon v-if="sortIcon('id')" size="x-small" class="ml-1">{{ sortIcon('id') }}</v-icon>
        </div>
        <div class="batch-grid__cell" @click="sortChartsByKey('name')" @contextmenu="sortChartsReverse" :title="t('sort-tip')">
          {{ t('info.name') }}
          <v-icon v-if="sortIcon('name')" size="x-small" class="ml-1">{{ sortIcon('name') }}</v-icon>
        </div>
        <div class="batch-grid__cell" @click="sortChartsByKey('level')" @contextmenu="sortChartsReverse" :title="t('sort-by-level-tip')">
          {{ t('info.difficulty') }}
          <v-icon v-if="sortIcon('level')" size="x-small" class="ml-1">{{ sortIcon('level') }}</v-icon>
        </div>
        <div class="batch-grid__cell" @click="sortChartsByKey('charter')" @contextmenu="sortChartsReverse" :title="t('sort-tip')">
          {{ t('info.charter') }}
          <v-icon v-if="sortIcon('charter')" size="x-small" class="ml-1">{{ sortIcon('charter') }}</v-icon>
        </div>
        <div class="batch-grid__cell" @click="sortChartsByKey('path')" @contextmenu="sortChartsReverse" :title="t('sort-tip')">
          {{ t('info.chart') }}
          <v-icon v-if="sortIcon('path')" size="x-small" class="ml-1">{{ sortIcon('path') }}</v-icon>
        </div>
        <div class="batch-grid__cell batch-grid__cell--center">{{ t('chart-info') }}</div>
        <div class="batch-grid__cell batch-grid__cell--center">{{ t('preview') }}</div>
      </div>
      <VDivider></VDivider>

      <div class="batch-view__list-body">
        <!-- 只能拖进来，所以空列表给的是拖放提示（没有选择按钮）：
             中间只留一行文字，不画图标。 -->
        <div v-if="charts.length === 0" class="batch-view__empty">
          <p class="batch-view__empty-title">{{ t('choose.can-also-drop') }}</p>
        </div>
        <div v-else-if="filteredCharts.length === 0" class="batch-view__empty">
          <p class="batch-view__empty-title">{{ t('no-match') }}</p>
        </div>

        <v-virtual-scroll
          v-else
          :items="filteredCharts"
          :item-height="64"
          item-key="id"
          height="100%"
          style="overflow-y: scroll;"
        >
          <template v-slot:default="{ item }">
            <div class="batch-grid batch-grid--row">
              <div class="batch-grid__cell batch-grid__cell--center">
                <v-checkbox v-model="item.isSelect" density="compact" hide-details class="batch-grid__check"></v-checkbox>
              </div>

              <div class="batch-grid__cell">
                <span
                  class="batch-grid__ellipsis batch-grid__clickable"
                  :title="`${item.chartInfo.name}\n${t('chart-open-tip')}`"
                  @click="router.push({ name: 'render', query: { chart: item.path, info: JSON.stringify(item.chartInfo), config: JSON.stringify(preset.config) } })"
                  @contextmenu="showInFolder(item.path)"
                >{{ item.chartInfo.name }}</span>
              </div>

              <div class="batch-grid__cell">
                <span class="batch-grid__ellipsis" :title="cellLevel(item)">{{ cellLevel(item) }}</span>
              </div>

              <div class="batch-grid__cell">
                <span
                  class="batch-grid__ellipsis"
                  :class="{ 'batch-grid__clickable': canOpenOutputFile(item) }"
                  :title="cellDetail(item)"
                  @click="openChartFile(item)"
                >{{ cellDetail(item) }}</span>
              </div>

              <div class="batch-grid__cell">
                <span
                  class="batch-grid__ellipsis"
                  :class="{ 'batch-grid__clickable': item.status.type !== 'null' }"
                  :title="`${cellExtra(item)}\n${item.status.type === 'null' ? t('file-open-tip') : ''}`"
                  @click="activateExtraCell(item)"
                  @contextmenu="showInFolder(item.path)"
                >{{ cellExtra(item) }}</span>
              </div>

              <div class="batch-grid__cell batch-grid__cell--center">
                <v-btn size="small" variant="tonal" @click="openChartInfo(item)">{{ t('edit') }}</v-btn>
              </div>
              <div class="batch-grid__cell batch-grid__cell--center">
                <v-btn size="small" variant="tonal" :loading="loadingPreview" @click="previewChart(item)">{{ t('preview') }}</v-btn>
              </div>
            </div>
          </template>
        </v-virtual-scroll>

        <v-overlay v-model="loadingParsingChart" class="align-center justify-center" persistent noClickAnimation :close-on-content-click="false">
          <v-progress-circular indeterminate style="filter: none;"> </v-progress-circular>
        </v-overlay>
      </div>
    </div>

    <!-- 底部固定操作条：和渲染页同一条（`border-top` + 左右内边距），分两排：
           上排 = 筛选 / 打开输出文件夹 · 移除全部任务 / 三个开关
           下排 = 预设配置（最左）………… 全选 反选 移除 取消 渲染（右下角）
         高度：`--v-input-control-height` 从 Vuetify compact 的 40px 压到 28px（见样式）。 -->
    <div class="batch-view__actions">
      <div class="batch-view__bar batch-view__bar--head">
        <div class="batch-view__pickers config-root">
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

        <v-divider vertical class="mx-1" style="height: 24px;"></v-divider>

        <v-btn variant="text" size="small" prepend-icon="mdi-folder-open" @click="showOutputFolder">{{ t('task.show-folder') }}</v-btn>
        <v-btn variant="text" size="small" prepend-icon="mdi-delete-sweep-outline" @click="clearTasks">{{ t('clear-tasks') }}</v-btn>

        <v-divider vertical class="mx-1" style="height: 24px;"></v-divider>

        <CfgSwitch small :label="t('dis-select-start-render')" v-model="disSelectStartRender" />
        <CfgSwitch small :label="t('remove-start-render')" v-model="removeStartRender" />
        <CfgSwitch small :label="t('remove-after-render')" v-model="removeAfterRender" />
      </div>

      <div class="batch-view__bar batch-view__bar--foot">
        <div class="batch-view__pickers config-root">
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

    <!-- 谱面信息弹窗：按齿轮配置的格式（谱面信息的编辑也是同一套）⇒
         和齿轮弹窗（`components/PhiSubMenu.vue`）、「文件名格式」弹窗（`components/ConfigView.vue`）
         同一套皮肤：`.log-card-only-window.phi-config-dialog`（内容区底色 + 88% 不透明、直角、
         标题居中 1.5rem、`__body` 无额外内边距）、宽度用「内容区的 60%」（见 `measureConfigDialogWidth`）。
         齿轮弹窗底下没有按钮（点空白处 / Esc 关闭），这里同样不再放「关闭」按钮。 -->
    <v-dialog v-model="chartInfoDialog" :width="configDialogWidth" class="log-card-bg">
      <v-card class="log-card-only-window phi-config-dialog">
        <div class="phi-config-dialog__head">
          <span class="phi-config-dialog__title" v-t="'chart-info'"></span>
        </div>
        <v-card-text class="config-root phi-config-dialog__body batch-info__body">
          <!-- 补上 `ref="form"` —— 以前这个表单没有 ref，checkInfo() 里的
               `form.value!.validate()` 取到 undefined 会抛错，批量渲染发不出去 -->
          <v-form ref="form" v-if="editingChart" validateOn="eager">
            <!-- 谱面信息的编辑应该和渲染谱面信息一样只有那些元素，再多加个 Tip。
                 另外预览的Tip也放到谱面信息里，在简介后面」⇒ 字段与渲染页（RenderView.vue 的
                 `#chart-info`）逐条对齐：曲名 / 曲师 / 谱师 / 画师 / 谱面文件 / 音乐文件 / 曲绘文件
                 / 难度 / 定数 一行两个，然后 简介 与 Tip 各占一整行、Tip 紧跟在简介后面。
                 原来这里的 宽高比 / 偏移 / 判定线长度 / 总分 / Hold 打击特效间隔倍率 以及五个勾选框
                 （强制宽高比 / Note 等比缩放 / 折叠式入场 / 允许负长度 Hold / Hold 尾部遮罩）全部删掉：
                 它们属于渲染参数与谱面附加格，批量渲染统一用预设里的那份配置。 -->
            <div class="cfg-grid cfg-grid--two">
              <CfgField :label="t('info.name')">
                <v-textarea rows="1" auto-grow density="compact" hide-details="auto" v-model="editingChart.chartInfo.name"></v-textarea>
              </CfgField>
              <CfgField :label="t('info.composer')">
                <v-textarea rows="1" auto-grow density="compact" hide-details="auto" v-model="editingChart.chartInfo.composer"></v-textarea>
              </CfgField>
              <CfgField :label="t('info.charter')">
                <v-textarea rows="1" auto-grow density="compact" hide-details="auto" v-model="editingChart.chartInfo.charter"></v-textarea>
              </CfgField>
              <CfgField :label="t('info.illustrator')">
                <v-textarea rows="1" auto-grow density="compact" hide-details="auto" v-model="editingChart.chartInfo.illustrator"></v-textarea>
              </CfgField>
              <!-- 谱面文件 / 音乐文件 / 曲绘文件：与渲染页一样就是可编辑的文本框（渲染页也没做选择器） -->
              <CfgField :label="t('info.chart')">
                <v-text-field type="text" density="compact" hide-details="auto" v-model="editingChart.chartInfo.chart"></v-text-field>
              </CfgField>
              <CfgField :label="t('info.music')">
                <v-text-field type="text" density="compact" hide-details="auto" v-model="editingChart.chartInfo.music"></v-text-field>
              </CfgField>
              <CfgField :label="t('info.illustration')">
                <v-text-field type="text" density="compact" hide-details="auto" v-model="editingChart.chartInfo.illustration"></v-text-field>
              </CfgField>
              <CfgField :label="t('info.difficulty')">
                <v-textarea rows="1" auto-grow density="compact" hide-details="auto" v-model="editingChart.chartInfo.difficulty"></v-textarea>
              </CfgField>
              <CfgField :label="t('info.lvl')">
                <v-text-field type="number" density="compact" hide-details="auto" :rules="[RULES.positiveOrZero]" v-model="lvlText" @blur="normalizeLvl"></v-text-field>
              </CfgField>
            </div>
            <!-- 简介 与 Tip：都是 `full` 条目 ⇒ 单开一个网格，`:has()` 规则会去掉栏间竖线 -->
            <div class="cfg-grid">
              <CfgField :label="t('info.intro')" full>
                <v-text-field type="text" density="compact" hide-details="auto" v-model="editingChart.chartInfo.intro"></v-text-field>
              </CfgField>
              <CfgField :label="t('info.tip')" full>
                <v-textarea rows="1" auto-grow density="compact" hide-details="auto" :placeholder="t('info.tip-placeholder')" v-model="editingChart.chartInfo.tip"></v-textarea>
              </CfgField>
            </div>
          </v-form>
        </v-card-text>
      </v-card>
    </v-dialog>

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

    <v-dialog v-model="outputDialog" class="log-card-bg">
      <v-card class="log-card-window">
        <v-card-title style="margin-bottom: -16px;" v-t="'output'"></v-card-title>
        <v-card-text style="padding-bottom: 0px;">
          <div
            class="block whitespace-pre overflow-auto log-card-msg user-select"
            style="height: calc(100vh - 240px);"
            v-html="filteredOutputDialogMessage"
          ></div>
          <v-combobox
            class="mt-4"
            variant="outlined"
            v-model="filter"
            :items="filterItems"
            clearable
            multiple
            placeholder="Filter (comma separated)"
            @update:model-value="(val: string[]) => {
              filteredOutputDialogMessage = filterText(outputDialogMessage, val);
            }"
          ></v-combobox>
        </v-card-text>
        <v-card-actions class="justify-end">
          <v-btn class="hover-scale" variant="text" @click="outputDialog = false" v-t="'close'"></v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>

    <v-overlay v-model="fileHovering" contained class="align-center justify-center drop-zone-overlay" persistent :close-on-content-click="false">
      <div class="drop-pulse">
        <h1 v-t="'choose.drop'"></h1>
      </div>
    </v-overlay>
  </div>
</template>

<style scoped>

.v-progress-linear,
.v-progress-linear__determinate {
  transition: none;
}

/* 整页铺满内容区（和渲染页 `.render-view` 同一套外壳） */
.batch-view {
  position: relative;
  display: flex;
  flex: 1;
  flex-direction: column;
  min-height: 0;
  overflow: hidden;
}

/* 谱面列表：表头在上，行自己滚 */
.batch-view__list {
  display: flex;
  flex: 1;
  flex-direction: column;
  min-height: 0;
  padding: var(--phi-space-3) var(--phi-space-6) 0;
}

.batch-view__list-body {
  position: relative;
  flex: 1;
  min-height: 0;
}

/* 列表栅格：选择框 / 名称 / 难度·状态 / 谱师·详情 / 路径·输出 / 编辑 / 预览。
   表头（--head）与每一行（--row）共用这套列宽，所以永远对齐；
   改列宽只改这里一处。 */
.batch-grid {
  display: grid;
  align-items: center;
  grid-template-columns:
    48px
    minmax(0, 2.4fr)
    minmax(0, 1.1fr)
    minmax(0, 1.1fr)
    minmax(0, 2.2fr)
    84px
    84px;
  column-gap: var(--phi-space-3);
  padding: 0 var(--phi-space-4);
}

.batch-grid--head {
  flex: none;
  height: 40px;
  background: var(--phi-bg-layer-1);
  font-size: 0.85rem;
  color: var(--phi-label-secondary);
  user-select: none;
}

.batch-grid--head .batch-grid__cell {
  cursor: pointer;
}

.batch-grid--head .batch-grid__cell:hover {
  color: var(--phi-static-bluish-00);
}

.batch-grid--row {
  height: 64px;
  border-bottom: 0.5px solid var(--phi-border-l3);
}

.batch-grid--row:hover {
  background: var(--phi-interactive-hover);
}

.batch-grid__cell {
  display: flex;
  align-items: center;
  min-width: 0;
}

.batch-grid__cell--center {
  justify-content: center;
}

/* 长文本一律单行省略号，完整内容在 title 里 */
.batch-grid__ellipsis {
  display: block;
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.batch-grid__clickable {
  cursor: pointer;
}

.batch-grid__clickable:hover {
  color: var(--phi-accent);
  text-decoration: underline;
}

/* 勾选框在 64px 的行里本来会撑出一堆内边距 */
.batch-grid__check {
  flex: none;
}

/* 空列表：中间只有一行文字，不画图标 */
.batch-view__empty {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
}

.batch-view__empty-title {
  margin: 0;
  font-size: 1.05rem;
  color: var(--phi-label-secondary);
}

/* 底部固定操作条（同渲染页 `.render-view__actions`）；内容多的时候允许换行 */
.batch-view__actions {
  display: flex;
  flex: none;
  flex-direction: column;
  gap: var(--phi-space-1);
  padding: var(--phi-space-2) var(--phi-space-6);
  border-top: 0.5px solid var(--phi-border-l3);
}

/* 操作条里的一排（现在一共两排，见模板注释）。
   这里强制只显示一行 ⇒ 不再换行（原来 `flex-wrap: wrap`，
   窗口窄一点后面的开关就掉到第二行去）。 */
.batch-view__bar {
  display: flex;
  flex-wrap: nowrap;
  align-items: center;
  gap: var(--phi-space-2);
  min-width: 0;
}

/* 上排：筛选那组固定 1/5，后面五项（打开输出文件夹 · 移除全部任务 · 三个开关）
   均分剩下的 4/5（`flex: 1 1 0` + `min-width: 0`，`justify-content: center` 让内容居中）。
   两个 `v-divider` 是分隔线，不参与均分（下面的规则在它之后，同特异性后者生效）。 */
.batch-view__bar--head > .batch-view__pickers {
  flex: 0 0 20%;
  min-width: 0;
}

.batch-view__bar--head > :not(.batch-view__pickers) {
  flex: 1 1 0;
  min-width: 0;
  justify-content: center;
}

.batch-view__bar--head > .v-divider {
  flex: 0 0 auto;
}

/* 下排：「预设配置」那一组（标签 + 下拉框 + 铅笔按钮）占整排的 60% */
.batch-view__bar--foot > .batch-view__pickers {
  flex: 0 0 60%;
  min-width: 0;
}

/* 操作条最左边的「预设配置 + 筛选」（「预设配置和筛选都放到下面，高度缩小30%」）。
   不再是 cfg-grid 的条目，改成条内的行内小控件：
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

/* 两个下拉框都不再写死像素宽度，改成撑满各自那一组
   （预设配置那组 = 整排 60%，筛选那组 = 整排 1/5） */
.batch-view__picker--preset,
.batch-view__picker--filter {
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

/* 谱面信息弹窗内容高的时候自己滚（皮肤来自 config.css 的 `.phi-config-dialog__body`） */
.batch-info__body {
  max-height: calc(100vh - 220px);
  overflow-y: auto;
}

/* 预设弹窗标题行不再放「保存」按钮了（改动随改随存，见 `setWorkingPreset`），
   原来的 `.batch-config-dialog__save` 那条绝对定位样式一并删掉。 */

.v-text-field :deep(.v-field--focused) {
  border-color: var(--phi-accent) !important;
}

h2 {
  font-weight: 600;
  letter-spacing: -0.025em;
  background: linear-gradient(45deg, var(--phi-accent), var(--phi-static-brand-500));
  background-clip: text;
  -webkit-text-fill-color: transparent;
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

:deep(.v-slider__thumb) {
  background: var(--phi-accent) !important;

}

:deep(.v-slider__track-fill) {
  background: var(--phi-accent) !important;
}

</style>

<!-- 下拉栏（Vuetify 的菜单）是 teleport 到 `.v-overlay-container` 下的，scoped 样式够不着 ⇒
     这里单独一段**非 scoped** 样式；用 combobox 上的 `menu-props.contentClass` 限定作用范围，
     免得顺手改到设置页那个语言下拉。字号与字段里的输入框保持一致。 -->
<style>
.batch-preset-menu .v-list-item-title {
  font-size: 0.85rem;
}
</style>
