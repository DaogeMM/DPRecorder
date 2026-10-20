<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref, toRefs, watch } from 'vue';
import { useStorage } from '@vueuse/core';

import { useI18n } from 'vue-i18n';
const { t } = useI18n();

// 谱面是否已导入(由 RenderView 传入): 导入后在本页最下面显示"谱面信息"插槽内容
// showDropHint 只有"渲染参数"页会传(它是唯一支持拖动导入谱面的页面), 批量页不传, 因此批量页不会出现拖入提示
defineProps<{ hasChart?: boolean; showDropHint?: boolean }>();

/** 界面里任何一处配置改动都会（防抖后）从这里发出去：
 *  - `change` 只在这一批确实把界面改脏了时发：批量页拿它更新那份临时配置；
 *  - `state` **每批都发**，把「界面上的值 + 下拉框里那个名字 + 星号」一起给出去，
 *    渲染页拿它存工作配置（程序化写入也得存，否则切页回来只有标了星的名字、值还是旧的）。 */
const emit = defineEmits<{
  change: [config: RenderConfig];
  state: [state: WorkingState];
  /** 「总分 / 偏移」被程序化改写（套用预设、读回工作配置、调整偏移算回新值）：
   *  控件长在谱面信息表单里（RenderView 的插槽），那边要靠这个事件把显示同步过去。 */
  chartInfo: [values: { scoreTotal: number; offset: number }];
}>();

import { invoke } from '@tauri-apps/api/core';
import * as dialog from '@tauri-apps/plugin-dialog';

import { VForm } from 'vuetify/components';

import { RULES, toast, anyFilter, toastError } from '../common';
import { isOptionalAppendant } from '../appendant';
import { DEFAULT_RENDER_CONFIG, type Preset, type RenderConfig } from '../model';
import { useRenderConfig } from '../config/useRenderConfig';
import { jumpToFirstError } from '../formError';
import type { WorkingState } from '../config/types';

import CfgField from './CfgField.vue';
import CfgSwitch from './CfgSwitch.vue';
import DebugFieldSettings from './DebugFieldSettings.vue';
import PhiSection from './PhiSection.vue';
import PhiGroup from './PhiGroup.vue';
import PhiSubMenu from './PhiSubMenu.vue';
import WaterScreenSettings from './WaterScreenSettings.vue';

/** 「判定线调试」文本可显示的项与顺序（对应 dps 侧 `core::line` 里拼文本的那段）。 */
const LINE_DEBUG_FIELDS = ['id', 'parent', 'height', 'ulp', 'z', 'aui', 'anchor', 'kind', 'rot'] as const;
/** 「按键调试」文本可显示的项与顺序（对应 dps 侧 `core::note::debug_note_text`）。 */
const NOTE_DEBUG_FIELDS = ['line', 'time', 'height', 'base', 'angle', 'speed', 'above', 'fake'] as const;

// ===== 「渲染参数」页 =====
// 本文件只负责**排版**：分组、控件摆放、插槽、预设的增删改查。
// 每个参数的知识（默认值、界面 ↔ 配置的换算、联动、校验）都在 ../config/fields.ts 的规格表里，
// 界面状态与 buildConfig() / applyConfig() 由 ../config/useRenderConfig.ts 从那份表生成。
//
// 页面结构：一页到底，没有「普通 / 高级」页签，也没有「基础 / 进阶 / 专家」界面等级 ——
// 所有窗口与选项一直可见（窗口本身仍可折叠）。
//
// ---------- 新增 / 修改一个渲染参数时要动的地方 ----------
// 1. ../model 的 RenderConfig 与 DEFAULT_RENDER_CONFIG（必须和 src-tauri/src/render.rs 的
//    impl Default 一致）。**只加接口字段会被编译拦住**：../config/fields.ts 末尾有一条
//    「规格表必须覆盖 RenderConfig 的每一个键」的编译期断言，会直接点名缺哪个键。
// 2. ../config/fields.ts 里按分区加一条 FieldSpec（widget / label / tip / rules / enabled /
//    toConfig / toUi ...）。换算不常规的才要写 toConfig / toUi，其余按 widget 自动推。
// 3. 模板：在对应分区的 `<div class="cfg-grid">` 里加控件 —— 一行三格，每个控件一格：
//    输入类用 `<CfgField :label="t('xxx')">`，开关类用 `<CfgSwitch :label v-model>`（长文本才加 full）。
//    三栏 / 栏间竖线 / 方形外框 / 白字全部由 ../design/config.css 的 .cfg-* 提供，模板里不要再写内联样式。
//    （三级菜单的齿轮弹窗里是**两栏**，那段覆盖写在 PhiSubMenu.vue 的 scoped 样式里。）
//    控件的 :disabled 要写**整条上级链**（例 `!renderBg || !renderBgDim` —— 上级被禁用后
//    它自己的值仍然留着），并登记到下面那段「功能之间的先后关系」注释里。
// 4. i18n：src/locales/zh-CN/config.json 与 src/locales/en/config.json **都要**加标签键。
// 5. 改完跑 `node node_modules/vue-tsc/bin/vue-tsc.js --noEmit -p tsconfig.app.json --composite false`
//    （本仓库没有测试套件；模板也会一起被检查，所以改完 .vue 必须跑这个）。

const form = ref<VForm>();
/**
 * 「谱面 · 调试」整块显不显示：设置页里那条「显示调试」（`phi.debug.enabled`，纯界面偏好）。
 * 它的入口本身是藏起来的，见 `App.vue` 的 `onSettingsClick`（Ctrl+Shift+点击左栏「设置」）。
 */
const debugEnabled = useStorage<boolean>('phi.debug.enabled', false);
/**
 * 「谱面 · 调试」里那一行「雪降SP」三级菜单的开关。
 * 纯界面状态、不进配置：它只是齿轮弹窗的入口（演出本身由输入谱面的文件名强制开启）。
 */
const xuejiangSpDebug = ref(true);
/** 「谱面 · 调试」里那一行「报应SP」三级菜单的开关（同上，纯界面状态、不进配置）。 */
const baoyingSpDebug = ref(true);
const fileNameFormatDialog = ref(false);
/**
 * 「文件名格式」说明弹窗的宽度 —— 与齿轮弹窗（`PhiSubMenu.vue` 的 `dialogWidth`）同一算法：
 * **内容区 `<main class="phi-center">` 的 60%**。弹窗被 teleport 到 body，写百分比会按视口算
 * （把左侧导航也算进去），所以打开时量一次；下限 480px 免得窄窗口挤成一条。
 * 该弹窗与齿轮配置页共用一套样式。
 */
const fileNameFormatDialogWidth = ref(880);
function openFileNameFormatDialog() {
  const content = document.querySelector<HTMLElement>('.phi-center');
  const base = content?.clientWidth || window.innerWidth;
  fileNameFormatDialogWidth.value = Math.max(480, Math.round(base * 0.6));
  fileNameFormatDialog.value = true;
}

const debugLineDialog = ref(false);
/** 「判定线调试」齿轮弹窗的宽度：同一套算法（内容区的 60%，打开时量一次）。 */
const debugLineDialogWidth = ref(880);
function openDebugLineDialog() {
  const content = document.querySelector<HTMLElement>('.phi-center');
  const base = content?.clientWidth || window.innerWidth;
  debugLineDialogWidth.value = Math.max(480, Math.round(base * 0.6));
  debugLineDialog.value = true;
}

const debugNoteDialog = ref(false);
/** 「按键调试」齿轮弹窗的宽度：同一套算法（内容区的 60%，打开时量一次）。 */
const debugNoteDialogWidth = ref(880);
function openDebugNoteDialog() {
  const content = document.querySelector<HTMLElement>('.phi-center');
  const base = content?.clientWidth || window.innerWidth;
  debugNoteDialogWidth.value = Math.max(480, Math.round(base * 0.6));
  debugNoteDialog.value = true;
}

/**
 * 「新建预设」弹窗的宽度 —— 同一套算法（打开时量一次内容区 `.phi-center` 的宽度，
 * 弹窗被 teleport 到 body，写百分比会按视口算、把左侧导航也算进去），
 * 只是比例换成 **40%**；下限 360px 免得窄窗口挤成一条。
 */
const createDialogWidth = ref(480);
function openCreateDialog() {
  const content = document.querySelector<HTMLElement>('.phi-center');
  const base = content?.clientWidth || window.innerWidth;
  createDialogWidth.value = Math.max(360, Math.round(base * 0.4));
  createDialog.value = true;
}

const {
  ui,
  respacks,
  updateRespacks,
  buildConfig: buildConfigFromFields,
  applyConfig: applyConfigToUi,
  chartInfoBindings,
  bindChartInfoSlot,
  applyAspectRatio,
  applyResolution,
  updateMaxParticles,
  updateBitrate,
  encoderList,
  resolutionList,
  fpsList,
  speedList,
  bitrateList,
  bitrateCrfList,
  antiAliasingList,
  maxParticlesTextList,
  resolutionRule,
  isCrf,
  isBitrate,
} = useRenderConfig();

// 界面状态摊回裸名字，模板的写法与搬走之前完全一致。
// 名单 = 模板用到的那些键 + 几个界面独占的槽位（hwAccel / maxParticlesText /
// judgeRangeTransition / respack / encoder / judgeMode —— 它们不是 RenderConfig 的键）。
const {
  resolution, endingLength, renderLoading, hires, chartDebugLine, chartDebugLineFields, chartDebugNote, chartDebugNoteFields, chartRatio,
  fps, dynamicBitrateControl, bitrate, aggressiveChart, aggressiveNote,
  aggressiveParticle, challengeColor, challengeRank, noteScale, particle, particleSquare,
  playerAvatar, playerName, playerRks, sampleCount, speed, volumeMusic, volumeSfx, watermark,
  notecolor, hitcolor, linecolor, roman, chinese, combo, difficulty, judgeOffset, forceLimit,
  limitThreshold,
  loudnessEqualization, audioMixOptimization, fileNameFormat, renderLine, renderLineExtra,
  renderNote, renderDoubleHint, renderPendingNote, pendingNoteColor, renderUiPause, renderUiName, renderUiLevel, renderUiDifficulty,
  renderUiScore, renderUiCombo, renderUiBar, renderBg, renderBgDim, backgroundDim,
  // 谱面侧渲染参数：它们与谱面信息（ChartInfo）同名，`RenderView.buildParams()` 会并进 info，
  // **配置值优先** ⇒ 导入谱面不再覆盖这些设置（详见下面对「宽 : 高」与「偏移」的说明）
  forceAspectRatio, foldAnimation, pureConfigChart, pureConfigChartLineY, pureConfigChartSpeed,
  pureConfigChartSideLines, pureConfigChartSideX, pureConfigChartAngle, pureConfigChartNoise,
  pureConfigChartNoiseHeight, pureConfigChartNoiseWidth, pureConfigChartNoiseTime, pureConfigChartNoiseDelay,
  fixedLaneChart, fixedLaneGap, fixedLaneSpeed, fixedLaneLineY, fixedLaneInterval,
  fixedLaneHoldLength, fixedLaneThird, playSplitView,
  aspectRatio, noteUniformScale, holdPartialCover,
  negativeLengthHold, holdParticleIntervalRatio, lineLength, previewStart,
  previewEnd,
  renderBlockArea, simplifiedBlockArea, simplifiedBlockAreaText, blockAreaReuse, blockAreaLava,
  waterScreen, waterOpacity, waterFlowSpeed, waterCellScale, waterHp, waterDrownDelay, waterDamage,
  waterDamageInterval, waterDeathTime, waterDeathParticles, waterSfxVolume, waterSfxMax,
  blockAreaForcePosition, blockAreaForceHeight, blockAreaForceWidth, blockAreaForceShow,
  blockAreaForceTime, blockAreaForceDelay, blockAreaForceInvert, blockAreaColor,
  blockAreaNormalColor, simplifiedBlockAreaTextColor, simplifiedBlockAreaTextPaleColor,
  blockAreaAlpha, blockAreaDensity, blockAreaSpark, blockAreaNoiseBlotch, blockAreaDisplaceStrength,
  blockAreaHover, blockAreaHoverRadius,
  blockAreaPixelScale, blockAreaDisplaceSpeed, blockAreaNoiseRadius, blockAreaNoiseSmoothness,
  blockAreaEdgeNoise, directBlockHollow, blockAreaDistort, renderJudgeRange, judgeRangeWidth,
  judgeRangeWidthFlick, judgeRangeNoiseOcclusion,
  judgeRangeLength, judgeRangeTime, judgeRangeAlpha, judgeRangeColor, preserveFramebuffer,
  renderExtra, bgBlurriness, playStartTime, playEndTime, fade, alphaTint,
  // 「雪降SP」特殊演出的调试参数（谱面 → 调试）
  xuejiangSpFakeNotes, xuejiangSpFakeBarSeconds, xuejiangSpBurstCombo, xuejiangSpRollScale,
  xuejiangSpDiffFake, xuejiangSpDiffReal, xuejiangSpVolumePercentile, xuejiangSpVolumeGamma,
  xuejiangSpVolumeSmooth,
  xuejiangSpFaceHitStart, xuejiangSpFacePeriod, xuejiangSpFaceRounds,
  xuejiangSpFaceLeadShow, xuejiangSpFaceLeadRoll,
  xuejiangSpFaceTextShow, xuejiangSpFaceTextRoll, xuejiangSpFaceTextHit,
  baoyingSpWowHeight, baoyingSpWowOffsetX, baoyingSpWowOffsetY,
  baoyingSpTitleHeight, baoyingSpTitleOffsetX, baoyingSpTitleOffsetY,
  baoyingSpScoreOffsetX, baoyingSpScoreOffsetY,
  baoyingSpScoreHeight, baoyingSpScoreGap, baoyingSpScoreSqueeze,
  baoyingSpComboOffsetX, baoyingSpComboOffsetY,
  baoyingSpComboHeight, baoyingSpComboGap, baoyingSpComboSqueeze,
  baoyingSpComboLabelHeight, baoyingSpComboLabelOffsetX, baoyingSpComboLabelOffset,
  baoyingSpDiffHeight, baoyingSpDiffGapSpLv, baoyingSpDiffGapLvQuestion,
  baoyingSpDiffSqueeze, baoyingSpDiffOffsetX, baoyingSpDiffOffsetY, baoyingSpDigitsRtl,
  baoyingSpHitFxScale, baoyingSpHitFxRotate, baoyingSpParticleCount,
  // 界面独占的槽位
  hwAccel, maxParticlesText, judgeRangeTransition, respack, encoder, judgeMode,
} = toRefs(ui);

/** 配置里的 `render_bg_dim` 不由界面勾选，而是**跟着数值派生**：`background_dim > 0` 才压暗，
 *  0 就是不压暗。`immediate: true` 让初次挂载与载入预设时也同步一次（老预设里那个勾选框的值会被覆盖）。 */
watch(
  () => (backgroundDim ? Number(backgroundDim.value) : 0),
  (dim) => { if (renderBgDim) renderBgDim.value = dim > 0; },
  { immediate: true },
);

/* ---------- 「宽 : 高」是配置里的一个比值 ----------
 * 配置（`aspectRatio`）里只存一个数，界面上是并排的两个框，所以这里另存两个**本地** ref
 * （它们不是配置键，只是「16 : 9」这种显示形式），并双向同步：
 *   - `ui.aspectRatio` 变了（载入预设 / 切页回来 / `restoreWorking`）→ 按常见比例表还原成两个框；
 *   - 用户改框 → `commitAspect()` 把比值写回 `ui.aspectRatio`。
 * 比值与当前框相差 1e-6 以内就不动这两个框：否则「17 : 9」这种自定义比例会被格式化改写，
 * 打字打到一半也会被打断。 */
const aspectWidth = ref('16');
const aspectHeight = ref('9');
const ASPECT_PRESETS: [number, number][] = [
  [16, 9], [16, 10], [4, 3], [8, 5], [3, 2], [5, 4], [7, 5], [1, 1],
];
function formatAspect(ratio: number) {
  const w = parseFloat(aspectWidth.value);
  const h = parseFloat(aspectHeight.value);
  if (Number.isFinite(w) && Number.isFinite(h) && h !== 0 && Math.abs(w / h - ratio) < 1e-6) return;
  for (const [pw, ph] of ASPECT_PRESETS) {
    if (Math.abs(pw / ph - ratio) < 1e-4) {
      aspectWidth.value = String(pw);
      aspectHeight.value = String(ph);
      return;
    }
  }
  aspectWidth.value = String(ratio);
  aspectHeight.value = '1';
}
function commitAspect() {
  const w = parseFloat(aspectWidth.value);
  const h = parseFloat(aspectHeight.value);
  if (!Number.isFinite(w) || !Number.isFinite(h) || w <= 0 || h <= 0) return;
  ui.aspectRatio = w / h;
}
watch(() => (aspectRatio ? Number(aspectRatio.value) : 16 / 9), (ratio) => formatAspect(ratio), { immediate: true });

async function chooseAvatar() {
  let file = await dialog.open({
    filters: [
      {
        name: t('image-filter'),
        extensions: ['jpg', 'jpeg', 'png', 'webp', 'bmp'],
      },
      anyFilter(),
    ],
  });
  if (file) {
    ui.playerAvatar = file as string;
  }
}

// 材质包列表要**尽早**拿到：`resPackPath` 的 `toUi` 靠它把配置里的路径换成列表框里的那项，
// 列表还没回来时 `applyConfig` 会跳过材质包那一格（回到「列表一到就顺手改成第一项」的坑）。
refreshRespacks();

// 保存前先把几个「界面独有」的东西换算掉：
//   - 表单校验（不通过就 toast + 中止）
//   - 最大粒子数的自定义输入（不合法同样中止）
//   - 预览片段窗口不超过 15 秒（预览开始 / 结束时间现在是配置项，校验自然也该跟着走）
// 其余全部交给规格表生成的 buildConfigFromFields。
async function buildConfig(): Promise<RenderConfig | null> {
  const { valid, errors } = await form.value!.validate();
  if (!valid) {
    // 渲染参数表很长：不把用户带到出错的那一栏，按钮看起来就像「按了没反应」
    jumpToFirstError(errors);
    toast(t('has-error'), 'error');
    return null;
  }

  if (!updateMaxParticles()) {
    return null;
  }

  // `toRefs(ui)` 的索引签名让这两个名字在类型上可能是 undefined（其余键同理），取值时兜一下底
  const start = parseFloat(String(previewStart?.value ?? '')) || 0;
  const end = parseFloat(String(previewEnd?.value ?? '')) || 0;
  if (end && end - start > 15) {
    toast(t('error.preview-start-end-15s'), 'error');
    return null;
  }

  return buildConfigFromFields();
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
/* ---------------------- 改动通知 + 「未存进预设」的星号 ----------------------
   自动保存那套的一部分：界面里改一下就①通知外面存起来（渲染页存本地工作配置、
   批量页更新临时配置），②把预设下拉框里的名字标成 `*原名`（＝界面上的配置已经和原预设不一样了）。

   算「改过」的只有**用户**改的那些。会写 `ui` 的不只是用户：套用预设、材质包列表刷回来、
   谱面解析完按宽高比改分辨率都是程序化写入（写完叫一声 `markProgrammatic()`），
   所以基准是「上一批程序化写入落定之后构建出来的那份配置」(`baseline`)，而不是预设里存的那份。
   拿预设比不行：预设可能少几个键、键序也不同（后端结构体顺序 vs 字段表顺序），套用时既会按默认值补，
   又会被 `useRenderConfig` 里那些联动改写，于是「刚选中预设、一个字都没改」也会被算成改过。
   比对的是 `buildConfigFromFields()`（还没过表单校验的那份），所以把某个字段改回原值，星号自己就消失。
   防抖 400ms：拖一次滑杆、连点加减号不要每一下都往外发。 */
const dirty = ref(false);
let changeTimer: ReturnType<typeof setTimeout> | undefined;
/** 「没改过」的基准（怎么比见下面的 `canonical()`） */
let baseline = '';
/** 程序化写入 `ui` 的时间窗：窗口内的改动不算用户改的。用时间而不是布尔标志，
 *  是因为写 `ui` 与防抖回调之间隔着 400ms，标志什么时候被消费掉不好说。 */
let programmaticUntil = 0;
/** 上一次程序化写入落定之后构建出来的那份配置：只有「本批构建结果与它一模一样」才当成那次写入的回声 */
let programmaticBuilt = '';
/** 读回工作配置的那一批里，星号由读进来的那份说了算，别被这次写入的回声立刻抹掉 */
let keepDirtyOnProgrammatic = false;

/** 两份配置串起来比 —— 基准与当前值都出自 `buildConfigFromFields()`，键的先后天然一致，
 *  直接 `JSON.stringify` 就够（别用 `JSON.stringify(config, Object.keys(config))`：那种数组形式
 *  是**过滤键**而不是排序，会把 `resolution` / 颜色这些数组里的元素全变成 `null`）。 */
function canonical(config: RenderConfig): string {
  return JSON.stringify(config);
}

/** 程序化写入 `ui` 之后叫一声（套用配置、材质包列表回来、谱面改分辨率、切页回来读工作配置） */
function markProgrammatic() {
  programmaticUntil = Date.now() + 800;
  // 写入之后构建出来的那份才算「程序化写入的结果」（`applyConfig` 是同步写 `ui` 的，等一个 tick 就够）。
  // 光靠时间窗不行：用户在窗口里改的东西会被一起吞掉 —— 不标星、也不往外发。
  void nextTick(() => {
    programmaticBuilt = canonical(buildConfigFromFields());
  });
}

/** 把星号写进下拉框显示的名字里 —— 下拉框显示的是 model 自己的 `item-title`（`Preset.name`） */
function syncPresetLabel() {
  const base = presets.value.find((x) => x.key === preset.value.key);
  if (!base) return;
  const name = (dirty.value ? '*' : '') + base.name;
  if (preset.value.name !== name) preset.value = { ...base, name };
}

/** 当前界面就是 `preset.value` 的内容了：重新落一次基准（新建 / 替换预设之后用） */
function markClean() {
  baseline = canonical(buildConfigFromFields());
  dirty.value = false;
  syncPresetLabel();
}

/** 预设被删掉之后：界面上的值还在，但它已经不属于下拉框里显示的那个预设了 */
function markDirty() {
  dirty.value = true;
  syncPresetLabel();
}

/** 返回这一批改动算不算用户改的（程序化写入不发 `change`，免得外面把联动的结果当成用户改动） */
function updateDirty(): boolean {
  const built = canonical(buildConfigFromFields());
  // 程序化写入的回声：构建结果与那次写入的一模一样才算（用户在窗口里改过就不一样了）
  const echo = Date.now() <= programmaticUntil && built === programmaticBuilt;
  if (echo) {
    baseline = built;
    if (!keepDirtyOnProgrammatic) dirty.value = false;
    syncPresetLabel();
    return false;
  }
  keepDirtyOnProgrammatic = false;
  // 基准还是空的：这一批之前一次程序化写入都没有落定过（典型情况是弹窗刚打开时界面上的值
  // 恰好与预设一致，那次 `applyConfig` 什么也没改、watch 根本没响）。以前这里把这一批当成
  // 「基准」不发 `change`，于是**用户在弹窗里的第一处改动会整个丢掉**（批量页只听 `change`，
  // 那份工作配置就停在旧值上）。现在照旧发出去，只是没有基准可比，先按「改过」算。
  if (!baseline) {
    baseline = built;
    dirty.value = true;
    syncPresetLabel();
    return true;
  }
  dirty.value = built !== baseline;
  syncPresetLabel();
  return true;
}

/** 套用一份配置（下拉框里选中预设）：这一下不是用户改的，星号也要跟着这个预设重新算 */
function applyPreset(config: RenderConfig) {
  markProgrammatic();
  keepDirtyOnProgrammatic = false;
  applyConfigToUi(config);
  // 「总分 / 偏移」的控件在谱面信息表单里（自己一份状态），套用预设时也要跟着这份配置走，
  // 否则预设里存的总分 / 偏移读出来只留在配置里、界面上还是旧值。没谱面时由 RenderView 稍后补写。
  setChartInfoSlots({
    scoreTotal: config.scoreTotal ?? DEFAULT_RENDER_CONFIG.scoreTotal,
    offset: config.offset ?? DEFAULT_RENDER_CONFIG.offset,
  });
}

/** 材质包列表刷新：列表刚到时可能改写界面上的选择，那也不是用户改的 */
async function refreshRespacks() {
  if (await updateRespacks()) markProgrammatic();
}

/** 按配置里的宽高比改分辨率（`RenderView.vue` 解析完谱面后调的）：也不是用户改的 */
function applyAspectRatioFromConfig() {
  markProgrammatic();
  applyAspectRatio(Number(ui.aspectRatio) || 16 / 9);
}

/** 配置侧「总分 / 偏移」的当前值：控件在谱面信息表单里，那边用它回填输入框 */
function chartInfoValues() {
  return { scoreTotal: chartInfoBindings.scoreTotal(), offset: chartInfoBindings.offset() };
}

/** 把「总分 / 偏移」写进配置，并把结果广播给谱面信息表单（程序化改写时用） */
function setChartInfoSlots(values: { scoreTotal?: number; offset?: number }) {
  if (values.scoreTotal !== undefined) bindChartInfoSlot('scoreTotal', values.scoreTotal);
  if (values.offset !== undefined) bindChartInfoSlot('offset', values.offset);
  emit('chartInfo', chartInfoValues());
}

/** `RenderView.vue` 的「调整偏移」拿到后端算出来的秒数后调：写回配置（界面按毫秒编辑） */
function setOffsetSeconds(seconds: number) {
  setChartInfoSlots({ offset: seconds });
}

/** 一批改动落定（防抖到点、或者组件要卸载被提前叫醒）：把工作状态发出去 */
function settleChanges() {
  changeTimer = undefined;
  const built = buildConfigFromFields();
  const changed = updateDirty();
  emit('state', { config: built, presetKey: preset.value.key, presetName: preset.value.name, dirty: dirty.value });
  if (changed) emit('change', built);
}

watch(ui, () => {
  clearTimeout(changeTimer);
  changeTimer = setTimeout(settleChanges, 400);
}, { deep: true });

/* 卸载前把还没到点的那一批补发出去。防抖是 400ms，「调完就切页」时那批改动一次都没发出去，
   而父组件的落盘 watcher（`useStorage`）此刻也已经随作用域停掉了 —— 两边都得补上才不会丢。
   只在真有计时器在等时补发：界面没改过就不该发事件（否则批量页会把「打开弹窗看看、没改」也标成 `*`）。 */
onBeforeUnmount(() => {
  if (changeTimer === undefined) return;
  clearTimeout(changeTimer);
  settleChanges();
});

const presets = ref([DEFAULT_PRESET]);
const preset = ref(DEFAULT_PRESET);
async function updatePresets() {
  presets.value = await getPresets();
  // 列表里找不到当前这个 key（刚读回来的工作配置就属于这种：它可能是个临时名字），
  // 那就留着界面上那个名字 —— 换成列表第一项会把这个名字连星号一起改掉。
  preset.value = presets.value.find((x) => x.key === preset.value.key) || { ...presets.value[0]!, name: preset.value.name };
  syncPresetLabel();
}
updatePresets();

// 「新建预设」的弹窗状态。
// 不用浏览器原生 `prompt(t('preset-create-title'))` —— 在 Tauri 里会弹一个与应用完全两个样子的
// 系统输入框；这里用普通 `v-dialog`：皮肤与「文件名格式」说明弹窗、
// 设置页的重置确认弹窗一致（`.log-card-only-window.phi-config-dialog`），
// 内容 = 标题 + 一个名字输入框 + 取消 / 确定两个按钮。
// 点「创建」时先把配置构建出来（校验不过就不开弹窗），确认时才真正落盘。
const createDialog = ref(false);
const createName = ref('');
/** 「创建」那一刻构建好的配置，等用户在弹窗里敲完名字再落盘 */
let createConfig: RenderConfig | null = null;

/** 父组件把上次离开时的「工作配置」读回界面：值、下拉框里那个名字、星号一起还原。
 *  这一批写入不是用户改的（`markProgrammatic`），但星号要照旧显示（`keepDirtyOnProgrammatic`）；
 *  基准取「回来之后这份」，之后用户再改才重新亮星号。 */
function restoreWorking(state: WorkingState) {
  markProgrammatic();
  applyConfigToUi(state.config);
  setChartInfoSlots({
    scoreTotal: state.config.scoreTotal ?? DEFAULT_RENDER_CONFIG.scoreTotal,
    offset: state.config.offset ?? DEFAULT_RENDER_CONFIG.offset,
  });
  // 名字缺了（旧版本这个键里存的是一份裸配置）就用列表第一项的名字兜底，别让下拉框空着
  preset.value = { key: state.presetKey, name: state.presetName || presets.value[0]?.name || '', config: state.config };
  baseline = canonical(buildConfigFromFields());
  dirty.value = state.dirty;
  keepDirtyOnProgrammatic = true;
  syncPresetLabel();
}

async function openRespackFolder() {
  try {
    await invoke('open_respack_folder');
  } catch (e) {
    toastError(e);
  }
}

/** 点「创建」：先把配置构建出来（表单校验不过就直接中止，不打开弹窗），再问名字 */
async function createPreset() {
  let config = await buildConfig();
  if (!config) return;
  createConfig = config;
  createName.value = '';
  openCreateDialog();
}

/** 弹窗里点「确定」（或名字输入框按回车）：名字定下来了才真正落盘 */
async function confirmCreatePreset() {
  const config = createConfig;
  if (!config) return;
  const name = createName.value.trim();
  if (!name.length) return;
  if (name === 'default' || name === t('default-preset')) {
    toast(t('preset-cannot-use-default'), 'error');
    return;
  }
  try {
    await invoke('add_preset', { name, config });
    await updatePresets();
    preset.value = presets.value.find((x) => x.key === name) || presets.value[0]!;
    markClean();
    toast(t('preset-created'), 'success');
    createDialog.value = false;
    createConfig = null;
  } catch (e) {
    toastError(e);
  }
}
async function deletePreset() {
  try {
    await invoke('remove_preset', { name: preset.value.key });
    await updatePresets();
    markDirty();
    toast(t('preset-deleted'), 'success');
  } catch (e) {
    toastError(e);
  }
}
async function replacePreset() {
  let config = await buildConfig();
  if (!config) return;
  try {
    await invoke('remove_preset', { name: preset.value.key });
    await invoke('add_preset', { name: preset.value.key, config });
    await updatePresets();
    markClean();
    toast(t('preset-replaced'), 'success');
  } catch (e) {
    toastError(e);
  }
}

/**
 * 对外暴露的接口 —— `RenderView.vue` / `BatchView.vue` 用模板 ref 拿到的组件实例上只有这里的成员。
 *
 * ⚠️ `<script setup>` 的组件默认是**封闭**的：**没有 `defineExpose` 就一个成员都拿不到**。
 * 一旦这段暴露丢了，症状是预览直接报错、应用预设静默失效：
 *   - `configView.value!.buildConfig()` → `configView.value.buildConfig is not a function`（预览）
 *   - `configView.value?.applyConfig(config)` → 可选链不报错，于是预设读进界面时什么都没发生
 * 以后从父组件调用 ConfigView 的方法，记得同时在这里登记。
 */
defineExpose({
  buildConfig,
  // 谱面信息表单里的「总分 / 偏移」→ 配置里的同名字段（RenderView 解析完谱面、或表单改动时调）
  setChartInfoSlots,
  chartInfoValues,
  // 切页回来时把工作配置（值 + 名字 + 星号）读回界面
  restoreWorking,
  // 下面几个都包了一层：套用配置、材质包列表回来、谱面改分辨率这些**程序化**写入
  // 也要把星号基准重取一次，否则界面上的值明明没被用户动过，星号却过一会儿自己冒出来
  applyConfig: applyPreset,
  applyAspectRatio: applyAspectRatioFromConfig,
  setOffsetSeconds,
  applyResolution,
  updateMaxParticles,
  updateBitrate,
  updateRespacks: refreshRespacks,
});

</script>


<template>
  <!-- 单一根节点：ConfigView 被 RenderView / BatchView 两处复用，
       属性（含 RenderView 传下来的 --config-scroll-max-height）要能落到根元素再往下继承 -->
  <div class="config-root">
  <v-form ref="form" validateOn="eager" class="scroll-mask config-scroll">
    <!-- ===== 谱面信息（含曲绘横幅）=====
         放在整页最上面，作为「当前谱面」的抬头。
         横幅本体由 RenderView 经 chart-head 插槽传入（./ChartHero.vue），
         下方那张可编辑的表单仍写在 RenderView 里、走 chart-info 插槽。
         未导入谱面时不可折叠：横幅只显示一句拖放提示，下面没有字段。 -->
    <PhiSection v-if="hasChart || showDropHint" id="sec-chart-info" variant="hero" :collapsible="!!hasChart">
      <template #head>
        <slot name="chart-head">{{ t('title.chart-info') }}</slot>
      </template>
      <slot v-if="hasChart" name="chart-info"></slot>
    </PhiSection>

    <!-- ===== 渲染页现在是两级结构 =====
         一级 = PhiGroup（渲染 / 谱面 / 更多），二级 = 组里的 PhiSection。
         App.vue 的左栏导航（renderGroups）与这里的层级一一对应，改一边要同步另一边。
         「预设配置」不是一级分组，而是收在「渲染」组里、放在该组最上面。 -->

    <!-- ===== 渲染 ===== -->
    <PhiGroup id="grp-render" :title="t('title.render-group')">
      <!-- 预设配置：组内第一条，按组内规矩用 PhiSection（DOM id 是 sec-preset，
           App.vue 的 renderGroups 与之同步）。 -->
      <PhiSection id="sec-preset" :title="t('presets')">
        <!-- 按钮改成贴着输入框右边的一排小图标按钮（原来每个都占 1/5 宽的深色大按钮，太重）。
             图标含义写在 title 属性里，鼠标悬停能看到；下拉框自己不带 label，标题已在分区上。 -->
        <div class="preset-row">
          <!-- 内置的「默认」预设（`key === 'default'`）里那个名字显示成灰色：
               自己建的预设纯白，内置那份退一档，跟占位符、禁用项一个灰。
               别删按钮上那两条 `:disabled="preset.key === 'default'"` 也是同一个判断。 -->
          <v-combobox density="compact" hide-details="auto" @update:model-value="(val: Preset) => applyPreset(val.config)" :items="presets"
            item-title="name" v-model="preset" style="flex: 1;" :class="{ 'preset-default': preset.key === 'default' }"></v-combobox>
          <v-btn icon="mdi-refresh" variant="text" size="small" :title="t('preset-refresh')" @click="updatePresets"></v-btn>
          <v-btn icon="mdi-plus" variant="text" size="small" :title="t('preset-create')" @click="createPreset"></v-btn>
          <v-btn icon="mdi-delete-outline" variant="text" size="small" :title="t('preset-delete')" :disabled="preset.key === 'default'"
            @click="deletePreset"></v-btn>
          <v-btn icon="mdi-swap-horizontal" variant="text" size="small" :title="t('preset-replace')" :disabled="preset.key === 'default'"
            @click="replacePreset"></v-btn>

        </div>
      </PhiSection>

      <PhiSection id="sec-respack" :title="t('respack')">
        <div class="preset-row">
          <v-combobox density="compact" hide-details="auto" :items="respacks" item-title="name" v-model="respack" style="flex: 1;"></v-combobox>
          <!-- 选中的包不在 appendant/ 里：标出来，免得点了预览才看到下载提示 -->
          <!-- 可选包（appendant.official）缺了不标：它会自动退回内置默认皮肤 -->
          <v-icon v-if="respack && respack.present === false && !isOptionalAppendant(respack.name)" color="warning" :title="t('respack-missing-tip')">
            mdi-alert-circle-outline
          </v-icon>
          <v-btn icon="mdi-refresh" variant="text" size="small" :title="t('respack-refresh')" @click="refreshRespacks"></v-btn>
          <v-btn icon="mdi-folder-open-outline" variant="text" size="small" :title="t('respack-open')" @click="openRespackFolder"></v-btn>
        </div>
      </PhiSection>

      <!-- ===== 参数 =====
           本分区对应 DOM id `sec-params`。
           同一个分区内按以下次序重排：勾选框在前，文本框在后，
           最后是「文件名格式」与 Tip（Tip 是 RenderView
           插进来的 `#params-tip`）—— 这两项都是整行宽的长文本框，按惯例**单独放第二个
           `.cfg-grid`**（只有 full 条目的网格才不画栏间竖线，见下面那段注释）。
           谱面侧那几项（强制宽高比 / 折叠式入场 / 宽:高 / 预览开始时间 / 预览结束时间）
           也是**配置项**，所以由本文件直接渲染、不走插槽；「总分」「偏移」则搬进了谱面信息
           （见 RenderView 的 chart-info 插槽）。
           「保留帧缓冲（拖影效果）」也搬到了谱面组的「更多」（`sec-more-chart`）。
           这里只剩一个 `.cfg-grid`（同一分区里多个并列网格早已合并成一个）。 -->
      <PhiSection id="sec-params" :title="t('title.params')">
        <div class="cfg-grid">
          <!-- ===== 勾选框在前 ===== -->
          <CfgSwitch :label="t('hw-accel')" v-model="hwAccel" :tip="t('hw-accel-tips')" />
          <!-- 「高性能模式」在设置页（挨着「默认展开侧边栏」）：
               它是 app 配置项、由后端在提交渲染时注入，不是每份渲染配置的开关；
               所以本页不出现第二个「性能」开关。 -->
          <CfgSwitch :label="t('dynamic-bitrate-control')" v-model="dynamicBitrateControl" :tip="t('dynamic-bitrate-control-tip')" />
          <CfgSwitch :label="t('render-loading')" v-model="renderLoading" :tip="t('render-loading-tip')" />
          <!-- 三级菜单：激进优化紧跟渲染管线开关；
               子项收进齿轮弹窗，勾选框只管开关。
               子项里不用再判 `aggressiveChart` —— `useRenderConfig.ts:317-328` 的 watch 会在
               关闸门时把 note / particle 一起置 false。 -->
          <PhiSubMenu id="sec3-aggressive" :label="t('aggressive-chart')" :tip="t('aggressive-chart-tips')" v-model="aggressiveChart">
            <div class="cfg-grid">
              <CfgSwitch :label="t('aggressive-note')" v-model="aggressiveNote" :disabled="!renderNote" :tip="t('aggressive-note-tips')" />
              <CfgSwitch :label="t('aggressive-particle')" v-model="aggressiveParticle" :disabled="!particle && !particleSquare" :tip="t('aggressive-particle-tips')" />
            </div>
          </PhiSubMenu>
          <CfgSwitch :label="t('render-extra')" v-model="renderExtra" :tip="t('render-extra-tips')" />
          <!-- 谱面侧渲染参数：它们是配置项，会进预设、
               进 config.toml，也出现在批量页的预设弹窗里。 -->
          <CfgSwitch :label="t('info.force-aspect-ratio')" v-model="forceAspectRatio" :tip="t('info.force-aspect-ratio-tip')"></CfgSwitch>
          <CfgSwitch :label="t('info.fold-animation')" v-model="foldAnimation" :tip="t('info.fold-animation-tip')"></CfgSwitch>
          <!-- 「纯配置谱面」的三个控件都在谱面组的「更多」（`sec-more-chart`）：
               开关是勾选框，跟在那里的一排勾选框后面；两个数字框按本分区惯例排在文本框一片。 -->
          <!-- ===== 文本框在后 ===== -->
          <!-- 「偏移」与「总分」不在本分区：「渲染参数」的空状态（未导入谱面）里也能改。 -->
          <CfgField :label="t('resolution')" :tip="t('resolution-tip')">
            <v-combobox density="compact" hide-details="auto" :items="resolutionList" :rules="[resolutionRule]" v-model="resolution"></v-combobox>
          </CfgField>
          <CfgField :label="t('fps')" :tip="t('fps-tip')">
            <v-combobox density="compact" hide-details="auto" :items="fpsList" type="number" :rules="[RULES.int, RULES.positive]" v-model="fps"></v-combobox>
          </CfgField>
          <CfgField :label="t('anti-aliasing')" :tip="t('sample-count-tips')">
            <!-- 拖影画在单采样的累积缓冲上，抗锯齿在这种模式下没有意义，直接锁死 -->
            <v-autocomplete density="compact" hide-details="auto" v-model="sampleCount" :items="antiAliasingList" item-title="title" item-value="value" :disabled="preserveFramebuffer"></v-autocomplete>
          </CfgField>
          <CfgField :label="t('encoder')" :tip="t('video-encoder-tip')">
            <v-combobox density="compact" hide-details="auto" v-model="encoder" :items="encoderList"></v-combobox>
          </CfgField>
          <CfgField v-if="encoder !== encoderList[2]" :label="dynamicBitrateControl ? t('bitrate-crf') : t('bitrate')" :tip="dynamicBitrateControl ? t('bitrate-crf-tip') : undefined">
            <v-combobox v-if="dynamicBitrateControl" density="compact" hide-details="auto" :items="bitrateCrfList" type="number" :rules="[isCrf]" v-model="bitrate"></v-combobox>
            <v-combobox v-else density="compact" hide-details="auto" :items="bitrateList" :rules="[isBitrate]" v-model="bitrate"></v-combobox>
          </CfgField>
          <CfgField :label="`${t('width')} : ${t('height')}`">
            <!-- 配置里只存一个比值（`aspectRatio`），这两个框是它的显示形式（见本文件上面的
                 `aspectWidth / aspectHeight` 与 `commitAspect()`）；两个框都改完才写回比值 -->
            <v-text-field type="number" density="compact" hide-details="auto" style="max-width: 96px" :rules="[RULES.positive]" v-model="aspectWidth" @update:modelValue="commitAspect()"></v-text-field>
            <span class="cfg-colon">:</span>
            <v-text-field type="number" density="compact" hide-details="auto" style="max-width: 96px" :rules="[RULES.positive]" v-model="aspectHeight" @update:modelValue="commitAspect()"></v-text-field>
          </CfgField>
          <!-- 这一格是最大粒子数；「偏移」「总分」在谱面信息里（见 RenderView 的 chart-info 插槽）。 -->
          <CfgField :label="t('max-particles')"><v-combobox density="compact" hide-details="auto" :disabled="!particle && !particleSquare" :rules="[RULES.notEmpty]" :title="t('max-particles-tip')" :items="maxParticlesTextList" v-model="maxParticlesText"></v-combobox></CfgField>
          <CfgField :label="t('info.previewStart')" :tip="t('info.previewStart-tip')">
            <v-text-field type="number" density="compact" hide-details="auto" :rules="[RULES.positiveOrZero]" v-model="previewStart"></v-text-field>
          </CfgField>
          <CfgField :label="t('info.previewEnd')" :tip="t('info.previewEnd-tip')">
            <v-text-field type="number" density="compact" hide-details="auto" v-model="previewEnd"></v-text-field>
          </CfgField>
          <CfgField :label="t('ending-length')" :tip="t('ending-length-tip')"><v-text-field density="compact" hide-details="auto" v-model="endingLength" type="number" :rules="[RULES.notEmpty]"></v-text-field></CfgField>
          <CfgField :label="t('render-start-time')" :tip="t('render-start-time-tip')"><v-text-field density="compact" hide-details="auto" v-model="playStartTime" type="number" :rules="[RULES.positiveOrZero]"></v-text-field></CfgField>
          <CfgField :label="t('render-end-time')" :tip="t('render-end-time-tip')"><v-text-field density="compact" hide-details="auto" v-model="playEndTime" type="number" :rules="[RULES.positiveOrNull]"></v-text-field></CfgField>
        </div>
        <!-- ===== 文件名格式 + Tip（放在「参数」的最后）=====
             这两项都是整行宽的**长文本框**，**单独放一个「只有 full 条目」的网格**：
             `design/config.css:83` 那条 `:has()` 规则会给这种网格去掉栏间竖线；
             要是跟上面的三栏条目挤在同一个网格里，那两条竖线会横穿这两个长输入框。
             模板往往有几十个字符、放不进一行：`:tip` 会挂成条目根元素的原生 `title`，
             悬停就能看全（提示与说明弹窗里的变量表是同一句话，见 locale 的 `file-name-format-tip`）；
             输入框本身按 `config.css` 的 `.cfg-item--full` 规则占掉除条目名以外的整行。 -->
        <div class="cfg-grid">
          <CfgField :label="t('file-name-format')" :tip="fileNameFormat" full><v-text-field density="compact" hide-details="auto" v-model="fileNameFormat" type="text" :rules="[RULES.notEmpty, RULES.notNull]" append-inner-icon="mdi-help-circle-outline" @click:append-inner="openFileNameFormatDialog"></v-text-field></CfgField>
          <slot name="params-tip"></slot>
        </div>
      </PhiSection>
      <PhiSection id="sec-audio" :title="t('title.audio')">
        <!-- 全部挤成一行：音乐音量 / 音效音量 / 音效限幅（文本框）
             + 无损音频 / 响度均衡 / 混音优化 / 强制限幅（勾选框，格式与「渲染项目」一致） -->
        <div class="cfg-grid"><CfgField :label="t('volume-music')" v-if="!loudnessEqualization"><v-text-field density="compact" hide-details="auto" type="number" :rules="[RULES.positiveOrZero]" v-model="volumeMusic"></v-text-field></CfgField><CfgField :label="t('volume-music-ratio')" v-if="loudnessEqualization"><v-text-field density="compact" hide-details="auto" type="number" :rules="[RULES.positiveOrZero]" v-model="volumeMusic"></v-text-field></CfgField><CfgField :label="t('volume-sfx')"><v-text-field density="compact" hide-details="auto" type="number" :rules="[RULES.positiveOrZero]" v-model="volumeSfx"></v-text-field></CfgField><CfgField :label="t('limit-threshold')"><v-text-field density="compact" hide-details="auto" type="number" :rules="[RULES.positiveOrZero]" :disabled="!forceLimit" v-model="limitThreshold"></v-text-field></CfgField><CfgSwitch :label="t('hires')" v-model="hires" /><CfgSwitch :label="t('loudness-equalization')" v-model="loudnessEqualization" /><CfgSwitch :label="t('audio-mix-optimization')" v-model="audioMixOptimization" /><CfgSwitch :label="t('force-limit')" v-model="forceLimit" /></div>
      </PhiSection>
      <PhiSection id="sec-player" :title="t('title.player')">
        <!-- 全部挤成一行：头像 / RKS / 课题模式颜色 / 课题模式等级 / 玩家名（各占 1/5） -->
        <div class="cfg-grid"><CfgField :label="t('player-avatar')"><v-text-field density="compact" hide-details="auto" readonly accept="image/*" @click="chooseAvatar" @click.clear="playerAvatar = undefined" clearable :model-value="playerAvatar ? playerAvatar.split('\\').pop()!.split('/').pop() : ''"></v-text-field></CfgField><CfgField :label="t('player-rks')"><v-text-field density="compact" hide-details="auto" :rules="[RULES.positiveOrZero]" type="number" v-model="playerRks"></v-text-field></CfgField><CfgField :label="t('challenge-color')"><v-autocomplete density="compact" hide-details="auto" :rules="[RULES.notEmpty]" :items="t('challenge-colors').split(',')" v-model="challengeColor"></v-autocomplete></CfgField><CfgField :label="t('challenge-rank')"><v-text-field density="compact" hide-details="auto" :rules="[RULES.int, RULES.positiveOrZero]" type="number" v-model="challengeRank"></v-text-field></CfgField><CfgField :label="t('player-name')"><v-text-field density="compact" hide-details="auto" v-model="playerName"></v-text-field></CfgField></div>
      </PhiSection>
      <!-- ===== 更多（渲染组的尾巴）=====
           「其他语言」在本组末尾这个二级分区「更多」里，
           形态是「罗马模式」「中文模式」**两条独立开关**（各一个 ✅/❌）。 -->
      <PhiSection id="sec-more-render" :title="t('more')">
        <div class="cfg-grid">
          <!-- 两条各自独立的条目（各一个 ✅/❌）：后端是 `roman` / `chinese`
               两个独立布尔键，两条互不影响。
               `CfgRadio.vue` 因此暂时没有人引用（留在仓库里备用，样式在 `design/config.css` 的 `.cfg-radio*`）。 -->
          <CfgSwitch :label="t('roman')" v-model="roman" :tip="t('roman-tip')" />
          <CfgSwitch :label="t('chinese')" v-model="chinese" :tip="t('chinese-tip')" />
        </div>
      </PhiSection>
    </PhiGroup>

    <!-- ===== 谱面 ===== -->
    <PhiGroup id="grp-chart" :title="t('title.chart')">
      <!-- ===== 功能之间的先后关系 =====
           上级没启用时，下级一律绑定 disabled（变灰、不可改），表达式就是「取反那个上级开关」。
           注意要带上整条上级链: 上级被灰掉时它自己的值还留着，所以下级得把链条上每一级都取反写进去
           （例: 关掉「背景」后，「背景暗淡」与「背景模糊」也必须一起灰 -> !renderBg）：
             判定线       → 其他判定线 / 判定线染色 / 判定线调试
             按键         → 按键染色 / 按键缩放 / 按键调试 / 双押提示 / 按键激进优化
             打击特效     → 打击特效染色
             打击特效 或 特效粒子（任一开着）→ 粒子限制
             难度         → 自定义难度         连击数 → COMBO 文字
             背景         → 背景暗淡（0 = 不压暗）/ 背景模糊
             噪域         → 噪域火花 / 噪声斑点 / 噪域扭曲 / 各个噪域参数 /
                            简化噪域 →( 直接镂空 / 简化噪域不透明度 / 简化噪域颜色 /
                                       简化噪域文字 → 文字颜色、文字淡色 )
                            强制定位 →( 强制定位范围着色 / 矩形高度·宽度·时间窗 )
             判定范围     → 显现时间 / 不透明度 / 过渡时间 / 宽度 / 长度 / 颜色
             激进优化     → 按键激进优化 / 粒子激进优化 -->
      <!-- 显示名是「元素」/ "Elements"，但 locale 键 `render-item`、DOM id `sec-render-item`、
           `fields.ts` 里的分组键都还是旧的 `render-item`（只改了显示值）。
           下面两处注释里出现的「谱面元素」是旧显示名的引用，保留。 -->
      <PhiSection id="sec-render-item" :title="t('render-item')">
                  <div class="cfg-grid">
                    <!-- 本网格的次序：按键 双押提示 打击特效 特效粒子 判定线 噪域
                         背景 暂停按钮 曲目名称 分数 难度 定数 连击数 进度条
                         （前半是元素开关，后半是界面叠加项）。
                         「判定线」这一格排第 5，与「判定线」分区表头那个 ⏻ 是同一个
                         `renderLine`，所以那边照样能开关、子项照样跟着变灰。 -->
                    <CfgSwitch :label="t('render-note')" v-model="renderNote" />
                    <CfgSwitch :label="t('render-double-hint')" v-model="renderDoubleHint" :disabled="!renderNote" :tip="t('render-double-hint-tip')" />
                    <CfgSwitch :label="t('render-particle')" v-model="particle" />
                    <CfgSwitch :label="t('render-particle-square')" v-model="particleSquare" :tip="t('render-particle-square-tip')" />
                    <CfgSwitch :label="t('render-line')" v-model="renderLine" />
                    <CfgSwitch :label="t('render-block-area')" v-model="renderBlockArea" :tip="t('render-block-area-tips')" />
                    <!-- 背景：「拖影效果」开着时必须关掉（那一趟会整屏盖掉上一帧，见 `sec-more-chart`），
                         所以那时这个开关是灰的（值由 useRenderConfig 的联动强制写成 false）。 -->
                    <CfgSwitch :label="t('render-background')" v-model="renderBg" :tip="t('render-background-tip')" />
                    <CfgSwitch :label="t('render-ui-pause')" v-model="renderUiPause" />
                    <CfgSwitch :label="t('render-ui-name')" v-model="renderUiName" />
                    <CfgSwitch :label="t('render-ui-score')" v-model="renderUiScore" />
                    <CfgSwitch :label="t('render-ui-difficulty')" v-model="renderUiDifficulty" />
                    <CfgSwitch :label="t('render-ui-level')" v-model="renderUiLevel" :disabled="!renderUiDifficulty" :tip="t('render-ui-level-tip')" />
                    <CfgSwitch :label="t('render-ui-combo')" v-model="renderUiCombo" />
                    <CfgSwitch :label="t('render-ui-bar')" v-model="renderUiBar" :tip="t('render-ui-bar-tip')" />
          <!-- `chart-ratio`（谱面缩放）在下面的「界面」分区；`speed`（速度）与
               `fade`（上隐/下隐）在谱面组的「更多」分区。
               这个分区剩下的都是开关，网格里不混数值项。 -->
        </div>
      </PhiSection>
      <!-- ===== 界面 =====
           分区显示名是「界面」，locale 键仍是 `title.background`。 -->
      <PhiSection id="sec-chart-bg" :title="t('title.background')">
        <div class="cfg-grid">
          <!-- 配置里的 `render_bg_dim` 由下面这个数值派生（见 script 里的 watch），0 就是不压暗。 -->
          <CfgField :label="t('chart-ratio')"><v-text-field density="compact" hide-details="auto" type="number" :title="t('chart-ratio-tip')" :rules="[RULES.notEmpty, RULES.positive]" :model-value="String(chartRatio)" @update:model-value="(v: string) => { const n = parseFloat(v); if (Number.isFinite(n)) chartRatio = n; }"></v-text-field></CfgField>
          <!-- 0~1 范围的项用数值输入框（`CfgSlider` 全项目已不再被引用），0 等于不压暗（见上面的 watch）。 -->
          <CfgField :label="t('background-dim')"><v-text-field density="compact" hide-details="auto" type="number" :disabled="!renderBg" :title="t('background-dim-tip')" :rules="[RULES.notEmpty, RULES.positiveOrZero]" :model-value="String(backgroundDim)" @update:model-value="(v: string) => { const n = parseFloat(v); if (Number.isFinite(n)) backgroundDim = n; }"></v-text-field></CfgField>
          <CfgField :label="t('bg-blurriness')" :tip="t('bg-blurriness-tip')"><v-text-field density="compact" hide-details="auto" :disabled="!renderBg" v-model="bgBlurriness" type="number" :rules="[RULES.positiveOrZero, RULES.less10000]"></v-text-field></CfgField>
          <CfgField :label="t('watermark')" :tip="t('watermark-tip')"><v-textarea density="compact" hide-details="auto" rows="1" auto-grow v-model="watermark"></v-textarea></CfgField>
          <CfgField :label="t('combo')" :tip="t('combo-tip')"><v-textarea density="compact" hide-details="auto" rows="1" auto-grow :disabled="!renderUiCombo" :rules="[RULES.notCOMBO]" v-model="combo"></v-textarea></CfgField>
          <CfgField :label="t('difficulty')" :tip="t('difficulty-tip')"><v-textarea density="compact" hide-details="auto" rows="1" auto-grow :disabled="!renderUiDifficulty" v-model="difficulty"></v-textarea></CfgField>
        </div>
      </PhiSection>
      <!-- 二级总开关：「按键」标题行上那个开关就是「渲染项目」里的「按键」开关
           （`renderNote`），两处是同一个值，关掉时整块收起。 -->
      <PhiSection id="sec-chart-key" :title="t('title.keys')" v-model:gate="renderNote">
        <!-- Note 等比缩放 / Hold 尾部遮罩 / 允许负长度 Hold / Hold 打击特效间隔倍率
             （它们与谱面信息同名，也是配置项，会进预设与 config.toml） -->
        <div class="cfg-grid">
          <CfgSwitch :label="t('info.note-uniform-scale')" v-model="noteUniformScale" :disabled="!renderNote" :tip="t('info.note-uniform-scale-tip')" />
          <CfgSwitch :label="t('info.hold-partial-cover')" v-model="holdPartialCover" :disabled="!renderNote" :tip="t('info.hold-partial-cover-tip')" />
          <CfgSwitch :label="t('info.negative-length-hold')" v-model="negativeLengthHold" :disabled="!renderNote" :tip="t('info.negative-length-hold-tip')" />
          <!-- Hold 打击特效跟着「特效粒子」走，所以它的上级链比上面三个多一条 -->
          <CfgField :label="t('info.hold-particle-interval-ratio')" :tip="t('info.hold-particle-interval-ratio-tip')">
            <v-text-field type="number" density="compact" hide-details="auto" :disabled="!renderNote || (!particle && !particleSquare)" :rules="[RULES.positive]" v-model="holdParticleIntervalRatio"></v-text-field>
          </CfgField>
          <CfgField :label="t('note-scale')"><v-text-field density="compact" hide-details="auto" type="number" :disabled="!renderNote" :title="t('note-scale-tip')" :rules="[RULES.notEmpty, RULES.positiveOrZero]" :model-value="String(noteScale)" @update:model-value="(v: string) => { const n = parseFloat(v); if (Number.isFinite(n)) noteScale = n; }"></v-text-field></CfgField>
          <CfgField :label="t('chart-debug-note')"><v-text-field density="compact" hide-details="auto" type="number" :disabled="!renderNote" :title="t('chart-debug-note-tip')" :rules="[RULES.notEmpty, RULES.positiveOrZero]" :model-value="String(chartDebugNote)" @update:model-value="(v: string) => { const n = parseFloat(v); if (Number.isFinite(n)) chartDebugNote = n; }" :append-inner-icon="chartDebugNote > 0 ? 'mdi-cog-outline' : undefined" @click:append-inner="openDebugNoteDialog"></v-text-field></CfgField>
          <!-- 按键染色 / 打击特效染色都在本分区 -->
          <CfgField :label="t('notecolor')"><v-textarea density="compact" hide-details="auto" rows="1" auto-grow :disabled="!renderNote" v-model="notecolor" :rules="[RULES.hexColorOptional]"></v-textarea></CfgField>
          <CfgField :label="t('hitcolor')"><v-textarea density="compact" hide-details="auto" rows="1" auto-grow :disabled="!particle" v-model="hitcolor" :rules="[RULES.hexColorOptional]"></v-textarea></CfgField>
        </div>
      </PhiSection>
      <!-- ===== 判定线 =====
           本二级分区收纳与判定线有关的项，位置在「按键」之后、「噪域」之前。
           下面的「其他判定线」/「判定线染色」/「谱面调试 - 判定线」三项都带 `:disabled="!renderLine"`
           —— 关掉时它们一起变灰（闸门在表头上）。
           「判定线」那个 `<CfgSwitch :label="t('render-line')" v-model="renderLine" />`
           在上面「元素」的网格里（排在「特效粒子」之后、「噪域」之前）；
           这里只留表头的 ⏻/❌ 闸门（`v-model:gate="renderLine"`），两者本来就是同一个值。 -->
      <PhiSection id="sec-chart-line" :title="t('title.judge-line')" v-model:gate="renderLine">
        <div class="cfg-grid">
          <CfgSwitch :label="t('render-line-extra')" v-model="renderLineExtra" :disabled="!renderLine" :tip="t('render-line-extra-tips')" />
          <CfgField :label="t('linecolor')"><v-textarea density="compact" hide-details="auto" rows="1" auto-grow :disabled="!renderLine" v-model="linecolor" :rules="[RULES.hexColorOptional]"></v-textarea></CfgField>
          <CfgField :label="t('chart-debug-line')"><v-text-field density="compact" hide-details="auto" type="number" :disabled="!renderLine" :title="t('chart-debug-line-tip')" :rules="[RULES.notEmpty, RULES.positiveOrZero]" :model-value="String(chartDebugLine)" @update:model-value="(v: string) => { const n = parseFloat(v); if (Number.isFinite(n)) chartDebugLine = n; }" :append-inner-icon="chartDebugLine > 0 ? 'mdi-cog-outline' : undefined" @click:append-inner="openDebugLineDialog"></v-text-field></CfgField>
          <!-- 「判定线长度」与谱面信息同名，也是配置项 -->
          <CfgField :label="t('info.lineLength')" :tip="t('info.lineLength-tip')">
            <v-text-field type="number" density="compact" hide-details="auto" :disabled="!renderLine" :rules="[RULES.less10000]" v-model="lineLength"></v-text-field>
          </CfgField>
        </div>
      </PhiSection>
      <PhiSection id="sec-block-area" :title="t('title.block-area')" v-model:gate="renderBlockArea" :gate-tip="t('render-block-area-tips')">
        <!-- 本分区只剩噪域自己的参数：`simplifiedBlockArea`（简化噪域）是一条三级闸门行
             （勾选框只管开关，子项收进齿轮弹窗），后面是完整噪域的数值项。
             注：`blockAreaLava`（岩浆噪域）与 `blockAreaForcePosition`（强制定位）已搬到
             谱面组的「更多」（下面 `sec-more-chart`）；它们与「简化噪域」的互斥、
             对 `renderBlockArea` 的依赖都跟着一起走，没有改。
             另：上面三项（噪声斑点 / 噪域扭曲 / 噪域火花）**不再**因岩浆噪域而禁用或清零 ——
             岩浆噪域开启时由引擎按「有效值」忽略这三项（`DPS/dps/src/core/block_area/native/shader.rs`
             里 `set_fill_style` 的 `|| lava` / `&& !lava`）。 -->
        <div class="cfg-grid">
          <CfgSwitch :label="t('block-area-noise-blotch')" v-model="blockAreaNoiseBlotch" :disabled="!renderBlockArea" :tip="t('block-area-noise-blotch-tip')" />
          <CfgSwitch :label="t('block-area-distort')" v-model="blockAreaDistort" :disabled="!renderBlockArea" :tip="t('block-area-distort-tip')" />
          <CfgSwitch :label="t('block-area-spark')" v-model="blockAreaSpark" :disabled="!renderBlockArea" :tip="t('block-area-spark-tip')" />
          <CfgSwitch :label="t('block-area-hover')" v-model="blockAreaHover" :disabled="!renderBlockArea" :tip="t('block-area-hover-tip')" />
          <PhiSubMenu id="sec3-simplified-block-area" :label="t('simplified-block-area')" :tip="t('simplified-block-area-tips')" :disabled="!renderBlockArea" v-model="simplifiedBlockArea">
            <div class="cfg-grid">
              <CfgSwitch :label="t('direct-block-hollow')" v-model="directBlockHollow" :disabled="!renderBlockArea" :tip="t('direct-block-hollow-tip')" />
              <CfgSwitch :label="t('simplified-block-area-text')" v-model="simplifiedBlockAreaText" :disabled="!renderBlockArea" :tip="t('simplified-block-area-text-tip')" />
              <CfgSwitch :label="t('block-area-reuse')" v-model="blockAreaReuse" :disabled="!renderBlockArea" :tip="t('block-area-reuse-tip')" />
              <CfgField :label="t('block-area-alpha')"><v-text-field density="compact" hide-details="auto" :disabled="!renderBlockArea" :title="t('block-area-alpha-tip')" v-model="blockAreaAlpha" type="number" :rules="[RULES.notEmpty, RULES.alpha01]"></v-text-field></CfgField>
              <CfgField :label="t('block-area-color')"><v-textarea density="compact" hide-details="auto" rows="1" auto-grow :disabled="!renderBlockArea" :title="t('block-area-color-tip')" v-model="blockAreaColor" :rules="[RULES.hexColorOptional]"></v-textarea></CfgField>
              <CfgField :label="t('simplified-block-area-text-color')"><v-textarea density="compact" hide-details="auto" rows="1" auto-grow :disabled="!renderBlockArea || !simplifiedBlockAreaText" :title="t('simplified-block-area-text-color-tip')" v-model="simplifiedBlockAreaTextColor" :rules="[RULES.hexColorOptional]"></v-textarea></CfgField>
              <CfgField :label="t('simplified-block-area-text-pale-color')"><v-textarea density="compact" hide-details="auto" rows="1" auto-grow :disabled="!renderBlockArea || !simplifiedBlockAreaText" :title="t('simplified-block-area-text-pale-color-tip')" v-model="simplifiedBlockAreaTextPaleColor" :rules="[RULES.hexColorOptional]"></v-textarea></CfgField>
            </div>
          </PhiSubMenu>
          <CfgField :label="t('block-area-density')"><v-text-field density="compact" hide-details="auto" :disabled="!renderBlockArea" :title="t('block-area-density-tip')" v-model="blockAreaDensity" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
          <CfgField :label="t('block-area-normal-color')"><v-textarea density="compact" hide-details="auto" rows="1" auto-grow :disabled="!renderBlockArea" :title="t('block-area-normal-color-tip')" v-model="blockAreaNormalColor" :rules="[RULES.hexColorOptional]"></v-textarea></CfgField>
          <CfgField :label="t('block-area-displace-strength')"><v-text-field density="compact" hide-details="auto" :disabled="!renderBlockArea" :title="t('block-area-displace-strength-tip')" v-model="blockAreaDisplaceStrength" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
          <CfgField :label="t('block-area-pixel-scale')"><v-text-field density="compact" hide-details="auto" :disabled="!renderBlockArea" :title="t('block-area-pixel-scale-tip')" v-model="blockAreaPixelScale" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
          <CfgField :label="t('block-area-displace-speed')"><v-text-field density="compact" hide-details="auto" :disabled="!renderBlockArea" :title="t('block-area-displace-speed-tip')" v-model="blockAreaDisplaceSpeed" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
          <CfgField :label="t('block-area-noise-radius')"><v-text-field density="compact" hide-details="auto" :disabled="!renderBlockArea" :title="t('block-area-noise-radius-tip')" v-model="blockAreaNoiseRadius" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
          <CfgField :label="t('block-area-hover-radius')"><v-text-field density="compact" hide-details="auto" :disabled="!renderBlockArea || !blockAreaHover" :title="t('block-area-hover-radius-tip')" v-model="blockAreaHoverRadius" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
          <CfgField :label="t('block-area-noise-smoothness')"><v-text-field density="compact" hide-details="auto" :disabled="!renderBlockArea" :title="t('block-area-noise-smoothness-tip')" v-model="blockAreaNoiseSmoothness" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
          <CfgField :label="t('block-area-edge-noise')"><v-text-field density="compact" hide-details="auto" :disabled="!renderBlockArea" :title="t('block-area-edge-noise-tip')" v-model="blockAreaEdgeNoise" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
        </div>
      </PhiSection>
      <!-- ===== 更多（谱面组的尾巴）=====
           本二级分区收着：
           - 「谱面进水」三级菜单行：✅/❌ 就是 `waterScreen`（配置值，与预设一起存），
             11 个参数（`WaterScreenSettings`）收进齿轮弹窗。这一行只占**一格**（一行三个里的第一个）；
           - 判定范围三级菜单行 / 判定模式 / 判定偏移；
           - 从「噪域」搬来的两项：岩浆噪域（`blockAreaLava`）与
             强制定位（`blockAreaForcePosition` 三级菜单行），排在「判定模式」之前。
           本分区按全项目惯例排：勾选框与三级菜单行在前，文本框（判定模式 / 判定偏移 /
           速度 / 上隐）在后。 -->
      <PhiSection id="sec-more-chart" :title="t('more')">
        <div class="cfg-grid">
          <!-- 透明度染色：普通条目（不是三级菜单行），与下面的「谱面进水」同处一个网格。 -->
          <CfgSwitch :label="t('alpha-tint')" v-model="alphaTint" :tip="t('alpha-tint-tip')" />
          <!-- 拖影效果 = 「保留帧缓冲」：每帧先把上一帧整屏铺回来再叠本帧，残影一直留着。
               画在单采样的累积缓冲上，所以开着时抗锯齿会被强制关掉，见 `config/useRenderConfig.ts`。 -->
          <CfgSwitch :label="t('preserve-framebuffer')" v-model="preserveFramebuffer" :tip="t('preserve-framebuffer-tips')" />
          <!-- 「纯配置谱面」：把判定线的移动 / 旋转 / 消失 / 速度拍平成常量，只保留每个按键在屏幕上的
               横向位置；判定时刻接近竖直的按键另分到左右两条竖判定线上；判定时刻落在噪域里的按键
               在它周围长出一块噪域矩形。开关与它的参数（判定线高度 / 按键流速 / 竖判定线的两个参数 /
               按键噪域与它的四个参数：矩形高度 / 矩形宽度 / 时间窗 / 延后视窗）同处这一个齿轮弹窗里。 -->
          <PhiSubMenu id="sec3-pure-config" :label="t('pure-config-chart')" :tip="t('pure-config-chart-tip')" v-model="pureConfigChart">
            <div class="cfg-grid">
              <CfgSwitch :label="t('pure-config-chart-side-lines')" v-model="pureConfigChartSideLines" :tip="t('pure-config-chart-side-lines-tip')" />
              <CfgSwitch :label="t('pure-config-chart-noise')" v-model="pureConfigChartNoise" :tip="t('pure-config-chart-noise-tip')" />
              <CfgSwitch :label="t('play-split-view')" v-model="playSplitView" :tip="t('play-split-view-tip')" />
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
          <!-- 「定轨谱面」：把整张谱面拍平成一条横向基准线 + 若干条固定轨道，按键按判定时刻吸附到
               轨道上（正中那条固定给 Flick / Drag，常规单侧两条、拥挤时才启用第三条）。
               开关与四个参数同处这一个齿轮弹窗里。 -->
          <PhiSubMenu id="sec3-fixed-lane" :label="t('fixed-lane-chart')" :tip="t('fixed-lane-chart-tip')" v-model="fixedLaneChart">
            <div class="cfg-grid">
              <CfgField :label="t('fixed-lane-gap')" :tip="t('fixed-lane-gap-tip')"><v-text-field density="compact" hide-details="auto" v-model="fixedLaneGap" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('fixed-lane-speed')" :tip="t('fixed-lane-speed-tip')"><v-text-field density="compact" hide-details="auto" v-model="fixedLaneSpeed" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('fixed-lane-line-y')" :tip="t('fixed-lane-line-y-tip')"><v-text-field density="compact" hide-details="auto" v-model="fixedLaneLineY" type="number" :rules="[RULES.notEmpty, RULES.alpha01]"></v-text-field></CfgField>
              <CfgField :label="t('fixed-lane-interval')" :tip="t('fixed-lane-interval-tip')"><v-text-field density="compact" hide-details="auto" v-model="fixedLaneInterval" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgSwitch :label="t('fixed-lane-hold-length')" :tip="t('fixed-lane-hold-length-tip')" v-model="fixedLaneHoldLength" />
              <CfgSwitch :label="t('fixed-lane-third')" :tip="t('fixed-lane-third-tip')" v-model="fixedLaneThird" />
              <CfgSwitch :label="t('play-split-view')" v-model="playSplitView" :tip="t('play-split-view-tip')" />
            </div>
          </PhiSubMenu>
          <PhiSubMenu id="sec3-chart-water" :label="t('title.chart-water')" :tip="t('water-screen-tip')" v-model="waterScreen">
            <WaterScreenSettings
                    v-model:water-screen="waterScreen"
                    v-model:water-opacity="waterOpacity"
                    v-model:water-flow-speed="waterFlowSpeed"
                    v-model:water-cell-scale="waterCellScale"
                    v-model:water-hp="waterHp"
                    v-model:water-drown-delay="waterDrownDelay"
                    v-model:water-damage="waterDamage"
                    v-model:water-damage-interval="waterDamageInterval"
                    v-model:water-death-time="waterDeathTime"
                    v-model:water-death-particles="waterDeathParticles"
                    v-model:water-sfx-volume="waterSfxVolume"
                    v-model:water-sfx-max="waterSfxMax"
                  ></WaterScreenSettings>
          </PhiSubMenu>
          <!-- 下面这一整块（判定范围三级菜单 + 判定模式 + 判定偏移）都在本分区，
               `:disabled` / 依赖关系见各条自己的说明。 -->
          <PhiSubMenu id="sec3-judge-range" :label="t('render-judge-range')" :tip="t('render-judge-range-tips')" v-model="renderJudgeRange">
            <div class="cfg-grid">
              <!-- 噪域遮蔽：只在噪域本身开着的时候才有意义（判定范围要在噪域那块矩形处淡出），
                   所以跟着「噪域」分区总开关一起禁用。按一行两个的排布排在最前。 -->
              <CfgSwitch :label="t('judge-range-noise-occlusion')" v-model="judgeRangeNoiseOcclusion" :disabled="!renderBlockArea && !simplifiedBlockArea" :tip="t('judge-range-noise-occlusion-tip')" />
              <CfgField :label="t('judge-range-time')"><v-text-field density="compact" hide-details="auto" :title="t('judge-range-time-tip')" :model-value="String(judgeRangeTime)" @update:model-value="(v: string) => { const n = parseFloat(v); if (Number.isFinite(n)) judgeRangeTime = n; }" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('judge-range-alpha')"><v-text-field density="compact" hide-details="auto" :title="t('judge-range-alpha-tip')" v-model="judgeRangeAlpha" type="number" :rules="[RULES.notEmpty, RULES.alpha01]"></v-text-field></CfgField>
              <CfgField :label="t('judge-range-transition')"><v-text-field density="compact" hide-details="auto" :title="t('judge-range-transition-tip')" v-model="judgeRangeTransition" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('judge-range-width')"><v-text-field density="compact" hide-details="auto" :title="t('judge-range-width-tip')" :model-value="String(judgeRangeWidth)" @update:model-value="(v: string) => { const n = parseFloat(v); if (Number.isFinite(n)) judgeRangeWidth = n; }" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('judge-range-width-flick')"><v-text-field density="compact" hide-details="auto" :title="t('judge-range-width-flick-tip')" :model-value="String(judgeRangeWidthFlick)" @update:model-value="(v: string) => { const n = parseFloat(v); if (Number.isFinite(n)) judgeRangeWidthFlick = n; }" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('judge-range-length')"><v-text-field density="compact" hide-details="auto" :title="t('judge-range-length-tip')" :model-value="String(judgeRangeLength)" @update:model-value="(v: string) => { const n = parseFloat(v); if (Number.isFinite(n)) judgeRangeLength = n; }" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('judge-range-color')"><v-textarea density="compact" hide-details="auto" rows="1" auto-grow :title="t('judge-range-color-tip')" v-model="judgeRangeColor" :rules="[RULES.hexColorOptional]"></v-textarea></CfgField>
            </div>
          </PhiSubMenu>
          <!-- 待判按键：把最近一个将要判定的按键染色再画一遍。它画在噪域那一趟之后，
               所以压得住噪域（不受「判定范围是否让位给噪域」影响）；没有音符可画时自然不显示。
               染色颜色收进这一行的**齿轮弹窗**里（齿轮的三级菜单形态见 `PhiSubMenu.vue`）。 -->
          <PhiSubMenu id="sec3-pending-note" :label="t('render-pending-note')" :tip="t('render-pending-note-tip')" :disabled="!renderNote" v-model="renderPendingNote">
            <div class="cfg-grid">
              <CfgField :label="t('pending-note-color')"><v-textarea density="compact" hide-details="auto" rows="1" auto-grow :title="t('pending-note-color-tip')" v-model="pendingNoteColor" :rules="[RULES.hexColorOptional]"></v-textarea></CfgField>
            </div>
          </PhiSubMenu>
          <!-- 岩浆噪域 / 强制定位：紧跟「判定范围」、排在「判定模式」之前
                （本分区惯例是勾选框 / 三级菜单行在前，文本框在后）。
                互斥关系（岩浆噪域 ⇔ 简化噪域）与 :disabled 如下：
                受「噪域」分区总开关 renderBlockArea 管。
                「岩浆噪域开启 ⇒ 界面禁用并清零上面那三项」那套联动不存在
                （`:disabled` 里没有 `|| blockAreaLava`，useRenderConfig.ts 也没有对应 watch），
                改由引擎在代码层面直接忽略这三项。 -->
          <CfgSwitch :label="t('block-area-lava')" v-model="blockAreaLava" :disabled="!renderBlockArea || simplifiedBlockArea" :tip="t('block-area-lava-tip')" />
          <PhiSubMenu id="sec3-force-position" :label="t('block-area-force-position')" :tip="t('block-area-force-position-tip')" :disabled="!renderBlockArea" v-model="blockAreaForcePosition">
            <div class="cfg-grid">
              <CfgSwitch :label="t('block-area-force-show')" v-model="blockAreaForceShow" :disabled="!renderBlockArea" :tip="t('block-area-force-show-tip')" />
              <CfgSwitch :label="t('block-area-force-invert')" v-model="blockAreaForceInvert" :disabled="!renderBlockArea" :tip="t('block-area-force-invert-tip')" />
              <CfgField :label="t('block-area-force-height')"><v-text-field density="compact" hide-details="auto" :disabled="!renderBlockArea" :title="t('block-area-force-height-tip')" v-model="blockAreaForceHeight" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('block-area-force-width')"><v-text-field density="compact" hide-details="auto" :disabled="!renderBlockArea" :title="t('block-area-force-width-tip')" v-model="blockAreaForceWidth" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('block-area-force-time')"><v-text-field density="compact" hide-details="auto" :disabled="!renderBlockArea" :title="t('block-area-force-time-tip')" v-model="blockAreaForceTime" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('block-area-force-delay')"><v-text-field density="compact" hide-details="auto" :disabled="!renderBlockArea" :title="t('block-area-force-delay-tip')" v-model="blockAreaForceDelay" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
            </div>
          </PhiSubMenu>
          <CfgField :label="t('judge-mode')" :tip="t('judge-mode-tip')"><v-autocomplete density="compact" hide-details="auto" :rules="[RULES.notEmpty]" :items="t('judge-modes').split(',')" v-model="judgeMode"></v-autocomplete></CfgField>
          <CfgField :label="t('judgeOffset')" :tip="t('judgeOffset-tip')"><v-text-field density="compact" hide-details="auto" v-model="judgeOffset" type="number" :rules="[RULES.notEmpty]"></v-text-field></CfgField>
          <!-- 速度与上/下隐在本分区（谱面组的「更多」），取值与校验规则见下。 -->
          <CfgField :label="t('speed')" :tip="t('speed-tip')"><v-combobox density="compact" hide-details="auto" v-model="speed" type="number" :items="speedList" :rules="[RULES.notEmpty, RULES.positive, RULES.less100, RULES.greater0_01]"></v-combobox></CfgField>
          <CfgField :label="t('fade')"><v-text-field density="compact" hide-details="auto" :title="t('fade-tip')" v-model="fade" type="number" :rules="[RULES.notEmpty]"></v-text-field></CfgField>
        </div>
      </PhiSection>

      <!-- 「谱面 · 调试」：整块只在设置页勾了「显示调试」时出现（`phi.debug.enabled`，
           入口由 Ctrl+Shift+点击左栏「设置」露出）。里面每个调试项各占一行三级菜单
           （参数收进齿轮弹窗），以后再加别的调试就再摆一行。 -->
      <PhiSection v-if="debugEnabled" id="sec-chart-debug" :title="t('title.xuejiang-sp-debug')">
        <div class="cfg-grid">
          <PhiSubMenu id="sec3-xuejiang-sp" :label="t('xuejiang-sp-group')" :tip="t('xuejiang-sp-group-tip')" v-model="xuejiangSpDebug">
            <div class="cfg-grid">
              <CfgField :label="t('xuejiang-sp-burst-combo')" :tip="t('xuejiang-sp-burst-combo-tip')"><v-text-field density="compact" hide-details="auto" v-model="xuejiangSpBurstCombo" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('xuejiang-sp-fake-notes')" :tip="t('xuejiang-sp-fake-notes-tip')"><v-text-field density="compact" hide-details="auto" v-model="xuejiangSpFakeNotes" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('xuejiang-sp-fake-bar-seconds')" :tip="t('xuejiang-sp-fake-bar-seconds-tip')"><v-text-field density="compact" hide-details="auto" v-model="xuejiangSpFakeBarSeconds" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('xuejiang-sp-roll-scale')" :tip="t('xuejiang-sp-roll-scale-tip')"><v-text-field density="compact" hide-details="auto" v-model="xuejiangSpRollScale" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('xuejiang-sp-volume-percentile')" :tip="t('xuejiang-sp-volume-percentile-tip')"><v-text-field density="compact" hide-details="auto" v-model="xuejiangSpVolumePercentile" type="number" :rules="[RULES.notEmpty, RULES.alpha01]"></v-text-field></CfgField>
              <CfgField :label="t('xuejiang-sp-volume-gamma')" :tip="t('xuejiang-sp-volume-gamma-tip')"><v-text-field density="compact" hide-details="auto" v-model="xuejiangSpVolumeGamma" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('xuejiang-sp-volume-smooth')" :tip="t('xuejiang-sp-volume-smooth-tip')"><v-text-field density="compact" hide-details="auto" v-model="xuejiangSpVolumeSmooth" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('xuejiang-sp-diff-fake')" :tip="t('xuejiang-sp-diff-fake-tip')"><v-text-field density="compact" hide-details="auto" v-model="xuejiangSpDiffFake" :rules="[RULES.notEmpty]"></v-text-field></CfgField>
              <CfgField :label="t('xuejiang-sp-diff-real')" :tip="t('xuejiang-sp-diff-real-tip')"><v-text-field density="compact" hide-details="auto" v-model="xuejiangSpDiffReal" :rules="[RULES.notEmpty]"></v-text-field></CfgField>
              <CfgField :label="t('xuejiang-sp-face-hit-start')" :tip="t('xuejiang-sp-face-hit-start-tip')"><v-text-field density="compact" hide-details="auto" v-model="xuejiangSpFaceHitStart" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('xuejiang-sp-face-period')" :tip="t('xuejiang-sp-face-period-tip')"><v-text-field density="compact" hide-details="auto" v-model="xuejiangSpFacePeriod" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('xuejiang-sp-face-rounds')" :tip="t('xuejiang-sp-face-rounds-tip')"><v-text-field density="compact" hide-details="auto" v-model="xuejiangSpFaceRounds" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('xuejiang-sp-face-lead-show')" :tip="t('xuejiang-sp-face-lead-show-tip')"><v-text-field density="compact" hide-details="auto" v-model="xuejiangSpFaceLeadShow" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('xuejiang-sp-face-lead-roll')" :tip="t('xuejiang-sp-face-lead-roll-tip')"><v-text-field density="compact" hide-details="auto" v-model="xuejiangSpFaceLeadRoll" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('xuejiang-sp-face-text-show')" :tip="t('xuejiang-sp-face-text-show-tip')"><v-text-field density="compact" hide-details="auto" v-model="xuejiangSpFaceTextShow" :rules="[RULES.notEmpty]"></v-text-field></CfgField>
              <CfgField :label="t('xuejiang-sp-face-text-roll')" :tip="t('xuejiang-sp-face-text-roll-tip')"><v-text-field density="compact" hide-details="auto" v-model="xuejiangSpFaceTextRoll" :rules="[RULES.notEmpty]"></v-text-field></CfgField>
              <CfgField :label="t('xuejiang-sp-face-text-hit')" :tip="t('xuejiang-sp-face-text-hit-tip')"><v-text-field density="compact" hide-details="auto" v-model="xuejiangSpFaceTextHit" :rules="[RULES.notEmpty]"></v-text-field></CfgField>
            </div>
          </PhiSubMenu>
          <PhiSubMenu id="sec3-baoying-sp" :label="t('baoying-sp-group')" :tip="t('baoying-sp-group-tip')" v-model="baoyingSpDebug">
            <div class="cfg-grid">
              <CfgField :label="t('baoying-sp-wow-height')" :tip="t('baoying-sp-wow-height-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpWowHeight" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-wow-offset-x')" :tip="t('baoying-sp-wow-offset-x-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpWowOffsetX" type="number" :rules="[RULES.notEmpty, RULES.number]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-wow-offset-y')" :tip="t('baoying-sp-wow-offset-y-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpWowOffsetY" type="number" :rules="[RULES.notEmpty, RULES.number]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-title-height')" :tip="t('baoying-sp-title-height-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpTitleHeight" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-title-offset-x')" :tip="t('baoying-sp-title-offset-x-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpTitleOffsetX" type="number" :rules="[RULES.notEmpty, RULES.number]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-title-offset-y')" :tip="t('baoying-sp-title-offset-y-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpTitleOffsetY" type="number" :rules="[RULES.notEmpty, RULES.number]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-score-offset-x')" :tip="t('baoying-sp-score-offset-x-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpScoreOffsetX" type="number" :rules="[RULES.notEmpty, RULES.number]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-score-offset-y')" :tip="t('baoying-sp-score-offset-y-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpScoreOffsetY" type="number" :rules="[RULES.notEmpty, RULES.number]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-score-height')" :tip="t('baoying-sp-score-height-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpScoreHeight" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-score-gap')" :tip="t('baoying-sp-score-gap-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpScoreGap" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-score-squeeze')" :tip="t('baoying-sp-score-squeeze-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpScoreSqueeze" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-combo-offset-x')" :tip="t('baoying-sp-combo-offset-x-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpComboOffsetX" type="number" :rules="[RULES.notEmpty, RULES.number]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-combo-offset-y')" :tip="t('baoying-sp-combo-offset-y-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpComboOffsetY" type="number" :rules="[RULES.notEmpty, RULES.number]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-combo-height')" :tip="t('baoying-sp-combo-height-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpComboHeight" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-combo-gap')" :tip="t('baoying-sp-combo-gap-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpComboGap" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-combo-squeeze')" :tip="t('baoying-sp-combo-squeeze-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpComboSqueeze" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-combo-label-height')" :tip="t('baoying-sp-combo-label-height-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpComboLabelHeight" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-combo-label-offset-x')" :tip="t('baoying-sp-combo-label-offset-x-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpComboLabelOffsetX" type="number" :rules="[RULES.notEmpty, RULES.number]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-combo-label-offset')" :tip="t('baoying-sp-combo-label-offset-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpComboLabelOffset" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-diff-height')" :tip="t('baoying-sp-diff-height-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpDiffHeight" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-diff-gap-sp-lv')" :tip="t('baoying-sp-diff-gap-sp-lv-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpDiffGapSpLv" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-diff-gap-lv-question')" :tip="t('baoying-sp-diff-gap-lv-question-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpDiffGapLvQuestion" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-diff-squeeze')" :tip="t('baoying-sp-diff-squeeze-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpDiffSqueeze" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-diff-offset-x')" :tip="t('baoying-sp-diff-offset-x-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpDiffOffsetX" type="number" :rules="[RULES.notEmpty, RULES.number]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-diff-offset-y')" :tip="t('baoying-sp-diff-offset-y-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpDiffOffsetY" type="number" :rules="[RULES.notEmpty, RULES.number]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-digits-rtl')" :tip="t('baoying-sp-digits-rtl-tip')"><CfgSwitch v-model="baoyingSpDigitsRtl" /></CfgField>
              <CfgField :label="t('baoying-sp-hit-fx-scale')" :tip="t('baoying-sp-hit-fx-scale-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpHitFxScale" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
              <CfgField :label="t('baoying-sp-hit-fx-rotate')" :tip="t('baoying-sp-hit-fx-rotate-tip')"><CfgSwitch v-model="baoyingSpHitFxRotate" /></CfgField>
              <CfgField :label="t('baoying-sp-particle-count')" :tip="t('baoying-sp-particle-count-tip')"><v-text-field density="compact" hide-details="auto" v-model="baoyingSpParticleCount" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
            </div>
          </PhiSubMenu>
        </div>
      </PhiSection>
    </PhiGroup>


  </v-form>

  <!-- 「判定线调试」「按键调试」的齿轮配置页：只有对应那一项 >0 时齿轮才出现（见上面的输入框）。
       皮肤与「文件名格式」弹窗一致（`.phi-config-dialog*`），内容是一排勾选框，勾中的项
       会拼成逗号名单存进 `chartDebugLineFields` / `chartDebugNoteFields`，由 dps 侧决定调试文本
       显示什么。 -->
  <v-dialog v-model="debugLineDialog" :width="debugLineDialogWidth" class="log-card-bg">
    <v-card class="log-card-only-window phi-config-dialog">
      <div class="phi-config-dialog__head">
        <span class="phi-config-dialog__title" v-t="'chart-debug-line'"></span>
      </div>
      <v-card-text class="config-root phi-config-dialog__body">
        <DebugFieldSettings v-model="chartDebugLineFields" :fields="LINE_DEBUG_FIELDS" prefix="chart-debug-line-field" />
      </v-card-text>
    </v-card>
  </v-dialog>

  <v-dialog v-model="debugNoteDialog" :width="debugNoteDialogWidth" class="log-card-bg">
    <v-card class="log-card-only-window phi-config-dialog">
      <div class="phi-config-dialog__head">
        <span class="phi-config-dialog__title" v-t="'chart-debug-note'"></span>
      </div>
      <v-card-text class="config-root phi-config-dialog__body">
        <DebugFieldSettings v-model="chartDebugNoteFields" :fields="NOTE_DEBUG_FIELDS" prefix="chart-debug-note-field" />
      </v-card-text>
    </v-card>
  </v-dialog>

  <v-dialog v-model="fileNameFormatDialog" :width="fileNameFormatDialogWidth" class="log-card-bg">
    <!-- 外观与齿轮弹窗（`PhiSubMenu.vue`）一致：
         皮肤写在 `design/config.css` 的 `.phi-config-dialog*` 里
         （底色取内容区那块 + 88% 不透明、直角、标题居中 1.5rem、底下**没有关闭按钮**，
         只靠点空白处或 Esc 关闭），宽度也用齿轮弹窗那套「内容区的 60%」（见上面的
         `openFileNameFormatDialog`）。标题不再是 `v-card-title`，改成和齿轮弹窗一样的
         `div.phi-config-dialog__head > span.phi-config-dialog__title`；正文换成配置页那套
         `.config-root` + `.cfg-grid` + `CfgField`（所以输入框外框 / 条目名纯白都和配置页一致）。
         背景交给 .log-card-only-window，**不要**在这里内联
         `background: rgba(var(--v-theme-dialog), 0.4) !important`：`--v-theme-dialog` 全项目
         从未定义 ⇒ var() 失效让整条声明变成 unset，卡片变透明，下面的设置项直接透出来。 -->
    <v-card class="log-card-only-window phi-config-dialog">
      <div class="phi-config-dialog__head">
        <span class="phi-config-dialog__title" v-t="'file-name-format'"></span>
      </div>
      <v-card-text class="config-root phi-config-dialog__body">
        <div class="cfg-grid">
          <CfgField :label="t('file-name-format')" :tip="fileNameFormat" full><v-text-field density="compact" hide-details="auto" v-model="fileNameFormat" type="text" :rules="[RULES.notEmpty, RULES.notNull]"></v-text-field></CfgField>
        </div>
        <pre class="file-name-format-tip">{{ t('file-name-format-tip') }}</pre>
      </v-card-text>
    </v-card>
  </v-dialog>

  <!-- 「新建预设」的名字输入弹窗（不用浏览器原生 prompt()，见上面 createPreset
       的注释）。皮肤与「文件名格式」弹窗一致；宽度也照它的算法（内容区的 40%，
       打开时量一次，见上面的 openCreateDialog）；按钮排照设置页那个重置确认弹窗
       （`v-card-actions.justify-end` + `variant="text"` 两个按钮）。 -->
  <v-dialog v-model="createDialog" :width="createDialogWidth" class="log-card-bg">
    <v-card class="log-card-only-window phi-config-dialog">
      <div class="phi-config-dialog__head">
        <span class="phi-config-dialog__title" v-t="'preset-add-title'"></span>
      </div>
      <v-card-text class="config-root phi-config-dialog__body">
        <div class="cfg-grid">
          <CfgField :label="t('preset-create-title')" full>
            <!-- autofocus：弹出时光标要直接落在输入框里（与原生 prompt() 的行为一致） -->
            <v-text-field autofocus density="compact" hide-details="auto" v-model="createName" type="text"
              :rules="[RULES.notEmpty, RULES.notNull]" @keyup.enter="confirmCreatePreset"></v-text-field>
          </CfgField>
        </div>
      </v-card-text>
      <v-card-actions class="justify-end">
        <v-btn class="hover-scale" variant="text" @click="createDialog = false" v-t="'cancel'"></v-btn>
        <v-btn class="hover-scale" variant="text" :disabled="!createName.trim()" @click="confirmCreatePreset"
          v-t="'confirm'"></v-btn>
      </v-card-actions>
    </v-card>
  </v-dialog>
  </div>
</template>

<style scoped>
.gradient-primary {
  background: var(--phi-accent) !important;
  color: var(--phi-label-inverted) !important;
}

/* 滚动容器。
   渲染页把整页滚动交给外层（.render-view__scroll），传 --config-scroll-max-height: none
   让自己不再自成滚动区；批量页的预设弹窗里没有外层滚动区，就用这个默认上限。 */
.config-scroll {
  max-height: var(--config-scroll-max-height, calc(100vh - 320px));
  margin-top: 0;
  padding-top: 20px;
  overflow-x: hidden;
  overflow-y: auto;
}

/* 「图形 / 其他」里的输入框原本是 cols="3"（12 列栅格的 1/4，一行 4 个）。
   这里改成固定 1/8 宽 —— 一行正好 8 个（默认窗口 1920x1080）；
   只有 4 个的旧行自动占左半边，仍对齐在同一个 8 列网格上。 */
.scroll-mask :deep(.v-col.v-col-3) {
  flex: 0 0 12.5%;
  max-width: 12.5%;
}
/* 输出第二行: 文件名格式独占一整行(8 格) —— 剩下的 8 个字段正好铺满下面一行 */
.scroll-mask :deep(.v-col.v-col-3.output-file-name) {
  flex: 0 0 100%;
  max-width: 100%;
}

/* 预设配置 / 资源包那一行：下拉框占满剩余宽度，右边跟一组小图标按钮。
   按钮之间用 12px，输入框与第一个按钮之间再额外留 8px —— 否则按钮会贴着输入框的下划线。 */
.preset-row {
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: var(--phi-space-3);
}
.preset-row > :first-child {
  margin-right: var(--phi-space-2);
}

/* 选中内置的「默认」预设时，下拉框里的名字用灰色 —— 自己建的预设仍是纯白。
   `.config-root` 那几条纯白规则的选择器权重更低（0,2,0），这里带 scoped 属性就是 0,3,0，
   所以不用 `!important`。`input` 那一行也要写：Vuetify 把名字画在 `<input>` 自己的 color 上。 */
.preset-default :deep(.v-field__input),
.preset-default :deep(.v-field input),
.preset-default :deep(.v-select__selection) {
  color: var(--phi-label-tertiary);
}

/* 「文件名格式」弹窗里的变量说明块。
   用 <pre> 是为了保留换行与缩进对齐，但浏览器默认给 <pre> 的是 monospace 兜底字体，
   中文会掉到难看的默认字体上；这里跟随全局 UI 字体（含中文回退），并放松行距。
   左内边距 = `.cfg-grid` 的 4px + 条目自带的 `0 12px`，让说明文字与上面那个 `full` 条目的
   条目名左边缘对齐（弹窗皮肤见 `design/config.css` 的 `.phi-config-dialog*`）。 */
.file-name-format-tip {
  font-family: var(--phi-font-family);
  font-size: 0.8125rem;
  line-height: 1.75;
  color: var(--phi-label-secondary);
  white-space: pre-wrap;
  margin-top: var(--phi-space-3);
  padding: 0 var(--phi-space-3) 0 calc(var(--phi-space-1) + var(--phi-space-3));
}



</style>
