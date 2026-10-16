import { ref, watch } from 'vue';

/**
 * 谱面定数的显示格式。
 *
 * 谱面信息的定数一律强制保留一位小数。定数在各处的来源不太一样
 * （`ChartInfo.lvl` 可能是数字，也可能是谱面里写的那串文本），所以统一从这里过一道：
 * 数字一律 `toFixed(1)`（`13` → `13.0`、`13.62` → `13.6`），0 / 缺失 / 解析不出来返回
 * 空串，调用方据此决定「不显示」。
 */
export function formatLvl(lvl: unknown): string {
  const value = typeof lvl === 'string' ? parseFloat(lvl) : lvl;
  if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) return '';
  return value.toFixed(1);
}

/**
 * 把一段现成文本里的 `Lv.13` / `Lv 13.6` 都补成一位小数。
 *
 * 批量页的「难度」列、`ChartHero` 徽章的兜底文案会直接显示谱面里写的
 * `difficulty` 整串（例如 `IN Lv.13`），定数就藏在里面，所以过一遍正则。
 * 认不出的写法（没有 `Lv` 前缀、后面不是数字）原样保留。
 */
export function formatLvlInText(text: string): string {
  return text.replace(/lv\.?\s*(\d+(?:\.\d+)?)/gi, (whole: string, num: string) => {
    const lvl = formatLvl(num);
    return lvl ? `Lv.${lvl}` : whole;
  });
}

/**
 * 「定数」输入框（渲染页 / 批量页的谱面信息表单）用的字符串模型。
 *
 * 表单里那个字段是数字输入框，直接 `v-model` 到数值上的话，输入 `13` 永远显示成 `13`
 * （这个字段同样保留一位小数）。所以输入框自己拿一个字符串：
 *   - 输入过程原样保留（`13.` 这种中间状态不会被改写），只把数值同步回字段；
 *   - **失焦**时补成一位小数（`13` → `13.0`、`13.26` → `13.3`）；
 *   - 外部改了数值（导入谱面、切换谱面）就同步进来；数值相等说明是用户正在输入，不动它。
 */
export function useLvlField(source: () => unknown, apply: (value: number) => void) {
  const text = ref(formatLvl(source()));
  watch(source, (value) => {
    if (parseFloat(text.value) !== Number(value ?? 0)) text.value = formatLvl(value);
  });
  watch(text, (value) => {
    const trimmed = value.trim();
    apply(trimmed === '' ? 0 : (parseFloat(trimmed) || 0));
  });
  /** 失焦时收尾：把当前文本规整成一位小数 */
  function normalize() {
    text.value = formatLvl(parseFloat(text.value));
  }
  return { text, normalize };
}
