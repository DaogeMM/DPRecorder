/**
 * 配置 → 谱面信息（`ChartInfo`）的桥。
 *
 * 这里列的键在 dps 里是**谱面信息**的字段，但在界面上是配置项
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
 *
 * 唯一例外是**难度文本**：`chartSideOf()` 在「谱面修改」开着时会把它整串换成
 * `（功能名 → 功能名）` 的标记（见 `chartModMarker()`）—— 那不是配置项，而是
 * 「这张谱已经不是你导入的那张了」的说明。融合过的谱面（难度文本已经是
 * `（谱面融合）`）再把修改功能一起打开时，融合摆在标记最前面：
 * `（谱面融合 → 按键替换）`。
 */
import type { RenderConfig } from '../model';

/**
 * 需要从配置覆盖进 `ChartInfo` 的键 —— 名字与 `dps::info::ChartInfo` 的 camelCase 字段同名。
 *
 * `backgroundDim` 也在里面：它是老配置项，stdin 分支没有 `render.rs` 那条覆盖，
 * 于是 GUI 里「背景压暗」的数值可能被谱面信息里的同名值顶掉；一起按配置值走。
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

/**
 * 「谱面修改」里**独占一类**的那几项，顺序照 dps 的判定顺序
 * （`core::chart_mod::Mods::from_config`：纯配置谱面 → 定轨谱面 → 隐藏按键 → 陈列按键 → 固定流速）。
 *
 * 这几类都是把整张谱面重排一遍（或整体改可见性），同一时刻只有最靠前的那一类生效，
 * 所以标记里也只该出现它一个。
 */
const MOD_CLASSES = [
  { key: 'pure-config-chart', open: (config: RenderConfig) => config.pureConfigChart },
  { key: 'fixed-lane-chart', open: (config: RenderConfig) => config.fixedLaneChart },
  { key: 'chart-mod-hide', open: (config: RenderConfig) => config.chartModHide },
  { key: 'chart-mod-arrange', open: (config: RenderConfig) => config.chartModArrange },
  { key: 'chart-mod-fixed-speed', open: (config: RenderConfig) => config.chartModFixedSpeed },
] as const;

/**
 * 按键那一类：可以同时开好几个，落地顺序由 `chartModOrder`（界面勾选的先后）决定，
 * 表里没提到的按这里的行序接在后面 —— 与 dps 的 `Mods::from_config` 同一套规则。
 * `name` 就是 `Step::name()` 里那两个名字（`random` = 随机按键、`random_speed` = 随机流速）。
 */
const MOD_STEPS = [
  { name: 'replace', key: 'chart-mod-replace', open: (config: RenderConfig) => config.chartModReplace },
  { name: 'random', key: 'chart-mod-random', open: (config: RenderConfig) => config.chartModRandom },
  { name: 'random_speed', key: 'chart-mod-random-speed', open: (config: RenderConfig) => config.chartModRandomSpeed },
  { name: 'random_pos', key: 'chart-mod-random-pos', open: (config: RenderConfig) => config.chartModRandomPos },
  { name: 'spread', key: 'chart-mod-spread-hold', open: (config: RenderConfig) => config.chartModSpreadHold },
  { name: 'extend', key: 'chart-mod-extend-hold', open: (config: RenderConfig) => config.chartModExtendHold },
  { name: 'copy', key: 'chart-mod-copy', open: (config: RenderConfig) => config.chartModCopy },
] as const;

/** 这一趟要落地的「谱面修改」功能，按落地顺序给它们的文案键 */
function chartModKeys(config: RenderConfig): string[] {
  for (const item of MOD_CLASSES) if (item.open(config)) return [item.key];

  const opened = MOD_STEPS.filter((item) => item.open(config));
  const keys: string[] = [];
  const done: string[] = [];
  for (const name of config.chartModOrder ?? []) {
    if (done.includes(name)) continue;
    const item = opened.find((it) => it.name === name);
    if (!item) continue;
    done.push(name);
    keys.push(item.key);
  }
  for (const item of opened) {
    if (done.includes(item.name)) continue;
    done.push(item.name);
    keys.push(item.key);
  }
  return keys;
}

/** 谱面融合写进难度文本的标记，与 `src-tauri/src/merge.rs` 的 `DIFF_TEXT` 一致。 */
const MERGE_TAG = '谱面融合';
const MERGE_DIFF_TEXT = `（${MERGE_TAG}）`;

/**
 * 「谱面修改」把难度文本改成的那串标记：`（按键替换 → 复制音符）`；一个功能都没开时给 `null`。
 *
 * 谱面文件里的难度文本说的是原谱，开着修改出片后就名不副实了，所以渲染时整串换掉
 * （与谱面融合写 `（谱面融合）` 同一个口径）。多个功能按真实落地顺序用 ` → ` 连起来。
 *
 * `difficulty` 是这张谱自己现在的难度文本：谱面融合出来的包写的就是 `（谱面融合）`，
 * 这时把融合摆在最前面（`（谱面融合 → 按键替换）`），别让修改的标记把它顶掉。
 * 只开修改、没融合的谱面按 `difficulty` 无关的固定写法走。
 */
export function chartModMarker(config: RenderConfig, t: (key: string) => string, difficulty?: string | null): string | null {
  const keys = chartModKeys(config);
  if (!keys.length) return null;

  const merged = difficulty?.trim() === MERGE_DIFF_TEXT;
  const labels = [...(merged ? [MERGE_TAG] : []), ...keys.map((key) => t(key))];
  return `（${labels.join(' → ')}）`;
}

/**
 * 取配置里这批键，拼成一个可以直接展开进 `info` 的小对象。
 *
 * `difficultyOverride` 给非空值时连难度文本一起覆盖（`chartModMarker()` 的结果）：
 * 难度文本属于谱面信息，但「修改过」这件事由配置决定，只能在这里一并交出去。
 */
export function chartSideOf(config: RenderConfig, difficultyOverride?: string | null): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const key of CHART_SIDE_KEYS) out[key] = config[key];
  if (difficultyOverride) out.difficulty = difficultyOverride;
  return out;
}
