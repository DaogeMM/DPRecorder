export interface ChartInfo {
  name: string;
  difficulty: string;
  lvl: number;
  diff: string;
  charter: string;
  composer: string;
  illustrator: string;

  chart: string;
  music: string;
  illustration: string;
  unlockVideo: string;

  previewStart: number;
  previewEnd: number | null;
  aspectRatio: number;
  forceAspectRatio: boolean;
  backgroundDim: number;
  lineLength: number;
  offset: number;
  tip: string | null;
  tags: string[];

  intro: string,

  holdPartialCover: boolean;
  negativeLengthHold: boolean;
  noteUniformScale: boolean;
  scoreTotal: number;
  holdParticleIntervalRatio: number;
  foldAnimation: boolean;
}

export type TaskStatus =
  | {
      type: 'null';
    }
  | {
      type: 'pending';
    }
  | {
      type: 'loading';
    }
  | {
      type: 'mixing';
    }
  | {
      type: 'mixing_sfx';
      progress: number;
    }
  | {
      type: 'rendering';
      progress: number;
      fps: number;
      estimate: number;
    }
  | {
      type: 'paused';
      progress: number;
    }
  | {
      type: 'done';
      duration: number;
      output: string;
    }
  | {
      type: 'canceled';
      output: string;
    }
  | {
      type: 'failed';
      output: string;
    };

export interface Task {
  id: number;
  name: string;
  output: string;
  path: string;
  info: ChartInfo;
  config: RenderConfig;
  cover: string;
  status: TaskStatus;
}

/**
 * 前端侧的渲染配置（camelCase）。必须和后端 `src-tauri/src/render.rs` 的 `RenderConfig` 逐字段对应：
 * `ConfigView.vue` 的 `buildConfig()` 会把这里的值 JSON 序列化后交给 `post_render`，
 * 后端用 `#[serde(rename_all = "camelCase", default)]` 反序列化。
 *
 * ---------- 新增 / 修改一个渲染参数时的完整步骤（漏一处就会出现「界面有、渲染不认」或反之） ----------
 * 1. 本接口加字段 + 下面 `DEFAULT_RENDER_CONFIG` 加默认值。默认值**必须和后端
 *    `impl Default for RenderConfig` 一致**，否则界面显示的默认值和命令行直接渲染的默认值会不同。
 * 2. `src/components/ConfigView.vue` 四处：顶部按分区加 `ref(DEFAULT_RENDER_CONFIG.xxx)`；
 *    `buildConfig()` 里输出（数字 / 字符串要转换，例 `parseFloat(String(x))`）；
 *    `applyConfig()` 里读回（例 `String(config.xxx)`）；模板在对应分区加控件
 *    （输入框 `class="mx-2"`，勾选用 `TipCheck` 或 `<v-checkbox>`，并放进该区块的勾选行）。
 * 3. 若它是某个开关的下级，控件加 `:disabled="..."`，表达式要写**整条上级链**
 *    （例 `!renderBg || !renderBgDim` —— 上级被禁用后它自己的值仍然留着），并在 `ConfigView.vue`
 *    开头那段「高级页: 功能之间的先后关系」注释里登记；上级与下级要联动时照现有的 watch 写法加。
 * 4. i18n：`src/locales/zh-CN/config.json` 与 `src/locales/en/config.json` **都要**加标签键
 *    （只加一个语言时另一个会显示字面量 key）；提示键照邻居的写法（有的叫 `-tip`、有的叫 `-tips`）。
 * 5. 后端步骤见 `src-tauri/src/render.rs` 里 `RenderConfig` 结构体上方的注释（字段 / `to_config()` /
 *    `impl Default` / `src-tauri/config.toml` 与 dev 运行时读的 exe 同目录 `config.toml`）。
 *    只有 DPS 渲染真的要用到这个值时才改它（`DPS/dps/src/config.rs` 加字段并在 `core/` 里读）。
 * 6. 改完跑 `& .\node_modules\.bin\vue-tsc.CMD --noEmit -p tsconfig.app.json --composite false`
 *    （本仓库没有测试套件；`pnpm type-check` 在没有 TTY 的环境里会因 `ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY` 中止）。
 */
export interface RenderConfig {
  resolution: number[];
  endingLength: number;
  renderLoading: boolean;
  hires: boolean;
  chartDebugLine: number;
  /** 判定线调试文本显示哪些项，逗号分隔（取值见 dps 的 `core::line` 里拼文本的地方） */
  chartDebugLineFields: string;
  chartDebugNote: number;
  /** 按键调试文本显示哪些项，逗号分隔（取值见 dps 的 `core::note::debug_note_text`） */
  chartDebugNoteFields: string;
  chartRatio: number;
  allGood: boolean;
  allBad: boolean;
  /** 「落点模拟」：AUTOPLAY 下长条按下时把落点钉在按键当时的中心，之后判定照着这个固定落点
   *  模拟真实手感（落点移出判定范围并持续一小会儿就会断连），不再是无条件 Perfect。 */
  touchSim: boolean;
  /** 「落点宽度」：落点的半径（谱面坐标，画成半透明蓝色球）。0 = 跟随判定范围。 */
  touchSimWidth: number;
  fps: number;
  hardwareAccel: boolean;
  /** 高性能模式：勾选后渲染时尽量把工作交给 GPU（高能显卡偏好 + 帧读回翻转省掉 ffmpeg 的 CPU vflip）。
   *  只影响渲染怎么跑，不改画面；默认关闭（关闭时渲染结果与从前逐字节一致）。
   *  **勾选框不在这一页**（控件在设置页，值存 app 配置 `config.toml`）：
   *  这里保留字段是因为渲染配置本身仍能表达它，CLI 与 `post_render` 注入都走它。 */
  highPerformance: boolean;
  hevc: boolean;
  mpeg4: boolean;
  customEncoder: string | null;
  dynamicBitrateControl: boolean;
  bitrate: string;

  aggressiveChart: boolean;
  aggressiveNote: boolean;
  aggressiveParticle: boolean;
  challengeColor: string;
  challengeRank: number;
  noteScale: number;
  particle: boolean;
  particleSquare: boolean;
  /** 打击特效随机反转：按概率把打击特效贴图做中心对称翻转（同时左右 + 上下，等价于把贴图转 180°）；只作用于 Tap / Drag / Flick，Hold 不翻 */
  hitFxRandomFlip: boolean;
  /** 打击特效随机反转的概率（0~1）：每颗打击特效粒子各自按它决定翻不翻，0 = 永不翻转 */
  hitFxRandomFlipChance: number;
  /** 打击特效跟随按键旋转：开着时打击特效贴图跟着按键的判定线旋转角一起转 */
  hitFxFollowRotation: boolean;
  playerAvatar: string | null;
  playerName: string;
  playerRks: number;
  sampleCount: number;
  fxaa: boolean;
  resPackPath: string | null;
  speed: number;
  volumeMusic: number;
  volumeSfx: number;
  watermark: string;
  notecolor: string;
  rrHitFxColor: string;
  rrHitFxScale: number;
  rrHitFxSpeed: number;
  hitcolor: string;
  linecolor: string;
  roman: boolean;
  chinese: boolean;
  combo: string;
  lvl: number;
  diff: string;
  difficulty: string;
  judgeOffset: number;
  forceLimit: boolean;
  limitThreshold: number;
  loudnessEqualization: boolean;
  audioMixOptimization: boolean;
  fileNameFormat: string;

  renderLine: boolean;
  renderLineExtra: boolean;
  renderNote: boolean;
  renderDoubleHint: boolean;
  /** 待判按键：把最近一个将要判定的按键染色再画一遍（画在噪域之上）。 */
  renderPendingNote: boolean;
  /** 待判按键染色用的 rgb（0~1），默认红色 */
  pendingNoteColor: [number, number, number];
  renderUiPause: boolean;
  renderUiName: boolean;
  renderUiLevel: boolean;
  renderUiDifficulty: boolean;
  renderUiScore: boolean;
  renderUiCombo: boolean;
  renderUiBar: boolean;
  /* ---------- 「雪降SP」特殊演出的调试参数 ----------
   * 演出本身由输入谱面的文件名强制开启（不是配置项，这里没有对应的开关），
   * 下面这些只影响观感，正常出片不用动。 */
  /** 伪装谱面的音符数：分数按 `combo / 这个数 × 总分` 算 */
  xuejiangSpFakeNotes: number;
  /** 伪装阶段进度条按音乐时间匀速走满所需的秒数（120 秒时 240 COMBO 处约 0.72） */
  xuejiangSpFakeBarSeconds: number;
  /** 揭开伪装的 COMBO 数 */
  xuejiangSpBurstCombo: number;
  /** 分数倒滚窗口的倍率（1 = 滚到下一个音符判定的那一刻） */
  xuejiangSpRollScale: number;
  /** 伪装阶段右下角的难度文本 */
  xuejiangSpDiffFake: string;
  /** 揭开伪装后右下角的难度文本 */
  xuejiangSpDiffReal: string;
  /** 音量进度条的归一化分位（0~1） */
  xuejiangSpVolumePercentile: number;
  /** 音量进度条响应的指数（1 = 线性，2 = 平方） */
  xuejiangSpVolumeGamma: number;
  /** 音量进度条的平滑时间常数（秒） */
  xuejiangSpVolumeSmooth: number;
  /** 连击数换成表情的第一次判定时刻（秒） */
  xuejiangSpFaceHitStart: number;
  /** 表情轮次的间隔（秒） */
  xuejiangSpFacePeriod: number;
  /** 表情一共几轮（0 = 不显示） */
  xuejiangSpFaceRounds: number;
  /** 每轮从「展示」到判定提前多少秒开始 */
  xuejiangSpFaceLeadShow: number;
  /** 每轮从「开始滚动」到判定提前多少秒开始 */
  xuejiangSpFaceLeadRoll: number;
  /** 音符刚出现时连击数位置的表情 */
  xuejiangSpFaceTextShow: string;
  /** 音符开始滚动时连击数位置的表情 */
  xuejiangSpFaceTextRoll: string;
  /** 音符被判定时连击数位置的表情 */
  xuejiangSpFaceTextHit: string;
  /** 「报应SP」界面左上角 WOW! 贴图的高度 */
  baoyingSpWowHeight: number;
  /** 「报应SP」界面 WOW! 贴图向右的偏移 */
  baoyingSpWowOffsetX: number;
  /** 「报应SP」界面 WOW! 贴图向下的偏移 */
  baoyingSpWowOffsetY: number;
  /** 「报应SP」界面左下角曲名贴图的高度 */
  baoyingSpTitleHeight: number;
  /** 「报应SP」界面曲名贴图向右的偏移 */
  baoyingSpTitleOffsetX: number;
  /** 「报应SP」界面曲名贴图向下的偏移 */
  baoyingSpTitleOffsetY: number;
  /** 「报应SP」界面右上角分数向右的偏移 */
  baoyingSpScoreOffsetX: number;
  /** 「报应SP」界面右上角分数向下的偏移 */
  baoyingSpScoreOffsetY: number;
  /** 「报应SP」界面右上角分数单个数字的高度 */
  baoyingSpScoreHeight: number;
  /** 「报应SP」界面右上角分数相邻数字之间的间距 */
  baoyingSpScoreGap: number;
  /** 「报应SP」界面分数数字的横向挤压倍率 */
  baoyingSpScoreSqueeze: number;
  /** 「报应SP」界面中上连击数向右的偏移 */
  baoyingSpComboOffsetX: number;
  /** 「报应SP」界面中上连击数向下的偏移 */
  baoyingSpComboOffsetY: number;
  /** 「报应SP」界面中上连击数单个数字的高度 */
  baoyingSpComboHeight: number;
  /** 「报应SP」界面中上连击数相邻数字之间的间距 */
  baoyingSpComboGap: number;
  /** 「报应SP」界面连击数字的横向挤压倍率 */
  baoyingSpComboSqueeze: number;
  /** 「报应SP」界面连击数下方 `COMBO` 贴图的高度 */
  baoyingSpComboLabelHeight: number;
  /** 「报应SP」界面 `COMBO` 贴图向右的偏移 */
  baoyingSpComboLabelOffsetX: number;
  /** 「报应SP」界面 `COMBO` 贴图与连击数字之间的间距 */
  baoyingSpComboLabelOffset: number;
  /** 「报应SP」界面右下角 `SP LV.?` 贴图的高度 */
  baoyingSpDiffHeight: number;
  /** 「报应SP」界面 `SP` 与 `LV.` 之间的间距 */
  baoyingSpDiffGapSpLv: number;
  /** 「报应SP」界面 `LV.` 与 `?` 之间的间距 */
  baoyingSpDiffGapLvQuestion: number;
  /** 「报应SP」界面难度贴图的横向挤压倍率 */
  baoyingSpDiffSqueeze: number;
  /** 「报应SP」界面难度贴图向右的偏移 */
  baoyingSpDiffOffsetX: number;
  /** 「报应SP」界面难度贴图向下的偏移 */
  baoyingSpDiffOffsetY: number;
  /** 「报应SP」界面分数与连击数从右往左排 */
  baoyingSpDigitsRtl: boolean;
  /** 「报应SP」打击特效贴图缩放（小于等于 0 表示沿用资源包设置） */
  baoyingSpHitFxScale: number;
  /** 「报应SP」打击特效贴图是否随判定线旋转 */
  baoyingSpHitFxRotate: boolean;
  /** 「报应SP」打击特效粒子数（0 表示不画粒子） */
  baoyingSpParticleCount: number;
  /** 「痉挛SP」演出第一段（`SpasmodicChan`）插画高度占画面高度的比例 */
  jingluanSpChanHeight: number;
  /** 「痉挛SP」演出第一段插画的上下偏移（正数向下，按画面高度计） */
  jingluanSpChanOffsetY: number;
  /** 「痉挛SP」演出第一段插画淡入的秒数 */
  jingluanSpChanFadeIn: number;
  /** 「痉挛SP」演出第一段插画淡出的秒数 */
  jingluanSpChanFadeOut: number;
  /** 「痉挛SP」演出第二段（「光」）插画高度占画面高度的比例 */
  jingluanSpGuangHeight: number;
  /** 「痉挛SP」演出第二段插画的上下偏移（正数向下，按画面高度计） */
  jingluanSpGuangOffsetY: number;
  /** 「痉挛SP」演出第二段插画淡入的秒数 */
  jingluanSpGuangFadeIn: number;
  /** 「痉挛SP」演出第二段插画淡出的秒数 */
  jingluanSpGuangFadeOut: number;
  /** 「痉挛SP」演出第三段（「もぺもぺ」）插画高度占画面高度的比例 */
  jingluanSpMopemopeHeight: number;
  /** 「痉挛SP」演出第三段插画的上下偏移（正数向下，按画面高度计） */
  jingluanSpMopemopeOffsetY: number;
  /** 「痉挛SP」演出第三段插画淡入的秒数 */
  jingluanSpMopemopeFadeIn: number;
  /** 「痉挛SP」演出第三段插画淡出的秒数 */
  jingluanSpMopemopeFadeOut: number;
  /** 「痉挛SP」演出插画的压暗程度（0 = 不压暗，1 = 全黑；与背景压暗无关） */
  jingluanSpDim: number;
  /** 「もぺもぺSP」演出第一种红的乘算系数 rgb（0~1，用在那一串「闪」上，比第二种亮） */
  mopemopeSpColorA: [number, number, number];
  /** 「もぺもぺSP」演出第二种红的乘算系数 rgb（0~1，用在三段长镜头上，比第一种更暗更纯） */
  mopemopeSpColorB: [number, number, number];
  /** 「QZKAGO」演出：整段起点（秒） */
  qzkagoSpStart: number;
  /** 「QZKAGO」演出：白屏渐入时长（秒） */
  qzkagoSpWhiteIn: number;
  /** 「QZKAGO」演出：白屏渐出时长（秒） */
  qzkagoSpWhiteOut: number;
  /** 「Clock Paradox」演出：表盘大小倍率（1 = 与参考录像一致） */
  clockParadoxSpScale: number;
  /** 「Clock Paradox」演出：表盘圆心水平偏移（像素，按 1920×1080 计） */
  clockParadoxSpOffsetX: number;
  /** 「Clock Paradox」演出：表盘圆心垂直偏移（像素，正值向上） */
  clockParadoxSpOffsetY: number;
  /** 「Clock Paradox」演出：第 1 次出现的亮起时刻（秒） */
  clockParadoxSpFlash1On: number;
  /** 「Clock Paradox」演出：第 1 次出现的开始淡出时刻（秒） */
  clockParadoxSpFlash1Fade: number;
  /** 「Clock Paradox」演出：第 2 次出现的亮起时刻（秒） */
  clockParadoxSpFlash2On: number;
  /** 「Clock Paradox」演出：第 2 次出现的开始淡出时刻（秒） */
  clockParadoxSpFlash2Fade: number;
  /** 「Clock Paradox」演出：第 3 次出现的亮起时刻（秒） */
  clockParadoxSpFlash3On: number;
  /** 「Clock Paradox」演出：第 3 次出现的开始淡出时刻（秒） */
  clockParadoxSpFlash3Fade: number;
  /** 「Clock Paradox」演出：第 4 次出现的亮起时刻（秒） */
  clockParadoxSpFlash4On: number;
  /** 「Clock Paradox」演出：第 4 次出现的开始淡出时刻（秒） */
  clockParadoxSpFlash4Fade: number;
  renderBg: boolean;
  renderBgDim: boolean;
  /** 背景暗淡：黑色遮罩的不透明度（0~1），覆盖谱面信息里的同名值，默认 0.1 */
  backgroundDim: number;

  /* ---------- 谱面侧渲染参数（原本只存在于「谱面信息」里，现在也进配置） ----------
   * 这些键与 `dps::info::ChartInfo` 同名：`buildParams()` 会把它们并进发给后端的 `info`
   * （**配置值优先** ⇒ 导入谱面不再覆盖它们），于是它们既进预设、也进 config.toml。
   * 真正属于单张谱面的那些文字框（名称 / 曲师 / 谱师 / 画师 / 难度文本 / 定数 / 简介 / Tip…）
   * 仍然不进配置 —— 见 `RenderView.vue` 的 `#chart-info` 插槽。 */
  /** 强制宽高比：开启时按 `aspectRatio` 强制画面比例，关闭时只保证画面不超过该比例 */
  forceAspectRatio: boolean;
  /** 折叠式入场：按键的入场动画改成折叠式 */
  foldAnimation: boolean;
  /** 画面宽高比（= 宽 ÷ 高，默认 16:9 ≈ 1.7777778） */
  aspectRatio: number;
  /** 音符统一缩放 */
  noteUniformScale: boolean;
  /** 长按部分遮挡 */
  holdPartialCover: boolean;
  /** 长按反向遮挡（负长度长按） */
  negativeLengthHold: boolean;
  /** 长按打击特效的间隔倍率（默认 1） */
  holdParticleIntervalRatio: number;
  /** 判定线长度（默认 6） */
  lineLength: number;
  /** 总分（默认 1000000） */
  scoreTotal: number;
  /** 预览开始时间（秒，默认 0） */
  previewStart: number;
  /** 预览结束时间（秒，默认 0 = 到谱面结尾） */
  previewEnd: number;
  /** 谱面偏移（**秒**，界面上按毫秒编辑，默认 0）；渲染时与谱面文件自身的偏移相加 */
  offset: number;
  renderBlockArea: boolean;
  simplifiedBlockArea: boolean;
  /** 噪域打击光晕：按键被判中时在噪域上点亮一圈光晕（官方游玩时的触摸效果，默认开） */
  blockAreaHover: boolean;
  /** 噪域打击光晕半径：光晕作用半径（官方默认 0.5） */
  blockAreaHoverRadius: number;
  /** 简化噪域文字：在简化噪域正中间印上噪域编号（disable 白字 / enable 文字色，默认 #FF6094） */
  simplifiedBlockAreaText: boolean;
  /** 噪域复用：把几何与动画形状相同的噪域条目认成同一个噪域的多次出现，编号共享 */
  blockAreaReuse: boolean;
  blockAreaLava: boolean;
  waterScreen: boolean;
  waterOpacity: number;
  waterFlowSpeed: number;
  waterCellScale: number;
  waterHp: number;
  waterDrownDelay: number;
  waterDamage: number;
  waterDamageInterval: number;
  waterDeathTime: number;
  waterDeathParticles: number;
  waterSfxVolume: number;
  /** 受伤 / 死亡音效的并发上限（1~8） */
  waterSfxMax: number;
  /** 强制定位：忽略谱面自带的噪域，整屏铺一块噪域，并在每个按键周围挖掉一个矩形 */
  blockAreaForcePosition: boolean;
  /** 强制定位：按键矩形的高度 = 该值 × 屏幕高度（默认 0.2 = 20% 屏高） */
  blockAreaForceHeight: number;
  /** 强制定位：按键矩形的半宽 = 该值 × (屏幕高度 / 10)，基准与判定范围一致（默认 1.9） */
  blockAreaForceWidth: number;
  /** 强制定位：把矩形本身也着色画出来（默认关闭 ⇒ 只挖洞） */
  blockAreaForceShow: boolean;
  /** 强制定位：挖洞的时间窗（秒，默认 3）——只有判定时刻进入本值以内的按键才挖洞 */
  blockAreaForceTime: number;
  /** 强制定位延后视窗（秒，默认 0.3）：判定完之后洞再停留这么久才被噪域盖回来 */
  blockAreaForceDelay: number;
  /** 反向强制定位：把强制定位的矩形本身当噪域画出来，其余地方不铺噪域（默认关闭） */
  blockAreaForceInvert: boolean;
  /** 简化取消合并：每个简化噪域各画一趟，重叠的 enable 噪域不合并成一个连通区域 */
  blockAreaColor: [number, number, number];
  /** 噪域颜色：普通（完整）噪域的颜色（rgb 0~1），只取色相去整体旋转官方配色，默认 0.9 / 0.1 / 0.1 */
  blockAreaNormalColor: [number, number, number];
  /** 噪域文字颜色：简化噪域编号在 enable 之后的颜色（rgb 0~1），默认 #FF6094（1 / 0.3765 / 0.5804） */
  simplifiedBlockAreaTextColor: [number, number, number];
  /** 噪域文字淡色：简化噪域编号在 enable 之前的颜色（rgb 0~1），默认白 */
  simplifiedBlockAreaTextPaleColor: [number, number, number];
  blockAreaAlpha: number;
  /** 噪域密度：噪域边缘那圈像素颗粒的疏密，默认 2.5，越大颗粒越细越密 */
  blockAreaDensity: number;
  /** 噪域火花：完整噪域里那层火花点阵（默认 true） */
  blockAreaSpark: boolean;
  blockAreaNoiseBlotch: boolean;
  blockAreaDisplaceStrength: number;
  blockAreaPixelScale: number;
  blockAreaDisplaceSpeed: number;
  blockAreaNoiseRadius: number;
  blockAreaNoiseSmoothness: number;
  blockAreaEdgeNoise: number;
  directBlockHollow: boolean;
  /** 噪域扭曲：关掉后噪域不扭曲（填充位移与边缘破碎归零），只剩纯色/场景快照 + 透明度 */
  blockAreaDistort: boolean;
  renderJudgeRange: boolean;
  /** Tap / Hold 判定范围半宽 = 该值 × (屏幕高度 / 10) */
  judgeRangeWidth: number;
  /** Flick / Drag 判定范围半宽 = 该值 × (屏幕高度 / 10)，与 Tap / Hold 同一单位（默认 2.1） */
  judgeRangeWidthFlick: number;
  judgeRangeLength: number;
  judgeRangeTime: number;
  judgeRangeFadeTime: number;
  judgeRangeAlpha: number;
  judgeRangeColor: [number, number, number];
  /** 判定范围与噪域重合的部分是否让给噪域（官方行为 = 开） */
  judgeRangeNoiseOcclusion: boolean;
  preserveFramebuffer: boolean;
  renderExtra: boolean;
  /** 「纯配置谱面」：解析完谱面后把判定线拍平，只留下按键在屏幕上的横向位置（谱面文件不受影响） */
  pureConfigChart: boolean;
  /** 「纯配置谱面」的判定线高度：0 是屏幕底边、1 是屏幕顶边、0.5 是正中 */
  pureConfigChartLineY: number;
  /** 「纯配置谱面」的按键流速：判定线高度与长按尾端都按它算 */
  pureConfigChartSpeed: number;
  /** 「纯配置谱面 · 竖判定线」：判定时刻接近竖直的按键分到屏幕左右两条竖判定线上 */
  pureConfigChartSideLines: boolean;
  /** 「纯配置谱面」竖判定线距最近那条屏幕竖边的距离：0.5 是屏幕正中、0.1 是离边一成 */
  pureConfigChartSideX: number;
  /** 「纯配置谱面」竖直判定的角度阈值：与水平方向相差多少度以内仍算横向按键，默认 45 */
  pureConfigChartAngle: number;
  /** 「纯配置谱面 · 按键噪域」：判定时刻落在噪域里的按键，渲染时在它周围长出一块噪域矩形 */
  pureConfigChartNoise: boolean;
  /** 「纯配置谱面 · 按键噪域」矩形高度：全高 = 该值 × 屏幕高度（默认 0.2 ⇒ 20% 屏高） */
  pureConfigChartNoiseHeight: number;
  /** 「纯配置谱面 · 按键噪域」矩形宽度：半宽 = 该值 × 屏幕高度 / 10（与判定范围同一套基准） */
  pureConfigChartNoiseWidth: number;
  /** 「纯配置谱面 · 按键噪域」时间窗（秒）：判定时刻进入本值以内的按键才长矩形（默认 3） */
  pureConfigChartNoiseTime: number;
  /** 「纯配置谱面 · 按键噪域」延后视窗（秒）：判定完成后矩形再停留这么久才消失（默认 0.3） */
  pureConfigChartNoiseDelay: number;
  /** 「定轨谱面」：把所有判定线拍平成一条横向基准线，按键按判定时刻吸附到固定轨道（谱面文件不受影响） */
  fixedLaneChart: boolean;
  /** 「定轨谱面」相邻轨道间隔：屏宽的几成，0.14 = 屏宽 14% */
  fixedLaneGap: number;
  /** 「定轨谱面」按键流速：越大落得越快 */
  fixedLaneSpeed: number;
  /** 「定轨谱面」判定线高度：0 是屏幕底边、1 是屏幕顶边、0.5 是正中 */
  fixedLaneLineY: number;
  /** 「定轨谱面」同一轨道上两个按键之间至少留几秒（长按按尾巴算） */
  fixedLaneInterval: number;
  /** 「定轨谱面」是否按原谱面画长条（Hold）的长度：关掉只画头部 */
  fixedLaneHoldLength: boolean;
  /** 「定轨谱面」是否启用单侧第 3 条轨道：关掉时排不下的按键会叠在最近的轨道上 */
  fixedLaneThird: boolean;
  /** 「游玩分屏」：试玩时开两个窗口，主窗口放原谱面、副窗口放改过的谱面（仅对纯配置 / 定轨谱面生效） */
  playSplitView: boolean;
  bgBlurriness: number;

  maxParticles: number;
  playStartTime: number;
  playEndTime: number | null;

  fade: number;
  alphaTint: boolean;
}

export const DEFAULT_RENDER_CONFIG: RenderConfig = {
  resolution: [1920, 1080],
  endingLength: 0.0,
  renderLoading: false,
  hires: false,
  chartDebugLine: 0.,
  chartDebugLineFields: 'id,parent,height,ulp,z,aui,anchor,kind',
  chartDebugNote: 0.,
  chartDebugNoteFields: 'line,time,height,base,speed,above,fake,angle',
  chartRatio: 1,
  allGood: false,
  allBad: false,
  touchSim: false,
  touchSimWidth: 0.12,
  fps: 60,
  hardwareAccel: true,
  // 高性能模式默认关闭：默认渲染流程保持与从前一致（见 RenderConfig 接口上的注释）
  highPerformance: false,
  hevc: false,
  mpeg4: false,
  customEncoder: null,
  dynamicBitrateControl: true,
  bitrate: '28',

  // 谱面激进优化：默认关闭（打开「按键激进优化 / 粒子激进优化」时会自动跟着打开，见 ConfigView.vue 的 watch）
  aggressiveChart: false,
  aggressiveNote: false,
  aggressiveParticle: false,
  challengeColor: 'rainbow',
  challengeRank: 52,
  noteScale: 1,
  particle: true,
  particleSquare: true,
  // 打击特效反转 / 跟随旋转默认都关；概率给 0.5，打开开关就是「一半翻一半不翻」
  hitFxRandomFlip: false,
  hitFxRandomFlipChance: 0.5,
  hitFxFollowRotation: false,
  playerAvatar: null,
  // 玩家名：填上后端 RenderConfig::default() 的默认值（src-tauri/src/render.rs 里是 "DaogeMM"），
  // 否则界面里这个输入框是空的，而后端不带参数渲染时用的是 DaogeMM
  playerName: 'DaogeMM',
  playerRks: 17.00,
  sampleCount: 8,
  fxaa: false,
  resPackPath: null,
  speed: 1,
  volumeMusic: 0.5,
  volumeSfx: 0.4,
  forceLimit: true,
  limitThreshold: 0.5,
  loudnessEqualization: false,
  audioMixOptimization: true,
  watermark: '',
  notecolor: '',
  rrHitFxColor: '#FBA373',
  rrHitFxScale: 1,
  rrHitFxSpeed: 1,
  hitcolor: '',
  linecolor: '',
  roman: false,
  chinese: false,
  combo: 'AUTOPLAY',
  lvl: 1,
  diff: '',
  difficulty: '',
  judgeOffset: 0,
  fileNameFormat: '%date% %time% %file%',
  renderLine: true,
  renderLineExtra: true,
  renderNote: true,
  renderDoubleHint: true,
  renderPendingNote: false,
  pendingNoteColor: [1, 0.18, 0.18],
  renderUiPause: true,
  renderUiName: true,
  renderUiLevel: true,
  renderUiDifficulty: true,
  renderUiScore: true,
  renderUiCombo: true,
  renderUiBar: true,
  xuejiangSpFakeNotes: 305,
  xuejiangSpFakeBarSeconds: 120,
  xuejiangSpBurstCombo: 240,
  xuejiangSpRollScale: 0.75,
  xuejiangSpDiffFake: 'IN Lv.I2',
  xuejiangSpDiffReal: 'SP Lv.?',
  xuejiangSpVolumePercentile: 0.99,
  xuejiangSpVolumeGamma: 1.05,
  xuejiangSpVolumeSmooth: 0.07,
  xuejiangSpFaceHitStart: 170.7692,
  xuejiangSpFacePeriod: 4.1026,
  xuejiangSpFaceRounds: 4,
  xuejiangSpFaceLeadShow: 3.5897,
  xuejiangSpFaceLeadRoll: 2.8205,
  xuejiangSpFaceTextShow: '(⊙o⊙)',
  xuejiangSpFaceTextRoll: '(⊙ω⊙)',
  xuejiangSpFaceTextHit: 'Ｏ(≧▽≦)Ｏ',
  baoyingSpWowHeight: 0.043,
  baoyingSpWowOffsetX: 0.02,
  baoyingSpWowOffsetY: 0.02,
  baoyingSpTitleHeight: 0.056,
  baoyingSpTitleOffsetX: 0.03,
  baoyingSpTitleOffsetY: 0,
  baoyingSpScoreOffsetX: 0,
  baoyingSpScoreOffsetY: 0.01,
  baoyingSpScoreHeight: 0.05,
  baoyingSpScoreGap: 0.003,
  baoyingSpScoreSqueeze: 1.1,
  baoyingSpComboOffsetX: 0,
  baoyingSpComboOffsetY: 0.01,
  baoyingSpComboHeight: 0.055,
  baoyingSpComboGap: 0,
  baoyingSpComboSqueeze: 1.1,
  baoyingSpComboLabelHeight: 0.055,
  baoyingSpComboLabelOffsetX: 0,
  baoyingSpComboLabelOffset: 0.01,
  baoyingSpDiffHeight: 0.05,
  baoyingSpDiffGapSpLv: 0.1,
  baoyingSpDiffGapLvQuestion: 0,
  baoyingSpDiffSqueeze: 1.2,
  baoyingSpDiffOffsetX: -0.05,
  baoyingSpDiffOffsetY: 0.029,
  baoyingSpDigitsRtl: true,
  baoyingSpHitFxScale: 0.5,
  baoyingSpHitFxRotate: false,
  baoyingSpParticleCount: 0,
  jingluanSpChanHeight: 0.99,
  jingluanSpChanOffsetY: -0.1,
  jingluanSpChanFadeIn: 0,
  jingluanSpChanFadeOut: 0,
  jingluanSpGuangHeight: 1.0,
  jingluanSpGuangOffsetY: 0,
  jingluanSpGuangFadeIn: 0,
  jingluanSpGuangFadeOut: 0,
  jingluanSpMopemopeHeight: 1.0,
  jingluanSpMopemopeOffsetY: 0,
  jingluanSpMopemopeFadeIn: 0,
  jingluanSpMopemopeFadeOut: 0,
  jingluanSpDim: 0.18,
  mopemopeSpColorA: [0.4118, 0.298, 0.1412],
  mopemopeSpColorB: [0.3412, 0.2314, 0.0588],
  qzkagoSpStart: 77.07,
  qzkagoSpWhiteIn: 65 / 60,
  qzkagoSpWhiteOut: 30 / 60,
  clockParadoxSpScale: 1,
  clockParadoxSpOffsetX: 0,
  clockParadoxSpOffsetY: 0,
  clockParadoxSpFlash1On: 108.020,
  clockParadoxSpFlash1Fade: 108.453,
  clockParadoxSpFlash2On: 109.720,
  clockParadoxSpFlash2Fade: 110.170,
  clockParadoxSpFlash3On: 111.42,
  clockParadoxSpFlash3Fade: 111.870,
  clockParadoxSpFlash4On: 142.287,
  clockParadoxSpFlash4Fade: 145.077,
  renderBg: true,
  renderBgDim: true,
  backgroundDim: 0.1,
  // 谱面侧渲染参数：默认值与 RenderView 原来的 CHART_ROW_DEFAULTS 一致（导入谱面不再覆盖它们）
  forceAspectRatio: true,
  foldAnimation: true,
  aspectRatio: 16 / 9,
  noteUniformScale: false,
  holdPartialCover: false,
  negativeLengthHold: false,
  holdParticleIntervalRatio: 1,
  lineLength: 6,
  scoreTotal: 1000000,
  previewStart: 0,
  previewEnd: 0,
  offset: 0,
  renderBlockArea: true,
  simplifiedBlockArea: false,
  blockAreaHover: true,
  blockAreaHoverRadius: 0.5,
  simplifiedBlockAreaText: false,
  blockAreaReuse: false,
  blockAreaLava: false,
  waterScreen: false,
  waterOpacity: 0.5,
  waterFlowSpeed: 0.15,
  waterCellScale: 16.,
  waterHp: 1.,
  waterDrownDelay: 1.,
  waterDamage: 1.,
  waterDamageInterval: 1.,
  waterDeathTime: 0.5,
  waterDeathParticles: 8.,
  waterSfxVolume: 0.5,
  waterSfxMax: 500.,
  blockAreaForcePosition: false,
  blockAreaForceHeight: 0.2,
  blockAreaForceWidth: 1.9,
  blockAreaForceShow: false,
  blockAreaForceTime: 3.0,
  blockAreaForceDelay: 0.3,
  blockAreaForceInvert: false,
  blockAreaColor: [0.9, 0.1, 0.1],
  blockAreaNormalColor: [0.9, 0.1, 0.1],
  simplifiedBlockAreaTextColor: [1, 0.3765, 0.5804],
  simplifiedBlockAreaTextPaleColor: [1, 1, 1],
  blockAreaAlpha: 0.25,
  blockAreaDensity: 2.5,
  blockAreaSpark: true,
  blockAreaNoiseBlotch: true,
  blockAreaDisplaceStrength: 0.15,
  blockAreaPixelScale: 6.0,
  blockAreaDisplaceSpeed: 1.5,
  blockAreaNoiseRadius: 0.48,
  blockAreaNoiseSmoothness: 1.0,
  blockAreaEdgeNoise: 1.0,
  directBlockHollow: false,
  blockAreaDistort: true,
  // 判定范围：默认关闭（与 src-tauri/src/render.rs 的 RenderConfig::default() 和 config.toml 一致）
  renderJudgeRange: false,
  judgeRangeWidth: 1.9,
  // Flick / Drag 的半宽 = 该值 × (屏幕高度 / 10)，与 Tap / Hold 同一单位（略宽于 1.9）
  judgeRangeWidthFlick: 2.1,
  judgeRangeLength: 5.0,
  // 判定范围：显现时间 1.0s、淡入结束时间 0.3s（界面上的「过渡时间」= 显现时间 − 淡入结束时间 = 0.7）
  judgeRangeTime: 1.0,
  judgeRangeFadeTime: 0.3,
  judgeRangeAlpha: 0.1,
  judgeRangeColor: [1, 1, 1],
  // 判定范围被噪域遮蔽：默认开（官方行为）
  judgeRangeNoiseOcclusion: true,
  preserveFramebuffer: false,
  renderExtra: true,
  pureConfigChart: false,
  pureConfigChartLineY: 0.1,
  pureConfigChartSpeed: 2.8,
  pureConfigChartSideLines: true,
  pureConfigChartSideX: 0.05,
  pureConfigChartAngle: 45,
  pureConfigChartNoise: true,
  pureConfigChartNoiseHeight: 0.1,
  pureConfigChartNoiseWidth: 1.1,
  pureConfigChartNoiseTime: 0.13,
  pureConfigChartNoiseDelay: 0,
  // 定轨谱面：默认关闭
  fixedLaneChart: false,
  fixedLaneGap: 0.14,
  fixedLaneSpeed: 2.8,
  fixedLaneLineY: 0.1,
  fixedLaneInterval: 0.2,
  fixedLaneHoldLength: true,
  fixedLaneThird: false,
  playSplitView: false,
  bgBlurriness: 80,

  maxParticles: 5000,
  playStartTime: 0.0,
  playEndTime: null,

  fade: 0.0,
  alphaTint: false,
};

export interface AppConfig {
  rpeDir: string | null;
  outputDir: string | null;
  encoderAvc: string | null;
  encoderHevc: string | null;
  ffmpegPath: string | null;
  printStderr: boolean;
  showDetailedLog: boolean;
  enablePlayButton: boolean;
  previewFullscreen: boolean;
  /** 高性能模式（默认关闭）：只影响渲染进程**怎么跑**，不改画面。
   *  勾选后提交渲染 / 游玩时由后端套用到那个渲染进程上：Windows 下优先挑高性能显卡
   *  （独显），并在用上硬件编码器时让 GPU 的帧读回直接给出翻转好的画面、省掉 ffmpeg 的
   *  `-vf vflip`。它是「这台机器该怎么跑」的偏好，所以住在 app 配置里而不是每份渲染配置里。 */
  highPerformance: boolean;
}

export const DEFAULT_APP_CONFIG: AppConfig = {
  rpeDir: null,
  outputDir: null,
  encoderAvc: null,
  encoderHevc: null,
  ffmpegPath: null,
  printStderr: false,
  showDetailedLog: true,
  enablePlayButton: true,
  previewFullscreen: true,
  // 高性能模式默认关闭：默认渲染流程保持与从前一致
  highPerformance: false,
}

export interface RPEChart {
  name: string;
  id: string;
  path: string;
  illustration: string;
  charter: string;
}

export type Assets = {
  browser_download_url: string,
  name: string,
}

export type Release = {
  id: number,
  assets: Assets[],
  tag_name: string,
  body: string,
  message: string, // error message
};

export interface RenderChart {
  id: number;
  path: string;
  isSelect: boolean;
  chartInfo: ChartInfo;

  taskId: number | null;
  output: string;
  status: TaskStatus;
}

export interface Preset {
  name: string;
  key: string;
  config: RenderConfig;
}

export interface FileDropEvent {
  paths: string[];
  position: { x: number; y: number };
}
