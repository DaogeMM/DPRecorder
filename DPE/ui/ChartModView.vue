<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, toRefs, watch } from 'vue';

import { useI18n } from 'vue-i18n';
const { t } = useI18n();

import { StorageSerializers, useStorage } from '@vueuse/core';

import { invoke } from '@tauri-apps/api/core';
import { useRouter } from 'vue-router';
import { RULES, toast, toastError } from '@/common';
import { DEFAULT_RENDER_CONFIG, type RenderChart } from '@/model';
import { useChartModOrder } from '@/config/chartModOrder';
import { chainLead } from '@/config/chartSide';
import { sameRenderConfig, useRenderConfig } from '@/config/useRenderConfig';
import type { WorkingState } from '@/config/types';
import { finishPageLoad } from '@/pageLoad';

import CfgField from '@/components/CfgField.vue';
import CfgSwitch from '@/components/CfgSwitch.vue';
import PhiGroup from '@/components/PhiGroup.vue';
import PhiSection from '@/components/PhiSection.vue';
import PhiSubMenu from '@/components/PhiSubMenu.vue';

/**
 * DPE 的「谱面修改」页。
 *
 * 这些功能不改谱面文件，只改内存里刚解析出来的判定线与按键（dps 里的入口是 `core::chart_mod`
 * 与 `Chart::apply_chart_mods`），所以预览、渲染、分屏走的是同一份转换。
 *
 * 控件原来长在渲染页的「谱面修改」分区里，搬过来之后**字段仍然属于渲染配置**：配置只有一份
 * （`RenderConfig`，落盘在 `config.toml` / 预设 / 本页读写的那份工作配置里），本页只是换了个
 * 入口去改它。所以这里：
 *   - 自己开一份 `useRenderConfig()`（它不是单例，每次调用都是独立的一组 ref），
 *     进来时从渲染页那份工作配置 `RenderView.Config` 套用，改完按同样的形状写回去；
 *   - 写回时把 `dirty` 置真：渲染页那边会把预设名显示成 `*原名`，跟用户在渲染页手改的效果一致。
 *
 * 顺序表（`chartModOrder`）的规则在 `./config/chartModOrder`，两页共用同一份实现。
 */
const { ui, buildConfig, applyConfig } = useRenderConfig();

/** `toRefs(ui)` 出来的那份 ref 表：模板用命名绑定，`slot()` 按下标取同一批 ref */
const uiRefs = toRefs(ui);

const {
  pureConfigChart, pureConfigChartSideLines, pureConfigChartNoise, pureConfigChartLineY,
  pureConfigChartSpeed, pureConfigChartSideX, pureConfigChartAngle, pureConfigChartNoiseHeight,
  pureConfigChartNoiseWidth, pureConfigChartNoiseTime, pureConfigChartNoiseDelay,
  chartModHide, chartModArrange, chartModArrangeGap, chartModArrangeLineY, chartModArrangeSpeed,
  chartModReplace, chartModReplaceTap, chartModReplaceDrag, chartModReplaceHold, chartModReplaceFlick,
  chartModRandom, chartModRandomSpeed, chartModRandomSpeedMin, chartModRandomSpeedMax,
  chartModRandomPos, chartModRandomPosRange, chartModSpreadHold, chartModSpreadHoldNoteType,
  chartModSpreadHoldInterval, chartModExtendHold, chartModExtendHoldBlank, chartModCopy,
  chartModCopyTimes, chartModCopyOffset, chartModOrder,
  // 互斥那一类里还有「定轨谱面」与「固定流速」，它们的开关在渲染页隐藏的「测试」分区里，
  // 本页不画它们，但互斥判断要用到，所以照样取出来喂给下面的顺序表
  fixedLaneChart, chartModFixedSpeed,
} = uiRefs;

/**
 * 「谱面融合」：把勾中的同一首歌的几个难度拼成一张谱面。
 *
 * 它不是渲染配置里的一项（dps 没有对应的开关，融合发生在生成这一步），所以开关只是本页的
 * 局部状态、不随预设走；但互斥关系照样生效，规则与另外几类一样交给 `config/chartModOrder`。
 * 定义在 `useChartModOrder` 之前：互斥表要在建表时就拿到这个 ref。
 */
const chartModMerge = ref(false);

const { chartModIndex, syncChartModOrder } = useChartModOrder({
  chartModOrder,
  pureConfigChart,
  fixedLaneChart,
  chartModHide,
  chartModArrange,
  chartModFixedSpeed,
  chartModMerge,
  chartModReplace,
  chartModRandom,
  chartModRandomSpeed,
  chartModRandomPos,
  chartModSpreadHold,
  chartModExtendHold,
  chartModCopy,
});

/** 渲染页那份工作配置：本页读它、也写回它（两页因此始终是同一份配置） */
const workingConfig = useStorage<WorkingState | null>('RenderView.Config', null, localStorage, {
  serializer: StorageSerializers.object,
});

applyConfig(workingConfig.value?.config ?? DEFAULT_RENDER_CONFIG);
syncChartModOrder();

/** 上一次写回的那份，用来判断「这次真的改了没有」——没改就不该把预设名标成改过 */
let baseline = buildConfig();

let settleTimer: number | undefined;
function settleChanges() {
  settleTimer = undefined;
  const config = buildConfig();
  if (sameRenderConfig(config, baseline)) return;
  baseline = config;
  const previous = workingConfig.value;
  workingConfig.value = {
    config,
    presetKey: previous?.presetKey ?? '',
    presetName: previous?.presetName ?? '',
    dirty: true,
  };
}

watch(ui, () => {
  clearTimeout(settleTimer);
  settleTimer = setTimeout(settleChanges, 400);
}, { deep: true });

// 切页时把还没到点的那一批补发出去（防抖 400ms，调完就切页会一次都没发）
onBeforeUnmount(() => {
  if (settleTimer === undefined) return;
  clearTimeout(settleTimer);
  settleChanges();
});

// 页内没有需要等的异步初始化，挂载即可揭开内容区那层转圈
finishPageLoad();

// ---------- 文件栏与输出 ----------
/** 取界面槽位的当前值：`toRefs(ui)` 的索引签名让每个 ref 都可能是 undefined，
 *  这里统一收成 `unknown`，写进 config.toml 时也不做类型转换。 */
const slot = (name: string) => (uiRefs as Record<string, { value?: unknown } | undefined>)[name]?.value;

// 生成的动作是：复制源谱面包，把这次的修改写成它自己的 `config.toml`，落到
// `<程序目录>/dpe/modify`。渲染页导入那张谱面时会把配置合并进工作配置，于是修改跟着
// 谱面走 —— 谱面文件里的判定线与按键一个字节都没动，所以不需要引擎具备谱面写回能力。

/** 源谱面：谱面列表里勾上的**全部**谱面 —— 勾几张就一次生成几张，顺序跟列表一致；本页不再单独挑文件。 */
const chartList = useStorage<RenderChart[]>('BatchView.ChartList@dpe', []);
const selectedPaths = computed(() =>
  chartList.value.filter((chart) => chart.isSelect).map((chart) => chart.path),
);

/** 输出目录（固定 `<程序目录>/dpe/modify`）与已有的产物 */
const router = useRouter();

const outputDir = ref('');
const outputs = ref<{ name: string; path: string; modified: number }[]>([]);

async function refreshOutputs() {
  try {
    outputDir.value = (await invoke('get_dpe_modify_dir_path')) as string;
    outputs.value = (await invoke('tool_video_outputs', {
      dir: outputDir.value,
    })) as { name: string; path: string; modified: number }[];
  } catch (error) {
    toastError(error);
  }
}

/** 预览：直接起预览窗口（`dpe.exe --input <谱面>`），不绕去渲染页。 */
function openOutput(row: { path: string }) {
  // 把界面上的这份渲染配置一起给过去，预览窗口的画面才跟 DPR 自己的预览一致
  const config = workingConfig.value?.config ?? DEFAULT_RENDER_CONFIG;
  invoke('dpe_preview', { path: row.path, config }).catch(toastError);
}

function openOutputDir() {
  invoke('open_dir', { path: outputDir.value }).catch(toastError);
}

async function removeOutput(path: string) {
  try {
    await invoke('delete_file', { path });
    await refreshOutputs();
  } catch (error) {
    toastError(error);
  }
}

/** 一类修改：`flag` 是它的开关，`entries` 是它要写进 config.toml 的键 */
interface ModSpec {
  key: string;
  flag: string;
  extra: () => Record<string, unknown>;
}

const MOD_SPECS: ModSpec[] = [
  { key: 'hide', flag: 'chartModHide', extra: () => ({}) },
  {
    key: 'arrange',
    flag: 'chartModArrange',
    extra: () => ({
      chartModArrangeGap: slot('chartModArrangeGap'),
      chartModArrangeLineY: slot('chartModArrangeLineY'),
      chartModArrangeSpeed: slot('chartModArrangeSpeed'),
    }),
  },
  {
    key: 'replace',
    flag: 'chartModReplace',
    extra: () => ({
      chartModReplaceTap: slot('chartModReplaceTap'),
      chartModReplaceDrag: slot('chartModReplaceDrag'),
      chartModReplaceHold: slot('chartModReplaceHold'),
      chartModReplaceFlick: slot('chartModReplaceFlick'),
    }),
  },
  { key: 'random', flag: 'chartModRandom', extra: () => ({}) },
  {
    key: 'random_speed',
    flag: 'chartModRandomSpeed',
    extra: () => ({
      chartModRandomSpeedMin: slot('chartModRandomSpeedMin'),
      chartModRandomSpeedMax: slot('chartModRandomSpeedMax'),
    }),
  },
  {
    key: 'random_pos',
    flag: 'chartModRandomPos',
    extra: () => ({ chartModRandomPosRange: slot('chartModRandomPosRange') }),
  },
  {
    key: 'spread',
    flag: 'chartModSpreadHold',
    extra: () => ({
      chartModSpreadHoldNoteType: slot('chartModSpreadHoldNoteType'),
      chartModSpreadHoldInterval: slot('chartModSpreadHoldInterval'),
    }),
  },
  { key: 'extend', flag: 'chartModExtendHold', extra: () => ({ chartModExtendHoldBlank: slot('chartModExtendHoldBlank') }) },
  {
    key: 'copy',
    flag: 'chartModCopy',
    extra: () => ({ chartModCopyTimes: slot('chartModCopyTimes'), chartModCopyOffset: slot('chartModCopyOffset') }),
  },
];

const MOD_LABELS: Record<string, string> = {
  hide: 'chart-mod-hide',
  arrange: 'chart-mod-arrange',
  replace: 'chart-mod-replace',
  random: 'chart-mod-random',
  random_speed: 'chart-mod-random-speed',
  random_pos: 'chart-mod-random-pos',
  spread: 'chart-mod-spread-hold',
  extend: 'chart-mod-extend-hold',
  copy: 'chart-mod-copy',
};

/** 当前生效的修改，按顺序表排 —— 顺序表决定链式转换的先后，所以文件名也照它拼 */
const activeKeys = computed<string[]>(() => {
  const enabled = MOD_SPECS.filter((spec) => slot(spec.flag) === true).map((spec) => spec.key);
  const order = ((slot('chartModOrder') as string[] | undefined) ?? []).filter((key) => enabled.includes(key));
  return [...order, ...enabled.filter((key) => !order.includes(key))];
});

const isPureConfig = computed(() => slot('pureConfigChart') === true);

/** 生效的修改名，按落地顺序 —— 文件名后缀与包里的难度文本都照它拼 */
const modLabels = computed<string[]>(() => {
  if (isPureConfig.value) return [t('pure-config-chart')];
  if (chartModMerge.value) return [t('chart-mod-merge')];
  return activeKeys.value.map((key) => t(MOD_LABELS[key] ?? key));
});

/** 勾选谱面的难度文本里原有的那一环 `（内容）`：生成时它排在链首，文件名照它接下去。
 *  勾了多张时各谱面未必一样，就不显示（实际文件名以生成为准）。 */
const sourceLead = computed(() => {
  const picked = chartList.value.filter((chart) => chart.isSelect);
  return picked.length === 1 ? chainLead(picked[0]?.chartInfo.difficulty) : null;
});

/** 输出文件名里的修改标记：`(随机流速→随机按键→随机位置)`，难度文本由后端按同一份名字拼 */
const generateSuffix = computed(() => {
  const parts = [...(sourceLead.value ? [sourceLead.value] : []), ...modLabels.value];
  // 链首本身可能带空格（`随机流速 → 随机按键`），文件名里统一成不带空格的箭头
  return `(${parts.map((part) => part.split(' → ').join('→')).join('→')})`;
});

const canGenerate = computed(
  () =>
    selectedPaths.value.length > 0 &&
    (isPureConfig.value || chartModMerge.value || activeKeys.value.length > 0),
);

const generating = ref(false);

/**
 * 「谱面融合」：判据与批量页的融合一致 —— 至少两张、曲名与音乐文件都对得上。
 *
 * 这几句先在前端判一遍只为能说清是哪两张对不上（后端 `dpe_merge_charts` 还会再判一次）。
 */
async function generateMerged() {
  const picked = chartList.value.filter((chart) => chart.isSelect);
  if (picked.length < 2) {
    toastError(t('merge-need-two'));
    return;
  }
  const base = picked[0];
  if (!base) return;
  const sameSong = (chart: RenderChart) =>
    (chart.chartInfo.name ?? '').trim() === (base.chartInfo.name ?? '').trim() &&
    (chart.chartInfo.music ?? '').trim() === (base.chartInfo.music ?? '').trim();
  if (!picked.every(sameSong)) {
    toastError(t('merge-not-same-song'));
    return;
  }

  generating.value = true;
  try {
    const made = (await invoke('dpe_merge_charts', {
      paths: picked.map((chart) => chart.path),
    })) as string;
    await refreshOutputs();
    toast(t('generate-done', { name: made.split(/[\\/]/).pop() ?? '' }), 'success');
  } catch (error) {
    toastError(error);
  } finally {
    generating.value = false;
  }
}

async function generate() {
  if (!canGenerate.value) {
    toast(t('generate-need'), 'warning');
    return;
  }
  // 谱面融合是另一条路：先把勾中的谱面拼成一张，再写进本页的输出目录（不走逐张改的那条）
  if (chartModMerge.value) {
    await generateMerged();
    return;
  }
  const entries: Record<string, unknown> = {};
  if (isPureConfig.value) {
    Object.assign(entries, {
      pureConfigChart: true,
      pureConfigChartSideLines: slot('pureConfigChartSideLines'),
      pureConfigChartSideX: slot('pureConfigChartSideX'),
      pureConfigChartAngle: slot('pureConfigChartAngle'),
      pureConfigChartNoise: slot('pureConfigChartNoise'),
      pureConfigChartNoiseHeight: slot('pureConfigChartNoiseHeight'),
      pureConfigChartNoiseWidth: slot('pureConfigChartNoiseWidth'),
      pureConfigChartNoiseTime: slot('pureConfigChartNoiseTime'),
      pureConfigChartNoiseDelay: slot('pureConfigChartNoiseDelay'),
      pureConfigChartLineY: slot('pureConfigChartLineY'),
      pureConfigChartSpeed: slot('pureConfigChartSpeed'),
    });
  } else {
    for (const spec of MOD_SPECS) {
      if (!activeKeys.value.includes(spec.key)) continue;
      entries[spec.flag] = true;
      Object.assign(entries, spec.extra());
    }
    entries.chartModOrder = activeKeys.value;
  }

  // 逐张生成：每张各自复制源包、写自己那份 `config.toml`，一张失败不打断后面几张。
  generating.value = true;
  const made: string[] = [];
  let firstError: unknown = null;
  try {
    for (const source of selectedPaths.value) {
      try {
        made.push(
          (await invoke('dpe_generate_modified', {
            source,
            labels: modLabels.value,
            entries,
          })) as string,
        );
      } catch (error) {
        firstError ??= error;
      }
    }
    await refreshOutputs();
  } catch (error) {
    firstError ??= error;
  } finally {
    generating.value = false;
  }
  if (made.length > 1) {
    toast(t('generate-done-many', { n: made.length }), 'success');
  } else if (made.length === 1) {
    toast(t('generate-done', { name: made[0]?.split(/[\\/]/).pop() ?? '' }), 'success');
  }
  if (firstError) toastError(firstError);
}

onMounted(async () => {
  await refreshOutputs();
});

</script>

<template>
  <div class="render-view">
    <div class="render-view__scroll scroll-mask config-scroll">
      <div class="config-root">
        <PhiGroup id="grp-chart" :title="t('title.chart')">
      <!-- ===== 谱面修改 =====
           这些功能同属一层：谱面文件不动，只改内存里刚解析出来的判定线与按键，所以预览、渲染、分屏
           走的是同一份转换，dps 里的入口只有一个（`Chart::apply_chart_mods`）。
           **七类互斥**：纯配置谱面 / 定轨谱面 / 隐藏按键 / 陈列按键 / 固定流速 / 谱面融合 / 按键那一类，
           同一时刻只有一类生效 —— 勾上哪一类，另外几类整个关掉（见 `config/chartModOrder`）。这几类各自
           都是把整张谱面重排一遍（或整体改可见性），叠在一起只会互相覆盖，所以不给选。
           按键那一类（按键替换、随机、随机位置、长条填充、长条延长、复制音符）内部**可以同时开**，
           落地顺序就是界面上勾选的先后：勾上的那一项追加到顺序表末尾，齿轮左边那个数字就是它在这一次
           转换里的次序。配置里记着这张表（`chartModOrder`），dps 按同一个次序逐个落地 —— 先开哪个，
           后面的功能就在它改完的谱面上再改一遍。载入预设 / 手改 config.toml 里没这张表时按下面这个行序补齐。
           只有按键那一类有齿轮序号；纯配置谱面、隐藏按键、陈列按键都是单独一步。每一行左边 ✅/❌ 是
           这一项的开关、齿轮里是它的参数。 -->
      <PhiSection id="sec-chart-mod" :title="t('title.chart-mod')">
        <div class="cfg-grid">
          <!-- 「纯配置谱面」：把判定线的移动 / 旋转 / 消失 / 速度拍平成常量，只保留每个按键在屏幕上的
               横向位置；判定时刻接近竖直的按键另分到左右两条竖判定线上；判定时刻落在噪域里的按键
               在它周围长出一块噪域矩形。开关与它的参数（判定线高度 / 按键流速 / 竖判定线的两个参数 /
               按键噪域与它的四个参数：矩形高度 / 矩形宽度 / 时间窗 / 延后视窗）同处这一个齿轮弹窗里。 -->
          <PhiSubMenu id="sec3-pure-config" :label="t('pure-config-chart')" :tip="t('pure-config-chart-tip')" v-model="pureConfigChart">
            <div class="cfg-grid">
              <CfgSwitch :label="t('pure-config-chart-side-lines')" v-model="pureConfigChartSideLines" :tip="t('pure-config-chart-side-lines-tip')" />
              <CfgSwitch :label="t('pure-config-chart-noise')" v-model="pureConfigChartNoise" :tip="t('pure-config-chart-noise-tip')" />
              <CfgField :label="t('pure-config-chart-line-y')" :tip="t('pure-config-chart-line-y-tip')"><v-text-field density="compact" hide-details="auto" v-model="pureConfigChartLineY" type="number" :rules="[RULES.notEmpty, RULES.alpha01]"></v-text-field></CfgField>
              <CfgField :label="t('pure-config-chart-speed')" :tip="t('pure-config-chart-speed-tip')"><v-text-field density="compact" hide-details="auto" v-model="pureConfigChartSpeed" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('pure-config-chart-side-x')" :tip="t('pure-config-chart-side-x-tip')"><v-text-field density="compact" hide-details="auto" :disabled="!pureConfigChartSideLines" v-model="pureConfigChartSideX" type="number" :rules="[RULES.notEmpty, RULES.alpha01]"></v-text-field></CfgField>
              <CfgField :label="t('pure-config-chart-angle')" :tip="t('pure-config-chart-angle-tip')"><v-text-field density="compact" hide-details="auto" :disabled="!pureConfigChartSideLines" v-model="pureConfigChartAngle" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero, RULES.less100]"></v-text-field></CfgField>
              <CfgField :label="t('pure-config-chart-noise-height')" :tip="t('pure-config-chart-noise-height-tip')"><v-text-field density="compact" hide-details="auto" :disabled="!pureConfigChartNoise" v-model="pureConfigChartNoiseHeight" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('pure-config-chart-noise-width')" :tip="t('pure-config-chart-noise-width-tip')"><v-text-field density="compact" hide-details="auto" :disabled="!pureConfigChartNoise" v-model="pureConfigChartNoiseWidth" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('pure-config-chart-noise-time')" :tip="t('pure-config-chart-noise-time-tip')"><v-text-field density="compact" hide-details="auto" :disabled="!pureConfigChartNoise" v-model="pureConfigChartNoiseTime" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('pure-config-chart-noise-delay')" :tip="t('pure-config-chart-noise-delay-tip')"><v-text-field density="compact" hide-details="auto" :disabled="!pureConfigChartNoise" v-model="pureConfigChartNoiseDelay" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
            </div>
          </PhiSubMenu>
          <!-- 「隐藏按键」：按键整颗不画出来，判定照常按时间那条链路走（不搬 modify 的隐藏机制，
               而是在渲染前给按键打一个「不画」的标记）。没有齿轮、也没有序号 —— 它是单独一类，
               与纯配置谱面 / 定轨谱面 / 陈列按键 / 按键那一类互斥。 -->
          <CfgSwitch :label="t('chart-mod-hide')" v-model="chartModHide" :tip="t('chart-mod-hide-tip')" />
          <!-- 「陈列按键」会把整张谱面收到一条基准线上，等于重排一遍，所以它和纯配置谱面 / 定轨
               谱面 / 隐藏按键 / 固定流速一样是单独一类：没有序号，与上面那几类互斥（见本分区开头的注释）。 -->
          <!-- 再往后是按键那六个功能（见 dps 的 `core::chart_mod`），各占一个齿轮弹窗、开关就是齿轮
               左边那个 ✅/❌。它们可以同时开，齿轮左边那个数字是这一次转换里的落地次序（1 = 最先落地），
               顺序就是勾选的先后，见本分区开头的注释。 -->
          <PhiSubMenu id="sec3-chart-mod-arrange" :label="t('chart-mod-arrange')" :tip="t('chart-mod-arrange-tip')" v-model="chartModArrange">
            <div class="cfg-grid">
              <CfgField :label="t('chart-mod-arrange-gap')" :tip="t('chart-mod-arrange-gap-tip')"><v-text-field density="compact" hide-details="auto" v-model="chartModArrangeGap" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('chart-mod-arrange-line-y')" :tip="t('chart-mod-arrange-line-y-tip')"><v-text-field density="compact" hide-details="auto" v-model="chartModArrangeLineY" type="number" :rules="[RULES.notEmpty, RULES.alpha01]"></v-text-field></CfgField>
              <CfgField :label="t('chart-mod-arrange-speed')" :tip="t('chart-mod-arrange-speed-tip')"><v-text-field density="compact" hide-details="auto" v-model="chartModArrangeSpeed" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
            </div>
          </PhiSubMenu>
          <!-- 「谱面融合」：把勾中的同一首歌的几个难度拼成一张谱面（判据与批量页的融合一致：
               至少两张、曲名与音乐文件都对得上、格式相同），产出照样落在本页的输出表里。
               整张拼一张，所以它和上面几类、下面按键那一类互斥；它不改 dps 的配置，
               没有齿轮、没有序号，也不进 `chartModOrder`。 -->
          <CfgSwitch :label="t('chart-mod-merge')" :tip="t('chart-mod-merge-tip')" v-model="chartModMerge" />
          <!-- 「按键替换」是四组映射：每种源键型各一个下拉框，选成它自己就等于不动。 -->
          <PhiSubMenu id="sec3-chart-mod-replace" :label="t('chart-mod-replace')" :tip="t('chart-mod-replace-tip')" :badge="chartModIndex('replace')" v-model="chartModReplace">
            <div class="cfg-grid">
              <CfgField :label="t('note-types').split(',')[0]" :tip="t('chart-mod-replace-row-tip')"><v-autocomplete density="compact" hide-details="auto" :items="t('note-types').split(',')" :rules="[RULES.notEmpty]" v-model="chartModReplaceTap"></v-autocomplete></CfgField>
              <CfgField :label="t('note-types').split(',')[1]" :tip="t('chart-mod-replace-row-tip')"><v-autocomplete density="compact" hide-details="auto" :items="t('note-types').split(',')" :rules="[RULES.notEmpty]" v-model="chartModReplaceDrag"></v-autocomplete></CfgField>
              <CfgField :label="t('note-types').split(',')[2]" :tip="t('chart-mod-replace-row-tip')"><v-autocomplete density="compact" hide-details="auto" :items="t('note-types').split(',')" :rules="[RULES.notEmpty]" v-model="chartModReplaceHold"></v-autocomplete></CfgField>
              <CfgField :label="t('note-types').split(',')[3]" :tip="t('chart-mod-replace-row-tip')"><v-autocomplete density="compact" hide-details="auto" :items="t('note-types').split(',')" :rules="[RULES.notEmpty]" v-model="chartModReplaceFlick"></v-autocomplete></CfgField>
            </div>
          </PhiSubMenu>
          <PhiSubMenu id="sec3-chart-mod-random" :label="t('chart-mod-random')" :tip="t('chart-mod-random-tip')" :badge="chartModIndex('random')" v-model="chartModRandom"></PhiSubMenu>
          <PhiSubMenu id="sec3-chart-mod-random-speed" :label="t('chart-mod-random-speed')" :tip="t('chart-mod-random-speed-tip')" :badge="chartModIndex('random_speed')" v-model="chartModRandomSpeed">
            <div class="cfg-grid">
              <CfgField :label="t('chart-mod-random-speed-min')" :tip="t('chart-mod-random-speed-min-tip')"><v-text-field density="compact" hide-details="auto" v-model="chartModRandomSpeedMin" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('chart-mod-random-speed-max')" :tip="t('chart-mod-random-speed-max-tip')"><v-text-field density="compact" hide-details="auto" v-model="chartModRandomSpeedMax" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
            </div>
          </PhiSubMenu>
          <PhiSubMenu id="sec3-chart-mod-random-pos" :label="t('chart-mod-random-pos')" :tip="t('chart-mod-random-pos-tip')" :badge="chartModIndex('random_pos')" v-model="chartModRandomPos">
            <div class="cfg-grid">
              <CfgField :label="t('chart-mod-random-pos-range')" :tip="t('chart-mod-random-pos-range-tip')"><v-text-field density="compact" hide-details="auto" v-model="chartModRandomPosRange" type="number" :rules="[RULES.notEmpty, RULES.alpha01]"></v-text-field></CfgField>
            </div>
          </PhiSubMenu>
          <PhiSubMenu id="sec3-chart-mod-spread-hold" :label="t('chart-mod-spread-hold')" :tip="t('chart-mod-spread-hold-tip')" :badge="chartModIndex('spread')" v-model="chartModSpreadHold">
            <div class="cfg-grid">
              <CfgField :label="t('chart-mod-spread-hold-note-type')" :tip="t('chart-mod-spread-hold-note-type-tip')"><v-autocomplete density="compact" hide-details="auto" :items="t('note-types').split(',')" :rules="[RULES.notEmpty]" v-model="chartModSpreadHoldNoteType"></v-autocomplete></CfgField>
              <CfgField :label="t('chart-mod-spread-hold-interval')" :tip="t('chart-mod-spread-hold-interval-tip')"><v-text-field density="compact" hide-details="auto" v-model="chartModSpreadHoldInterval" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
            </div>
          </PhiSubMenu>
          <PhiSubMenu id="sec3-chart-mod-extend-hold" :label="t('chart-mod-extend-hold')" :tip="t('chart-mod-extend-hold-tip')" :badge="chartModIndex('extend')" v-model="chartModExtendHold">
            <div class="cfg-grid">
              <CfgField :label="t('chart-mod-extend-hold-blank')" :tip="t('chart-mod-extend-hold-blank-tip')"><v-text-field density="compact" hide-details="auto" v-model="chartModExtendHoldBlank" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
            </div>
          </PhiSubMenu>
          <PhiSubMenu id="sec3-chart-mod-copy" :label="t('chart-mod-copy')" :tip="t('chart-mod-copy-tip')" :badge="chartModIndex('copy')" v-model="chartModCopy">
            <div class="cfg-grid">
              <CfgField :label="t('chart-mod-copy-times')" :tip="t('chart-mod-copy-times-tip')"><v-text-field density="compact" hide-details="auto" v-model="chartModCopyTimes" type="number" :rules="[RULES.notEmpty, RULES.int, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('chart-mod-copy-offset')" :tip="t('chart-mod-copy-offset-tip')"><v-text-field density="compact" hide-details="auto" :disabled="!chartModCopy" v-model="chartModCopyOffset" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
            </div>
          </PhiSubMenu>
        </div>
      </PhiSection>
        </PhiGroup>

        <!-- 输出：`<程序目录>/dpe/modify` 里已经生成过的谱面，格式与噪域播放器那张表一致。
             列表空的时候什么都不画（与噪域播放器一致），只留上面那行标题 —— 不写「还没有生成过」这类
             占位文字：这一页默认就是空的，占位句只会让空页面看起来像出了错。 -->
        <PhiGroup id="grp-dpe-output" :title="t('dpe-output')">
          <v-table v-if="outputs.length" class="dpe-table" density="compact">
            <thead>
              <tr>
                <th class="dpe-col-name">{{ t('dpe-col-name') }}</th>
                <th>{{ t('dpe-col-path') }}</th>
                <th class="dpe-col-act">{{ t('dpe-col-preview') }}</th>
                <th class="dpe-col-act">{{ t('dpe-col-delete') }}</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="row in outputs" :key="row.path">
                <td class="dpe-col-name">
                  <button type="button" class="dpe-link" :title="t('dpe-open-render')" @click="openOutput(row)">
                    {{ row.name }}
                  </button>
                </td>
                <td class="dpe-path" :title="row.path">{{ row.path }}</td>
                <td class="dpe-col-act">
                  <v-btn size="small" variant="text" @click="openOutput(row)">{{ t('dpe-col-preview') }}</v-btn>
                </td>
                <td class="dpe-col-act">
                  <v-btn size="small" variant="text" color="error" @click="removeOutput(row.path)">{{ t('dpe-col-delete') }}</v-btn>
                </td>
              </tr>
            </tbody>
          </v-table>
        </PhiGroup>
      </div>
    </div>

    <!-- 贴底操作条：左边只在这一趟真的能生成时写一句产物名（后缀由生效的修改拼出来）；
         没选谱面 / 一类修改都没开的时候这里留空，不挂那句引导语 —— 引导语改由点「生成谱面」时的
         提示给出（见 `generate()`），默认状态下操作条上不出现任何字。 -->
    <div class="render-view__actions">
      <span v-if="canGenerate" class="dpe-suffix">{{ generateSuffix }}</span>
      <v-spacer></v-spacer>
      <v-btn variant="text" size="small" @click="openOutputDir">{{ t('open-output-folder') }}</v-btn>
      <v-btn variant="tonal" class="gradient-primary" :loading="generating" @click="generate">
        {{ t('generate-chart') }}
      </v-btn>
    </div>
  </div>
</template>

<style scoped>
/* 与「噪域播放器」同一套骨架：滚动区在上、操作条贴底，内容区不设更窄的上限。 */
.render-view {
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
}

.render-view__scroll {
  flex: 1 1 auto;
  min-height: 0;
  overflow: hidden auto;
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

/* 输出列表：表格线只留行间一条细线，列宽按内容走（与噪域播放器那张表同一套写法） */
/* 主操作按钮（生成谱面）：与渲染页那几个按钮同一个强调色。
   这条规则在渲染页 / 批量页各自的 scoped 样式里，这里也留一份，否则按钮是默认的深灰。 */
.gradient-primary {
  background: var(--phi-accent) !important;
  color: var(--phi-label-inverted) !important;
}

.dpe-table {
  width: 100%;
  table-layout: fixed;
  background: transparent;
  font-size: 0.85rem;
}

.dpe-table :deep(th) {
  height: 34px;
  font-size: 0.8rem;
  font-weight: 500;
  color: var(--phi-label-tertiary);
  border-bottom: 0.5px solid var(--phi-border-l3);
}

.dpe-table :deep(td) {
  height: 38px;
  padding-top: 0;
  padding-bottom: 0;
  border-bottom: 0.5px solid var(--phi-border-l3);
}

.dpe-table :deep(tbody tr:hover) {
  background: var(--phi-bg-hover, transparent);
}

/* 文件名列：现在后缀是 `(A→B)` 这种长串，窄了会折成三行，所以按原来 26% 的 1.8 倍放宽 */
.dpe-col-name {
  width: 47%;
}

.dpe-col-act {
  width: 110px;
  text-align: center;
}

.dpe-table :deep(.dpe-col-act) {
  text-align: center;
}

.dpe-link {
  padding: 0;
  border: none;
  background: none;
  font: inherit;
  color: var(--phi-label-primary);
  cursor: pointer;
}

.dpe-link:hover {
  color: var(--phi-accent);
}

.dpe-path {
  overflow: hidden;
  font-size: var(--phi-font-size-caption);
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--phi-label-caption);
}

.dpe-suffix {
  overflow: hidden;
  font-size: var(--phi-font-size-caption);
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--phi-label-caption);
}
</style>
