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

/** 配置里的 [r, g, b]（0~1）→ 界面上的 HEX 文本 */
export function rgbToHex(rgb: number[] | undefined | null): string {
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
  );
}

/** 界面上的 HEX 文本 → 配置里的 [r, g, b]（0~1）。认不出来就回落白色。 */
export function hexToRgb(hex: string): [number, number, number] {
  const m = /^#?([0-9a-fA-F]{6})$/.exec(String(hex).trim());
  if (!m) return [...DEFAULT_RGB] as [number, number, number];
  const n = parseInt(m[1]!, 16);
  return [((n >> 16) & 0xff) / 255, ((n >> 8) & 0xff) / 255, (n & 0xff) / 255];
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
  if (field.widget === 'slider' || field.uiValue === 'number') return value;
  const d = DEFAULT_RENDER_CONFIG[key];
  if (typeof d === 'boolean') return !!value;
  if (typeof d === 'number') return String(value);
  return value ?? '';
}

/**
 * 规格表里这个键的默认配置值（该键没写 toConfig 时用）。
 * 与 defaultUiValue 对称：按配置默认值的类型解读界面槽位。
 */
export function defaultConfigValue(key: keyof RenderConfig, field: FieldSpec, slot: unknown): unknown {
  if (field.widget === 'color') return hexToRgb(slot as string);
  if (field.widget === 'slider') return Number(slot);
  const d = DEFAULT_RENDER_CONFIG[key];
  if (typeof d === 'boolean') return !!slot;
  if (typeof d === 'number') return field.integer ? parseInt(String(slot)) : parseFloat(String(slot));
  return slot;
}
