/**
 * 二级分区总开关的共享状态表。
 *
 * 为什么要有这个文件：PhiSection 的总开关有两个来源 —— 绑配置的（`v-model:gate`，
 * 如按键 = `renderNote`、噪域 = `renderBlockArea`、谱面进水 = `waterScreen`）
 * 和纯界面的（`gate-ui`，如多语言）。两种值都只有 PhiSection 自己知道，
 * 但左侧目录（`App.vue` 的左栏）要显示「启用 / 未启用」，于是让 PhiSection 在
 * setup 时把自己的总开关值登记到这张模块级表里，左栏按分区 id 只读查询。
 *
 * 为什么不用 useRenderConfig()：它是普通组合式函数（每次调用都新建一批 ref），
 * 只在 ConfigView 里调用，App.vue 里再调一次拿到的是另一份状态，联动不起来。
 * 这里只存「开 / 关」这一个布尔值，与配置读写无关，不需要整套配置状态。
 *
 * 依赖方向： components/PhiSection.vue →（写）本文件 ←（读）App.vue
 */
import { reactive } from 'vue';

/** 分区 id → 总开关状态。表里没有的 id = 该分区没有总开关。 */
const gates = reactive<Record<string, boolean>>({});

/** 登记 / 更新 / 注销（`on` 传 undefined = 注销）某个分区的总开关状态。由 PhiSection 调用。 */
export function setSectionGate(id: string, on: boolean | undefined) {
  if (on === undefined) delete gates[id];
  else gates[id] = on;
}

/**
 * 左栏用：`'off'` = 有总开关且没勾；`'on'` = 有总开关且勾着；`'none'` = 这栏没有总开关。
 * 在模板里直接调用即可保持响应式（读的是 reactive 对象上的键）。
 */
export function sectionGateState(id: string): 'on' | 'off' | 'none' {
  const on = gates[id];
  return on === undefined ? 'none' : on ? 'on' : 'off';
}
