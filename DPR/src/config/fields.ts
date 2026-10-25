/**
 * 配置层：渲染参数规格表。
 *
 * 这里是「一个渲染参数到底怎么在界面值和配置值之间换算」的**唯一出处**。
 * ./useRenderConfig.ts 拿这张表生成界面状态、buildConfig()、applyConfig()；
 * 以后 ./ConfigField.vue 与 ./sections/* 也会拿它生成控件（标签/提示/校验/可用性到那时再挪进来，
 * 现在留在 ConfigView.vue 的模板里，避免同一个信息写两份、互相漂移）。
 *
 * 为什么要有这张表：以前新增一个参数要同时改
 *   model.ts 接口 / model.ts 默认值 / ConfigView 的 ref / buildConfig() / applyConfig() / 模板 六处，
 * 漏掉 buildConfig 或 applyConfig 就是「界面能改、渲染不认」的静默失效。
 * 现在键的集合由下面这张表决定，`_AllRenderConfigKeysCovered` 那行断言会在**编译期**
 * 检查表是否覆盖了 RenderConfig 的每一个键 —— 漏一个就直接报错点名。
 */
import type { RenderConfig } from '../model';
import type { FieldSpec, FieldTable, SectionId } from './types';

/** 挑战模式颜色：界面上是本地化名字，落盘成这几个英文词（下标一一对应） */
export const STD_CHALLENGE_COLORS = ['white', 'green', 'blue', 'red', 'golden', 'rainbow'] as const;

/**
 * 「Rr」谱面演出的打击特效默认色。
 *
 * 只用于**界面回显**：这一项留空表示「用全局 hitcolor / 资源包自带颜色」，
 * 但空输入框看起来像「默认值没生效」，所以留空时界面照样显示这个色。
 */
const RR_HIT_FX_COLOR = '#FBA373';

/* ============================== 输出 ============================== */
const OUTPUT = {
  resolution: {
    widget: 'combobox',
    uiKey: 'resolution',
    note: '界面是 "宽x高" 字符串，配置是 [宽, 高]',
    toConfig: (ui) => ui.resolution.split(/[xX]/).map((x: string) => parseInt(x)),
    toUi: (value) => value.join('x'),
  },
  fps: { widget: 'combobox', integer: true },
  sampleCount: {
    widget: 'derived',
    uiKey: 'sampleCount',
    note: '界面值是抗锯齿下拉的字符串：-1 = FXAA，其余是 MSAA 采样数',
    toConfig: (ui) => {
      const n = parseInt(ui.sampleCount);
      return n >= 1 ? n : 1;
    },
    toUi: (value, config) => (config.fxaa ? '-1' : String(value)),
  },
  fxaa: {
    widget: 'derived',
    uiKey: 'sampleCount',
    syncToUi: false,
    note: '由抗锯齿下拉选到 -1 决定，界面上没有独立控件',
    toConfig: (ui) => ui.sampleCount === '-1',
  },
  hevc: {
    widget: 'derived',
    uiKey: 'encoder',
    syncToUi: false,
    note: '由「编码器」下拉的第 2 项决定',
    toConfig: (ui, ctx) => ui.encoder === ctx.encoderList[1],
  },
  mpeg4: {
    widget: 'derived',
    uiKey: 'encoder',
    syncToUi: false,
    note: '由「编码器」下拉的第 3 项决定',
    toConfig: (ui, ctx) => ui.encoder === ctx.encoderList[2],
  },
  customEncoder: {
    widget: 'derived',
    uiKey: 'encoder',
    syncToUi: false,
    note: '编码器下拉里手输的自由文本；选的是预置项时落盘成 null',
    toConfig: (ui, ctx) => (ctx.encoderList.includes(ui.encoder) ? null : ui.encoder),
  },
  dynamicBitrateControl: {
    widget: 'checkbox',
    note: '界面上有独立开关，但选到 mpeg4 时后端固定走动态码率，落盘要跟着或上 mpeg4',
    toConfig: (ui, ctx) => ui.encoder === ctx.encoderList[2] || ui.dynamicBitrateControl,
    toUi: (_value, config) => config.mpeg4 || config.dynamicBitrateControl,
  },
  bitrate: {
    widget: 'combobox',
    note: 'mpeg4 只支持动态码率，这种情况落盘固定成 "7"',
    toConfig: (ui, ctx) => (ui.encoder === ctx.encoderList[2] ? '7' : ui.bitrate),
  },
  endingLength: { widget: 'number' },
  playStartTime: { widget: 'number' },
  playEndTime: {
    widget: 'number',
    note: '界面留空 = null。旧实现是 parseFloat("") = NaN，靠 JSON.stringify(NaN) 变成 null 才勉强对，现在显式返回 null',
    toConfig: (ui) => {
      const n = parseFloat(ui.playEndTime);
      return Number.isFinite(n) ? n : null;
    },
  },
  fileNameFormat: { widget: 'text' },
  // 界面上的名字是 hwAccel（模板一直这么绑），配置键才叫 hardwareAccel
  hardwareAccel: { widget: 'checkbox', uiKey: 'hwAccel' },
  // 「高性能模式」的勾选框**不在渲染页**，在设置页（值存 app 配置，见 SettingsView.vue）。
  // 这里仍然登记一笔，是因为它是 `RenderConfig` 的字段：不登记的话文件末尾那个
  // `MissingKeys` / `AssertNever` 断言会编译不过；后端 `post_render` 也照旧往这个字段里注入。
  highPerformance: { widget: 'checkbox' },
  renderLoading: { widget: 'checkbox' },
} satisfies Partial<FieldTable>;

/* ============================== 玩家 ============================== */
const PLAYER = {
  playerAvatar: {
    widget: 'avatar',
    note: '界面存路径字符串（没选 = undefined），配置是 string | null',
    toConfig: (ui) => (ui.playerAvatar && ui.playerAvatar.length ? ui.playerAvatar : null),
    toUi: (value) => value || undefined,
  },
  playerName: { widget: 'text' },
  playerRks: { widget: 'number' },
  challengeColor: {
    widget: 'autocomplete',
    note: '界面是本地化色名，配置是 STD_CHALLENGE_COLORS 里的英文词',
    // 两边都可能对不上（配置里的词不在表里 / 界面语言变了）：都回落到第一个，和旧实现的下标 0 行为一致
    toConfig: (ui, ctx) => STD_CHALLENGE_COLORS[ctx.challengeColors.indexOf(ui.challengeColor)] ?? STD_CHALLENGE_COLORS[0],
    toUi: (value, _config, ctx) =>
      ctx.challengeColors[STD_CHALLENGE_COLORS.indexOf(value as (typeof STD_CHALLENGE_COLORS)[number])] ?? ctx.challengeColors[0] ?? value,
  },
  challengeRank: { widget: 'number', integer: true },
} satisfies Partial<FieldTable>;

/* ============================== 音频 ============================== */
const AUDIO = {
  hires: { widget: 'checkbox' },
  loudnessEqualization: { widget: 'checkbox' },
  audioMixOptimization: { widget: 'checkbox' },
  forceLimit: { widget: 'checkbox' },
  volumeMusic: { widget: 'slider' },
  volumeSfx: { widget: 'slider' },
  limitThreshold: { widget: 'slider' },
} satisfies Partial<FieldTable>;

/* ============================= 材质包 ============================= */
const RESPACK = {
  resPackPath: {
    widget: 'combobox',
    uiKey: 'respack',
    note: '界面存的是整个材质包对象（含名字给下拉显示），配置只存路径',
    // `?.` 是防御：材质包列表是异步拉的，列表还没回来时 `ui.respack` 可能是空的，
    // 这时候构建配置不能抛错 —— 空的就当「没选材质包」（后端回落内置材质）。
    toConfig: (ui) => ui.respack?.path ?? null,
    toUi: (value, _config, ctx) => ctx.respacks.find((x) => x.path === value) || ctx.respacks[0],
  },
} satisfies Partial<FieldTable>;

/* =========================== 谱面信息 =============================
 * 这一区的控件由 RenderView 通过 `chart-info` 插槽填进来（解析谱面后才有）：
 *   - `diff` 难度标识（`AT`）、`lvl` 定数（`18.0`）都能手改；
 *   - `difficulty` 难度文本，整串（`AT Lv.18.0`）默认随上面两项拼出来（见 RenderView 的 watch）。
 * 三者的值都不会被配置覆盖（不在 `config/chartSide.ts` 的名单里）。 */
const CHART_INFO = {
  lvl: {
    widget: 'number',
    uiValue: 'number',
    syncToUi: false,
    note: '⚠️ 既有 bug：applyConfig 从来没写过 lvl（只写了 diff），所以发出去的永远是 TS 默认值。这里保持现状不动，等确认后再改',
  },
  diff: { widget: 'text' },
} satisfies Partial<FieldTable>;

/* =========================== 渲染项目 ============================= */
const RENDER_ITEM = {
  renderLine: { widget: 'checkbox' },
  renderNote: { widget: 'checkbox' },
  renderDoubleHint: { widget: 'checkbox' },
  renderPendingNote: { widget: 'checkbox' },
  pendingNoteColor: { widget: 'color' },
  renderUiPause: { widget: 'checkbox' },
  renderUiName: { widget: 'checkbox' },
  renderUiDifficulty: { widget: 'checkbox' },
  renderUiLevel: { widget: 'checkbox' },
  renderUiScore: { widget: 'checkbox' },
  renderUiCombo: { widget: 'checkbox' },
  renderUiBar: { widget: 'checkbox' },
  renderBg: { widget: 'checkbox' },
  particle: { widget: 'checkbox' },
  particleSquare: { widget: 'checkbox' },
  hitFxRandomFlip: { widget: 'checkbox' },
  hitFxRandomFlipChance: { widget: 'number' },
  hitFxFollowRotation: { widget: 'checkbox' },
  renderBlockArea: { widget: 'checkbox' },
  blockAreaHover: { widget: 'checkbox' },
  blockAreaHoverRadius: { widget: 'number' },
} satisfies Partial<FieldTable>;

/* ====================== 谱面 / 背景与文字 =========================
 * 末尾那一组是**谱面侧渲染参数**：它们与 dps 的 `ChartInfo` 同名，`RenderView.vue` 的
 * `buildParams()` 会把它们并进发给后端的 `info`（**配置值优先** ⇒ 导入谱面不再覆盖它们），
 * 于是它们进预设、进 config.toml，也出现在批量页的预设弹窗里。
 * 「谱面信息」里那些真正属于单张谱面的文字框不在配置里（见 RenderView 的 `#chart-info`）。
 * 分区 id 只是「这些键算哪个一级分区的」这个归属信息，控件仍然写在 ConfigView 手写模板里
 * （`sec-params` / `sec-chart-key` / `sec-chart-line`）。 */
const CHART = {
  renderBgDim: { widget: 'checkbox' },
  chartRatio: { widget: 'number', uiValue: 'number' },
  noteScale: { widget: 'number', uiValue: 'number' },
  speed: { widget: 'combobox' },
  fade: { widget: 'number' },
  chartDebugLine: { widget: 'number', uiValue: 'number' },
  chartDebugLineFields: {
    widget: 'text',
    note: '没有独立控件：只在「判定线调试」旁边齿轮弹窗里按勾选拼成逗号名单，渲染时由 dps 的 line 文本拼装读取',
  },
  chartDebugNote: { widget: 'number', uiValue: 'number' },
  chartDebugNoteFields: {
    widget: 'text',
    note: '没有独立控件：只在「按键调试」旁边齿轮弹窗里按勾选拼成逗号名单，渲染时由 dps 的 note::debug_note_text 读取',
  },
  backgroundDim: { widget: 'number', uiValue: 'number' },
  bgBlurriness: { widget: 'number' },
  watermark: { widget: 'textarea' },
  combo: { widget: 'textarea' },
  difficulty: { widget: 'textarea' },
  // ------------------------- 谱面侧渲染参数 -------------------------
  // 「纯配置谱面」三项归属「谱面」这一族，但控件按「谱面 → 更多」摆放（见 ConfigView.vue 的
  // `sec-more-chart`）：`section` 只决定配置族，不决定界面位置。
  pureConfigChart: { widget: 'checkbox' },
  // 这两个**不能**写 `uiValue: 'number'`：那样界面槽位里存的是数字，而文本框的校验规则
  // （`RULES.*`）第一句就是 `value.trim()`，会当场抛 `value.trim is not a function`。
  // 不写 `uiValue` 时 `defaultUiValue` 把数字存成字符串、`defaultConfigValue` 再解析回数字
  // （见 config/convert.ts），规则拿到的就是文本。
  pureConfigChartLineY: { widget: 'number' },
  pureConfigChartSpeed: { widget: 'number' },
  // 竖判定线的三个参数与上面几项同族（同样不能写 `uiValue: 'number'`）。
  pureConfigChartSideLines: { widget: 'checkbox' },
  pureConfigChartSideX: { widget: 'number' },
  pureConfigChartAngle: { widget: 'number' },
  pureConfigChartNoise: { widget: 'checkbox' },
  // 噪域矩形的四个参数（高度 / 宽度 / 时间窗 / 延后视窗）同族。
  pureConfigChartNoiseHeight: { widget: 'number' },
  pureConfigChartNoiseWidth: { widget: 'number' },
  pureConfigChartNoiseTime: { widget: 'number' },
  pureConfigChartNoiseDelay: { widget: 'number' },
  // 「定轨谱面」：一个开关 + 四个参数，与纯配置谱面同族（同样不能写 `uiValue: 'number'`）。
  fixedLaneChart: { widget: 'checkbox' },
  fixedLaneGap: { widget: 'number' },
  fixedLaneSpeed: { widget: 'number' },
  fixedLaneLineY: { widget: 'number' },
  fixedLaneInterval: { widget: 'number' },
  fixedLaneHoldLength: { widget: 'checkbox' },
  fixedLaneThird: { widget: 'checkbox' },
  // 「游玩分屏」：只影响试玩入口（开两个窗口），面板开关放在纯配置 / 定轨谱面那一栏里。
  playSplitView: { widget: 'checkbox' },
  forceAspectRatio: { widget: 'checkbox' },
  foldAnimation: { widget: 'checkbox' },
  aspectRatio: {
    widget: 'number',
    uiValue: 'number',
    note: '界面是「宽 : 高」两个框，换算是 ConfigView 里的本地 ref + watcher（打字时不打断），这里只存比值本身',
  },
  noteUniformScale: { widget: 'checkbox' },
  holdPartialCover: { widget: 'checkbox' },
  negativeLengthHold: { widget: 'checkbox' },
  holdParticleIntervalRatio: { widget: 'number' },
  lineLength: { widget: 'number' },
  // 总分 / 偏移的配置值仍随预设与 config.toml 存，但**控件搬到了「谱面信息」**
  // （RenderView 的 chart-info 插槽）。这里用 `derived` + `syncToUi: false`：
  //   - `applyConfig()` 不把它们写进界面（表单那边由 ConfigView 用 `bindChartInfoSlot` 填）；
  //   - `toConfig` 从内部槽取 —— 内部槽建在**配置形态**上（总分整数、偏移秒），
  //     由谱面信息表单写入，见 `useRenderConfig.ts` 的 chartInfoBindings。
  scoreTotal: {
    widget: 'derived',
    uiKey: 'scoreTotalUi',
    syncToUi: false,
    toConfig: (ui) => Number(ui.scoreTotal),
    note: '控件在谱面信息里，值存在内部槽 ui.scoreTotal（整数）',
  },
  previewStart: { widget: 'number' },
  previewEnd: { widget: 'number', note: '0 = 到谱面结尾' },
  offset: {
    widget: 'derived',
    uiKey: 'offsetUi',
    syncToUi: false,
    toConfig: (ui) => Number(ui.offset),
    note: '控件在谱面信息里（界面单位毫秒），值存在内部槽 ui.offset（秒）',
  },
  // ---------------- 「雪降SP」特殊演出的调试参数 ----------------
  // 演出由输入谱面的文件名强制开启（没有开关），这几个只调观感，控件按「谱面 → 调试」摆放
  // （见 ConfigView.vue 的 `sec-chart-debug`）：`section` 只决定配置族，不决定界面位置。
  // 同样不能写 `uiValue: 'number'`：文本框的校验规则要拿字符串。
  xuejiangSpFakeNotes: { widget: 'number' },
  xuejiangSpFakeBarSeconds: { widget: 'number' },
  xuejiangSpBurstCombo: { widget: 'number' },
  xuejiangSpRollScale: { widget: 'number' },
  xuejiangSpDiffFake: { widget: 'text' },
  xuejiangSpDiffReal: { widget: 'text' },
  xuejiangSpVolumePercentile: { widget: 'number' },
  xuejiangSpVolumeGamma: { widget: 'number' },
  xuejiangSpVolumeSmooth: { widget: 'number' },
  xuejiangSpFaceHitStart: { widget: 'number' },
  xuejiangSpFacePeriod: { widget: 'number' },
  xuejiangSpFaceRounds: { widget: 'number' },
  xuejiangSpFaceLeadShow: { widget: 'number' },
  xuejiangSpFaceLeadRoll: { widget: 'number' },
  xuejiangSpFaceTextShow: { widget: 'text' },
  xuejiangSpFaceTextRoll: { widget: 'text' },
  xuejiangSpFaceTextHit: { widget: 'text' },
  baoyingSpWowHeight: { widget: 'number' },
  baoyingSpWowOffsetX: { widget: 'number' },
  baoyingSpWowOffsetY: { widget: 'number' },
  baoyingSpTitleHeight: { widget: 'number' },
  baoyingSpTitleOffsetX: { widget: 'number' },
  baoyingSpTitleOffsetY: { widget: 'number' },
  baoyingSpScoreOffsetX: { widget: 'number' },
  baoyingSpScoreOffsetY: { widget: 'number' },
  baoyingSpScoreHeight: { widget: 'number' },
  baoyingSpScoreGap: { widget: 'number' },
  baoyingSpScoreSqueeze: { widget: 'number' },
  baoyingSpComboOffsetX: { widget: 'number' },
  baoyingSpComboOffsetY: { widget: 'number' },
  baoyingSpComboHeight: { widget: 'number' },
  baoyingSpComboGap: { widget: 'number' },
  baoyingSpComboSqueeze: { widget: 'number' },
  baoyingSpComboLabelHeight: { widget: 'number' },
  baoyingSpComboLabelOffsetX: { widget: 'number' },
  baoyingSpComboLabelOffset: { widget: 'number' },
  baoyingSpDiffHeight: { widget: 'number' },
  baoyingSpDiffGapSpLv: { widget: 'number' },
  baoyingSpDiffGapLvQuestion: { widget: 'number' },
  baoyingSpDiffSqueeze: { widget: 'number' },
  baoyingSpDiffOffsetX: { widget: 'number' },
  baoyingSpDiffOffsetY: { widget: 'number' },
  baoyingSpDigitsRtl: { widget: 'checkbox' },
  baoyingSpHitFxScale: { widget: 'number' },
  baoyingSpHitFxRotate: { widget: 'checkbox' },
  baoyingSpParticleCount: { widget: 'number' },
  jingluanSpChanHeight: { widget: 'number' },
  jingluanSpChanOffsetY: { widget: 'number' },
  jingluanSpChanFadeIn: { widget: 'number' },
  jingluanSpChanFadeOut: { widget: 'number' },
  jingluanSpGuangHeight: { widget: 'number' },
  jingluanSpGuangOffsetY: { widget: 'number' },
  jingluanSpGuangFadeIn: { widget: 'number' },
  jingluanSpGuangFadeOut: { widget: 'number' },
  jingluanSpMopemopeHeight: { widget: 'number' },
  jingluanSpMopemopeOffsetY: { widget: 'number' },
  jingluanSpMopemopeFadeIn: { widget: 'number' },
  jingluanSpMopemopeFadeOut: { widget: 'number' },
  jingluanSpDim: { widget: 'number' },
  mopemopeSpColorA: { widget: 'color' },
  mopemopeSpColorB: { widget: 'color' },
  qzkagoSpStart: { widget: 'number' },
  qzkagoSpWhiteIn: { widget: 'number' },
  qzkagoSpWhiteOut: { widget: 'number' },
  clockParadoxSpScale: { widget: 'number' },
  clockParadoxSpOffsetX: { widget: 'number' },
  clockParadoxSpOffsetY: { widget: 'number' },
  clockParadoxSpFlash1On: { widget: 'number' },
  clockParadoxSpFlash1Fade: { widget: 'number' },
  clockParadoxSpFlash2On: { widget: 'number' },
  clockParadoxSpFlash2Fade: { widget: 'number' },
  clockParadoxSpFlash3On: { widget: 'number' },
  clockParadoxSpFlash3Fade: { widget: 'number' },
  clockParadoxSpFlash4On: { widget: 'number' },
  clockParadoxSpFlash4Fade: { widget: 'number' },
} satisfies Partial<FieldTable>;

/* ========================= 判定 / 染色 =========================== */
const JUDGE = {
  renderJudgeRange: { widget: 'checkbox' },
  alphaTint: { widget: 'checkbox' },
  allGood: {
    widget: 'derived',
    uiKey: 'judgeMode',
    syncToUi: false,
    note: '由「判定模式」下拉的第 2 项决定',
    toConfig: (ui, ctx) => ui.judgeMode === ctx.judgeModes[1],
  },
  allBad: {
    widget: 'derived',
    uiKey: 'judgeMode',
    syncToUi: false,
    // 以前这里写死 `() => false`（界面上没有这一项），于是预设里的 all_bad 一读一存就被抹成 false。
    // 现在「判定模式」下拉有第三项（`judge-modes` 的最后一个），选它就把 all_bad 发出去。
    note: '由「判定模式」下拉的第 3 项决定',
    toConfig: (ui, ctx) => ui.judgeMode === ctx.judgeModes[2],
  },
  judgeOffset: {
    widget: 'number',
    integer: true,
    note: '界面单位是毫秒，配置单位是秒',
    toConfig: (ui) => parseInt(ui.judgeOffset) / 1000,
    toUi: (value) => String(value * 1000),
  },
  judgeRangeTime: { widget: 'number', uiValue: 'number' },
  judgeRangeAlpha: { widget: 'number' },
  judgeRangeFadeTime: {
    widget: 'derived',
    uiKey: 'judgeRangeTransition',
    note: '界面按「过渡时间」输入 = 显示时间 − 淡入结束时刻；配置存淡入结束时刻本身',
    toConfig: (ui) => Math.max(0, ui.judgeRangeTime - (parseFloat(String(ui.judgeRangeTransition)) || 0)),
    toUi: (_value, config) => String(Math.max(0, config.judgeRangeTime - config.judgeRangeFadeTime)),
  },
  judgeRangeWidth: { widget: 'number', uiValue: 'number' },
  judgeRangeWidthFlick: { widget: 'number', uiValue: 'number' },
  judgeRangeLength: { widget: 'number', uiValue: 'number' },
  judgeRangeColor: { widget: 'color' },
  judgeRangeNoiseOcclusion: { widget: 'checkbox' },
  notecolor: { widget: 'textarea' },
  rrHitFxColor: {
    widget: 'textarea',
    // 配置里留空 = 用全局 hitcolor / 资源包颜色；界面按默认色回显，避免看着像「没填」
    toUi: (value) => (value ? String(value) : RR_HIT_FX_COLOR),
  },
  rrHitFxScale: { widget: 'number' },
  rrHitFxSpeed: { widget: 'number' },
  hitcolor: { widget: 'textarea' },
  linecolor: { widget: 'textarea' },
} satisfies Partial<FieldTable>;

/* ============================== 噪域 ============================== */
/* `section` 只是配置族归类，不代表界面位置：`blockAreaLava`（岩浆噪域）与四个
   `blockAreaForce*`（强制定位）在界面上位于谱面组的「更多」（ConfigView.vue 的 `sec-more-chart`）。 */
const BLOCK_AREA = {
  blockAreaNoiseBlotch: { widget: 'checkbox' },
  blockAreaDistort: { widget: 'checkbox' },
  blockAreaSpark: { widget: 'checkbox' },
  simplifiedBlockArea: { widget: 'checkbox' },
  simplifiedBlockAreaText: { widget: 'checkbox' },
  blockAreaReuse: { widget: 'checkbox' },
  blockAreaLava: { widget: 'checkbox' },
  directBlockHollow: { widget: 'checkbox' },
  blockAreaForcePosition: { widget: 'checkbox' },
  blockAreaForceShow: { widget: 'checkbox' },
  blockAreaForceInvert: { widget: 'checkbox' },
  blockAreaDensity: { widget: 'number' },
  blockAreaNormalColor: { widget: 'color' },
  blockAreaDisplaceStrength: { widget: 'number' },
  blockAreaPixelScale: { widget: 'number' },
  blockAreaDisplaceSpeed: { widget: 'number' },
  blockAreaNoiseRadius: { widget: 'number' },
  blockAreaNoiseSmoothness: { widget: 'number' },
  blockAreaEdgeNoise: { widget: 'number' },
  blockAreaAlpha: { widget: 'number' },
  blockAreaColor: { widget: 'color' },
  simplifiedBlockAreaTextColor: { widget: 'color' },
  simplifiedBlockAreaTextPaleColor: { widget: 'color' },
  blockAreaForceHeight: { widget: 'number' },
  blockAreaForceWidth: { widget: 'number' },
  blockAreaForceTime: { widget: 'number' },
  blockAreaForceDelay: { widget: 'number' },
} satisfies Partial<FieldTable>;

/* ============================ 屏幕进水 ============================ */
const WATER_SCREEN = {
  waterScreen: { widget: 'checkbox' },
  waterOpacity: { widget: 'number' },
  waterFlowSpeed: { widget: 'number' },
  waterCellScale: { widget: 'number' },
  waterHp: { widget: 'number' },
  waterDrownDelay: { widget: 'number' },
  waterDamage: { widget: 'number' },
  waterDamageInterval: { widget: 'number' },
  waterDeathTime: { widget: 'number' },
  waterDeathParticles: { widget: 'number' },
  waterSfxVolume: { widget: 'number' },
  waterSfxMax: { widget: 'number' },
} satisfies Partial<FieldTable>;

/* ========================== 性能与兼容 ============================ */
const PERFORMANCE = {
  preserveFramebuffer: { widget: 'checkbox' },
  aggressiveChart: { widget: 'checkbox' },
  aggressiveNote: { widget: 'checkbox' },
  aggressiveParticle: { widget: 'checkbox' },
  renderLineExtra: { widget: 'checkbox' },
  renderExtra: { widget: 'checkbox' },
  roman: { widget: 'checkbox' },
  chinese: { widget: 'checkbox' },
  maxParticles: {
    widget: 'derived',
    uiKey: 'maxParticlesText',
    note: '界面是文本下拉（低/中/高 或手输整数），换算由 ConfigView 的 updateMaxParticles() 负责：它在非法输入时要弹 toast 并中止保存，所以不能放进这里的纯函数换算',
    toConfig: (ui) => ui.maxParticles,
    toUi: (value, _config, ctx) => {
      const i = ctx.maxParticlesList.indexOf(value);
      return i >= 0 ? (ctx.maxParticlesTextList[i] ?? String(value)) : String(value);
    },
  },
} satisfies Partial<FieldTable>;

/** 分区 → 该分区下的键（对象字面量的书写顺序就是界面上的顺序） */
const SECTIONS = {
  output: OUTPUT,
  player: PLAYER,
  audio: AUDIO,
  respack: RESPACK,
  'chart-info': CHART_INFO,
  'render-item': RENDER_ITEM,
  chart: CHART,
  judge: JUDGE,
  'block-area': BLOCK_AREA,
  'water-screen': WATER_SCREEN,
  performance: PERFORMANCE,
} satisfies Record<Exclude<SectionId, 'preset'>, Partial<FieldTable>>;

/* ------------------------- 编译期完整性断言 -------------------------
 * 漏键时下面这行会报「Type '"xxx"' does not satisfy the constraint 'never'」，直接点名缺哪个键。 */
type CoveredKeys = { [S in keyof typeof SECTIONS]: keyof (typeof SECTIONS)[S] }[keyof typeof SECTIONS];
type MissingKeys = Exclude<keyof RenderConfig, CoveredKeys>;
type AssertNever<T extends never> = T;
// eslint-disable-next-line @typescript-eslint/no-unused-vars
type _AllRenderConfigKeysCovered = AssertNever<MissingKeys>;

/* ------------------------------ 拍平 ------------------------------ */
function flattenSections(): FieldTable {
  const out: Record<string, FieldSpec> = {};
  for (const [section, table] of Object.entries(SECTIONS)) {
    for (const [key, field] of Object.entries(table as Record<string, FieldSpec>)) {
      out[key] = { ...field, section: section as SectionId };
    }
  }
  return out as unknown as FieldTable;
}

/** 每个配置键 → 它的规格（section 已自动填好） */
export const RENDER_FIELDS: FieldTable = flattenSections();

/** 分区 → 键名数组，顺序即界面顺序。'preset' 区没有任何配置键（只有预设下拉 + 几个按钮）。 */
export const SECTION_FIELD_KEYS: Record<SectionId, string[]> = {
  preset: [],
  output: Object.keys(OUTPUT),
  player: Object.keys(PLAYER),
  audio: Object.keys(AUDIO),
  respack: Object.keys(RESPACK),
  'chart-info': Object.keys(CHART_INFO),
  'render-item': Object.keys(RENDER_ITEM),
  chart: Object.keys(CHART),
  judge: Object.keys(JUDGE),
  'block-area': Object.keys(BLOCK_AREA),
  'water-screen': Object.keys(WATER_SCREEN),
  performance: Object.keys(PERFORMANCE),
};

/** 规格表里的所有键名（供自检 / 调试用） */
export const RENDER_FIELD_KEYS = Object.keys(RENDER_FIELDS);
