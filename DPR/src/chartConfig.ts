import { invoke } from '@tauri-apps/api/core';

import type { RenderConfig } from './model';

/**
 * 谱面包根上的 `config.toml`：谱面自带的**强制**渲染配置（只有 DPR 读它，引擎不认这个文件）。
 *
 * 返回 `null` 表示这张谱面没带配置文件（常态）。文件在但读不出来（不是合法 TOML、键名写错）
 * 会抛错，由调用方提示 —— 静默忽略会让「谱面配置没生效」查不出原因。
 * 后端的白名单（`src-tauri/src/chart_config.rs` 的 `ALLOWED`）决定哪些键能生效。
 */
export async function readChartConfig(path: string): Promise<Partial<RenderConfig> | null> {
  if (!path) return null;
  return (await invoke('read_chart_config', { path })) as Partial<RenderConfig> | null;
}

/**
 * 把谱面自带的强制项盖到一份渲染配置上（返回新对象，不改原配置）。
 * 没带配置文件时原样返回，等于「什么都由用户配置决定」。
 */
export function withChartConfig(
  config: RenderConfig,
  forced: Partial<RenderConfig> | null,
): RenderConfig {
  return forced ? { ...config, ...forced } : config;
}
