<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount, watch, nextTick, computed } from 'vue';
import { useRouter } from 'vue-router';

import { useI18n } from 'vue-i18n';
const { t } = useI18n();

import { invoke } from '@tauri-apps/api/core';
import { event } from '@tauri-apps/api';

import { toastError, RULES, toast, anyFilter, isString, openExternal } from './common';
import { DEFAULT_APP_CONFIG } from './model';
import {
  APPENDANT_DOWNLOAD_PASSWORD,
  APPENDANT_DOWNLOAD_URL,
  appendantItem,
  appendantUrl,
  fetchAppendants,
  fetchRespacks,
  fetchRespackDir,
  isBaoyingSpChart,
  isJingluanSpChart,
  isOptionalAppendant,
  isQzkagoChart,
  isRrChart,
  lavaPack,
  missingAppendant,
  openAppendantDir,
  waterPack,
} from './appendant';
import type { AppendantItem } from './appendant';
import { formatLvl, useLvlField } from './lvl';
import type { AppConfig, ChartInfo, FileDropEvent, RenderConfig } from './model';
import type { WorkingState } from './config/types';
import { chartSideOf } from './config/chartSide';
import { jumpToFirstError } from './formError';

import { VForm } from 'vuetify/components';

import ConfigView from './components/ConfigView.vue';
import ChartHero from './components/ChartHero.vue';
import CfgField from './components/CfgField.vue';

import moment from 'moment';
import * as dialog from "@tauri-apps/plugin-dialog"

import { listen } from "@tauri-apps/api/event";
import { StorageSerializers, useStorage } from '@vueuse/core';
import { finishPageLoad } from './pageLoad';

const router = useRouter();

// 流程顺序: 只有一步「渲染参数」(谱面信息就在这一页最下面); 提交后只弹绿色提示, 没有「渲染视频」完成页
// 谱面只支持拖动导入, 没有"选择谱面 / 谱面信息"两个独立步骤
//
// 渲染参数(分辨率 / 编码器 / 音频 / 高级开关…)的控件都在 ./components/ConfigView.vue，
// 新增参数的完整步骤见那个文件顶部的注释（以及 ../model.ts、src-tauri/src/render.rs 里同样的说明）。
// 这个文件负责的是另外三件事:
//   - 谱面信息表单（chart-info 插槽: 名称 / 难度文本 / 谱师 / 音乐制作 / 插画设计 / 简介 / 标签 …），
//     直接绑定 chartInfo —— 这些文字框是用户明确排除在配置之外的「谱面信息」
//   - 最近一次导入的谱面本身（chartPath / chartInfo）与 Tip 的本地记忆（见下面的 ChartRowState）
//   - buildParams() 调用 configView 的 buildConfig()，再把 { path, info, config } 交给 invoke('post_render')
// 谱面侧的渲染参数（宽高比 / 强制宽高比 / 折叠式入场 / 判定线长度 / 总分 / 预览起止 / 偏移 /
// Note 等比缩放等）现在是 `RenderConfig` 的字段，控件都在 ConfigView.vue：
// buildParams() 里用 chartSideOf(config) 把它们覆盖进 info（配置值优先，导入谱面不再回填）。
// 整页只有渲染参数一页(谱面信息就在这一页最下面)。提交渲染后不再切到「渲染视频」完成页,
// 只弹一个绿色提示(见 moveNext), 进度与结果都在「任务」页里看。
// 整页直接铺满内容区（外面没有 v-stepper 卡片）。

// 只有"选择谱面"之后才知道谱面宽高比, 由此决定是否允许自动调整分辨率
const aspectRatioApplied = ref(false);

const chartInfo = ref<ChartInfo>();

// 谱面信息里的「定数」输入框：显示一律补成一位小数（见 `./lvl.ts`），
// 输入过程仍然随手打（`13.` 不会被改写），数值照旧写回 `chartInfo.lvl`。
const { text: lvlText, normalize: normalizeLvl } = useLvlField(
  () => chartInfo.value?.lvl,
  (value) => {
    if (chartInfo.value) chartInfo.value.lvl = value;
  },
);

// 「难度文本」整串默认由「难度标识」+「定数」拼出来（`AT` + `18.0` → `AT Lv.18.0`）。
// 只在两者变化时覆盖，手改过的「难度文本」会一直留到下一次改动它们为止。
watch(
  [() => chartInfo.value?.diff, () => chartInfo.value?.lvl],
  ([diff, lvl]) => {
    if (!chartInfo.value) return;
    const tier = String(diff ?? '').trim();
    const lvlValue = formatLvl(lvl);
    const parts = [tier, lvlValue ? `Lv.${lvlValue}` : ''].filter(Boolean);
    if (parts.length) chartInfo.value.difficulty = parts.join(' ');
  },
);

// 谱面信息里的「偏移」（毫秒）与「总分」：值住在渲染配置里（`RenderConfig.offset` / `scoreTotal`），
// 控件在本页的谱面信息表单里。下面两个字符串是表单那一侧的镜像：
//   - 解析完谱面 / 套用预设时，ConfigView 通过 `setChartInfoSlots()` 把配置里的值写进来；
//   - 用户在这里改了，就回写进 ConfigView（`configView.setChartInfoSlots()`）——buildConfig() 会取它。
// 用字符串是为了跟其它输入框一样随手打、不清空中间态；换算与校验交给 buildConfig() 与 rules。
const offsetText = ref('');
const scoreTotalText = ref('');
const chartInfoSlotsReady = ref(false); // 配置里的值还没拿到之前，别把空字符串写回去

// 「游玩」按钮默认不显示, 由「设置」里的「开启游玩按键」决定(读的是后端 config.toml)
const appConfig = ref(DEFAULT_APP_CONFIG);
// 不在 `onMounted` 里等它：读得早、也方便页面初始化遮罩把它一起盖住（见 `./pageLoad`）
const appConfigReady = invoke('read_config')
  .then((config) => {
    appConfig.value = config as AppConfig;
  })
  .catch(() => {
    // 读不到配置就按默认值处理: 不显示「游玩」
  });

const chartPath = ref('');

const loadingParsingChart = ref(false);

// 谱面宽高比只在第一次拿到时同步到分辨率, 之后尊重用户自己的设置。
// 「宽 : 高」现在是配置项（`RenderConfig.aspectRatio`），所以这里同步的是**配置里那个比值**，
// 两个输入框与换算都在 `ConfigView.vue`（`aspectWidth / aspectHeight` + `commitAspect()`）。
async function applyChartAspectRatio() {
  if (aspectRatioApplied.value) return;
  if (!chartInfo.value) return;
  aspectRatioApplied.value = true;
  await nextTick();
  configView.value?.applyAspectRatio();
}

async function loadChart(file: string, info?: ChartInfo, config?: RenderConfig) {
  try {
    loadingParsingChart.value = true;
    chartPath.value = file;
    if (info) {
      chartInfo.value = info;
      await new Promise(r => setTimeout(r, 0));
    } else {
      chartInfo.value = (await invoke('parse_chart', { path: file })) as ChartInfo;
    }
    // 谱面里带的那批值（宽高比 / 偏移 / 判定线长度 / 总分 / 预览起止 / 各开关）不能覆盖配置：
    // 它们是**配置项**（见 model.ts 的「谱面侧渲染参数」与 config/chartSide.ts）——
    // 配置里存的是什么就用什么，否则「把预设存下来」就没有意义了。
    // 不强制跳转: 不切页签、不改当前步骤, 用户就停在自己正在看的地方
    await nextTick();
    if (config) {
      await nextTick();
      configView.value?.applyConfig(config);
    }
    applyChartAspectRatio();
  } catch (e) {
    toastError(e);
  } finally {
    loadingParsingChart.value = false;
  }
}

/* 「谱面信息」插槽里剩下的那点东西 —— Tip（简介下面那个文本框）—— 与**最近一次导入的谱面本身**
   （`chartPath` + `chartInfo`，也就是谱面信息里那些文字框和标签）常驻本地，照工作配置存一份：
   切页/重启回来还在。

    宽高比 / 偏移 / 判定线长度 / 总分 / 预览起止 /
    Note 等比缩放等开关，以及 `aspectWidth / aspectHeight / offsetText` 全部是
   **配置项**（`RenderConfig`，见 ../model.ts 的「谱面侧渲染参数」），控件与换算都在
   `components/ConfigView.vue`，所以这里不保存它们。 */
type ChartRowState = {
  /** 「谱面信息」的 Tip（唯一还留在这边的谱面侧文本） */
  tip?: string;
  chartPath?: string;
  chartInfo?: ChartInfo | null;
};
const chartTip = ref('');
const chartRowStorage = useStorage<ChartRowState | null>('RenderView.ChartRow', null, localStorage, { serializer: StorageSerializers.object });
let chartRowTimer: ReturnType<typeof setTimeout> | undefined;

function saveChartRow() {
  chartRowTimer = undefined;
  const state: ChartRowState = {
    tip: chartTip.value,
    chartPath: chartPath.value,
    chartInfo: chartInfo.value ?? null,
  };
  chartRowStorage.value = state;
  // 与工作配置同理：`useStorage` 的落盘 watcher 会随组件卸载停掉，这里同步写一份
  localStorage.setItem('RenderView.ChartRow', JSON.stringify(state));
}

watch(
  () => JSON.stringify({
    tip: chartTip.value,
    chartPath: chartPath.value,
    chartInfo: chartInfo.value,
  }),
  () => {
    clearTimeout(chartRowTimer);
    chartRowTimer = setTimeout(saveChartRow, 400);
  },
);

onBeforeUnmount(() => {
  if (chartRowTimer === undefined) return;
  clearTimeout(chartRowTimer);
  saveChartRow();
});

const fileHovering = ref(false);

listen('tauri://drag-over', () => (fileHovering.value = true));
listen('tauri://drag-leave', () => (fileHovering.value = false));
listen('tauri://drag-drop', async (event) => {
  const files = (event.payload as FileDropEvent).paths;
  
  // 只支持拖动导入谱面: 拖入后直接解析, 并回到"渲染参数"页(谱面信息就在这一页最下面)
  fileHovering.value = false;
  await loadChart(files[0] as string);
});

document.addEventListener('keydown', async (event) => {
  if (document.hasFocus() && event.key === 'PageDown') {
    await moveNext();
  }
});

const form = ref<VForm>();

const configView = ref<typeof ConfigView>();

/* ------------------------- 附带资源包（appendant） -------------------------
 * 皮肤包与 Minecraft 材质包不随程序分发，用户自己下载 zip 放进 `<程序目录>/appendant`。
 * 选中了缺的包（或开了需要 Minecraft 材质的特效）时，不发请求，先弹窗告诉用户下载与放哪。 */
const appendantDialog = ref(false);
const appendantMissing = ref<AppendantItem[]>([]);
const appendantDir = ref('');

const appendantMissingText = computed(() =>
  appendantMissing.value
    .map((it) => {
      const dir = it.dir ?? appendantDir.value;
      // 包在、只是缺文件：把缺的名字列出来，用户才分得清「没下载」和「下载坏了」
      if (it.found && it.missing?.length)
        return t('appendant-incomplete-detail', { file: it.file, files: it.missing.join('、'), dir });
      return t('appendant-missing-detail', { file: it.file, dir });
    })
    .join('\n\n')
);

/**
 * 检查这次渲染要用到的资源包是不是都在。
 *
 * 缺包就弹窗并返回 `false`：资源加载不了，渲染进程会直接报错退出，不如在界面先拦一道。
 * 读不到状态（后端异常）时不拦，交给后面的正常流程报错。
 */
async function checkAppendants(config: RenderConfig) {
  let status;
  try {
    status = await fetchAppendants();
  } catch (e) {
    console.warn(e);
    return true;
  }
  const missing: AppendantItem[] = [];
  const addMissing = (item: AppendantItem | null | undefined) => {
    // 可选包（`appendant.official`）缺了不算缺：后端会自动退回内置默认皮肤
    if (item && !item.present && !isOptionalAppendant(item.file) && !missing.some((it) => it.file.toLowerCase() === item.file.toLowerCase()))
      missing.push(item);
  };
  addMissing(missingAppendant(status, config.resPackPath));
  // 下拉框里选的是 `respack/` 里的自备皮肤：包在、但缺文件时也要拦（缺文件的包加载到一半就报错，
  // 错误位置离用户的操作很远）。路径指到 `appendant/` 里的包时上面已经查过了。
  const respackFile = (config.resPackPath ?? '').replace(/\\/g, '/').split('/').pop() ?? '';
  if (config.resPackPath && !appendantItem(status, respackFile)) {
    try {
      const item = (await fetchRespacks()).find((it) => it.path === config.resPackPath);
      if (item && !item.present)
        addMissing({
          file: item.name,
          path: item.path,
          found: true,
          present: false,
          missing: item.missing,
          dir: await fetchRespackDir(),
        });
    } catch (e) {
      console.warn(e);
    }
  }
  // 岩浆噪域读 appendant.lava.zip（基岩贴图 + 岩浆帧），屏幕进水（含水下受伤音效）读 appendant.water.zip
  if (config.blockAreaLava) addMissing(lavaPack(status));
  if (config.waterScreen) addMissing(waterPack(status));
  // 「报应SP」谱面固定钉在 appendant.retri.zip 上，配置里选别的皮肤也换不掉
  if (isBaoyingSpChart(chartPath.value)) addMissing(appendantItem(status, 'appendant.retri.zip'));
  // 「痉挛SP」的演出插图只有 appendant.spas.zip 里有，缺了这段演出就什么都不画
  if (isJingluanSpChart(chartPath.value)) addMissing(appendantItem(status, 'appendant.spas.zip'));
  // 「QZKAGO」的 7 张整屏静图只有 appendant.qzk.zip 里有，缺了那 7.10 秒什么都不画
  if (isQzkagoChart(chartPath.value)) addMissing(appendantItem(status, 'appendant.qzk.zip'));
  // 「Rr」的按键贴图与 AP 打击特效只有 appendant.rr.zip 里有，缺包会退回默认皮肤
  if (isRrChart(chartPath.value)) addMissing(appendantItem(status, 'appendant.rr.zip'));
  if (!missing.length) return true;
  appendantDir.value = status.dir;
  appendantMissing.value = missing;
  appendantDialog.value = true;
  return false;
}

async function openMissingDownload() {
  const url = appendantMissing.value.map((it) => appendantUrl(it.file)).find((it) => !!it);
  if (!url) {
    toast(t('appendant-no-download'), 'info');
    return;
  }
  await openExternal(url);
}

async function buildParams() {
  if (!(await checkInfo())) return null;
  let config = await configView.value!.buildConfig();
  if (!config) return null;
  if (!(await checkAppendants(config))) return null;
  // 谱面侧渲染参数（宽高比 / 强制宽高比 / 折叠式入场 / 判定线长度 / 总分 / 预览起止 / 偏移 /
  // 背景压暗 …）现在是配置项：**以配置为准**，所以展开在谱面信息**之后**覆盖同名键。
  // 其余键（名称 / 难度 / 曲绘 / Tip …）仍然来自解析出来的 chartInfo。
  return {
    path: chartPath.value,
    info: chartInfo.value ? { ...chartInfo.value, ...chartSideOf(config) } : chartInfo.value,
    config,
  };
}

// 渲染参数(ConfigView)与谱面信息都走各自的表单校验, 这里不再重复弹 toast
async function postRender() {
  try {
    let params = await buildParams();
    if (!params) return false;
    await invoke('post_render', { params });
    return true;
  } catch (e) {
    toastError(e);
    return false;
  }
}

const loadingNext = ref(false);
const loadingPreview = ref(false);
const loadingPlay = ref(false);
const loadingTweakoffset = ref(false);

async function previewChart() {
  loadingPreview.value = true;
  try {
    let params = await buildParams();
    if (!params) return false;
    await invoke('preview_chart', { params });
    return true;
  } catch (e) {
    toastError(e);
    return false;
  } finally {
    loadingPreview.value = false;
  }
}

async function previewTweakoffset() {
  loadingTweakoffset.value = true;
  try {
    let params = await buildParams();
    if (!params) return false;
    let offset = await invoke('preview_tweakoffset', { params });
    if (offset != null) {
      // 偏移是配置项：后端算回来的秒数写回「偏移」那一格（界面按毫秒显示），
      // 下一次 buildParams() 会带着这个值再发给后端
      const ms = Math.round((offset as number) * 1000);
      offsetText.value = String(ms);
      configView.value?.setChartInfoSlots({ offset: ms / 1000 });
    }
    return true;
  } catch (e) {
    toastError(e);
    return false;
  } finally {
    loadingTweakoffset.value = false
  }
}

async function previewPlay() {
  loadingPlay.value = true;
  try {
    let params = await buildParams();
    if (!params) return false;
    await invoke('preview_play', { params });
    return true;
  } catch (e) {
    toastError(e);
    return false;
  } finally {
    loadingPlay.value = false;
  }
}

const renderMsg = ref(''),
  renderProgress = ref<number>(),
  renderDuration = ref<number>();
event.listen('render-msg', (msg) => (renderMsg.value = msg.payload as string));
event.listen('render-progress', (msg) => {
  let payload = msg.payload as { progress: number; fps: number; estimate: number };
  // 不能写 `t('render-status', …)`：语言包里没有这个键（i18n 的 missing 处理器会把键名原样返回），
  // 渲染中会显示字面量「render-status」。同一套占位符（progress / fps / estimate）现成的键是
  // `task.status.rendering`，用它。
  renderMsg.value = t('task.status.rendering', {
    progress: (payload.progress * 100).toFixed(2),
    fps: payload.fps,
    estimate: moment.duration(payload.estimate, 'seconds').humanize(true, { ss: 1 }),
  });
  renderProgress.value = payload.progress * 100;
  console.log(renderProgress.value);
});
event.listen('render-done', (msg) => {
  renderDuration.value = Math.round(msg.payload as number);
});

async function moveNext() {
  // 只有一步: 校验谱面信息与渲染参数, 提交成功后弹个绿色提示(进度/结果在「任务」页看)
  if (!chartInfo.value) return; // 与"渲染"按钮的禁用条件保持一致
  loadingNext.value = true;
  if (await postRender()) {
    toast(t('render-started'), 'success');
  }
  loadingNext.value = false;
}

const chartInQuery = router.currentRoute.value.query.chart;
const infoInQuery = router.currentRoute.value.query.info;
const configInQuery = router.currentRoute.value.query.config;

// 外部(绑定页/批量/任务)带谱面跳转: 直接载入谱面, 停在"渲染参数"页(谱面信息就在这一页最下面)
// 参数(分辨率等)沿用跳转方传来的内容
if (isString(chartInQuery)) {
  onMounted(async () => {
    await loadChart(chartInQuery, isString(infoInQuery) ? (JSON.parse(infoInQuery) as ChartInfo) : undefined, isString(configInQuery) ? (JSON.parse(configInQuery) as RenderConfig) : undefined);
    // 谱面解析完、那枚「游玩」按钮的状态也定下来之后，才揭开内容区那层转圈（见 `./pageLoad`）
    await appConfigReady;
    await nextTick();
    finishPageLoad();
  });
}

/* ---------------------- 渲染页的「工作配置」 ----------------------
   界面上改一下就往本地存一下（`localStorage`，`ConfigView` 发 `change` 事件过来），重启还在，
   而且**绝不写进任何命名预设** —— 预设那边只会把下拉框里的名字显示成 `*原名`。
   默认那份是 `null` 而不是 `{}`：vueuse 按默认值猜序列化器，`null` 会猜成「原样字符串」，
   所以这里显式指定用 JSON。带 `?config=` 跳进来时（批量页 / 任务页「载入到渲染页」）
   以传进来的那份为准，不拿本地这份去覆盖它。 */
const workingConfig = useStorage<WorkingState | null>('RenderView.Config', null, localStorage, { serializer: StorageSerializers.object });

/** 兼容老值：可能是一份没有预设名字的裸 `RenderConfig`，读到也要能用 */
function readWorking(): WorkingState | null {
  const saved = workingConfig.value as WorkingState | RenderConfig | null;
  if (!saved) return null;
  if ('config' in saved) return saved;
  return { config: saved as RenderConfig, presetKey: '', presetName: '', dirty: false };
}

function onConfigState(state: WorkingState) {
  workingConfig.value = state;
  // `useStorage` 的落盘是 setup 作用域里的一个 watch（vueuse 内部的 `pausableWatch`），
  // 组件卸载时它已经停了 —— 切页那一刻发出来的最后一批改动会丢，所以这里同步写一份。
  localStorage.setItem('RenderView.Config', JSON.stringify(state));
}

/** ConfigView 程序化改写了「总分 / 偏移」（套用预设、读回工作配置、调整偏移算回新值）：
 *  把值回填进谱面信息表单里那两个输入框。 */
let pushingFromForm = false;
function onChartInfoSlots(values: { scoreTotal: number; offset: number }) {
  chartInfoSlotsReady.value = true;
  // 这一批是表单自己改出来的回声：不要再写回去（会把正在敲的「12.3」取整成「12」）
  if (pushingFromForm) return;
  offsetText.value = String(Math.round(values.offset * 1000));
  scoreTotalText.value = String(values.scoreTotal);
}

/** 表单里改了「总分 / 偏移」：写回配置（配置是唯一的存储，这两个输入框只是它的控件） */
watch([offsetText, scoreTotalText], ([offset, scoreTotal]) => {
  if (!chartInfoSlotsReady.value) return;
  // 空串 / 半截输入（"-"、"1."）不算数：等用户敲出个数再写，别把配置写成 NaN
  const offsetMs = Number(offset);
  const total = Number(scoreTotal);
  if (!Number.isFinite(offsetMs) || !Number.isFinite(total)) return;
  pushingFromForm = true;
  try {
    configView.value?.setChartInfoSlots({ offset: offsetMs / 1000, scoreTotal: total });
  } finally {
    pushingFromForm = false;
  }
});

if (!isString(chartInQuery)) {
  onMounted(async () => {
    try {
      // 先把「谱面信息」这边的常驻值放回去（Tip 与最近一次导入的谱面本身；
      // 谱面侧渲染参数已经是配置项，由下面 `restoreWorking(saved)` 一并读回）
      const savedChart = chartRowStorage.value;
      if (savedChart) {
        // 旧版本把 Tip 存在 `chartRow.tip` 里，这里兼容着读一次（读不到就算了）
        const tip = savedChart.tip ?? (savedChart as { chartRow?: { tip?: string } }).chartRow?.tip;
        if (tip) chartTip.value = tip;
        // 最近一次导入的谱面本身也放回去：谱面信息里那些文字框、标签、以及「渲染」要用的路径
        if (savedChart.chartPath) chartPath.value = savedChart.chartPath;
        if (savedChart.chartInfo) chartInfo.value = savedChart.chartInfo;
      }
      const saved = readWorking();
      if (!saved) return;
      // 材质包列表要先到位：`applyConfig` 是按**路径**在列表里找材质包的
      // （`config/fields.ts` 的 `resPackPath.toUi`），列表还没加载完就会退回默认材质包，
      // 工作配置里存的材质包选择就白存了。
      await configView.value?.updateRespacks();
      configView.value?.restoreWorking(saved);
    } finally {
      // 「总分 / 偏移」的输入框要显示配置里的值（没存过工作配置时就是默认值）：
      // 那两个值由 ConfigView 持有，这里主动拉一次（restoreWorking / 套用预设也会推过来）
      const slots = configView.value?.chartInfoValues();
      if (slots) onChartInfoSlots(slots);
      // 工作配置（含下拉框名字与星号）已经落到界面上，这才揭开内容区那层转圈（见 `./pageLoad`）
      await appConfigReady;
      await nextTick();
      finishPageLoad();
    }
  });
}

const tagEditor = ref(false);
async function checkInfo() {
  const { valid, errors } = await form.value!.validate();
  if (!valid) {
    // 谱面信息在整页最下面，标红的小字用户看不到：把用户带到出错的控件上再提示
    toast(jumpToFirstError(errors) ?? t('has-error'), 'error');
    return false;
  }
  // 「偏移」与「预览开始 / 结束」是配置项（`RenderConfig`，见 ../model.ts）：
  // 它们由 ConfigView 的控件负责，并在 buildParams() 里覆盖进 info（config/chartSide.ts），
  // 所以这里不能从谱面信息这边取值（那会把谱面信息里的同名值改写成界面上的值）。
  // 这个函数只剩谱面信息自己的收尾：Tip 是空串就写 null（存 info.yml 时才是干净的 yml）。
  if (!chartInfo.value!.tip?.trim().length) chartInfo.value!.tip = null;

  return true;
}

async function saveInfo() {
  if (!(await checkInfo())) return;
  let outputPath = await dialog.save({
    title: t('phira-chart-info'),
    filters: [
      { name: 'Phira Chart Info File', extensions: ['yml'] },
      anyFilter(),
    ],
    defaultPath: 'info.yml',
  });
  if (!outputPath) return;
  try {
    await invoke('save_info', { path: outputPath, info: chartInfo.value });
    dialog.message(t('save-success'), { title: t('save-info') })
} catch (e) {
    toastError(e);
  }
}

async function readInfo() {
  let inputPath = await dialog.open({
    title: t('phira-chart-info'),
    filters: [
      { name: 'Phira Chart Info File', extensions: ['yml'] },
      anyFilter(),
    ],
  });
  if (!inputPath) return;
  try {
    chartInfo.value = await invoke('read_info', { path: inputPath }) as ChartInfo;
    // 宽高比是配置项，导入谱面不回填；这里只按谱面画幅比调一下分辨率（见 applyChartAspectRatio）
    applyChartAspectRatio();
    dialog.message(t('read-success'), { title: t('read-info') })
} catch (e) {
    toastError(e);
  }
}

const tagListText = t('info.tag-list').split(','); // wtf bro
const tagList = ['Regular', 'Troll', 'Plain', 'Visual']
const tagitems = [
    tagList[0],
    tagList[1],
    tagList[2],
    tagList[3],
]

watch(() => chartInfo.value?.tags ?? [], (newVal, oldVal) => {
  let addedPreset = newVal.find(item =>
    tagList.some(tag => tag.toLowerCase() === item.toLowerCase()) &&
    !oldVal.some(tag => tag === item) // 比较旧值
  );

  if (addedPreset) {
    const matchedTag = tagList.find(tag => tag.toLowerCase() === addedPreset?.toLowerCase());
    if (matchedTag) {
      addedPreset = matchedTag;
    }

    const customItems = newVal.filter(item =>
      !tagList.some(tag => tag.toLowerCase() === item.toLowerCase())
    );
    
    chartInfo.value!.tags = [...customItems, addedPreset];
  }
});

</script>

<template>
  <div class="render-view">
    <!-- 整页滚动交给这一层；ConfigView 自己不滚动（--config-scroll-max-height: none） -->
    <div class="render-view__scroll">
      <!-- 谱面信息固定在整页最上面(ConfigView 的 chart-head 横幅 + chart-info 表单两个插槽)
           横幅 = 曲绘背景 + 曲名/难度/定数/谱师; 未导入谱面时横幅自己显示拖放提示 -->
      <ConfigView ref="configView" :has-chart="!!chartInfo" :show-drop-hint="true" style="--config-scroll-max-height: none" @state="onConfigState" @chart-info="onChartInfoSlots">
          <template #chart-head>
            <ChartHero :info="chartInfo" :path="chartPath"></ChartHero>
          </template>
          <template #chart-info>
            <v-form ref="form" validateOn="eager" v-if="chartInfo">
          <!-- 格式与配置页一致：曲名、曲师、谱师、画师一行两个，谱面文件、音乐文件、
               曲绘文件、难度、定数一行三个，简介与 Tip 各自一行一个。用配置页那套
               `.cfg-grid` + `CfgField`（名称在左、控件在右，条目之间有栏间竖线）而不是 v-row / v-col。
               曲名 / 曲师 / 谱师 / 画师用 `cfg-grid--two`（两栏、栏间只留中间那一条竖线），
               简介仍是 `full` 独占整行。
               九个条目同属**一个网格**（曲名│曲师 / 谱师│画师 / 谱面文件│音乐文件 / 曲绘文件│难度 / 定数），
               只剩 简介 那个 `full` 网格单独一块。
               分网格的理由：`design/config.css:55-60` 的 `:has()` 规则只给「里面全是 full 条目」的网格
               去掉栏间竖线，full 与普通条目混在一个网格里时竖线会横穿整行的长输入框（Tip / 文件名格式踩过）。 -->
          <div class="cfg-grid cfg-grid--two">
            <CfgField :label="t('info.name')">
              <v-textarea rows="1" auto-grow density="compact" hide-details="auto" v-model="chartInfo.name"></v-textarea>
            </CfgField>
            <CfgField :label="t('info.composer')">
              <v-textarea rows="1" auto-grow density="compact" hide-details="auto" v-model="chartInfo.composer"></v-textarea>
            </CfgField>
            <CfgField :label="t('info.charter')">
              <v-textarea rows="1" auto-grow density="compact" hide-details="auto" v-model="chartInfo.charter"></v-textarea>
            </CfgField>
            <CfgField :label="t('info.illustrator')">
              <v-textarea rows="1" auto-grow density="compact" hide-details="auto" v-model="chartInfo.illustrator"></v-textarea>
            </CfgField>
            <!-- 谱面文件 / 音乐文件 / 曲绘文件 + 难度标识 / 定数 / 难度文本 / 偏移 / 总分：紧接在上面那四个后面，同属**一个**网格
                 （判定线长度在「谱面-判定线」）。
                 「难度文本」是整串（`AT Lv.18.0`），默认由难度标识 + 定数拼出来，也能手改。
                 「偏移」「总分」是**配置项**（`RenderConfig.offset` / `scoreTotal`，随预设存），
                 但它们的值每谱面都要调，所以控件留在这里、值由 RenderView 与 ConfigView 互通
                 （见下面的 watch 与 `setChartInfoSlots`）。 -->
            <CfgField :label="t('info.chart')">
              <v-text-field type="text" density="compact" hide-details="auto" v-model="chartInfo.chart"></v-text-field>
            </CfgField>
            <CfgField :label="t('info.music')">
              <v-text-field type="text" density="compact" hide-details="auto" v-model="chartInfo.music"></v-text-field>
            </CfgField>
            <CfgField :label="t('info.illustration')">
              <v-text-field type="text" density="compact" hide-details="auto" v-model="chartInfo.illustration"></v-text-field>
            </CfgField>
            <CfgField :label="t('info.diff')">
              <v-text-field type="text" density="compact" hide-details="auto" v-model="chartInfo.diff"></v-text-field>
            </CfgField>
            <CfgField :label="t('info.lvl')">
              <!-- 定数可以是 0 / 空（官谱里常见 `Lv.0.0`、也可能没写）：只拦负数和乱写，不拦空 -->
              <v-text-field type="number" density="compact" hide-details="auto" :rules="[RULES.positiveOrNull]" v-model="lvlText" @blur="normalizeLvl"></v-text-field>
            </CfgField>
            <CfgField :label="t('info.difficulty')">
              <v-textarea rows="1" auto-grow density="compact" hide-details="auto" v-model="chartInfo.difficulty"></v-textarea>
            </CfgField>
            <!-- 偏移（毫秒）与总分：值住在渲染配置里，控件在这里。
                 偏移的「调整偏移」按钮在整页底部（`loadingTweakoffset` 也是那边的本地状态） -->
            <CfgField :label="t('info.offset')">
              <v-text-field type="number" density="compact" hide-details="auto" :rules="[RULES.notEmpty]" v-model="offsetText"></v-text-field>
            </CfgField>
            <CfgField :label="t('info.score-total')">
              <v-text-field type="number" density="compact" hide-details="auto" :rules="[RULES.less4000000000, RULES.positive]" v-model="scoreTotalText"></v-text-field>
            </CfgField>
          </div>

          <!-- 简介 / Tip: 各独占一行 —— 单独一个网格：
               它们要是和上面那九项混在一个网格里，栏间那条竖线会从这一整行里穿过去。
               Tip 排在简介正下方。
               这个网格里全是 `full` 条目，`design/config.css:55-60` 的 `:has()` 规则会给它去掉栏间竖线。 -->
          <div class="cfg-grid">
            <CfgField :label="t('info.intro')" full>
              <v-text-field type="text" density="compact" hide-details="auto" v-model="chartInfo.intro"></v-text-field>
            </CfgField>
            <CfgField :label="t('info.tip')" full>
              <v-textarea rows="1" auto-grow density="compact" hide-details="auto" :placeholder="t('info.tip-placeholder')" v-model="chartTip"></v-textarea>
            </CfgField>
          </div>
          <!-- 「标签编辑器 / 读取信息 / 保存信息」三个按钮暂时隐藏; 要恢复就把下面的 v-if="false" 去掉 -->
          <div v-if="false" class="d-flex flex-row justify-end align-center my-2">
            <v-btn class="hover-scale mr-2" variant="text" @click="tagEditor = true" v-t="'info.tag-editor'"></v-btn>
            <v-btn class="hover-scale mr-2" variant="text" @click="readInfo" v-t="'read-info'"></v-btn>
            <v-btn class="hover-scale" variant="text" @click="saveInfo" v-t="'save-info'"></v-btn>
          </div>
            </v-form>
          </template>
          <!-- 「谱面侧渲染参数」的控件不在这一页：它们是**配置项**（`RenderConfig`，见 model.ts 的
               「谱面侧渲染参数」），控件与换算都在 `ConfigView.vue`，所以这里没有任何插槽
               （`#chart-extra` / `#line-extra` / `#params-switches` / `#params-fields` /
               `#params-preview` / `#params-offset` / `#params-score` 这 7 个都已移除）。
               这些值在 `buildParams()` 里通过 `chartSideOf(config)` 覆盖进发给后端的 info。 -->
      </ConfigView>
    </div>

    <!-- 内容区底部的固定操作条：这排按钮贴底、不随页面滚动 -->
    <div class="render-view__actions">
      <v-btn :loading="loadingTweakoffset" :disabled="!chartInfo" variant="text" @click="previewTweakoffset">{{ t('tweakoffset') }}</v-btn>
      <v-spacer />
      <!-- 「游玩」默认关闭, 在「设置」里打开才显示 -->
      <v-btn v-if="appConfig.enablePlayButton" :loading="loadingPlay" :disabled="!chartInfo" variant="tonal" @click="previewPlay" class="gradient-primary">{{ t('play') }}</v-btn>
      <v-btn :loading="loadingPreview" :disabled="!chartInfo" variant="tonal" @click="previewChart" class="gradient-primary">{{ t('preview') }}</v-btn>
      <v-btn :loading="loadingNext" variant="tonal" :disabled="!chartInfo" @click="moveNext" class="gradient-primary">{{ t('render') }}</v-btn>
    </div>

      <v-dialog v-model="tagEditor" width="auto" min-width="90%" class="log-card-bg">
        <v-card class="log-card-only-window">
          <v-card-title v-t="'info.tag-editor'"> </v-card-title>
          <v-card-text>

            <v-form v-if="chartInfo">
              <v-row>
                <v-col cols="12">
                  <v-combobox
                    v-model="chartInfo.tags"
                    :items="tagitems"
                    multiple
                    chips
                    :clearable="true"
                    :label="t('info.tags')"
                    :item-props="true"
                    allow-custom
                  ></v-combobox>
                </v-col>
                
              </v-row>

            </v-form>
          </v-card-text>
          <v-card-actions class="justify-end">
            <v-btn class="hover-scale" variant="text" @click="tagEditor = false" v-t="'close'"></v-btn>
          </v-card-actions>
        </v-card>
      </v-dialog>

    <v-overlay v-model="fileHovering" contained class="align-center justify-center drop-zone-overlay" persistent :close-on-content-click="false">
      <div class="drop-pulse">
        <h1 v-t="'choose.drop'"></h1>
      </div>
    </v-overlay>
    <!-- 拖入谱面后的解析遮罩 -->
    <v-overlay v-model="loadingParsingChart" contained class="align-center justify-center" persistent :close-on-content-click="false">
      <v-progress-circular indeterminate> </v-progress-circular>
    </v-overlay>
    <!-- 附带资源包缺失：皮肤包与材质包要用户自己下载后放进 appendant/ -->
    <v-dialog v-model="appendantDialog" width="auto" min-width="400px" class="log-card-bg">
      <v-card class="log-card-window">
        <v-card-title v-t="'appendant-missing-title'"></v-card-title>
        <v-card-text>
          <div class="block select wrap" style="max-height: 60vh; white-space: pre-wrap">{{ appendantMissingText }}</div>
          <!-- 两段文案里都有 `\n`：必须 pre-wrap，否则整个提示挤成一行 -->
          <div class="mt-3" style="white-space: pre-wrap">
            {{ t('appendant-download-hint', { url: APPENDANT_DOWNLOAD_URL, password: APPENDANT_DOWNLOAD_PASSWORD }) }}
          </div>
        </v-card-text>
        <v-card-actions class="justify-end">
          <v-btn variant="text" v-t="'appendant-open-download'" @click="openMissingDownload"></v-btn>
          <v-btn variant="text" v-t="'appendant-open-folder'" @click="openAppendantDir()"></v-btn>
          <v-btn color="btn" class="hover-scale" variant="text" @click="appendantDialog = false" v-t="'confirm'"></v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
  </div>
</template>

<style scoped>

.v-progress-linear,
.v-progress-linear__determinate {
  transition: none;
}

/* 主操作按钮：用主题强调色，不做悬停位移/阴影抬升 */
.gradient-primary {
  background: var(--phi-accent) !important;
  color: var(--phi-label-inverted) !important;
}

/* 整页铺满内容区（外面没有卡片、也没有 pa-8 内边距） */
.render-view {
  position: relative;
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

/* 贴底的固定操作条 */
.render-view__actions {
  display: flex;
  flex: none;
  align-items: center;
  gap: var(--phi-space-2);
  padding: var(--phi-space-2) var(--phi-space-6);
  border-top: 0.5px solid var(--phi-border-l3);
}

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
