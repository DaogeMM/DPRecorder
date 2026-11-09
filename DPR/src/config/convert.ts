/**
 * 配置层：规格表里没写 toConfig / toUi 时用的**默认换算**，以及颜色的 HEX ↔ [r,g,b]。
 *
 * 单独放一个文件是为了让它保持**零运行时依赖**（只 import 类型）：
 * 这样它既能被 useRenderConfig.ts / ConfigField.vue 用，也能被离线自检脚本直接跑
 * （`node_modules/esbuild` 打包成 .mjs 后用 node 执行，见 ../config-test）。
 */
import type { RenderConfig } from '../model';
import { DEFAULT_RENDER_CONFIG } from '../model';
import type { FieldSpec } from './types';

export const DEFAULT_RGB: [number, number, number] = [1, 1, 1];

/**
 * 配置里的浮点值最多留三位小数。
 *
 * 配置在内存里是 f32，序列化成 JSON 就带上了 `0.15000000596046448` 这种尾巴；界面回显
 * 与落盘都不该看得到它。三位小数对渲染参数足够：颜色最终按 8 位色深算（见 `rgbToHex`），
 * 舍入误差不到半个色阶，同一个 HEX 取整前后不变。
 */
export function roundFloat(value: number): number {
  if (!Number.isFinite(value)) return value;
  return Math.round(value * 1000) / 1000;
}

/**
 * 递归把配置值里的浮点取到三位小数：数字、数字数组、对象里的数字都取，其余原样返回。
 *
 * `buildConfig()` 收上来的值可能来自规格表里手写的 `toConfig`（减法、乘法算出来的），
 * 也可能来自别处塞进界面槽位的预设 —— 在这里统一过一遍，界面上不管怎么改，
 * 交给后端和落盘的配置都不带浮点尾巴。
 */
export function roundConfigFloats<T>(value: T): T {
  if (typeof value === 'number') return roundFloat(value) as unknown as T;
  if (Array.isArray(value)) return value.map((it) => roundConfigFloats(it)) as unknown as T;
  if (value && typeof value === 'object') {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) out[k] = roundConfigFloats(v);
    return out as unknown as T;
  }
  return value;
}

/**
 * 配置里的颜色有两种写法：`#RRGGBB` 文本与 [r, g, b] 数组（0~1）。
 *
 * 落盘与 IPC 上看到的都是文本（见 Rust 侧 `color_hex`），界面上一直传数组；
 * 下面几个函数两头都收，比较 / 显示前先统一成文本。
 */
export type ColorValue = string | number[] | null | undefined;

/** 配置里的颜色 → 界面上的 HEX 文本（大写）。已经是 HEX 的规范化成大写，认不出就原样返回。 */
export function rgbToHex(rgb: ColorValue): string {
  if (typeof rgb === 'string') {
    const m = /^#?([0-9a-fA-F]{6})$/.exec(rgb.trim());
    if (!m) return rgb.trim();
    return `#${m[1]!.toUpperCase()}`;
  }
  const c = rgb && rgb.length >= 3 ? rgb : DEFAULT_RGB;
  return (
    '#' +
    c
      .slice(0, 3)
      .map((v) =>
        Math.round(Math.min(1, Math.max(0, v)) * 255)
          .toString(16)
          .padStart(2, '0')
      )
      .join('')
      .toUpperCase()
  );
}

/** 任意写法（文本 / 数组 / 留空）→ 统一成能直接比大小的形式：有颜色就是 HEX 文本，留空还是原值。 */
export function colorText(value: unknown): unknown {
  if (value == null) return value;
  if (typeof value === 'string') return rgbToHex(value);
  if (Array.isArray(value)) return rgbToHex(value as number[]);
  return value;
}

/** 界面上的 HEX 文本 → 配置里的 [r, g, b]（0~1）。认不出来就回落白色。 */
export function hexToRgb(hex: string): [number, number, number] {
  const m = /^#?([0-9a-fA-F]{6})$/.exec(String(hex).trim());
  if (!m) return [...DEFAULT_RGB] as [number, number, number];
  const n = parseInt(m[1]!, 16);
  return [roundFloat(((n >> 16) & 0xff) / 255), roundFloat(((n >> 8) & 0xff) / 255), roundFloat((n & 0xff) / 255)];
}

/**
 * 规格表里这个键的默认显示值（该键没写 toUi 时用）。
 *
 * 判据依次是：color → HEX；slider / 显式 uiValue: 'number' → 原样存数字；否则按**配置默认值的类型**走
 * （布尔存布尔、数字存字符串、字符串/对象原样、null 变空串）。
 * 用配置默认值的类型而不是当前值，是为了让「界面状态的形状」在任何配置下都一致。
 *
 * slider 保持数字：老的 ConfigView 里 volumeMusic / volumeSfx / limitThreshold 都是 ref(number)，
 * 而且 forceLimit 的联动要拿它们比大小 —— 存成字符串会把比较变成字典序。
 */
export function defaultUiValue(key: keyof RenderConfig, field: FieldSpec, value: unknown): unknown {
  if (field.widget === 'color') return rgbToHex(value as number[]);
  if (field.widget === 'slider' || field.uiValue === 'number') {
    return typeof value === 'number' ? roundFloat(value) : value;
  }
  const d = DEFAULT_RENDER_CONFIG[key];
  if (typeof d === 'boolean') return !!value;
  if (typeof d === 'number') return String(roundFloat(Number(value)));
  return value ?? '';
}

/**
 * 规格表里这个键的默认配置值（该键没写 toConfig 时用）。
 * 与 defaultUiValue 对称：按配置默认值的类型解读界面槽位。
 */
export function defaultConfigValue(key: keyof RenderConfig, field: FieldSpec, slot: unknown): unknown {
  if (field.widget === 'color') {
    // 界面槽位是 HEX 文本；万一有人直接把数组塞进来（老预设 / 手改的草稿）也认
    if (Array.isArray(slot)) return slot.slice(0, 3).map((it) => roundFloat(Number(it)));
    return hexToRgb(slot as string);
  }
  if (field.widget === 'slider') return roundFloat(Number(slot));
  const d = DEFAULT_RENDER_CONFIG[key];
  if (typeof d === 'boolean') return !!slot;
  if (typeof d === 'number') {
    return roundFloat(field.integer ? parseInt(String(slot)) : parseFloat(String(slot)));
  }
  return slot;
}
