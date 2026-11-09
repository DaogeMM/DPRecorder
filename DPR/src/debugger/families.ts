/**
 * DPDebugger 右侧那八套演出动画的参数表（与 `assets/chart-animation.toml` 的八张表一一对应）。
 *
 * 这里只放「界面怎么摆、叫什么、写回 TOML 时键是什么」这些静态描述 —— 数值始终从
 * `RenderConfig` 取（初值由主窗口传进来，默认值由 `DEFAULT_RENDER_CONFIG` 提供）。
 * 加一项参数：先在 `assets/chart-animation.toml` 与 `dps::config::Config` 里加上这个键，
 * 再在下面找到对应的组，按 `FieldSpec` 补一行。
 */

/**
 * 一个参数行。
 *
 * `kind` 决定控件：`number` 是 `f32`/`f64`、`int` 是整数、`bool` 是开关、
 * `text` / `color` 是一行文本（`color` 本来存的就是 `#RRGGBB`）、
 * `color3` 是把 `[r, g, b]`（0~1）用 `#RRGGBB` 显示的三个分量。
 */
export interface FieldSpec {
  /** `RenderConfig` 上的字段名（camelCase），也是 `v-model` 的键 */
  key: string;
  /** 界面上显示的名字 */
  label: string;
  /** 单位 / 取值范围，显示在名字下面 */
  hint?: string;
  kind?: 'number' | 'int' | 'bool' | 'text' | 'color' | 'color3';
  step?: number;
  min?: number;
  max?: number;
}

export interface GroupSpec {
  title: string;
  fields: FieldSpec[];
}

export interface FamilySpec {
  title: string;
  /** `assets/chart-animation.toml` 里的表名 */
  table: string;
  /** `RenderConfig` 字段的前缀，写回 TOML 时据此把键名削成表里的短名 */
  prefix: string;
  /** 打开就跳到这里（秒）：这段演出出现的位置，省得手动拖进度条 */
  start: number;
  groups: GroupSpec[];
}

export const FAMILIES = {
  // ── 「雪降」：伪装分数、音量条、表情 ────────────────────────────────────────
  xuejiang: {
    title: '雪降 SP',
    table: 'xuejiang',
    prefix: 'xuejiangSp',
    start: 150,
    groups: [
      {
        title: '伪装分数',
        fields: [
          { key: 'xuejiangSpFakeNotes', label: '伪装音符数', hint: '分数按 combo ÷ 该数换算', kind: 'number', min: 1, step: 1 },
          { key: 'xuejiangSpFakeBarSeconds', label: '进度条走满秒数', hint: '按音乐时间匀速走满', kind: 'number', min: 1, step: 1 },
          { key: 'xuejiangSpBurstCombo', label: '揭开伪装的 COMBO', kind: 'int', min: 1, step: 1 },
          { key: 'xuejiangSpRollScale', label: '分数倒滚倍率', hint: '1 = 滚到下一个音符，0 = 立刻跳真实分', kind: 'number', min: 0, max: 2, step: 0.05 },
          { key: 'xuejiangSpDiffFake', label: '伪装难度文本', kind: 'text' },
          { key: 'xuejiangSpDiffReal', label: '真实难度文本', kind: 'text' },
        ],
      },
      {
        title: '音量条',
        fields: [
          { key: 'xuejiangSpVolumePercentile', label: '归一化分位', hint: '0~1', kind: 'number', min: 0, max: 1, step: 0.01 },
          { key: 'xuejiangSpVolumeGamma', label: '响应指数', hint: '1 线性，2 平方', kind: 'number', min: 0.1, max: 4, step: 0.05 },
          { key: 'xuejiangSpVolumeSmooth', label: '平滑时间常数', hint: '秒', kind: 'number', min: 0, max: 1, step: 0.01 },
        ],
      },
      {
        title: '表情',
        fields: [
          { key: 'xuejiangSpFaceHitStart', label: '首次判定时刻', hint: '秒', kind: 'number', step: 0.1 },
          { key: 'xuejiangSpFacePeriod', label: '轮次间隔', hint: '秒', kind: 'number', step: 0.1 },
          { key: 'xuejiangSpFaceRounds', label: '轮数', hint: '0 = 不显示', kind: 'int', min: 0, step: 1 },
          { key: 'xuejiangSpFaceLeadShow', label: '提前展示', hint: '秒', kind: 'number', step: 0.1 },
          { key: 'xuejiangSpFaceLeadRoll', label: '提前滚动', hint: '秒', kind: 'number', step: 0.1 },
          { key: 'xuejiangSpFaceTextShow', label: '展示阶段文本', kind: 'text' },
          { key: 'xuejiangSpFaceTextRoll', label: '滚动阶段文本', kind: 'text' },
          { key: 'xuejiangSpFaceTextHit', label: '判定阶段文本', kind: 'text' },
        ],
      },
    ],
  },

  // ── 「报应」：手绘贴图的大小 / 偏移 / 间距 ──────────────────────────────────
  baoying: {
    title: '报应 SP',
    table: 'baoying',
    prefix: 'baoyingSp',
    start: 0,
    groups: [
      {
        title: 'WOW! 贴图',
        fields: [
          { key: 'baoyingSpWowHeight', label: '高度', kind: 'number', min: 0, step: 0.001 },
          { key: 'baoyingSpWowOffsetX', label: '右移', kind: 'number', step: 0.001 },
          { key: 'baoyingSpWowOffsetY', label: '下移', kind: 'number', step: 0.001 },
        ],
      },
      {
        title: '曲名',
        fields: [
          { key: 'baoyingSpTitleHeight', label: '高度', kind: 'number', min: 0, step: 0.001 },
          { key: 'baoyingSpTitleOffsetX', label: '右移', kind: 'number', step: 0.001 },
          { key: 'baoyingSpTitleOffsetY', label: '下移', kind: 'number', step: 0.001 },
        ],
      },
      {
        title: '分数',
        fields: [
          { key: 'baoyingSpScoreOffsetX', label: '右移', kind: 'number', step: 0.001 },
          { key: 'baoyingSpScoreOffsetY', label: '下移', kind: 'number', step: 0.001 },
          { key: 'baoyingSpScoreHeight', label: '数字高度', kind: 'number', min: 0, step: 0.001 },
          { key: 'baoyingSpScoreGap', label: '数字间距', kind: 'number', step: 0.001 },
          { key: 'baoyingSpScoreSqueeze', label: '横向挤压', hint: '1 原始比例，小于 1 变窄', kind: 'number', min: 0, step: 0.01 },
        ],
      },
      {
        title: '连击数',
        fields: [
          { key: 'baoyingSpComboOffsetX', label: '右移', kind: 'number', step: 0.001 },
          { key: 'baoyingSpComboOffsetY', label: '下移', kind: 'number', step: 0.001 },
          { key: 'baoyingSpComboHeight', label: '数字高度', kind: 'number', min: 0, step: 0.001 },
          { key: 'baoyingSpComboGap', label: '数字间距', kind: 'number', step: 0.001 },
          { key: 'baoyingSpComboSqueeze', label: '横向挤压', kind: 'number', min: 0, step: 0.01 },
        ],
      },
      {
        title: 'COMBO 贴纸',
        fields: [
          { key: 'baoyingSpComboLabelHeight', label: '高度', kind: 'number', min: 0, step: 0.001 },
          { key: 'baoyingSpComboLabelOffsetX', label: '右移', kind: 'number', step: 0.001 },
          { key: 'baoyingSpComboLabelOffset', label: '与数字的间距', kind: 'number', step: 0.001 },
        ],
      },
      {
        title: '难度',
        fields: [
          { key: 'baoyingSpDiffHeight', label: '高度', kind: 'number', min: 0, step: 0.001 },
          { key: 'baoyingSpDiffGapSpLv', label: 'SP—LV. 间距', kind: 'number', step: 0.001 },
          { key: 'baoyingSpDiffGapLvQuestion', label: 'LV.—? 间距', kind: 'number', step: 0.001 },
          { key: 'baoyingSpDiffSqueeze', label: '横向挤压', kind: 'number', min: 0, step: 0.01 },
          { key: 'baoyingSpDiffOffsetX', label: '右移', kind: 'number', step: 0.001 },
          { key: 'baoyingSpDiffOffsetY', label: '下移', kind: 'number', step: 0.001 },
        ],
      },
      {
        title: '数字与打击特效',
        fields: [
          { key: 'baoyingSpDigitsRtl', label: '数字从右往左排', kind: 'bool' },
          { key: 'baoyingSpHitFxScale', label: '特效贴图缩放', hint: '≤ 0 沿用资源包', kind: 'number', step: 0.01 },
          { key: 'baoyingSpHitFxRotate', label: '特效随判定线旋转', kind: 'bool' },
          { key: 'baoyingSpParticleCount', label: '粒子数', hint: '0 = 不画', kind: 'int', min: 0, step: 1 },
        ],
      },
    ],
  },

  // ── 「痉挛」：三段插画 ──────────────────────────────────────────────────────
  jingluan: {
    title: '痉挛 SP',
    table: 'jingluan',
    prefix: 'jingluanSp',
    start: 0,
    groups: [
      {
        title: '第一段 SpasmodicChan',
        fields: [
          { key: 'jingluanSpChanHeight', label: '高度占比', kind: 'number', min: 0, step: 0.01 },
          { key: 'jingluanSpChanOffsetY', label: '上下偏移', hint: '正数向下', kind: 'number', step: 0.01 },
          { key: 'jingluanSpChanFadeIn', label: '淡入', hint: '秒，0 = 直接出现', kind: 'number', min: 0, step: 0.01 },
          { key: 'jingluanSpChanFadeOut', label: '淡出', hint: '秒，0 = 到点消失', kind: 'number', min: 0, step: 0.01 },
        ],
      },
      {
        title: '第二段 光',
        fields: [
          { key: 'jingluanSpGuangHeight', label: '高度占比', kind: 'number', min: 0, step: 0.01 },
          { key: 'jingluanSpGuangOffsetY', label: '上下偏移', kind: 'number', step: 0.01 },
          { key: 'jingluanSpGuangFadeIn', label: '淡入', hint: '秒', kind: 'number', min: 0, step: 0.01 },
          { key: 'jingluanSpGuangFadeOut', label: '淡出', hint: '秒', kind: 'number', min: 0, step: 0.01 },
        ],
      },
      {
        title: '第三段 もぺもぺ',
        fields: [
          { key: 'jingluanSpMopemopeHeight', label: '高度占比', kind: 'number', min: 0, step: 0.01 },
          { key: 'jingluanSpMopemopeOffsetY', label: '上下偏移', kind: 'number', step: 0.01 },
          { key: 'jingluanSpMopemopeFadeIn', label: '淡入', hint: '秒', kind: 'number', min: 0, step: 0.01 },
          { key: 'jingluanSpMopemopeFadeOut', label: '淡出', hint: '秒', kind: 'number', min: 0, step: 0.01 },
        ],
      },
      {
        title: '压暗',
        fields: [{ key: 'jingluanSpDim', label: '压暗程度', hint: '0 不压暗，1 全黑', kind: 'number', min: 0, max: 1, step: 0.01 }],
      },
    ],
  },

  // ── QZKAGO：整屏静图闪现 ────────────────────────────────────────────────────
  qzkago: {
    title: 'QZKAGO',
    table: 'qzkago',
    prefix: 'qzkagoSp',
    start: 77.07,
    groups: [
      {
        title: '静图闪现',
        fields: [
          { key: 'qzkagoSpStart', label: '起点', hint: '秒', kind: 'number', step: 0.01 },
          { key: 'qzkagoSpWhiteIn', label: '渐入时长', hint: '秒，0 = 硬切', kind: 'number', min: 0, step: 0.01 },
          { key: 'qzkagoSpWhiteOut', label: '渐出时长', hint: '秒，整段总长跟着它走', kind: 'number', min: 0, step: 0.01 },
        ],
      },
    ],
  },

  // ── Clock Paradox：正中大表盘 ───────────────────────────────────────────────
  clockParadox: {
    title: 'Clock Paradox',
    table: 'clockParadox',
    prefix: 'clockParadoxSp',
    start: 108.02,
    groups: [
      {
        title: '位置',
        fields: [
          { key: 'clockParadoxSpScale', label: '大小倍率', kind: 'number', min: 0, step: 0.01 },
          { key: 'clockParadoxSpOffsetX', label: '圆心 X 偏移', hint: '按 1920×1080 的像素', kind: 'number', step: 1 },
          { key: 'clockParadoxSpOffsetY', label: '圆心 Y 偏移', hint: '正值向上', kind: 'number', step: 1 },
        ],
      },
      {
        title: '亮起时刻',
        fields: [
          { key: 'clockParadoxSpFlash1On', label: '第 1 次', hint: '秒', kind: 'number', step: 0.01 },
          { key: 'clockParadoxSpFlash2On', label: '第 2 次', hint: '秒', kind: 'number', step: 0.01 },
          { key: 'clockParadoxSpFlash3On', label: '第 3 次', hint: '秒', kind: 'number', step: 0.01 },
          { key: 'clockParadoxSpFlash4On', label: '第 4 次', hint: '秒', kind: 'number', step: 0.01 },
        ],
      },
      {
        title: '淡出时刻',
        fields: [
          { key: 'clockParadoxSpFlash1Fade', label: '第 1 次', hint: '秒', kind: 'number', step: 0.01 },
          { key: 'clockParadoxSpFlash2Fade', label: '第 2 次', hint: '秒', kind: 'number', step: 0.01 },
          { key: 'clockParadoxSpFlash3Fade', label: '第 3 次', hint: '秒', kind: 'number', step: 0.01 },
          { key: 'clockParadoxSpFlash4Fade', label: '第 4 次', hint: '秒', kind: 'number', step: 0.01 },
        ],
      },
    ],
  },

  // ── 「もぺもぺ」：整屏红色压暗 ──────────────────────────────────────────────
  mopemope: {
    title: 'もぺもぺ',
    table: 'mopemope',
    prefix: 'mopemopeSp',
    start: 60,
    groups: [
      {
        title: '整屏颜色（RGB 0~1）',
        fields: [
          { key: 'mopemopeSpColorA', label: '闪屏颜色', hint: '区间首尾与频闪段', kind: 'color3' },
          { key: 'mopemopeSpColorB', label: '长镜头颜色', hint: '三段长镜头，更暗更纯', kind: 'color3' },
        ],
      },
    ],
  },

  // ── Message SP：七趟整屏后处理 + 方块粒子 ──────────────────────────────────
  message: {
    title: 'Message SP',
    table: 'message',
    prefix: 'message',
    // 这一族的变动全挤在 75 秒之后，108.9 秒正好是「拍暗 + 饱和清零」那一刻，
    // 从这里往后拖能连着看到黑场、白场、万花筒三段。
    start: 108.9,
    groups: [
      {
        title: '总览',
        fields: [
          { key: 'messageTimeline', label: '时间轴', hint: '关掉就全部用下面的固定值', kind: 'bool' },
          { key: 'messageTimeOffset', label: '整体平移', hint: '秒，正 = 演出更晚', kind: 'number', step: 0.01 },
          { key: 'messageBgHideTime', label: '藏起背景', hint: '秒，0 = 不藏', kind: 'number', step: 0.01 },
        ],
      },
      {
        title: '各趟开关',
        fields: [
          { key: 'messagePassKaleido', label: '万花筒', kind: 'bool' },
          { key: 'messagePassVignette', label: '暗角', kind: 'bool' },
          { key: 'messagePassGlitch', label: '故障', kind: 'bool' },
          { key: 'messagePassBand', label: '光带', kind: 'bool' },
          { key: 'messagePassLens', label: '镜头畸变', kind: 'bool' },
          { key: 'messagePassEsc', label: '曝光 / 饱和', kind: 'bool' },
          { key: 'messagePassBloom', label: '泛光', kind: 'bool' },
          { key: 'messagePassParticles', label: '方块粒子', kind: 'bool' },
        ],
      },
      {
        title: '暗角',
        fields: [
          { key: 'messageVignetteSmoothness', label: '过渡宽度', kind: 'number', min: 0, step: 0.01 },
          { key: 'messageVignetteRadius', label: '压暗半径', hint: '负值 = 整屏压黑', kind: 'number', step: 0.01 },
          { key: 'messageVignetteDarkness', label: '压暗强度', hint: '大于 1 会反相', kind: 'number', min: 0, step: 0.01 },
          { key: 'messageVignetteNoise', label: '噪点亮度', kind: 'number', min: 0, step: 0.01 },
          { key: 'messageVignetteLine', label: '横线亮度', kind: 'number', min: 0, step: 0.01 },
          { key: 'messageVignetteStrength', label: '时间轴倍率', kind: 'number', min: 0, step: 0.01 },
        ],
      },
      {
        title: '镜头畸变',
        fields: [
          { key: 'messageLensSize', label: '缩放半径', hint: '越大畸变越弱', kind: 'number', min: 1, step: 1 },
          { key: 'messageLensConvergence', label: '收敛量', hint: '负值向内缩', kind: 'number', step: 0.1 },
          { key: 'messageLensEdge', label: '边缘羽化', kind: 'number', min: 0, step: 0.001 },
          { key: 'messageLensAspect', label: '宽高比', kind: 'number', min: 0.01, step: 0.01 },
        ],
      },
      {
        title: '故障',
        fields: [
          { key: 'messageGlitchRange', label: '位移幅度', kind: 'number', min: 0, step: 0.001 },
          { key: 'messageGlitchStrength', label: '时间轴倍率', kind: 'number', min: 0, step: 0.01 },
        ],
      },
      {
        title: '光带',
        fields: [
          { key: 'messageBandWidth', label: '带宽', hint: '1 = 整屏反相', kind: 'number', min: 0, step: 0.01 },
          { key: 'messageBandSmoothness', label: '带边羽化', kind: 'number', min: 0, step: 0.001 },
          { key: 'messageBandBrightness', label: '亮度', kind: 'number', step: 0.01 },
          { key: 'messageBandVertical', label: '竖带', kind: 'bool' },
        ],
      },
      {
        title: '万花筒',
        fields: [
          { key: 'messageKaleidoMix', label: '混合比例', hint: '0 = 全用原画面', kind: 'number', min: 0, max: 1, step: 0.01 },
          { key: 'messageKaleidoStrength', label: '时间轴倍率', kind: 'number', min: 0, step: 0.01 },
          { key: 'messageKaleidoContrast', label: '贴图对比度', kind: 'number', step: 0.01 },
          { key: 'messageKaleidoSaturation', label: '贴图饱和度', kind: 'number', min: 0, step: 0.1 },
          { key: 'messageKaleidoHue', label: '色相偏移', hint: '0~1 是一整圈', kind: 'number', step: 0.01 },
          { key: 'messageKaleidoCenter', label: '中心半径', kind: 'number', min: 0, step: 0.01 },
          { key: 'messageKaleidoSmoothness', label: '中心羽化', kind: 'number', min: 0, step: 0.01 },
        ],
      },
      {
        title: '曝光 / 泛光',
        fields: [
          { key: 'messageExposure', label: '亮度', kind: 'number', min: 0, step: 0.01 },
          { key: 'messageSaturation', label: '饱和度', kind: 'number', min: 0, step: 0.01 },
          { key: 'messageContrast', label: '对比度', kind: 'number', min: 0, step: 0.01 },
          { key: 'messageBloomBrightness', label: '泛光亮度', hint: '0 = 不跑这一趟', kind: 'number', min: 0, step: 0.01 },
          { key: 'messageBloomStrength', label: '泛光倍率', kind: 'number', min: 0, step: 0.01 },
          { key: 'messageBloomIntensity', label: '合成强度', hint: '大于 1 更糊', kind: 'number', min: 0, step: 0.01 },
          { key: 'messageBloomThreshold', label: '亮部阈值', kind: 'number', min: 0, max: 1, step: 0.001 },
          { key: 'messageBloomRadius', label: '采样半径', kind: 'number', min: 0, step: 0.1 },
        ],
      },
      {
        title: '方块粒子',
        fields: [
          { key: 'messageParticleStart', label: '出场时刻', hint: '秒', kind: 'number', step: 0.01 },
          { key: 'messageParticleLifetime', label: '存活秒数', hint: '秒', kind: 'number', min: 0.1, step: 0.1 },
          { key: 'messageParticleCount', label: '每层个数', kind: 'int', min: 0, max: 400, step: 1 },
          { key: 'messageParticleSize', label: '边长', hint: '按画面高度的一半计', kind: 'number', min: 0, step: 0.005 },
          { key: 'messageParticleSpeed', label: '上飘速度', hint: '同上，每秒', kind: 'number', step: 0.01 },
          { key: 'messageParticleHideY', label: '淡出起点', hint: '0 = 底边、1 = 顶边', kind: 'number', min: 0, max: 1, step: 0.01 },
          { key: 'messageParticleHideSmoothness', label: '淡出羽化', kind: 'number', min: 0, step: 0.01 },
          { key: 'messageParticleGlow', label: '光晕', kind: 'bool' },
          { key: 'messageParticleOpacity', label: '透明度', kind: 'number', min: 0, max: 1, step: 0.01 },
        ],
      },
    ],
  },

  // ── Rr：打击特效 ───────────────────────────────────────────────────────────
  rr: {
    title: 'Rr',
    table: 'rr',
    prefix: 'rr',
    start: 0,
    groups: [
      {
        title: '打击特效',
        fields: [
          { key: 'rrHitFxColor', label: '颜色覆盖', hint: '留空用 hitcolor', kind: 'color' },
          { key: 'rrHitFxScale', label: '缩放', hint: '乘在资源包的值上', kind: 'number', min: 0, step: 0.01 },
          { key: 'rrHitFxSpeed', label: '动画速度', hint: '2 = 快一倍', kind: 'number', min: 0, step: 0.01 },
        ],
      },
    ],
  },
} satisfies Record<string, FamilySpec>;
