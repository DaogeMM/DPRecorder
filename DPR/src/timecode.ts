/**
 * 时间码与秒数互转。
 *
 * 需要它的地方都是「手输 / 粘贴一个时间值」：时间轴上跳转、演出参数里那些以秒为单位的
 * 时刻。既能直接写秒数（`160`、`52.723`），也能写从播放器 / 视频里抄来的
 * `分:秒.毫秒`（`1:53.12` → 113.12）、`时:分:秒.毫秒`。
 */

/** 一段文字 → 秒数；认不出（空、负数、两段以上非数字）返回 null。 */
export function parseTimecode(text: string): number | null {
  const raw = text.trim();
  if (!raw) return null;
  if (raw.includes(':')) {
    const parts = raw.split(':');
    if (parts.length > 3) return null;
    let seconds = 0;
    for (const part of parts) {
      const value = Number(part);
      if (!Number.isFinite(value) || value < 0) return null;
      seconds = seconds * 60 + value;
    }
    return seconds;
  }
  const seconds = Number(raw);
  return Number.isFinite(seconds) && seconds >= 0 ? seconds : null;
}

/** 秒数 → `分:秒.毫秒`（分钟补两位、毫秒三位），与时间轴读数同一套。 */
export function formatTimecode(seconds: number): string {
  const value = Math.max(0, seconds || 0);
  const minutes = Math.floor(value / 60);
  // 四舍五入到毫秒后可能进位成整 60000，先按毫秒取整再拆秒，避免出现 `00:60.000`
  const millis = Math.round((value - minutes * 60) * 1000);
  const secs = Math.floor(millis / 1000);
  return `${String(minutes).padStart(2, '0')}:${String(secs).padStart(2, '0')}.${String(millis % 1000).padStart(3, '0')}`;
}

/**
 * 输入框里的一行文字 → 存进配置的秒数。
 *
 * 保持「配置里存的就是秒数」：`1:53.12` 落成 `113.12`，纯数字原样取回；
 * 认不出来返回 null，调用方把原文还原回输入框，别把用户敲到一半的字吞掉。
 */
export function secondsFromText(text: string): number | null {
  const seconds = parseTimecode(text);
  if (seconds == null) return null;
  return Math.round(seconds * 1000) / 1000;
}
