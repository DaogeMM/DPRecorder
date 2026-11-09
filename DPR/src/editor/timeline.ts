import type { DraftLane } from '@/model';

/**
 * 时间轴共用的小算式：主轨道铺满音乐时长，自己加的轨道按片段的起止时间摆，
 * 时间轴总长取两者里最大的那个。编辑栏、时间轴、编辑窗口都按这一份算，
 * 免得同一段秒数在三处各算一遍、结果对不上。
 */

/** 新加的轨道片段默认多长（秒）。 */
export const DEFAULT_LANE_LENGTH = 10;

/** 时间轴至少画多长（秒）：还没放音乐时也得有刻度可看。 */
export const MIN_TIMELINE_SPAN = 10;

/** 自己加的轨道里最靠后的片段末端（秒）；没有轨道就是 0。 */
export function laneEnd(lanes: DraftLane[] | undefined): number {
  return (lanes ?? []).reduce(
    (max, lane) => Math.max(max, (lane.start || 0) + (lane.length || 0)),
    0,
  );
}

/** 时间轴总长（秒）：音乐时长与自己加的轨道片段末端取大，再兜一个最小值。 */
export function timelineSpan(duration: number, lanes: DraftLane[] | undefined): number {
  return Math.max(duration || 0, laneEnd(lanes), MIN_TIMELINE_SPAN);
}

/**
 * 一条自己加的轨道上的片段落在哪一段（秒）。
 *
 * 长度还没填时给一段默认宽度 —— 否则那个块在画布上宽度是 0，看着像没放东西。
 */
export function clipRange(lane: DraftLane, total: number): { start: number; length: number } {
  const span = total > 0 ? total : 1;
  const start = Math.min(Math.max(lane.start ?? 0, 0), span);
  const want = lane.length > 0 ? lane.length : Math.min(DEFAULT_LANE_LENGTH, span - start);
  return { start, length: Math.min(Math.max(want, 0), span - start) };
}

/** 秒数换成 `mm:ss`，预览、时间轴、编辑栏都写这一种。 */
export function clock(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds || 0));
  return `${String(Math.floor(total / 60)).padStart(2, '0')}:${String(total % 60).padStart(2, '0')}`;
}

/** 时间轴拖动吸附的步长（秒）：再细的值在屏幕上对不上像素，拖完还得手动改。 */
export const TIME_SNAP = 0.1;

/** 把秒数吸到 `TIME_SNAP` 的整数倍上。 */
export function snapTime(seconds: number): number {
  const value = Number.isFinite(seconds) ? seconds : 0;
  return Math.round(value / TIME_SNAP) * TIME_SNAP;
}

/** 秒数换成 `mm:ss.mmm`：播放头读数要能看出毫秒级的差别（与调试窗口共用一份实现）。 */
export { formatTimecode } from '../timecode';

/** 输入框给回来的可能是字符串（清空时是空串），统一收成非负的秒数。 */
export function toSeconds(value: unknown): number {
  const num = typeof value === 'number' ? value : Number(value);
  return Number.isFinite(num) && num > 0 ? num : 0;
}
