import type { DraftClip, NoiseMaterial } from '@/model';

import { defaultTextNoise } from './text-noise';

/**
 * 歌词文件 → 一条特效轨上的多个片段。
 *
 * 后端 `parse_editor_lyrics` 只把文件读成「一行一句 + 起止时间」，这里负责把它们拼成
 * `DraftClip`：一句一个片段，每个片段都是一个「文字噪域」，文字就是那一句。
 *
 * 时间怎么定：
 * - `.srt` / `.ass` / `.vtt` 自带每句的起止，照用；
 * - `.lrc` 只有起点，结束时间取**下一句的起点**（同一时刻的几句取同一个「下一个起点」，
 *   所以那几句的起止完全一样）；
 * - 最后一句没有下一句，固定给 [`LYRIC_TAIL`] 秒。
 *
 * 同一时刻的几句（字幕折行、原文与译文）各自成段、时间重叠，这里再把它们上下错开一行摆，
 * 不然几句会叠在同一个位置谁也看不清。字号整份统一，长句放不下就放不下，不自动缩。
 */

/** 后端解析出来的一句歌词（秒）。 */
export interface LyricLine {
  start: number;
  /** 结束时间；`lrc` 里没有，由下一句的起点补。 */
  end?: number | null;
  text: string;
}

/** 一句最短留这么久：两个时间戳撞在一起（或字幕写坏了）时，片段还看得见、拖得动。 */
export const LYRIC_MIN_LENGTH = 0.2;

/** 最后一句没有下一句时固定给这么长（秒）。 */
export const LYRIC_TAIL = 4;

/** 歌词的字高（占屏高比例）。整份统一，只有换字体 / 改字号才能让它跟着变。 */
export const LYRIC_FONT_SIZE = 0.16;

/** 同一时刻的几句上下错开一行（字高 × 默认行高）。 */
const LINE_STEP = LYRIC_FONT_SIZE * 1.25;

/** 上下错开时最多占屏幕中间这一段，句数太多就压紧，别把字挤出画面。 */
const STACK_SPAN = 0.86;

/** 两个时间戳差这么点就算同一时刻（时间轴按 0.1 秒对齐，够用）。 */
const SAME_TIME = 1e-3;

/**
 * 把歌词行拼成特效片段：一句一段，起止按上面的规则算，`material` 用草稿自己的噪域材质。
 *
 * 一次调用就是一批：同一批里顺号编号、批与批之间夹一个随机短码 —— 同一份歌词可以铺进
 * 已经有片段的特效轨（甚至铺两遍），片段 id 不会撞上已有的那些。
 */
export function lyricClips(
  laneId: string,
  lines: LyricLine[],
  material: NoiseMaterial,
): DraftClip[] {
  const rows = lines.filter((it) => it.text.trim().length > 0);
  if (rows.length === 0) return [];
  const batch = Math.random().toString(36).slice(2, 6);

  // 「下一个起点」只在所有句子的起点里找（不是数组里的下一项）：排序可能不严格，
  // 而且同一时刻的几句要落到同一个结束时间上。
  const starts = [...new Set(rows.map((it) => it.start))].sort((a, b) => a - b);
  const nextStart = (at: number) => starts.find((it) => it > at + SAME_TIME);

  // 同一时刻有几句：用来决定上下错开几行、每行错开多少
  const crowd = new Map<number, number[]>();
  rows.forEach((row, index) => {
    const key = starts.find((it) => Math.abs(it - row.start) <= SAME_TIME) ?? row.start;
    const group = crowd.get(key) ?? [];
    group.push(index);
    crowd.set(key, group);
  });

  return rows.map((row, index) => {
    const group = crowd.get(starts.find((it) => Math.abs(it - row.start) <= SAME_TIME) ?? row.start) ?? [index];
    const offset = group.indexOf(index);
    const step = group.length > 1 ? Math.min(LINE_STEP, STACK_SPAN / (group.length - 1)) : 0;
    const y = 0.5 + (offset - (group.length - 1) / 2) * step;
    const end = row.end ?? nextStart(row.start) ?? row.start + LYRIC_TAIL;
    return {
      id: `${laneId}-c${batch}${index + 1}`,
      start: row.start,
      length: Math.max(LYRIC_MIN_LENGTH, end - row.start),
      effect: {
        ...defaultTextNoise(),
        text: row.text,
        fontSize: LYRIC_FONT_SIZE,
        y,
        material: { ...material },
      },
    };
  });
}
