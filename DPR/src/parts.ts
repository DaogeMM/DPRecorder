import { useStorage } from '@vueuse/core';

/**
 * 窗口的几个部分。
 *
 * DPRecorder 只有一个窗口：标题栏里 `DPRecorder` 名字右边的切换器决定窗口里放哪一部分，
 * 左栏一级导航、切换时落到的首页路由都按当前部分走（见 `App.vue` 的 `currentPart` / `switchPart`）。
 *
 * 窗口的两个部分（切换器上写 `Render` / `Editor`）：
 * - Render：录制与渲染这条线（内部仍叫 dpr）。
 * - Editor：谱面编辑与工具这条线（内部仍叫 dpe），改谱面时主程序另起一份预览进程看效果。
 * - DPS = DPSimulator：移动端的第三部分，但它的界面要另做一套，不进这个窗口的切换器。
 */
export type PartId = 'dpr' | 'dpe';

export interface PartNavItem {
  /** 路由名，同时是左栏标签的 i18n 键 */
  name: string;
  icon: string;
}

export interface Part {
  id: PartId;
  /** 切换器上显示的名字（专有名词，不随语言翻译） */
  label: string;
  /** 切到这个部分时落到的路由名 */
  home: string;
  /** 左栏一级导航 */
  nav: PartNavItem[];
  /**
   * 属于这一部分的路由名。
   * 直接打开某个路由（刷新、外部跳转）时，切换器按这张表把当前部分对回来；
   * 左栏里没有列出的路由（设置页）也要写在这里。
   */
  routes: string[];
}

export const PARTS: Part[] = [
  {
    id: 'dpr',
    label: 'Render',
    home: 'render',
    nav: [
      { name: 'render', icon: 'mdi-auto-fix' },
      // DPR 自己的谱面列表，只用来批量渲染（DPE 的谱面列表是另一份，两边不共用）
      { name: 'batch', icon: 'mdi-filmstrip-box-multiple' },
      { name: 'tasks', icon: 'mdi-server' },
    ],
    routes: ['render', 'bind', 'batch', 'tasks', 'settings'],
  },
  {
    id: 'dpe',
    label: 'Editor',
    home: 'chart-edit',
    nav: [
      // 编辑窗口在「谱面修改」上面：先把谱面挑出来，再决定改什么
      { name: 'chart-edit', icon: 'mdi-file-edit-outline' },
      // 谱面列表不再单独一项：它折进了「谱面修改」页顶部
      { name: 'chart-mod', icon: 'mdi-wrench-outline' },
      { name: 'noise-player', icon: 'mdi-movie-open-star-outline' },
      { name: 'malody-import', icon: 'mdi-file-import-outline' },
    ],
    // 设置页与 DPR 共用同一份（标题随部分变），所以这里也要写上，
    // 不然在 DPE 里点设置就会被当成 DPR 的路由、切换器跟着弹回 DPR。
    routes: ['chart-edit', 'chart-mod', 'noise-player', 'malody-import', 'settings'],
  },
];

/** 当前部分的存储键：关掉再开回到原来那一部分 */
const PART_KEY = 'DPRecorder.part';

export function usePart() {
  return useStorage<PartId>(PART_KEY, 'dpr');
}
