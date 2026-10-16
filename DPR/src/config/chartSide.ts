/**
 * 配置 → 谱面信息（`ChartInfo`）的桥。
 *
 * 这里列的键在 dps 里是**谱面信息**的字段，但 2026-10-11 起它们在界面上是配置项
 * （`RenderConfig`，见 ../model.ts 的「谱面侧渲染参数」）：
 *   - 存得下来：预设（`config/presets.toml`）、`config.toml`、渲染页的工作配置；
 *   - 出片时**配置值优先** ⇒ 导入谱面不再覆盖它们（「可以被保存」的前提）。
 *
 * `RenderView.vue` 与 `BatchView.vue` 的 `buildParams()` 都调用 `chartSideOf()`，
 * 并把它展开在解析出来的 `chartInfo` **之后**（后面的键覆盖前面的），
 * 于是发给后端的 `info` 里这些键永远是配置里的那份。
 *
 * 为什么在前端合并、而不是在 Rust 里覆盖：GUI 的四条命令里只有 `render`
 * 走 `render.rs` 的 stdin 分支（`lib.rs:426` 的 `render::main(false)`），
 * `preview` / `play` / `tweakoffset` 三条走 `preview.rs` ⇒ 只有这一处能一次覆盖全部路径；
 * 命令行 `--render` 那条分支（`render.rs` 的 CLI 分支，只有
 * `info.background_dim = config.background_dim;` 一行）的行为也不受影响。
 */
import type { RenderConfig } from '../model';

/**
 * 需要从配置覆盖进 `ChartInfo` 的键 —— 名字与 `dps::info::ChartInfo` 的 camelCase 字段同名。
 *
 * `backgroundDim` 也在里面：它是老配置项，但 stdin 分支以前没有 `render.rs` 那条覆盖，
 * 于是 GUI 里「背景压暗」的数值可能被谱面信息里的同名值顶掉；顺路一起按配置值走。
 */
export const CHART_SIDE_KEYS = [
  'forceAspectRatio',
  'foldAnimation',
  'aspectRatio',
  'noteUniformScale',
  'holdPartialCover',
  'negativeLengthHold',
  'holdParticleIntervalRatio',
  'lineLength',
  'scoreTotal',
  'previewStart',
  'previewEnd',
  'offset',
  'backgroundDim',
] as const satisfies readonly (keyof RenderConfig)[];

/** 取配置里这批键，拼成一个可以直接展开进 `info` 的小对象 */
export function chartSideOf(config: RenderConfig): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const key of CHART_SIDE_KEYS) out[key] = config[key];
  return out;
}
