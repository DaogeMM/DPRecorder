//! 歌词文件解析：把 `.srt` / `.lrc` / `.ass` / `.vtt` 摊成一行行带时间的歌词。
//!
//! 编辑窗口拿到这些行以后，**一句一个片段**地摊到一条特效轨上（每个片段是一个文字噪域），
//! 所以这里只做两件事：读出每句的起止时间、把文字里的样式标记去掉。
//!
//! - `.srt` / `.vtt` / `.ass` 自带每句的起止，`end` 有值；
//! - `.lrc` 只有每句的起点，`end` 留空，由前端拿「下一句的起点」补；最后一句没有下一句。
//!
//! 一句里再折行（字幕折行、ass 的 `\N`）的，每行各算一句、时间相同 —— 它们在时间轴上重叠，
//! 编辑窗口把同一时刻的几段上下错开一行摆。
//!
//! 编码不假定 UTF-8：LRC 里 GBK 很常见，先试 UTF-8，解不开再交给 chardetng 猜。

use std::path::Path;

use anyhow::{bail, Context, Result};
use chardetng::EncodingDetector;
use serde::{Deserialize, Serialize};

/// 一句歌词：[`Self::start`] 秒开始，[`Self::end`] 秒结束（`lrc` 里没有结束时间）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LyricLine {
    pub start: f64,
    /// 结束时间；`None` 表示这个格式里没有（`lrc`），由前端照下一句补。
    pub end: Option<f64>,
    pub text: String,
}

/// 认得的歌词扩展名（小写、不带点）。
pub const LYRIC_EXTENSIONS: [&str; 5] = ["srt", "lrc", "ass", "ssa", "vtt"];

/// 这个文件是不是歌词（只看扩展名）。
pub fn is_lyric(path: &Path) -> bool {
    path.extension()
        .map(|it| it.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|ext| LYRIC_EXTENSIONS.contains(&ext.as_str()))
}

/// 读一份歌词文件并解析；一行都没读出来时报错（由前端提示）。
pub fn parse(path: &Path) -> Result<Vec<LyricLine>> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("读取歌词文件失败：{}", path.display()))?;
    let text = decode(&bytes);
    let ext = path
        .extension()
        .map(|it| it.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let mut lines = match ext.as_str() {
        "srt" => parse_srt(&text),
        "vtt" => parse_srt(&text),
        "lrc" => parse_lrc(&text),
        "ass" | "ssa" => parse_ass(&text),
        other => bail!("认不出这个歌词格式：.{other}（支持 srt / lrc / ass / ssa / vtt）"),
    };
    for line in &mut lines {
        // 结束时间早于起点的是坏数据，当成「没有结束时间」，让前端按下一句补
        if line.end.is_some_and(|end| end < line.start) {
            line.end = None;
        }
    }
    lines.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    if lines.is_empty() {
        bail!("这份歌词里没读到带时间的句子：{}", path.display());
    }
    Ok(lines)
}

/// 把文件字节解成文本：UTF-8 解不开时交给 chardetng 猜编码，换行统一成 `\n`、去掉 BOM。
fn decode(bytes: &[u8]) -> String {
    let text = match std::str::from_utf8(bytes) {
        Ok(it) => it.to_string(),
        Err(_) => {
            let mut det = EncodingDetector::new();
            det.feed(bytes, true);
            let encoding = det.guess(None, true);
            encoding.decode(bytes).0.into_owned()
        }
    };
    text.trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

/// 把 `00:01:02,500` / `0:01:02.50` / `01:02.500` / `01:02` 这类时间戳换成秒。
///
/// 分隔符同时认 `:` 与 `,` / `.`（字幕用逗号、ass 用点），最后一段是小数就按小数算，
/// 前面的从高位往低位依次是时、分、秒 —— 位数不足就按低位对齐（`01:02` 是 1 分 2 秒）。
/// 不是时间戳的（`[ar:歌手]`、`[offset:+200]` 这类元信息）返回 `None`，调用方直接当标签丢掉。
fn timestamp(text: &str) -> Option<f64> {
    let raw = text.trim();
    if raw.is_empty() {
        return None;
    }
    let mut parts: Vec<&str> = raw.split([':', ',', '.']).map(str::trim).collect();
    let fraction = if raw.contains(',') || raw.contains('.') { parts.pop() } else { None };
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut seconds = 0.0;
    for part in parts {
        let value: f64 = part.parse().ok()?;
        seconds = seconds * 60.0 + value;
    }
    if let Some(frac) = fraction {
        let digits = frac.len().clamp(1, 9) as i32;
        let value: f64 = frac.parse().ok()?;
        seconds += value / 10f64.powi(digits);
    }
    Some(seconds)
}

/// 去掉文字里的样式标记：`<i>` / `<c.xxx>` / `<v 说话人>`、ass 的 `{\an8}`、增强 LRC 的 `<00:12.34>`。
///
/// 找不到配对的 `>` / `}` 时把那个开括号当普通字符（“I <3 you” 这种不该被吃掉后半句）。
fn plain(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find(['<', '{']) {
        let open = rest[at..].chars().next().unwrap_or('<');
        let close = if open == '<' { '>' } else { '}' };
        let after = &rest[at + open.len_utf8()..];
        match after.find(close) {
            Some(end) => {
                out.push_str(&rest[..at]);
                rest = &after[end + close.len_utf8()..];
            }
            None => {
                out.push_str(&rest[..at + open.len_utf8()]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// 一行 `起始 --> 结束`：字幕 / vtt 的每个 cue 都靠它定位。右边可能还挂着 vtt 的排版设置
/// （`align:start position:0%`），只取第一段。
fn cue_line(line: &str) -> Option<(f64, f64)> {
    let (left, right) = line.split_once("-->")?;
    let start = timestamp(left)?;
    let end = timestamp(right.trim().split_whitespace().next().unwrap_or_default())?;
    Some((start, end))
}

/// 一段 cue 的文字（可能折成好几行）各自成一句，时间相同。
fn push_text(out: &mut Vec<LyricLine>, start: f64, end: Option<f64>, body: &[String]) {
    for line in body {
        let text = plain(line);
        if !text.is_empty() {
            out.push(LyricLine { start, end, text });
        }
    }
}

/// srt 与 vtt 的结构一样：一行 `起始 --> 结束`，下面是这一段文字，空行分段。
///
/// 序号行、`WEBVTT` 头、`NOTE` 段都会被跳过（它们没有 `-->` 也没有正文要收）。
fn parse_srt(text: &str) -> Vec<LyricLine> {
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some((start, end)) = cue_line(line) else { continue };
        let mut body = Vec::new();
        while let Some(next) = lines.next() {
            if next.trim().is_empty() {
                break;
            }
            body.push(next.to_string());
        }
        push_text(&mut out, start, Some(end), &body);
    }
    out
}

/// lrc：`[mm:ss.xx]词`，一行可以挂好几个时间标签（`[00:12.34][00:20.00]词`）。
///
/// 只有起点没有终点；`[ar:]` / `[ti:]` / `[offset:]` 这类元信息标签直接丢掉。
/// `offset` 不参与换算：各家播放器对正负号的解释不一致，宁可原样不动让用户自己挪。
fn parse_lrc(text: &str) -> Vec<LyricLine> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut rest = line.trim();
        let mut starts = Vec::new();
        while let Some(tail) = rest.strip_prefix('[') {
            let Some((tag, after)) = tail.split_once(']') else { break };
            if let Some(at) = timestamp(tag) {
                starts.push(at);
            }
            rest = after.trim_start();
        }
        let text = plain(rest);
        if text.is_empty() {
            continue;
        }
        for start in starts {
            out.push(LyricLine { start, end: None, text: text.clone() });
        }
    }
    out
}

/// ass / ssa：只读 `[Events]` 段的 `Dialogue:` 行，列顺序由 `Format:` 行给。
///
/// `Comment:` 行与别的段（样式、定位）不要；一句里的 `\N` / `\n` 是折行，拆成各自成一句，
/// `\h` 是不断行空格，换成普通空格。`Format:` 缺失时按 ass / ssa 通用的列顺序兜底。
fn parse_ass(text: &str) -> Vec<LyricLine> {
    const DEFAULT_COLUMNS: [&str; 10] = [
        "Layer", "Start", "End", "Style", "Name", "MarginL", "MarginR", "MarginV", "Effect", "Text",
    ];
    let mut out = Vec::new();
    let mut columns: Vec<String> = DEFAULT_COLUMNS.iter().map(|it| it.to_string()).collect();
    let mut in_events = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_events = line.eq_ignore_ascii_case("[Events]");
            continue;
        }
        if !in_events {
            continue;
        }
        if let Some(format) = line.strip_prefix("Format:") {
            columns = format.split(',').map(|it| it.trim().to_string()).collect();
            continue;
        }
        let Some(body) = line.strip_prefix("Dialogue:") else { continue };
        let fields: Vec<&str> = body.splitn(columns.len(), ',').collect();
        let field = |name: &str| {
            columns
                .iter()
                .position(|it| it.eq_ignore_ascii_case(name))
                .and_then(|index| fields.get(index))
                .map(|it| it.trim())
        };
        let Some(start) = field("Start").and_then(timestamp) else { continue };
        let end = field("End").and_then(timestamp);
        let raw = field("Text").unwrap_or_default().replace("\\h", " ");
        let pieces: Vec<&str> = raw.split("\\N").flat_map(|it| it.split("\\n")).collect();
        for piece in pieces {
            let text = plain(piece);
            if !text.is_empty() {
                out.push(LyricLine { start, end, text });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 取（起点，文字）方便断言。
    fn views(lines: &[LyricLine]) -> Vec<(f64, &str)> {
        lines.iter().map(|it| (it.start, it.text.as_str())).collect()
    }

    #[test]
    fn timestamp_reads_every_style() {
        assert_eq!(timestamp("00:01:02,500"), Some(62.5));
        assert_eq!(timestamp("0:01:02.50"), Some(62.5));
        assert_eq!(timestamp("01:02.500"), Some(62.5));
        assert_eq!(timestamp("12:34"), Some(754.0));
        assert_eq!(timestamp("00:00:00"), Some(0.0));
        // 元信息标签不是时间
        assert_eq!(timestamp("ar:歌手"), None);
        assert_eq!(timestamp("offset:+200"), None);
        assert_eq!(timestamp(""), None);
    }

    #[test]
    fn plain_drops_only_markup() {
        assert_eq!(plain("{\\an8}第一句"), "第一句");
        assert_eq!(plain("<v 某人>第二句"), "第二句");
        assert_eq!(plain("<00:12.34>增强"), "增强");
        // 没有配对的括号当普通字符
        assert_eq!(plain("I <3 you"), "I <3 you");
    }

    #[test]
    fn srt_splits_wrapped_lines() {
        let text = "1\n00:00:01,000 --> 00:00:04,000\n第一句\n第二句\n\n2\n00:00:05,000 --> 00:00:07,500\n<i>第三句</i>\n";
        let lines = parse_srt(text);
        assert_eq!(views(&lines), vec![(1.0, "第一句"), (1.0, "第二句"), (5.0, "第三句")]);
        assert_eq!(lines[0].end, Some(4.0));
        assert_eq!(lines[2].end, Some(7.5));
    }

    #[test]
    fn vtt_shares_the_srt_reader() {
        let text = "WEBVTT\n\nNOTE 这一段不要\n\n1\n00:01.000 --> 00:02.500 align:start position:0%\n<c.yellow>嗨</c>\n";
        let lines = parse_srt(text);
        assert_eq!(views(&lines), vec![(1.0, "嗨")]);
        assert_eq!(lines[0].end, Some(2.5));
    }

    #[test]
    fn lrc_reads_tags_and_drops_meta() {
        let text = "[ti:歌名]\n[offset:+200]\n[00:01.00]第一句\n[00:03.50][00:10.00]重复的一句\n[00:05.2]\n";
        let lines = parse_lrc(text);
        assert_eq!(
            views(&lines),
            vec![(1.0, "第一句"), (3.5, "重复的一句"), (10.0, "重复的一句")]
        );
        // lrc 没有终点
        assert!(lines.iter().all(|it| it.end.is_none()));
    }

    #[test]
    fn ass_reads_dialogue_and_splits_breaks() {
        let text = "[Script Info]\nTitle: x\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:04.00,Default,,0,0,0,,{\\an8}第一句\\N第二句\nComment: 0,0:00:05.00,0:00:06.00,Default,,0,0,0,,不要\nDialogue: 0,0:00:07.00,0:00:09.00,Default,,0,0,0,,带,逗号的一句\n";
        let lines = parse_ass(text);
        assert_eq!(
            views(&lines),
            vec![(1.0, "第一句"), (1.0, "第二句"), (7.0, "带,逗号的一句")]
        );
        assert_eq!(lines[0].end, Some(4.0));
    }

    #[test]
    fn ass_follows_format_column_order() {
        let text = "[Events]\nFormat: Start, End, Text, Layer\nDialogue: 0:00:02.00,0:00:03.00,先文字,0\n";
        let lines = parse_ass(text);
        assert_eq!(views(&lines), vec![(2.0, "先文字")]);
    }

    #[test]
    fn decode_falls_back_to_gbk() {
        // 歌词文件里 GBK 比 UTF-8 还常见，解不开时得能猜回来
        let (gbk, _, _) = encoding_rs::GBK.encode("忽然之间我听见了你的声音也看见了你的心");
        assert_eq!(decode(&gbk), "忽然之间我听见了你的声音也看见了你的心");
        assert_eq!(decode("\u{feff}带 BOM".as_bytes()), "带 BOM");
    }

    #[test]
    fn parse_sorts_and_rejects_empty() {
        // 文件里的顺序不一定按时间写（歌里补一句、插一句很常见）：parse 负责按起点排好
        let path = std::env::temp_dir().join(format!("dpr-lyrics-sort-{}.lrc", std::process::id()));
        std::fs::write(&path, "[00:10.00]后\n[00:02.00]前\n").unwrap();
        let lines = parse(&path).unwrap();
        assert_eq!(views(&lines), vec![(2.0, "前"), (10.0, "后")]);

        // 一句带时间的都没有时要报错，别静悄悄铺一条空轨道
        std::fs::write(&path, "[ti:只有元信息]\n").unwrap();
        assert!(parse(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parse_picks_the_reader_by_extension() {
        // 扩展名决定走哪个解析器（vtt 与 srt 共用一套，ass / ssa 共用一套）
        let dir = std::env::temp_dir();
        let cases = [
            ("srt", "1\n00:00:01,000 --> 00:00:03,000\n字幕一句\n"),
            ("vtt", "WEBVTT\n\n00:00:01.000 --> 00:00:03.000\nvtt 一句\n"),
            ("lrc", "[00:01.00]歌词一句\n[00:03.00]\n"),
            (
                "ass",
                "[Events]\nFormat: Layer, Start, End, Text\nDialogue: 0,0:00:01.00,0:00:03.00,ass 一句\n",
            ),
            (
                "ssa",
                "[Events]\nFormat: Layer, Start, End, Text\nDialogue: 0,0:00:01.00,0:00:03.00,ssa 一句\n",
            ),
        ];
        for (ext, body) in cases {
            let path = dir.join(format!("dpr-lyrics-dispatch-{}.{ext}", std::process::id()));
            std::fs::write(&path, body).unwrap();
            let lines = parse(&path).unwrap_or_else(|e| panic!(".{ext} 没解析出来：{e}"));
            assert_eq!(lines.len(), 1, ".{ext}");
            assert_eq!(lines[0].start, 1.0, ".{ext}");
            // lrc 只有起点，结束时间留空，由前端照下一句补
            assert_eq!(lines[0].end, if ext == "lrc" { None } else { Some(3.0) }, ".{ext}");
            let _ = std::fs::remove_file(&path);
        }

        // 不认识的扩展名要说清楚支持哪些
        let path = dir.join(format!("dpr-lyrics-dispatch-{}.txt", std::process::id()));
        std::fs::write(&path, "[00:01.00]x\n").unwrap();
        assert!(parse(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
