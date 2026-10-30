//! 噪域播放器专用的紧凑噪域表（谱面顶层字段 `simpleBlockArea`）。
//!
//! 与官方 `blockAreaList` 记的是**同一件事**：一堆「某块矩形在某段时间里是什么颜色」的
//! 条目。区别只在写法 —— 那边一条是一个 JSON 对象（两个百分比、四个时间、一个颜色，
//! 约 170 字节），这边一条是 6~9 字节的二进制。噪域播放器一分钟的 720p 视频就能出
//! 八十多万条，同一份内容用 JSON 对象是 144 MB，用这套是 12 MB 左右；解析也快一个数量级
//! （几十万个小对象的 `serde` 反序列化 vs 一次 base64 + 顺序读整数）。
//!
//! 只由 DPR 的噪域播放器写，只有 DPS 读；官方谱面里不会出现这个字段，`blockAreaList`
//! 那边一个字节都不动，两种写法可以同时存在（条目接在后面）。
//!
//! 字段形状：
//!
//! ```json
//! "simpleBlockArea": { "cols": 170, "rows": 373, "data": "base64…" }
//! ```
//!
//! `cols` / `rows` 是采样网格，`data` 解出来是一串记录，每条：
//!
//! * `cell`：2 字节小端，格序号 `row * cols + col`；
//! * `t0`：LEB128 变长整数，开始时刻（毫秒），**相对上一条 `t0` 的差**
//!   （记录按开始时刻升序，差非负，绝大多数是 0 或一两帧，一个字节就够）；
//! * `t1`：LEB128 变长整数，持续时间（毫秒，相对本条 `t0`）；
//! * 颜色：3 字节 RGB（0~255）。
//!
//! 时间落到毫秒：噪域播放器最高 60 帧/秒，一格 16.7 ms，毫秒级误差看不出来。
//!
//! 语义与播放器写 `blockAreaList` 时一致：`appearTime = enableTime = t0`、
//! `disableTime = disappearTime = t1`（起止都是瞬间到位，既没有渐显也没有 Ready 闪烁）。

use super::BlockArea;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use macroquad::prelude::vec2;
use serde_json::Value;
use tracing::warn;

/// 谱面里放这张表的字段名。
pub const FIELD: &str = "simpleBlockArea";

/// 一条记录：`(格序号, 开始毫秒, 结束毫秒, RGB)`。
pub type Entry = (u16, u32, u32, [u8; 3]);

/// 把一堆记录压成 `data` 的字节串。
///
/// `progress` 每 4096 条回调一次 `(已处理条数, 总条数)`：写谱面这一步按条目报进度，
/// 一分钟的谱面要写几十万条，不报进度界面会一直停在一个数字上。
pub fn pack<I>(entries: I, mut progress: impl FnMut(u64, u64)) -> Vec<u8>
where
    I: IntoIterator<Item = Entry>,
{
    let entries = entries.into_iter();
    let total = entries.size_hint().1.unwrap_or_default() as u64;
    let mut data = Vec::with_capacity((total as usize).saturating_mul(7));
    let mut previous = 0u32;
    let mut done = 0u64;
    for (cell, start, end, color) in entries {
        data.extend_from_slice(&cell.to_le_bytes());
        put_varint(&mut data, start.saturating_sub(previous) as u64);
        put_varint(&mut data, end.saturating_sub(start) as u64);
        data.extend_from_slice(&color);
        previous = start;
        done += 1;
        if done % 4096 == 0 {
            progress(done, total);
        }
    }
    progress(done, total);
    data
}

/// base64（标准字母表，带 `=` 填充）。整段一次编码：`data` 最多几十 MB，多一份拷贝可以接受。
pub fn encode(data: &[u8]) -> String {
    STANDARD.encode(data)
}

/// `simpleBlockArea` 对象的**前半段**：`{"cols":170,"rows":373,"data":"`。
///
/// 后面的 base64 太长，调用方直接 `write_all` 进去，不必先拼成一个多大的 `String`。
pub fn field_prefix(cols: u32, rows: u32) -> String {
    format!(r#"{{"cols":{cols},"rows":{rows},"data":""#)
}

/// 解析谱面里的 `simpleBlockArea`。
///
/// 坏数据整段丢弃（只提示一句）—— 与 `blockAreaList` 的口径一致：一条坏的不该让整张谱面
/// 打不开。返回的 `index` / `group` / `slot` 先按顺序填好，调用方把它们和 `blockAreaList`
/// 的条目拼在一起之后要重新编号（`group_block_areas` 会重算 `group` / `slot`）。
pub fn parse(value: Option<&Value>) -> Vec<BlockArea> {
    let Some(value) = value else {
        return Vec::new();
    };
    let cols = value.get("cols").and_then(Value::as_u64).unwrap_or_default() as u32;
    let rows = value.get("rows").and_then(Value::as_u64).unwrap_or_default() as u32;
    let (Some(data), true) = (value.get("data").and_then(Value::as_str), cols > 0 && rows > 0) else {
        warn!("simpleBlockArea: 缺少 cols / rows / data，已忽略");
        return Vec::new();
    };
    let bytes = match STANDARD.decode(data) {
        Ok(bytes) => bytes,
        Err(error) => {
            warn!("simpleBlockArea: base64 解不开，已忽略：{error}");
            return Vec::new();
        }
    };

    let cells = cols as u64 * rows as u64;
    let (width, height) = (1. / cols as f32, 1. / rows as f32);
    let mut areas = Vec::new();
    let mut at = 0usize;
    let mut start = 0u32;
    while at + 2 <= bytes.len() {
        let cell = u16::from_le_bytes([bytes[at], bytes[at + 1]]) as u64;
        at += 2;
        let Some(delta) = varint(&bytes, &mut at) else {
            warn!("simpleBlockArea: 数据在 t0 处截断，后面的记录已忽略");
            break;
        };
        let Some(hold) = varint(&bytes, &mut at) else {
            warn!("simpleBlockArea: 数据在 t1 处截断，后面的记录已忽略");
            break;
        };
        if at + 3 > bytes.len() {
            warn!("simpleBlockArea: 数据在颜色处截断，后面的记录已忽略");
            break;
        }
        let color = [bytes[at], bytes[at + 1], bytes[at + 2]];
        at += 3;
        // 记完再算：截断时 `at` 已经越过这条，上面的告警不会重复刷
        start = start.saturating_add(delta as u32);
        if cell >= cells {
            warn!("simpleBlockArea: 格序号 {cell} 超出 {cols}x{rows} 网格，这一条已忽略");
            continue;
        }
        let column = (cell % cols as u64) as f32;
        let row = (cell / cols as u64) as f32;
        // 谱面坐标 y 向上，第 0 行在最上面
        let top = 1. - row * height;
        let left = column * width;
        let appear = start as f64 / 1000.;
        let disappear = (start as u64 + hold as u64) as f64 / 1000.;
        let index = areas.len();
        areas.push(BlockArea {
            bottom_left: vec2(left, top - height),
            top_right: vec2(left + width, top),
            appear_time: appear,
            enable_time: appear,
            disable_time: disappear,
            disappear_time: disappear,
            is_subtract: false,
            color: Some([color[0] as f32 / 255., color[1] as f32 / 255., color[2] as f32 / 255.]),
            rotate_events: Vec::new(),
            move_events: Vec::new(),
            scale_events: Vec::new(),
            index,
            group: index,
            slot: index,
        });
    }
    areas
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = *bytes.get(*at)?;
        *at += 1;
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(cols: u32, rows: u32, entries: Vec<Entry>) -> Value {
        let data = pack(entries, |_, _| {});
        let text = format!(r#"{{"cols":{cols},"rows":{rows},"data":"{}"}}"#, encode(&data));
        serde_json::from_str(&text).expect("生成的 JSON 应当能读回来")
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    /// 往返：几何、四个时间、颜色、序号都要与写进去的一致。
    #[test]
    fn round_trips_geometry_time_and_color() {
        let entries = vec![
            (0, 0, 500, [255, 0, 0]),
            (1, 0, 500, [0, 255, 0]),
            // 2x2 网格里 `row * cols + col = 2` 是左下角那格，时间接着上一条往后 500 ms
            (2, 500, 750, [0, 0, 255]),
        ];
        let areas = parse(Some(&object(2, 2, entries)));
        assert_eq!(areas.len(), 3);

        let first = &areas[0];
        assert!(close(first.top_right.x, 0.5) && close(first.top_right.y, 1.), "第 0 格在左上角");
        assert!(close(first.bottom_left.x, 0.) && close(first.bottom_left.y, 0.5));
        assert!(close(first.appear_time as f32, 0.) && close(first.disappear_time as f32, 0.5));
        assert_eq!(first.color, Some([1., 0., 0.]));

        let last = &areas[2];
        assert!(close(last.top_right.x, 0.5) && close(last.bottom_left.y, 0.), "第 2 格在左下角");
        assert!(close(last.appear_time as f32, 0.5) && close(last.disappear_time as f32, 0.75));
        assert_eq!(last.color, Some([0., 0., 1.]));

        // 序号按顺序填好，调用方拼完再重排
        assert!(areas.iter().enumerate().all(|(i, area)| area.index == i));
    }

    /// 坏数据只丢这一段，不 panic：base64 解不开、格序号超出网格都不该让谱面打不开。
    #[test]
    fn broken_data_is_dropped() {
        assert!(parse(Some(&serde_json::json!({ "cols": 2, "rows": 2, "data": "不是 base64" }))).is_empty());
        assert!(parse(Some(&serde_json::json!({ "data": "AAAA" }))).is_empty());
        assert!(parse(None).is_empty());

        // 网格只有 2x2，格序号 9 那条要丢掉，剩下的一条照读
        let data = pack([(9, 0, 100, [1, 2, 3]), (0, 0, 100, [4, 5, 6])], |_, _| {});
        let value = serde_json::json!({ "cols": 2, "rows": 2, "data": encode(&data) });
        let areas = parse(Some(&value));
        assert_eq!(areas.len(), 1);
        assert_eq!(areas[0].color, Some([4. / 255., 5. / 255., 6. / 255.]));
    }
}
