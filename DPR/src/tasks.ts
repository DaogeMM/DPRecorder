/**
 * 全局任务列表（渲染弹窗在应用里全局显示）。
 *
 * 任务页、底部的渲染弹窗（`components/TaskStrip.vue`）、以及窗口任务栏进度都要看同一份
 * 数据，所以 `get_tasks` 的 700ms 轮询收在这里：每个 `useTasks()` 是一个订阅者，
 * 第一个订阅者启动轮询，最后一个退订时停。
 */
import { onScopeDispose, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { formatLvl } from './lvl';
import type { ChartInfo, Task } from './model';

const INTERVAL = 700;

/** 全局唯一的任务列表；第一次拉取完成前是 undefined */
const tasks = ref<Task[]>();

/** 立刻向 Rust 要一次最新列表 */
export async function refreshTasks() {
  try {
    tasks.value = await invoke<Task[]>('get_tasks');
  } catch (e) {
    console.error('Error updating task list:', e);
  }
}

/**
 * 渲染弹窗盯的那一条任务 —— 它显示当前渲染谱面的曲绘、曲名、难度与进度条。
 *
 * 一轮里可能同时有好几条在跑，这里取列表里第一条还活着的：
 * 有正在渲染的就优先它，否则按列表顺序取第一条（加载 / 混音 / 暂停）。
 */
const ACTIVE_TYPES = ['rendering', 'mixing_sfx', 'paused', 'mixing', 'loading'] as const;

export function pickActiveTask(list?: Task[]): Task | null {
  const active = (list ?? []).filter((task) =>
    (ACTIVE_TYPES as readonly string[]).includes(task.status.type),
  );
  if (!active.length) return null;
  return active.find((task) => task.status.type === 'rendering') ?? active[0] ?? null;
}

let timer: ReturnType<typeof setInterval> | undefined;
let subscribers = 0;

/** 在组件 setup 里调用；组件卸载后自动退订（没有订阅者时轮询停止） */
export function useTasks() {
  subscribers += 1;
  if (timer === undefined) {
    timer = setInterval(refreshTasks, INTERVAL);
    void refreshTasks();
  }
  onScopeDispose(() => {
    subscribers -= 1;
    if (subscribers <= 0 && timer !== undefined) {
      clearInterval(timer);
      timer = undefined;
      subscribers = 0;
    }
  });
  return tasks;
}

/**
 * 难度徽章的三段拆解，和 `components/ChartHero.vue` 里那枚 `IN Lv.13.6` 同一个算法：
 * `diff` 是纯档位（`IN`），`difficulty` 往往是带定数的整串（`IN Lv.13.6`），
 * 两者拼起来时要把 `difficulty` 结尾的 `Lv.x.y` 削掉。
 * 任务列表的折叠行与渲染弹窗共用。
 */
export function diffTier(info?: ChartInfo): string {
  const tier = String(info?.diff ?? '').trim();
  if (tier) return tier;
  return String(info?.difficulty ?? '').trim().replace(/\s*lv\.?\s*[\d.]+$/i, '');
}

export function diffLvl(info?: ChartInfo): string {
  // 定数统一一位小数（见 `lvl.ts`）
  return formatLvl(info?.lvl);
}

export function diffBadge(info?: ChartInfo): string {
  const tier = diffTier(info);
  const lvl = diffLvl(info);
  if (tier && lvl) return `${tier} Lv.${lvl}`;
  return tier || String(info?.difficulty ?? '').trim();
}
