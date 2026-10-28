/**
 * 配置层：类型定义。
 *
 * 这一层的目的是把「新增一个渲染参数」从「改 8 处、漏一处静默失效」压到「改 1 处」：
 *   - 一个参数的**全部**知识都写在 ./fields.ts 的一条 FieldSpec 里
 *     （默认值/类型来自 ../model 的 RenderConfig，界面标签、提示、校验、可用性、
 *      界面等级、所属分区、界面值与配置值的换算也都在这里）。
 *   - ref 状态、buildConfig()、applyConfig() 全部由这份规格表生成（见 ./useRenderConfig.ts）。
 *   - 界面控件从规格里取标签/提示/校验/可用性（见 ./ConfigField.vue）。
 *
 * 依赖方向： sections/* → config/* → design/* + model.ts
 * 反向依赖（config 里 import 某个 section）是不允许的。
 */
import type { RULES } from '../common';
import type { RenderConfig } from '../model';

/**
 * 内容区里的分区 id。
 * 同时是左侧导航的一级子项、内容区里的锚点元素 id、以及 App.vue 里 renderSections 的 id。
 */
export const SECTION_IDS = [
  'preset',
  'output',
  'player',
  'audio',
  'respack',
  'chart-info',
  'render-item',
  'chart',
  'judge',
  'block-area',
  'water-screen',
  'performance',
] as const;
export type SectionId = (typeof SECTION_IDS)[number];

/** 界面等级：1 基础 / 2 进阶（默认）/ 3 专家。只影响显示，不写进渲染配置、不影响出片。 */
export type UiLevel = 1 | 2 | 3;

/**
 * 渲染页的「工作配置」：界面上的那份配置，外加它在下拉框里叫什么、算不算改过。
 *
 * 渲染页的改动只存在浏览器本地（`RenderView.Config`），**绝不写进任何命名预设** ——
 * 预设那边只是把下拉框里的名字显示成 `*原名`。切页会整页销毁，所以要把
 * 「值 + 名字 + 星号」一起带走，回来才能连星号一起还原（见 `ConfigView` 的 `restoreWorking`）。
 */
export type WorkingState = {
  config: RenderConfig;
  presetKey: string;
  presetName: string;
  dirty: boolean;
};

/**
 * 控件形态。
 *
 * 'derived' 表示这个配置键在界面上**没有独立控件**，值由别的界面项换算出来
 * （例：hevc / mpeg4 / customEncoder 三个键都由「编码器」一个下拉决定）。
 * 这类键的 toConfig 必须显式给，界面上也不能 v-model 它们。
 */
export type FieldWidget =
  | 'checkbox' // v-checkbox（开关类配置一律用勾选框，全项目不再有 TipSwitch）
  | 'number' // v-text-field type="number"
  | 'text' // v-text-field type="text"
  | 'textarea' // v-textarea rows="1" auto-grow
  | 'color' // HEX 文本框 ↔ 配置里的 [r, g, b]（0~1）
  | 'autocomplete' // v-autocomplete（只能从 items 里选）
  | 'combobox' // v-combobox（可以自由输入）
  | 'slider' // v-slider
  | 'avatar' // 只读输入框 + 点击选文件
  | 'derived'; // 没有控件

/** 校验规则名：common.ts 里的 RULES，加上依赖 i18n 的四个本地规则（见 FieldContext.rules） */
export type RuleName = keyof typeof RULES | 'resolution' | 'sampleCount' | 'crf' | 'bitrate';

/**
 * 界面状态的口袋（宽松类型，只在 fields.ts 的 toConfig/toUi 里用）。
 * 对外暴露的强类型版本是 ./useRenderConfig.ts 的 PhiConfigUi —— 那边由规格表推导。
 */
export type UiState = Record<string, any>;

/** 运行时上下文：由 useRenderConfig() 注入（i18n、下拉项、本地校验器都在这里） */
export interface FieldContext {
  t: (key: string, ...args: any[]) => string;
  encoderList: string[];
  antiAliasing: { title: string; value: string }[];
  resolutionList: string[];
  fpsList: string[];
  speedList: string[];
  bitrateList: string[];
  bitrateCrfList: string[];
  maxParticlesTextList: string[];
  maxParticlesList: number[];
  challengeColors: string[];
  judgeModes: string[];
  /** 谱面修改的键型名（点按 / 拖动 / 长按 / 滑动），与 STD_NOTE_TYPES 下标一一对应 */
  noteTypes: string[];
  respacks: RespackItem[];
  /** 依赖 i18n 的本地校验器：resolution / sampleCount / crf / bitrate */
  rules: Record<string, (value: any) => true | string>;
}

export interface RespackItem {
  name: string;
  path: string | null;
  index: number;
  /**
   * 包能不能用：`respack/` 里的 zip 在、且必需文件齐全。
   *
   * 全是用户自备的皮肤包，缺了或里面缺文件时照样列在下拉框里（否则没法先选中再修），
   * 只是这一项为 false：界面标个图标，点预览 / 渲染时再提示下载。
   */
  present?: boolean;
  /** 包里缺哪些文件（齐全时为空），用来在提示里点名。 */
  missing?: string[];
  /**
   * 包里带不带按键皮肤（`SKIN_FILES` 齐全）。
   *
   * 界面在资源包那一栏点一个按键图标：带就亮到原色，不带（或缺文件）压成淡色。
   * 内置的「默认」项自带按键贴图，恒为 true。
   */
  keys?: boolean;
  /**
   * 包里带不带**完整的**噪域皮肤（`noise_on.*` + `noise_off.*` 两档齐全）。
   *
   * 带了就同时当「噪域皮肤」用：未激活 / 已激活两种状态分别用包里那张图平铺。缺一档就整包
   * 当没带，照旧用官方噪域；界面只标一个图标，不拦选择。
   */
  noise?: boolean;
}

/**
 * 一个渲染参数的完整规格。
 *
 * V 是这个键在 RenderConfig 里的类型（由 FieldTable 约束）。
 * 值的换算默认按 widget 推：
 *   - widget 'number'：界面存字符串，配置存数字（uiValue: 'number' 时界面也存数字，见下）
 *   - widget 'color' ：界面存 HEX 文本，配置存 [r, g, b]
 *   - 其余：界面与配置同型
 * 只有换算不常规的键才需要自己写 toConfig / toUi。
 */
export interface FieldSpec<V = any> {
  widget: FieldWidget;
  /** i18n 标签键（相对于 locales 里的 config.json）。'derived' 可以不给。 */
  label?: string;
  /** i18n 提示键（hover 时问号/标题里显示的那句）。 */
  tip?: string;
  /** 校验规则。顺序即校验顺序。 */
  rules?: RuleName[];
  /** 下拉项。给了函数就每次渲染时求值（i18n 会跟着语言切换）。 */
  items?: readonly any[] | ((ctx: FieldContext) => readonly any[]);
  min?: number;
  max?: number;
  step?: number;
  /** 数字型字段：true 用 parseInt，false（默认）用 parseFloat。 */
  integer?: boolean;
  /** 界面等级，默认 1。 */
  level?: UiLevel;
  /** 所属分区，由 fields.ts 的分区结构自动填，不用手写。 */
  section?: SectionId;
  /** 控件是否可用。拿到的是整个界面状态，所以能写整条上级链（例 `!renderBg || !renderBgDim`）。 */
  enabled?: (ui: UiState) => boolean;
  /** 界面状态里用的键名，默认与配置键同名。 */
  uiKey?: string;
  /**
   * 数字/布尔类字段在界面上的存法：
   *   'string' —— 界面存字符串（v-text-field type="number" 直接 v-model，保存时 Number 转换）
   *   'number' —— 界面存数字，输入非法时**不覆盖**（保留上一个有效值）
   * 不填时按 widget 推（number → 'string'，其余同配置类型）。
   */
  uiValue?: 'string' | 'number' | 'boolean';
  /** false = applyConfig 不写这个键的界面状态（键在界面上没有对应控件、又有意保留现状时用）。 */
  syncToUi?: boolean;
  /** 界面值 → 配置值。不填按 widget 推。 */
  toConfig?: (ui: UiState, ctx: FieldContext) => V;
  /** 配置值 → 界面值。不填按 widget 推；返回值就是要写进界面的东西。 */
  toUi?: (value: V, config: RenderConfig, ctx: FieldContext) => unknown;
  /** 写给后来人的说明：这个键由哪个控件驱动 / 有什么坑。 */
  note?: string;
}

/** 规格表：必须覆盖 RenderConfig 的**每一个**键，漏一个就编译不过（见 fields.ts）。 */
export type FieldTable = { [K in keyof RenderConfig]: FieldSpec<RenderConfig[K]> };
