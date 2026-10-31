import { useStorage } from '@vueuse/core';

/**
 * 窗口的几个部分。
 *
 * DPRecorder 只有一个窗口：标题栏里 `DPRecorder` 名字右边的切换器决定窗口里放哪一部分，
 * 左栏一级导航、切换时落到的首页路由都按当前部分走（见 `App.vue` 的 `currentPart` / `switchPart`）。
 *
 * - DPR：录制器（渲染 / 任务）。
 * - DPE：谱面编辑器（批量页 / 谱面修改 / 噪域播放器），改谱面时另开 `dpe.exe` 预览进程看效果
 *   （见 `DPE/README.md`）。
 *
 * 移动端的 DPS 是第三部分，但它的界面要另做一套，不进这个窗口的切换器。
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
    label: 'DPR',
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
    label: 'DPE',
    home: 'chart-list',
    nav: [
      // 谱面列表是 DPE 的默认页：挑要编辑的谱面（与 DPR 的批量页各存各的）
      { name: 'chart-list', icon: 'mdi-filmstrip-box-multiple' },
      { name: 'chart-mod', icon: 'mdi-wrench-outline' },
      { name: 'tool-video-noise', icon: 'mdi-movie-open-star-outline' },
    ],
    // 设置页与 DPR 共用同一份（标题随部分变），所以这里也要写上，
    // 不然在 DPE 里点设置就会被当成 DPR 的路由、切换器跟着弹回 DPR。
    routes: ['chart-list', 'chart-mod', 'tool-video-noise', 'settings'],
  },
];

/** 当前部分的存储键：关掉再开回到原来那一部分 */
const PART_KEY = 'DPRecorder.part';

export function usePart() {
  return useStorage<PartId>(PART_KEY, 'dpr');
}
