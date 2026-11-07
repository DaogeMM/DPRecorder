/**
 * 成片画幅（`resolution`）与宽高比（`aspectRatio`）的对齐规则。
 *
 * 单独成文件、不依赖任何运行时模块：离线自检脚本可以直接 import 它，不必拖进 vue / tauri。
 */
import type { RenderConfig } from '../model';

/**
 * 分辨率就是成片的画幅：它和 `aspectRatio` 对不上时，成片仍是分辨率的比例，画面按
 * `aspectRatio` 居中摆完四周留黑边（见 `dps` 的 `Resource::update_size`）。
 *
 * 界面上改比例 / 分辨率时两边是一起换的（`ConfigView` 的 `commitAspect` / `commitResolution`），
 * 但**批量渲染读的是预设里那份配置**、老版本存下的预设 / 工作配置也会是
 * 「4 : 3 配 1920×1080」这种旧搭配 —— 出片 / 预览前一律过一遍这里，否则界面写着 4 : 3、
 * 成片和预览窗口却是 16 : 9。
 *
 * 换算保留高度（垂直分辨率不动），宽取偶数：编码器要偶数边长，奇数只有 RGBA 读回那条慢路
 * （见 `render.rs` 的 `gpu_yuv`）。已经对得上就原样返回同一个对象，一个像素都不动。
 */
export function alignResolution(config: RenderConfig): RenderConfig {
  const resolution = config.resolution as unknown as number[] | undefined;
  const w = Number(resolution?.[0]);
  const h = Number(resolution?.[1]);
  const aspectRatio = Number(config.aspectRatio);
  if (!Number.isFinite(w) || !Number.isFinite(h) || w <= 0 || h <= 0) return config;
  if (!Number.isFinite(aspectRatio) || aspectRatio <= 0) return config;
  if (Math.abs(w / h - aspectRatio) <= 1e-3) return config;
  const width = Math.max(2, Math.round((h * aspectRatio) / 2) * 2);
  return { ...config, resolution: [width, h] } as RenderConfig;
}
