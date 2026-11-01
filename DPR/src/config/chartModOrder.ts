import { watch, type Ref } from 'vue';

/**
 * 谱面修改的互斥分组与「按键那一类」的落地次序。
 *
 * 这些功能同属一层：谱面文件不动，只改内存里刚解析出来的判定线与按键，所以预览、渲染、分屏
 * 走的是同一份转换，dps 里的入口只有一个（`core::chart_mod` + `Chart::apply_chart_mods`）。
 *
 * **七类互斥**：纯配置谱面 / 定轨谱面 / 隐藏按键 / 陈列按键 / 固定流速 / 谱面融合 / 按键那一类
 * （按键替换、随机按键、随机流速、随机位置、长条填充、长条延长、复制音符）—— 同一时刻只有一类
 * 生效，勾上哪一类就把另外几类整个关掉。这几类各自都是把整张谱面重排一遍（或整体改可见性），
 * 叠在一起只会互相覆盖，所以不给选。
 *
 * 「谱面融合」只出现在 DPE 的谱面修改页：它把勾中的几个难度拼成一张谱面，产出也写成一份包，
 * 与别的修改叠在一起没有意义。它不改 dps 的配置，所以既没有齿轮序号、也不进下面的顺序表。
 *
 * 按键那一类内部**可以同时开**，落地顺序就是界面上勾选的先后：勾上的那一项追加到顺序表末尾，
 * 齿轮左边那个数字就是它在这一次转换里的次序。配置里记着这张表（`chartModOrder`），dps 按同一个
 * 次序逐个落地 —— 先开哪个，后面的功能就在它改完的谱面上再改一遍。载入预设 / 手改 config.toml 里
 * 没这张表时按界面行序补齐（dps 那边也是同样的兜底规则，两边的次序才对得上）。
 *
 * 只有按键那一类有齿轮序号，另外几类各自都是单独一步。固定流速把整张谱面的下落节奏换掉，
 * 和别的功能叠在一起没有意义，所以不进顺序表。「游玩分屏」不是改谱面的功能，不受影响。
 *
 * 两块界面共用这一份：渲染页的「谱面修改」分区（`components/ConfigView.vue`）与 DPE 的谱面修改页
 * （`../ChartModView.vue`）。两边不会同时挂在屏上，但读写的是同一份 `chartModOrder`，
 * 所以规则只能有一处实现。
 */

/** 参与互斥判断的一项：`key` 是顺序表里的名字（只有按键那一类用得上） */
interface ChartModEntry {
  key: string;
  // `toRefs(ui)` 的索引签名让槽位在类型上可能是 undefined，这里跟着放宽
  flag: Ref<boolean | undefined> | undefined;
}

/** 谱面修改用到的那些界面槽位（`toRefs(ui)` 里取出来的一部分） */
export interface ChartModSlots {
  chartModOrder: Ref<string[] | undefined> | undefined;
  pureConfigChart: Ref<boolean | undefined> | undefined;
  fixedLaneChart: Ref<boolean | undefined> | undefined;
  chartModHide: Ref<boolean | undefined> | undefined;
  chartModArrange: Ref<boolean | undefined> | undefined;
  chartModFixedSpeed: Ref<boolean | undefined> | undefined;
  /** 「谱面融合」：只有 DPE 的谱面修改页有这个开关，渲染页传 undefined 就跳过互斥判断 */
  chartModMerge?: Ref<boolean | undefined> | undefined;
  chartModReplace: Ref<boolean | undefined> | undefined;
  chartModRandom: Ref<boolean | undefined> | undefined;
  chartModRandomSpeed: Ref<boolean | undefined> | undefined;
  chartModRandomPos: Ref<boolean | undefined> | undefined;
  chartModSpreadHold: Ref<boolean | undefined> | undefined;
  chartModExtendHold: Ref<boolean | undefined> | undefined;
  chartModCopy: Ref<boolean | undefined> | undefined;
}

export function useChartModOrder(slots: ChartModSlots) {
  const chartModGroups: ChartModEntry[] = [
    { key: 'pure', flag: slots.pureConfigChart },
    { key: 'lane', flag: slots.fixedLaneChart },
    { key: 'hide', flag: slots.chartModHide },
    { key: 'arrange', flag: slots.chartModArrange },
    { key: 'fixed_speed', flag: slots.chartModFixedSpeed },
    { key: 'merge', flag: slots.chartModMerge },
  ];
  const chartMods: ChartModEntry[] = [
    { key: 'replace', flag: slots.chartModReplace },
    { key: 'random', flag: slots.chartModRandom },
    { key: 'random_speed', flag: slots.chartModRandomSpeed },
    { key: 'random_pos', flag: slots.chartModRandomPos },
    { key: 'spread', flag: slots.chartModSpreadHold },
    { key: 'extend', flag: slots.chartModExtendHold },
    { key: 'copy', flag: slots.chartModCopy },
  ];

  /** 勾上某一类时，把另外几类整个关掉（`opened` 是这一次被勾上的那个开关） */
  function closeOtherChartMods(opened: Ref<boolean | undefined> | undefined) {
    const inMods = chartMods.some((it) => it.flag === opened);
    for (const group of chartModGroups) {
      if (!inMods && group.flag === opened) continue;
      if (group.flag?.value) group.flag.value = false;
    }
    if (inMods) return;
    for (const mod of chartMods) if (mod.flag?.value) mod.flag.value = false;
  }

  function chartModOrderList(): string[] {
    // `toRefs(ui)` 的索引签名让这个名字在类型上可能是 undefined；这里只读，
    // 写回统一走下面的 watch / syncChartModOrder（模板里每帧都会问一遍序号，不能在这里改状态）
    return slots.chartModOrder?.value ?? [];
  }

  /** 这一项在顺序表里的位置（1 起）；没开 / 不在表里就是 undefined */
  function chartModIndex(key: string): number | undefined {
    const index = chartModOrderList().indexOf(key);
    return index < 0 ? undefined : index + 1;
  }

  /** 让顺序表只留下「开着的项」、并按界面行序补齐漏记的那些（预设与手改配置里可能没有这张表） */
  function syncChartModOrder() {
    const order = slots.chartModOrder;
    if (!order) return;
    const list = chartModOrderList();
    const enabled = chartMods.filter((it) => it.flag?.value).map((it) => it.key);
    const next = list.filter((it) => enabled.includes(it));
    for (const key of enabled) if (!next.includes(key)) next.push(key);
    if (next.length !== list.length || next.some((it, i) => it !== list[i])) order.value = next;
  }

  for (const group of chartModGroups) {
    if (!group.flag) continue;
    watch(group.flag, (value) => {
      if (value) closeOtherChartMods(group.flag);
    });
  }
  for (const mod of chartMods) {
    if (!mod.flag) continue;
    watch(mod.flag, (value) => {
      if (value) closeOtherChartMods(mod.flag);
      const order = slots.chartModOrder;
      if (!order) return;
      if (!order.value) order.value = [];
      const list = order.value;
      const index = list.indexOf(mod.key);
      if (value) {
        if (index < 0) list.push(mod.key);
      } else if (index >= 0) {
        list.splice(index, 1);
      }
    });
  }
  syncChartModOrder();

  return { chartModIndex, syncChartModOrder };
}
