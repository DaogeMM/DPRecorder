/**
 * 配置层：界面状态 + 由规格表生成的 buildConfig() / applyConfig()。
 *
 * 以前这堆东西全在 ConfigView.vue 里（约 660 行）：
 *   ~100 个 ref + 手写 buildConfig() + 手写 applyConfig() + 一堆 watch。
 * 现在的做法是：
 *   - 界面状态由 ./fields.ts 的规格表**生成**，一个键一个槽位，不可能多也不可能少；
 *   - 界面值 ↔ 配置值的换算按规格表逐键跑，不再手抄两遍；
 *   - 只有「换算不看规格、而要看别的状态」的少数地方才写死（见下面 SYNC 与 WATCH 两节）。
 *
 * 依赖方向： ConfigView / sections/* → 本文件 → fields.ts → model.ts
 */
import { reactive, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { invoke } from '@tauri-apps/api/core';

import { isNumeric, toast } from '../common';
import { DEFAULT_RENDER_CONFIG, type RenderConfig } from '../model';
import { RENDER_FIELDS } from './fields';
import { defaultConfigValue, defaultUiValue } from './convert';
import type { FieldContext, FieldSpec, RespackItem, UiState } from './types';

export { DEFAULT_RGB, hexToRgb, rgbToHex } from './convert';

/* ------------------------------------------------------------------ *
 * 默认换算：见 ./convert.ts（那边保持零运行时依赖，离线自检脚本要用）
 * ------------------------------------------------------------------ */
type Key = keyof RenderConfig;

/**
 * 规格表的可遍历形式。
 * 直接 Object.entries(RENDER_FIELDS) 拿到的值类型是「所有键的 FieldSpec 的联合」，
 * 调用 field.toUi / field.toConfig 时参数会被推成 never（各分支参数类型的交集），
 * 所以这里统一放宽成 FieldSpec（= FieldSpec<any>）：单键的类型安全已经在 fields.ts 的
 * `satisfies Partial<FieldTable>` 与 FieldTable 的完整性断言里拿到了，循环里不需要重复。
 */
const FIELD_ENTRIES: [Key, FieldSpec][] = Object.entries(RENDER_FIELDS) as [Key, FieldSpec][];

/**
 * `syncToUi: false` 的那些键：界面上没有自己的控件，`applyConfig()` 也**故意不写**它们
 * （fxaa / hevc / mpeg4 / customEncoder / allGood / allBad 由「编码器」「判定模式」下拉和抗锯齿推导，
 * lvl 更是从来没人写过 —— 见 fields.ts 里那条既有 bug 的说明）。
 * 比「界面这份配置还是不是原来那个预设」时要把它们跳过，否则刚选中预设、一个字都没改也会被判成改过。
 */
const DERIVED_KEYS = new Set(FIELD_ENTRIES.filter(([, field]) => field.syncToUi === false).map(([k]) => k));

/**
 * 两份渲染配置「是不是同一份」。
 *
 * 用来判断「界面上的配置还等于那个预设吗」（预设名前的 `*`）、以及「批量页这份临时配置要不要换」。
 * 不能直接 `JSON.stringify(a) !== JSON.stringify(b)`：
 *   - 预设里存的旧配置可能**少几个键**（后加的字段），界面那边套用时会按默认值补上 ⇒ 少键不算改动；
 *   - 两边**键的顺序**未必一样（后端 serde 的顺序 vs 构建器 FIELD_ENTRIES 的顺序）⇒ 顺序不算改动；
 *   - 预设里可能有当前构建器**已经不再产出**的键 ⇒ 多出来的键也忽略；
 *   - `syncToUi: false` 的派生键（见 `DERIVED_KEYS`）压根没被套用进界面 ⇒ 也不比。
 * 所以：只比 `a`（＝界面构建出来的那份）会产出的键，值逐个比较。
 */
export function sameRenderConfig(a: RenderConfig, b: RenderConfig) {
  const merged = { ...DEFAULT_RENDER_CONFIG, ...b } as unknown as Record<string, unknown>;
  const built = a as unknown as Record<string, unknown>;
  return Object.keys(built).every((key) =>
    DERIVED_KEYS.has(key as Key) || JSON.stringify(built[key]) === JSON.stringify(merged[key]));
}

function uiDefault(key: Key, field: FieldSpec, value: unknown): unknown {
  return defaultUiValue(key, field, value);
}

function uiToConfig(key: Key, field: FieldSpec, slot: unknown): unknown {
  return defaultConfigValue(key, field, slot);
}

export function useRenderConfig() {
  const { t } = useI18n();

  /* ---------------------- 下拉项与本地校验 ---------------------- */
  const encoderList = ref(t('encoder-list').split(','));
  const resolutionList = [
    '1280x720',
    '1920x1080',
    '1620x1080',
    '1440x1080',
    '2560x1440',
    '2844x1600',
    '2388x1668',
    '3840x2160',
  ];
  const fpsList = ['30', '60', '120'];
  const bitrateList = ['2M', '5M', '7M'];
  const bitrateCrfList = ['24', '28', '35', '40'];
  const speedList = ['0.25', '0.5', '0.75', '1.0', '1.25', '1.5', '1.75', '2.0'];
  const antiAliasingList = [
    { title: t('off'), value: '1' },
    { title: t('fxaa'), value: '-1' },
    { title: `2x${t('msaa')}`, value: '2' },
    { title: `4x${t('msaa')}`, value: '4' },
    { title: `8x${t('msaa')}`, value: '8' },
    { title: `16x${t('msaa')}`, value: '16' },
  ];
  const maxParticlesTextList = t('max-particles-list').split(',');
  const maxParticlesList = [5000, 25000, 200000];
  const challengeColors = t('challenge-colors').split(',');
  const judgeModes = t('judge-modes').split(',');

  function parseResolution(resolution: string): [number, number] | null {
    const parts = String(resolution).split(/[xX]/g);
    if (parts.length !== 2) return null;
    const ws = parts[0]!.trim();
    const hs = parts[1]!.trim();
    if (!isNumeric(ws) || !isNumeric(hs)) return null;
    const w = parseInt(ws);
    const h = parseInt(hs);
    if (w <= 0 || h <= 0) return null;
    return [w, h];
  }
  const localRules: Record<string, (value: any) => true | string> = {
    resolution: (value: string) => parseResolution(value) !== null || t('rules.resolution'),
    sampleCount: (value: string) => (isNumeric(value) && Math.log2(Number(value)) % 1 === 0) || t('rules.sample-count'),
    crf: (value: string) => (Number.isInteger(Number(value)) && Number(value) >= 0 && Number(value) <= 51) || t('rules.crf'),
    bitrate: (value: string) => {
      if (!value || String(value).trim() === '') return t('rules.bitrate');
      const match = String(value).match(/^(\d+)(Kbps|Mbps|K|M)$/i);
      if (!match) return t('rules.bitrate');
      const number = Number(match[1]);
      const unit = match[2]!.toLowerCase();
      if ((unit === 'kbps' || unit === 'k') && number > 0 && number <= 1000000) return true;
      if ((unit === 'mbps' || unit === 'm') && number > 0 && number <= 1000) return true;
      return t('rules.bitrate');
    },
  };

  /* ------------------------ 材质包（异步） ------------------------ */
  type Respack = RespackItem;
  const DEFAULT_RESPACK: Respack = { name: t('respack-default'), path: null, index: 0, present: true, keys: true, noise: false };
  const respacks = ref<Respack[]>([DEFAULT_RESPACK]);

  /** 下拉框里的资源包：内置默认皮肤 + `respack/` 里用户自备的 zip。包在但缺文件的项也列着
   *  （`present` 假，`missing` 说明缺什么），选中后点预览 / 渲染时再提示 —— 不列出来就没法选中去修。
   *  带 `keys` / `noise` 的包分别提供按键皮肤与噪域贴图，界面按这两个标记点亮对应图标。 */
  async function getRespacks(): Promise<Respack[]> {
    const list = (await invoke('get_respacks')) as { name: string; path: string; present?: boolean; keys?: boolean; missing?: string[]; noise?: boolean }[];
    return [DEFAULT_RESPACK, ...list].map((obj, index) => ({
      name: obj.name,
      path: obj.path,
      index: index + 1,
      present: obj.present,
      keys: obj.keys,
      missing: obj.missing,
      noise: obj.noise,
    }));
  }

  /** 每次取值时现算：下拉项与校验器都跟着当前语言走 */
  function mkCtx(): FieldContext {
    return {
      t,
      encoderList: encoderList.value,
      antiAliasing: antiAliasingList,
      resolutionList,
      fpsList,
      speedList,
      bitrateList,
      bitrateCrfList,
      maxParticlesTextList,
      maxParticlesList,
      challengeColors,
      judgeModes,
      respacks: respacks.value,
      rules: localRules,
    };
  }

  /* --------------------------- 界面状态 ---------------------------
   * 每个键一个槽位：
   *   uiKey === 键名  -> 这个槽存的是**界面显示值**（字符串 / HEX / 布尔 / 数字）
   *   uiKey !== 键名  -> ui[键名] 是内部槽（保持配置形态，只有 toConfig 读它），
   *                      ui[uiKey] 才是显示槽
   * 例：maxParticles 的显示槽是 ui.maxParticlesText（"低"），内部槽 ui.maxParticles 是 5000。 */
  const ui = reactive<UiState>({});

  {
    const ctx = mkCtx();
    for (const [k, field] of FIELD_ENTRIES) {
      const key = k as Key;
      const defaultValue = DEFAULT_RENDER_CONFIG[key];
      const uk = field.uiKey ?? k;
      const display = field.toUi ? field.toUi(defaultValue, DEFAULT_RENDER_CONFIG, ctx) : uiDefault(key, field, defaultValue);
      if (uk === k) {
        // 显示槽就是键名本身（含 lvl 这种「界面上没有控件、toConfig 仍要读」的键）
        ui[k] = display;
      } else {
        // 内部槽：保持配置形态，只有 toConfig 读它。
        // 显示槽只在真有控件时写：syncToUi:false 的那几个（fxaa/hevc/mpeg4/customEncoder/
        // allGood/allBad）跟别人共用一个显示槽，写了会互相打架。
        ui[k] = defaultValue;
        if (field.syncToUi !== false && display !== undefined) ui[uk] = display;
      }
    }
    // 界面独有（配置里没有对应键）的两个下拉
    ui.encoder = encoderList.value[0];
    ui.judgeMode = judgeModes[0];
  }

  /* ---------------------- 界面独有的同步钩子 ----------------------
   * 这些是「配置 → 界面」里没法按单键规格表达的：一个下拉管着好几个配置键。 */
  function syncUiOnly(config: RenderConfig) {
    if (config.hevc) ui.encoder = encoderList.value[1];
    else if (config.mpeg4) ui.encoder = encoderList.value[2];
    else if (config.customEncoder) ui.encoder = config.customEncoder;
    else ui.encoder = encoderList.value[0];

    ui.judgeMode = config.allGood ? judgeModes[1] : config.allBad ? judgeModes[2] : judgeModes[0];
  }

  /* --------------------------- buildConfig ------------------------ */
  function buildConfig(): RenderConfig {
    const ctx = mkCtx();
    const out: Record<string, unknown> = {};
    for (const [k, field] of FIELD_ENTRIES) {
      out[k] = field.toConfig ? field.toConfig(ui, ctx) : uiToConfig(k as Key, field, ui[field.uiKey ?? k]);
    }
    return out as unknown as RenderConfig;
  }

  /* --------------------------- applyConfig ------------------------ */
  function applyConfig(config: RenderConfig) {
    const ctx = mkCtx();
    const defaults = DEFAULT_RENDER_CONFIG as unknown as Record<string, unknown>;
    for (const [k, field] of FIELD_ENTRIES) {
      if (field.syncToUi === false) continue;
      const key = k as Key;
      // 旧配置缺字段时的向后兼容：缺了就用默认值补上（以前只有一部分字段这么做）
      const raw = ((config as unknown as Record<string, unknown>)[k] ?? defaults[k]) as never;
      const uk = field.uiKey ?? k;
      const display = field.toUi ? field.toUi(raw, config, ctx) : uiDefault(key, field, raw);
      if (uk === k) {
        if (display !== undefined) ui[k] = display;
      } else {
        ui[k] = raw;
        if (display !== undefined) ui[uk] = display;
      }
    }
    syncUiOnly(config);
  }

  /* --------------------------- 分辨率换算 ------------------------- */
  function applyAspectRatio(aspectRatio: number) {
    const h = parseInt(String(ui.resolution).split('x')[1]!);
    if (aspectRatio <= 1.0) {
      ui.resolution = `${h}x${h}`;
    } else {
      const w = Math.floor(h * aspectRatio);
      ui.resolution = `${w}x${h}`;
    }
  }

  function applyResolution(w?: number, h?: number) {
    if (w) {
      const width = parseInt(String(ui.resolution).split('x')[0]!);
      const ratio = w / width;
      const height = Math.floor(parseInt(String(ui.resolution).split('x')[1]!) * ratio);
      ui.resolution = `${w}x${height}`;
    }
    if (h) {
      const height = parseInt(String(ui.resolution).split('x')[1]!);
      const ratio = h / height;
      const width = Math.floor(parseInt(String(ui.resolution).split('x')[0]!) * ratio);
      ui.resolution = `${width}x${h}`;
    }
  }

  /* ------------------------ 保存前的两个换算 ----------------------
   * 这两个要拦非法输入（弹 toast + 中止保存），所以不能做成纯函数换算。 */
  function updateMaxParticles(): boolean {
    const index = maxParticlesTextList.indexOf(ui.maxParticlesText);
    const textNum = Number(ui.maxParticlesText);
    if (index >= 0 && index < maxParticlesTextList.length) {
      ui.maxParticles = maxParticlesList[index]!;
    } else if (Number.isInteger(textNum) && textNum > 0) {
      ui.maxParticles = parseInt(ui.maxParticlesText);
    } else {
      toast(t('max-particles-error'), 'error');
      ui.maxParticles = 100000;
      return false;
    }
    return true;
  }

  function updateBitrate() {
    ui.bitrate = ui.dynamicBitrateControl ? bitrateCrfList[0]! : bitrateList[0]!;
  }

  /* ---------------------- 谱面信息侧的槽位绑定 ----------------------
   * 「总分」「偏移」的控件在谱面信息表单里（`RenderView` 的 chart-info 插槽），但它们的
   * 配置值仍随预设与 config.toml 存，所以规格表里仍留着这两个键：`syncToUi: false`
   * 表示 `applyConfig()` 不把预设里的值写进界面，而 `toConfig` 仍从内部槽读
   * （`ui.scoreTotal` / `ui.offset` —— 它们建在**配置形态**上：总分是整数、偏移是秒）。
   * 这两个函数让 `ConfigView` 读写那两个内部槽：
   *   写 ⇒ 表单上的值进配置（buildConfig 每次现读，提交渲染时与表单一致）；
   *   读 ⇒ 套用预设 / 读回工作配置之后，表单上显示的就是配置里那份值。 */
  const chartInfoBindings = {
    scoreTotal: () => ui.scoreTotal as number,
    offset: () => ui.offset as number,
  };
  function bindChartInfoSlot(key: 'scoreTotal' | 'offset', value: unknown) {
    ui[key] = value;
  }

  /* ---------------------------- 联动 ------------------------------
   * 照抄原来 ConfigView.vue 里的 watch，语义一字不差。 */
  watch(
    () => ui.volumeSfx,
    (volume) => {
      if (ui.forceLimit && volume > ui.limitThreshold) ui.limitThreshold = volume;
    }
  );
  watch(
    () => ui.limitThreshold,
    (limit) => {
      if (ui.forceLimit && limit < ui.volumeSfx) ui.volumeSfx = limit;
    }
  );
  watch(
    () => ui.loudnessEqualization,
    (value) => {
      if (value && ui.volumeMusic < 0.6) ui.volumeMusic = 1.0;
    }
  );
  watch([() => ui.renderBg, () => ui.sampleCount], ([bg]) => {
    if (bg) ui.preserveFramebuffer = false;
  });
  watch(
    () => ui.preserveFramebuffer,
    (pre) => {
      if (!pre) return;
      ui.renderBg = false;
    }
  );
  // 拖影画在一张单采样的累积缓冲上，抗锯齿拿不到残影、还会白算一遍：
  // 拖着开着就把抗锯齿选回「关」；反过来选了 MSAA / FXAA 就关掉拖影。
  watch(
    () => ui.preserveFramebuffer,
    (value) => {
      if (value) ui.sampleCount = '1';
    }
  );
  watch(
    () => ui.sampleCount,
    (sam) => {
      if (sam === '1') return;
      ui.preserveFramebuffer = false;
    }
  );
  watch(
    () => ui.roman,
    (value) => {
      if (value) ui.chinese = false;
    }
  );
  watch(
    () => ui.chinese,
    (value) => {
      if (value) ui.roman = false;
    }
  );
  watch(
    () => ui.aggressiveChart,
    (chart) => {
      if (!chart) {
        ui.aggressiveNote = false;
        ui.aggressiveParticle = false;
      }
    }
  );
  watch([() => ui.aggressiveNote, () => ui.aggressiveParticle], ([note, particle]) => {
    if (note || particle) ui.aggressiveChart = true;
  });

  /* --------------------------- 材质包刷新 ------------------------- */
  /** 重新读材质包列表，返回**是否真的改写了界面上选中的那个包**。
   *  值没变就不写：写一下会让 `ConfigView` 的深度 watch 当成界面上改动了
   *  （预设名前面那个过一会儿自己冒出来的 `*` 就是这么来的）。 */
  async function updateRespacks(): Promise<boolean> {
    respacks.value = await getRespacks();
    const current = ui.respack as Respack | undefined;
    const next = respacks.value.find((x) => x.name === current?.name) || respacks.value[0];
    if (!next) return false;
    // `noise` 也要比：换了包里的噪域贴图时界面上的图标得跟着变
    if (
      current &&
      next.name === current.name &&
      next.path === current.path &&
      !current.present === !next.present &&
      !current.noise === !next.noise
    )
      return false;
    ui.respack = next;
    return true;
  }

  return {
    ui,
    respacks,
    updateRespacks,
    buildConfig,
    applyConfig,
    applyAspectRatio,
    applyResolution,
    updateMaxParticles,
    updateBitrate,
    chartInfoBindings,
    bindChartInfoSlot,
    // 模板直接用的下拉项与校验器
    encoderList,
    resolutionList,
    fpsList,
    bitrateList,
    bitrateCrfList,
    speedList,
    antiAliasingList,
    maxParticlesTextList,
    maxParticlesList,
    challengeColors,
    judgeModes,
    resolutionRule: localRules.resolution!,
    sampleCountRule: localRules.sampleCount!,
    isCrf: localRules.crf!,
    isBitrate: localRules.bitrate!,
  };
}
