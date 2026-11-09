use crate::{info::ChartInfo};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use chardetng::EncodingDetector;
use concat_string::concat_string;
use macroquad::prelude::load_file;
use serde::Deserialize;
use serde_json::Value;
use std::{
    any::Any,
    collections::HashMap,
    fs,
    io::{Cursor, Read, Seek, Write},
    path::Path,
    sync::{Arc, Mutex},
};
use tracing::warn;
use zip::{write::FileOptions, CompressionMethod, ZipArchive, ZipWriter};

pub fn update_zip<R: Read + Seek>(zip: &mut ZipArchive<R>, patches: HashMap<String, Vec<u8>>) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut w = ZipWriter::new(Cursor::new(&mut buffer));
    let options = FileOptions::<()>::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o755);
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).unwrap();
        let path = match entry.enclosed_name() {
            Some(path) => path.to_owned(),
            None => continue,
        };
        let path = path.display().to_string();
        if entry.is_dir() {
            w.add_directory(path, options)?;
        } else if !patches.contains_key(&path) {
            w.start_file(&path, options)?;
            std::io::copy(&mut entry, &mut w)?;
        }
    }
    for (path, data) in patches.into_iter() {
        w.start_file(path, options)?;
        w.write_all(&data)?;
    }
    w.finish()?;
    Ok(buffer)
}

#[async_trait]
pub trait FileSystem: Send {
    async fn load_file(&mut self, path: &str) -> Result<Vec<u8>>;
    async fn exists(&mut self, path: &str) -> Result<bool>;
    fn list_root(&self) -> Result<Vec<String>>;
    fn clone_box(&self) -> Box<dyn FileSystem>;
    fn as_any(&mut self) -> &mut dyn Any;
}

#[derive(Clone)]
pub struct AssetsFileSystem(String);

#[async_trait]
impl FileSystem for AssetsFileSystem {
    async fn load_file(&mut self, path: &str) -> Result<Vec<u8>> {
        Ok(load_file(&concat_string!(self.0, path)).await?)
    }

    async fn exists(&mut self, path: &str) -> Result<bool> {
        // unlikely to be called
        Ok(load_file(&concat_string!(self.0, path)).await.is_ok())
    }

    fn list_root(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn clone_box(&self) -> Box<dyn FileSystem> {
        Box::new(self.clone())
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

/// wasm 侧的内存文件系统：谱面资源由页面调用 `fw_put` 塞进来，`load_file` 直接命中内存。
///
/// 没命中时退回 macroquad 的 HTTP 加载（`assets/…`），所以「页面内联资源」与
/// 「旁边摆一份 assets/ 目录」两种投递方式可以同时工作。
///
/// 为什么必须走内存而不是页面回调：引擎的 `load_file` 在 macroquad 的帧循环里被
/// await，页面侧用 Promise 回调回填会死锁（引擎不回到事件循环），用同步回调回填会
/// 直接 `RuntimeError: unreachable`。
#[cfg(target_arch = "wasm32")]
#[derive(Clone)]
pub struct WasmFileSystem(String);

/// 页面塞进来的资源表。首次 `fw_put` 时建立，之后只增不减。
#[cfg(target_arch = "wasm32")]
static WASM_FILES: Mutex<Option<HashMap<String, Vec<u8>>>> = Mutex::new(None);

/// 排查用：`WasmFileSystem::load_file` 依次请求过哪些路径（命中与否）。
#[cfg(target_arch = "wasm32")]
static FS_TRACE: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[cfg(target_arch = "wasm32")]
fn trace_load(msg: String) {
    if let Ok(mut trace) = FS_TRACE.lock() {
        if trace.len() < 512 {
            trace.push(msg);
        }
    }
}

/// 排查用：在 `debug_wasm_files()` 的 trace 里插一条标记（wasm 之外是空实现）。
pub fn wasm_mark(msg: &str) {
    #[cfg(target_arch = "wasm32")]
    trace_load(format!("MARK {msg}"));
    #[cfg(not(target_arch = "wasm32"))]
    let _ = msg;
}
/// 只查内存里的资源表（`fw_put` 塞的 + `globalThis.__dprw_files`），不走 HTTP 兜底。
///
/// 给「有就读、没有就算了」的可选文件用：`load_file` 在 wasm 上没命中时会去请求 URL，
/// 而 macroquad 对 404 的回调不保证回来（见 `WasmFileSystem::load_file` 的注释），
/// 拿它试探一个可能不存在的文件会把整个加载卡住。
#[cfg(target_arch = "wasm32")]
pub fn memory_file(key: &str) -> Option<Vec<u8>> {
    WASM_FILES
        .lock()
        .ok()
        .and_then(|files| files.as_ref()?.get(key).cloned())
        .or_else(|| js_file(key))
}

/// 非 wasm 构建没有内存资源表，恒为空。
#[cfg(not(target_arch = "wasm32"))]
pub fn memory_file(_key: &str) -> Option<Vec<u8>> {
    None
}

/// 页面在 `globalThis.__dprw_files` 上放的那张 `Map<string, Uint8Array>`。
///
/// `fw_put` 只能等 wasm 实例化之后调用，而宿主要求「先把谱面资源放好、引擎起来就能读」；
/// 页面在加载 wasm 之前把同一批字节挂到这个全局 Map 上，这里就能同步取到。
#[cfg(target_arch = "wasm32")]
fn js_file(key: &str) -> Option<Vec<u8>> {
    use wasm_bindgen::JsCast;

    let map = js_sys::Reflect::get(&js_sys::global(), &wasm_bindgen::JsValue::from_str("__dprw_files")).ok()?;
    // 这张 `Map` 是宿主（父窗口）造的，`dyn_into` 的 `instanceof` 检查跨 realm 一定失败。
    let map: js_sys::Map = map.unchecked_into();
    let value = map.get(&wasm_bindgen::JsValue::from_str(key));
    if value.is_undefined() || value.is_null() {
        return None;
    }
    // 不能 `dyn_into`：宿主的 `Uint8Array` 是父窗口那个 realm 里造的，`instanceof` 在本 realm
    // 一定为 false（跨 realm 的类型检查都这样），只能用 unchecked 直取。
    Some(value.unchecked_into::<js_sys::Uint8Array>().to_vec())
}

/// `globalThis.__dprw_files` 里所有键（只给 `list_root` 用）。
#[cfg(target_arch = "wasm32")]
fn js_keys() -> Vec<String> {
    use wasm_bindgen::JsCast;

    let Ok(map) = js_sys::Reflect::get(&js_sys::global(), &wasm_bindgen::JsValue::from_str("__dprw_files"))
        .map(|it| it.unchecked_into::<js_sys::Map>())
    else {
        return Vec::new();
    };
    let Ok(Some(iter)) = js_sys::try_iter(&map) else {
        return Vec::new();
    };
    let mut res = Vec::new();
    for entry in iter.flatten() {
        // `Map` 的迭代器给出来的是 `[key, value]` 数组，不是字符串；直接 `as_string()` 永远是 None。
        let key = if entry.is_array() {
            js_sys::Array::from(&entry).get(0)
        } else {
            entry
        };
        if let Some(key) = key.as_string() {
            res.push(key);
        }
    }
    res
}

#[cfg(target_arch = "wasm32")]
impl WasmFileSystem {
    fn get(&self, path: &str) -> Option<Vec<u8>> {
        let key = concat_string!(self.0, path);
        WASM_FILES
            .lock()
            .ok()
            .and_then(|files| files.as_ref()?.get(&key).cloned())
            .or_else(|| js_file(&key))
    }

    fn has(&self, path: &str) -> bool {
        self.get(path).is_some()
    }

    fn roots(&self) -> Vec<String> {
        let prefix = self.0.as_str();
        let mut keys: Vec<String> = WASM_FILES
            .lock()
            .ok()
            .and_then(|files| files.as_ref().map(|it| it.keys().cloned().collect()))
            .unwrap_or_default();
        keys.extend(js_keys());
        let mut res = Vec::new();
        for key in keys {
            let Some(rest) = key.strip_prefix(prefix) else {
                continue;
            };
            let name = rest.split('/').next().unwrap_or(rest);
            if !name.is_empty() && !res.iter().any(|it: &String| it == name) {
                res.push(name.to_string());
            }
        }
        res
    }
}

/// 页面把不存在的路径当成单页应用路由，返回的是 index.html：开头几字节就能认出来。
#[cfg(target_arch = "wasm32")]
fn is_html_page(data: &[u8]) -> bool {
    let head = &data[..data.len().min(15)];
    head.starts_with(b"<!doctype") || head.starts_with(b"<!DOCTYPE") || head.starts_with(b"<html")
}

#[cfg(target_arch = "wasm32")]
#[async_trait]
impl FileSystem for WasmFileSystem {
    async fn load_file(&mut self, path: &str) -> Result<Vec<u8>> {
        if let Some(data) = self.get(path) {
            trace_load(format!("hit {path}"));
            return Ok(data);
        }
        trace_load(format!("miss {path}"));
        // 先在带前缀的位置找（`respack/…`），找不到再退回 assets 根：引擎里有一部分音效
        // （`ending*.ogg`）本来就摆在根上，桌面版靠 `ResourcePack::load` 末尾那句
        // macroquad `load_file` 兜底。wasm 上那句在文件不存在时不会回调（HTTP 404 之后
        // future 永远 pending，整个加载就停在那里），所以把这条兜底挪到这里。
        let mut candidates: Vec<String> = Vec::new();
        if !self.0.is_empty() {
            candidates.push(concat_string!(self.0, path));
        }
        candidates.push(path.to_string());
        let mut last_err: Option<anyhow::Error> = None;
        for candidate in candidates {
            match load_file(&candidate).await {
                // 开发服务器（vite）对不存在的路径也会返回 200 + index.html 的 SPA 回退，
                // 直接拿去当资源用只会在解析阶段炸出「expected struct ChartInfo」这类怪错误。
                // 这里把 HTML 当成「没有这个文件」，`load_info` 之类多候选的调用才能继续往下试。
                Ok(data) if !is_html_page(&data) => return Ok(data),
                Ok(_) => {
                    trace_load(format!("html {candidate}"));
                    last_err = Some(anyhow::anyhow!("{} 不是资源文件（页面返回了 HTML）", candidate));
                }
                Err(err) => {
                    trace_load(format!("err {candidate}"));
                    last_err = Some(anyhow::anyhow!(err));
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("{path} 读不到")))
    }

    async fn exists(&mut self, path: &str) -> Result<bool> {
        if self.has(path) {
            return Ok(true);
        }
        let mut candidates: Vec<String> = Vec::new();
        if !self.0.is_empty() {
            candidates.push(concat_string!(self.0, path));
        }
        candidates.push(path.to_string());
        for candidate in candidates {
            if let Ok(data) = load_file(&candidate).await {
                if !is_html_page(&data) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn list_root(&self) -> Result<Vec<String>> {
        Ok(self.roots())
    }

    fn clone_box(&self) -> Box<dyn FileSystem> {
        Box::new(self.clone())
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

/// 页面把一份资源字节塞进 wasm 侧内存文件系统。`path` 用相对于 assets 根的名字，
/// 例如 `SP.json`、`info.txt`、`respack/hit.png`。
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn fw_put(path: &str, data: Vec<u8>) {
    let mut files = WASM_FILES.lock().expect("wasm 文件表被毒化");
    files
        .get_or_insert_with(HashMap::new)
        .insert(path.to_string(), data);
}

/// 排查用：wasm 这边看到的资源表键名（`fw_put` 塞的 + `globalThis.__dprw_files` 里的）。
#[cfg(target_arch = "wasm32")]
pub fn debug_wasm_files() -> Vec<String> {
    use wasm_bindgen::{JsCast, JsValue};

    let mut keys: Vec<String> = WASM_FILES
        .lock()
        .ok()
        .and_then(|files| files.as_ref().map(|it| it.keys().cloned().collect()))
        .unwrap_or_default();
    keys.extend(js_keys());
    keys.sort();

    // 在 wasm 侧就地核对全局对象，定位「页面有数据、引擎读不到」的原因。
    let global = js_sys::global();
    let probe = js_sys::Reflect::get(&global, &JsValue::from_str("__dprw_files"));
    let has = js_sys::Reflect::has(&global, &JsValue::from_str("__dprw_files")).unwrap_or(false);
    let mut deep = String::from("probe: no map");
    if let Ok(value) = probe {
        let map: js_sys::Map = value.unchecked_into();
        let key = JsValue::from_str("info.txt");
        let plain = map.get(&key);
        let has_key = map.has(&key);
        let get_fn = js_sys::Reflect::get(&map, &JsValue::from_str("get")).unwrap_or(JsValue::NULL);
        let get_fn: js_sys::Function = get_fn.unchecked_into();
        let called = get_fn.call1(&map, &key).unwrap_or(JsValue::NULL);
        deep = format!(
            "size={} has={} get={} call_get={} keys={:?}",
            map.size(),
            has_key,
            plain.js_typeof().as_string().unwrap_or_default(),
            called.js_typeof().as_string().unwrap_or_default(),
            js_keys(),
        );
    }
    keys.push(format!(
        "probe: typeof_global={} has={} {deep}",
        global.js_typeof().as_string().unwrap_or_default(),
        has,
    ));
    keys.push(format!(
        "trace: {:?}",
        FS_TRACE.lock().map(|it| it.clone()).unwrap_or_default()
    ));
    keys
}

#[derive(Clone)]
pub struct ExternalFileSystem(pub Arc<crate::dir::Dir>);
#[async_trait]
impl FileSystem for ExternalFileSystem {
    async fn load_file(&mut self, path: &str) -> Result<Vec<u8>> {
        #[cfg(target_arch = "wasm32")]
        {
            unimplemented!("cannot use external file system on wasm32: {path}")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut file = self.0.open(path)?;
            tokio::task::spawn_blocking(move || {
                let mut res = Vec::new();
                file.read_to_end(&mut res)?;
                Ok(res)
            })
            .await?
        }
    }

    async fn exists(&mut self, path: &str) -> Result<bool> {
        self.0.exists(path)
    }

    fn list_root(&self) -> Result<Vec<String>> {
        Ok(self.0.read_dir(".")?.filter_map(|res| res.ok()?.file_name().into_string().ok()).collect())
    }

    fn clone_box(&self) -> Box<dyn FileSystem> {
        Box::new(self.clone())
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

#[derive(Clone)]
pub struct ZipFileSystem(pub Arc<Mutex<ZipArchive<Cursor<Vec<u8>>>>>, String);

/// 打开一个 zip 归档；遇到文件名与 Info-ZIP Unicode 路径字段（0x7075）对不上的包时，
/// 去掉该字段再试一次。
///
/// 有的打包工具改过包内文件名却没同步这个扩展字段：CRC 对不上时 `zip` 判定整个中央目录不可信
/// （整个包打不开），CRC 对得上时它又会拿字段里那份旧名字当条目名（按新名字查不到）。
/// 两种都只影响这一个扩展字段，去掉它、按中央目录里存的文件名解析即可。
///
/// 公开给 DPR：「谱面融合」要把一个谱面包按条目重写成另一个包，用的就是它 + [`update_zip`]。
pub fn open_zip(bytes: Vec<u8>) -> Result<ZipArchive<Cursor<Vec<u8>>>> {
    if let Ok(zip) = ZipArchive::new(Cursor::new(bytes.clone())) {
        return Ok(zip);
    }
    Ok(ZipArchive::new(Cursor::new(strip_unicode_path_fields(bytes)))?)
}

/// 重写中央目录，去掉每个条目的 0x7075（Unicode 路径）扩展字段，其余字节原样保留。
/// 结构不认识（zip64、找不到 EOCD、条目越界）时原样返回，让调用方照常报错。
fn strip_unicode_path_fields(bytes: Vec<u8>) -> Vec<u8> {
    const EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    const CDFH_SIG: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
    const UNICODE_PATH_TAG: u16 = 0x7075;
    let Some(eocd) = bytes.windows(4).rposition(|window| window == EOCD_SIG) else {
        return bytes;
    };
    if eocd + 22 > bytes.len() {
        return bytes;
    }
    let entries = u16::from_le_bytes([bytes[eocd + 10], bytes[eocd + 11]]) as usize;
    let cd_size = u32::from_le_bytes([bytes[eocd + 12], bytes[eocd + 13], bytes[eocd + 14], bytes[eocd + 15]]) as usize;
    let cd_offset = u32::from_le_bytes([bytes[eocd + 16], bytes[eocd + 17], bytes[eocd + 18], bytes[eocd + 19]]) as usize;
    // zip64 的计数与偏移是占位值，这里不处理
    if entries == u16::MAX as usize || cd_size == u32::MAX as usize || cd_offset == u32::MAX as usize || cd_offset + cd_size > bytes.len() {
        return bytes;
    }

    let mut rebuilt = Vec::with_capacity(cd_size);
    let mut cursor = cd_offset;
    for _ in 0..entries {
        if cursor + 46 > bytes.len() || bytes[cursor..cursor + 4] != CDFH_SIG {
            return bytes;
        }
        let name_len = u16::from_le_bytes([bytes[cursor + 28], bytes[cursor + 29]]) as usize;
        let extra_len = u16::from_le_bytes([bytes[cursor + 30], bytes[cursor + 31]]) as usize;
        let comment_len = u16::from_le_bytes([bytes[cursor + 32], bytes[cursor + 33]]) as usize;
        let name_end = cursor + 46 + name_len;
        let extra_end = name_end + extra_len;
        let entry_end = extra_end + comment_len;
        if entry_end > bytes.len() {
            return bytes;
        }

        let mut kept_extra = Vec::with_capacity(extra_len);
        let mut field = name_end;
        while field + 4 <= extra_end {
            let tag = u16::from_le_bytes([bytes[field], bytes[field + 1]]);
            let size = u16::from_le_bytes([bytes[field + 2], bytes[field + 3]]) as usize;
            let field_end = field + 4 + size;
            if field_end > extra_end {
                return bytes;
            }
            if tag != UNICODE_PATH_TAG {
                kept_extra.extend_from_slice(&bytes[field..field_end]);
            }
            field = field_end;
        }

        let header_start = rebuilt.len();
        rebuilt.extend_from_slice(&bytes[cursor..cursor + 46]);
        rebuilt[header_start + 30..header_start + 32].copy_from_slice(&(kept_extra.len() as u16).to_le_bytes());
        rebuilt.extend_from_slice(&bytes[cursor + 46..name_end]);
        rebuilt.extend_from_slice(&kept_extra);
        rebuilt.extend_from_slice(&bytes[extra_end..entry_end]);
        cursor = entry_end;
    }

    let mut output = bytes[..cd_offset].to_vec();
    output.extend_from_slice(&rebuilt);
    output.extend_from_slice(&bytes[eocd..eocd + 12]);
    output.extend_from_slice(&(rebuilt.len() as u32).to_le_bytes());
    output.extend_from_slice(&(cd_offset as u32).to_le_bytes());
    output.extend_from_slice(&bytes[eocd + 20..]);
    output
}

impl ZipFileSystem {
    pub fn new(bytes: Vec<u8>) -> Result<Self> {
        let zip = open_zip(bytes)?;
        let root_dirs = zip
            .file_names()
            .filter(|it| it.ends_with('/') && it.find('/') == Some(it.len() - 1))
            .collect::<Vec<_>>();
        let only_one_folder =  zip.file_names().all(|name| {
            root_dirs.len() == 1 && name.starts_with(root_dirs[0])
        });
        let root = if only_one_folder { root_dirs[0].to_owned() } else { String::new() };
        Ok(Self(Arc::new(Mutex::new(zip)), root))
    }
}

#[async_trait]
impl FileSystem for ZipFileSystem {
    async fn load_file(&mut self, path: &str) -> Result<Vec<u8>> {
        let arc = Arc::clone(&self.0);
        let path = concat_string!(self.1, path);
        spawn_task(move || {
            let mut zip = arc.lock().unwrap();
            // 缺文件时 zip 只回一句「specified file not found in archive」，看不出是哪个路径；
            // 这里带上实际查询的名字，否则压缩包内缺文件的报错没法定位。
            let mut entry = zip.by_name(&path).with_context(|| format!("Failed to find {path} in archive"))?;
            let mut res = Vec::new();
            entry.read_to_end(&mut res)?;
            Ok(res)
        })
        .await
    }

    async fn exists(&mut self, path: &str) -> Result<bool> {
        Ok(self.0.lock().unwrap().by_name(&concat_string!(self.1, path)).is_ok())
    }

    fn list_root(&self) -> Result<Vec<String>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .file_names()
            .filter(|it| it.strip_prefix(&self.1).map_or(false, |it| !it.contains('/')))
            .map(str::to_owned)
            .collect())
    }

    fn clone_box(&self) -> Box<dyn FileSystem> {
        Box::new(self.clone())
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

/// 单个普通文件当成只有一项的文件系统。
///
/// 谱面可以只有一个 json（散装素材：一个谱面文件 + 任意音频 + 任意曲绘，各自在一条轨道上），
/// 这时 `load_info` 的 `fix_info` 会从这个 json 的 META 里认出曲名、难度与作者；
/// 音频与曲绘由调用方另外提供，不在这个文件系统里。
#[derive(Clone)]
pub struct SingleFileSystem {
    name: String,
    bytes: Arc<Vec<u8>>,
}

impl SingleFileSystem {
    pub fn new(name: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            bytes: Arc::new(bytes),
        }
    }
}

#[async_trait]
impl FileSystem for SingleFileSystem {
    async fn load_file(&mut self, path: &str) -> Result<Vec<u8>> {
        if path == self.name {
            Ok(self.bytes.as_ref().clone())
        } else {
            bail!("Failed to find {path}: only {} is available", self.name)
        }
    }

    async fn exists(&mut self, path: &str) -> Result<bool> {
        Ok(path == self.name)
    }

    fn list_root(&self) -> Result<Vec<String>> {
        Ok(vec![self.name.clone()])
    }

    fn clone_box(&self) -> Box<dyn FileSystem> {
        Box::new(self.clone())
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

pub struct PatchedFileSystem(pub Box<dyn FileSystem>, pub HashMap<String, Vec<u8>>);

#[async_trait]
impl FileSystem for PatchedFileSystem {
    async fn load_file(&mut self, path: &str) -> Result<Vec<u8>> {
        if let Some(data) = self.1.get(path) {
            Ok(data.clone())
        } else {
            self.0.load_file(path).await
        }
    }

    async fn exists(&mut self, path: &str) -> Result<bool> {
        Ok(self.0.exists(path).await? || self.1.contains_key(path))
    }

    fn list_root(&self) -> Result<Vec<String>> {
        let mut res = self.0.list_root()?;
        res.extend(self.1.keys().cloned());
        res.dedup();
        Ok(res)
    }

    fn clone_box(&self) -> Box<dyn FileSystem> {
        unimplemented!()
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

pub async fn spawn_task<R: Send + 'static>(f: impl FnOnce() -> Result<R> + Send + 'static) -> Result<R> {
    #[cfg(target_arch = "wasm32")]
    {
        f()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Ok(tokio::task::spawn_blocking(f).await??)
    }
}

fn infer_diff(info: &mut ChartInfo, level: &str) {
    if let Ok(val) = level
        .chars()
        .rev()
        .take_while(|it| it.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>()
        .parse::<u32>()
    {
        info.lvl = val as f32;
    }
}

fn info_from_kv<'a>(it: impl Iterator<Item = (&'a str, String)>, csv: bool) -> Result<ChartInfo> {
    let mut info = ChartInfo::default();
    for (key, value) in it {
        if csv && value.trim().is_empty() {
            continue;
        }
        let key = key.trim();
        if key == "Path" {
            continue;
        }
        if key == "Level" {
            infer_diff(&mut info, &value);
        }
        if key == "AspectRatio" {
            info.aspect_ratio = value.parse().context("invalid aspect ratio")?;
            continue;
        }
        if key == "BackgroundDim" {
            info.background_dim = value.parse().context("invalid background dim")?;
            continue;
        }
        if key == "NoteScale" || key == "ScaleRatio" {
            warn!("note scale is ignored");
            continue;
        }
        if key == "GlobalAlpha" {
            warn!("global alpha is ignored");
            continue;
        }
        //RPE160
        let mut deprecate = String::new();
        *match key {
            "Name" => &mut info.name,
            "Music" | "Song" => &mut info.music,
            "Chart" => &mut info.chart,
            "Image" | "Picture" => &mut info.illustration,
            "Level" => &mut info.difficulty,
            "Illustrator" => &mut info.illustrator,
            "Artist" | "Composer" | "Musician" => &mut info.composer,
            "Charter" | "Designer" => &mut info.charter,
            //_ => bail!("Unknown key: {key}"),
            "LastEditTime" => &mut deprecate,
            "Length" => &mut deprecate,
            "EditTime" => &mut deprecate,
            _ => &mut deprecate,
        } = value;
    }
    Ok(info)
}

fn info_from_txt(text: &str) -> Result<ChartInfo> {
    // 跳过空行
    let mut it = text.lines().filter(|it| {!it.is_empty()}).peekable();
    let first = it.next();
    if first != Some("#") && first != Some("\u{feff}#") {
        bail!("expected the first line to be #");
    }
    let kvs = it
        .map(|line| -> Result<(&str, String)> {
            let Some((key, value)) = line.split_once(": ") else {
            bail!("expected \"Key: Value\"");
        };
            Ok((key, value.to_string()))
        })
        .collect::<Result<Vec<_>>>()?;
    info_from_kv(kvs.into_iter(), false)
}

fn info_from_csv(text: &str) -> Result<ChartInfo> {
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(Cursor::new(text));
    // shitty design
    let headers = reader.headers()?.iter().map(str::to_owned).collect::<Vec<_>>();
    let record = reader.into_records().last().ok_or_else(|| anyhow!("expected csv records"))??; // ??
    info_from_kv(
        headers
            .iter()
            .zip(record.into_iter())
            .map(|(key, value)| (key.as_str(), value.to_owned())),
        true,
    )
}

pub async fn fix_info(fs: &mut dyn FileSystem, info: &mut ChartInfo) -> Result<()> {
    async fn get(fs: &mut dyn FileSystem, path: &mut String) -> Result<Option<String>> {
        Ok(if fs.exists(path).await? { Some(std::mem::take(path)) } else { None })
    }
    let mut chart = get(fs, &mut info.chart).await?;
    let mut music = get(fs, &mut info.music).await?;
    let mut illustration = get(fs, &mut info.illustration).await?;
    fn put(desc: &str, status: &mut Option<String>, value: String) {
        if status.as_ref() == Some(&value) {
            return;
        }
        if status.is_some() {
            warn!("found multiple {}, using the first one", desc);
        } else {
            *status = Some(value);
        }
    }
    for file in fs.list_root().context("cannot list files")? {
        if let Some((_, ext)) = file.rsplit_once('.') {
            match ext.to_ascii_lowercase().as_str() {
                "json" | "pec" => {
                    put("charts", &mut chart, file);
                }
                _ => {}
            }
        }
    }
    if let Some(chart) = &chart {
        info.chart = chart.to_owned();
        if let Ok(s) = String::from_utf8(fs.load_file(&info.chart).await?) {
            if let Ok(mut value) = serde_json::from_str::<Value>(&s) {
                #[derive(Deserialize)]
                struct RPEMeta {
                    name: String,
                    difficulty: String,
                    background: String,
                    charter: String,
                    composer: Option<String>,
                    illustration: Option<String>,
                    song: String,
                }
                if let Ok(mut meta) = serde_json::from_value::<RPEMeta>(value["META"].take()) {
                    info.name = meta.name;
                    infer_diff(info, &meta.difficulty);
                    info.difficulty = meta.difficulty;
                    info.charter = meta.charter;
                    if let Some(val) = meta.composer {
                        info.composer = val;
                    }
                    if let Some(val) = meta.illustration {
                        info.illustrator = val;
                    }
                    if illustration.is_none() {
                        illustration = get(fs, &mut meta.background).await?;
                    }
                    if music.is_none() {
                        music = get(fs, &mut meta.song).await?;
                    }
                }
            }
        }
    } else {
        bail!("cannot find chart");
    }
    for file in fs.list_root().context("cannot list files")? {
        if let Some((_, ext)) = file.rsplit_once('.') {
            match ext.to_ascii_lowercase().as_str() {
                "mp3" | "ogg" | "wav" | "flac" | "aac" => {
                    put("music files", &mut music, file);
                }
                "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp" | "avif" | "ppm" => {
                    put("illustrations", &mut illustration, file);
                }
                _ => {}
            }
        }
    }
    if let Some(music) = music {
        info.music = music;
    }
    if let Some(illustration) = illustration {
        info.illustration = illustration;
    }
    Ok(())
}

fn bytes_to_text_auto(data: &[u8]) -> String {
    let mut det = EncodingDetector::new();
    det.feed(data, true);
    let encoding = det.guess(None, true);
    let (s, _, _) = encoding.decode(data);
    s.into_owned()
}

/// YAML 里标量可能是字符串、数字或布尔，统一取成文本。
fn yaml_scalar_text(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(it) => Some(it.clone()),
        serde_yaml::Value::Number(it) => Some(it.to_string()),
        serde_yaml::Value::Bool(it) => Some(it.to_string()),
        _ => None,
    }
}

/// 读 `:info` / `info.yml`。
///
/// 这两种文件的约定与其它来源不同：`difficulty` 是**定数**（`18.0` 这种数字），
/// `level` 是**难度文本**（`AT Lv.18.0`）。ChartInfo 那边只认字符串，所以先在 YAML 层
/// 把定数搬到 `lvl`、把难度文本搬到 `difficulty`，再交给 serde 解析。
fn info_from_yaml(bytes: &[u8]) -> Result<ChartInfo> {
    let text = bytes_to_text_auto(bytes);
    let mut value: serde_yaml::Value = serde_yaml::from_str(&text)?;
    if let serde_yaml::Value::Mapping(map) = &mut value {
        let rating = map.get(serde_yaml::Value::String("difficulty".to_string())).and_then(yaml_scalar_text);
        let level = map.get(serde_yaml::Value::String("level".to_string())).and_then(yaml_scalar_text);
        if let Some(rating) = rating {
            if let Ok(rating_value) = rating.trim().parse::<f64>() {
                map.insert(
                    serde_yaml::Value::String("lvl".to_string()),
                    serde_yaml::Value::Number(serde_yaml::Number::from(rating_value)),
                );
            }
            map.insert(
                serde_yaml::Value::String("difficulty".to_string()),
                serde_yaml::Value::String(rating),
            );
        }
        if let Some(level) = level.filter(|it| !it.trim().is_empty()) {
            map.insert(
                serde_yaml::Value::String("difficulty".to_string()),
                serde_yaml::Value::String(level),
            );
        }
    }
    Ok(serde_yaml::from_value(value)?)
}

pub async fn load_info(fs: &mut dyn FileSystem) -> Result<ChartInfo> {
    let mut info = if let Ok(bytes) = fs.load_file(":info").await {
        info_from_yaml(&bytes)?
    } else if let Ok(bytes) = fs.load_file("info.yml").await {
        info_from_yaml(&bytes)?
    } else if let Ok(bytes) = fs.load_file("info.txt").await {
        info_from_txt(&bytes_to_text_auto(&bytes))?
    } else if let Ok(bytes) = fs.load_file("info.csv").await {
        info_from_csv(&bytes_to_text_auto(&bytes))?
    } else {
        warn!("none of info.yml, info.txt and info.csv is found, inferring");
        let mut info = ChartInfo::default();
        fix_info(fs, &mut info).await?;
        info
    };
    
    split_difficulty(&mut info);

    Ok(info)
}

/// 把整串难度文本拆成档位与定数：`AT Lv.18.0` → `diff = AT`、`lvl = 18.0`。
///
/// 定数不一定是个数字（例如 `SP Lv.?`），解析不出来时只取档位、`lvl` 留原值 ——
/// 不然整段都跳过，`diff` 会停在默认的 `UK`，界面上的「难度标识」就跟着错。
fn split_difficulty(info: &mut ChartInfo) {
    if info.difficulty.is_empty() {
        return;
    }
    let raw_diff_text = &info.difficulty;
    let mut parts = raw_diff_text.split_whitespace();
    if let (Some(diff_tag), Some(lv_segment)) = (parts.next(), parts.next()) {
        if let Some(lv_num_str) = lv_segment.strip_prefix("Lv.") {
            info.diff = diff_tag.to_string();
            if let Ok(lv_val) = lv_num_str.parse::<f32>() {
                info.lvl = lv_val;
            }
        }
    }
}

pub fn fs_from_file(path: &Path) -> Result<Box<dyn FileSystem + Send + Sync + 'static>> {
    let meta = fs::metadata(path)?;
    Ok(if meta.is_file() {
        let bytes = fs::read(path).with_context(|| format!("failed to read from {}", path.display()))?;
        // `.pez` / `.zip` 必须是合法包；其余单文件（散装谱面的 `chart.json`）当成只有一项的
        // 文件系统，`load_info` 的 `fix_info` 才能从它里面认出谱面。
        if is_archive_name(path) {
            Box::new(ZipFileSystem::new(bytes).with_context(|| format!("cannot open {} as zip archive", path.display()))?)
        } else {
            match ZipFileSystem::new(bytes.clone()) {
                Ok(zip) => Box::new(zip),
                Err(_) => Box::new(SingleFileSystem::new(file_name(path), bytes)),
            }
        }
    } else {
        Box::new(ExternalFileSystem(Arc::new(crate::dir::Dir::new(path)?)))
    })
}

/// 文件名（不含目录），空路径给空串。
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|it| it.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 扩展名是 `.pez` / `.zip` 的必须按包解析，读不开就报错，不退回单文件。
fn is_archive_name(path: &Path) -> bool {
    path.extension()
        .map(|it| it.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|it| it == "pez" || it == "zip")
}

/// 按名字取 assets 根下的文件系统。wasm 下返回内存实现（页面用 `fw_put` 填，
/// 没填的路径仍退回 HTTP），桌面下返回 `AssetsFileSystem`。
pub fn fs_from_assets(name: impl Into<String>) -> Result<Box<dyn FileSystem + Send + Sync + 'static>> {
    #[cfg(target_arch = "wasm32")]
    {
        Ok(Box::new(WasmFileSystem(name.into())))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Ok(Box::new(AssetsFileSystem(name.into())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `:info` / `info.yml` 里 `difficulty` 是定数（数字）、`level` 是难度文本。
    #[test]
    fn yaml_info_takes_difficulty_as_rating() {
        let info = info_from_yaml(
            "name: テスト\ndifficulty: 18.0\nlevel: AT Lv.18.0\ncharter: 名無し\n".as_bytes(),
        )
        .unwrap();
        assert_eq!(info.lvl, 18.0);
        assert_eq!(info.difficulty, "AT Lv.18.0");
        assert_eq!(info.level, "AT Lv.18.0");
        assert_eq!(info.name, "テスト");
    }

    /// 定数写成 `?` 的难度文本（`SP Lv.?`）也要取到档位，只把定数留成原值。
    #[test]
    fn difficulty_text_with_unknown_rating_keeps_tag() {
        let mut info = ChartInfo::default();
        info.difficulty = "SP Lv.?".to_string();
        split_difficulty(&mut info);
        assert_eq!(info.diff, "SP");
        assert_eq!(info.lvl, ChartInfo::default().lvl);
    }

    /// `difficulty` 写成字符串时同样当定数；没有 `level` 就保留原串。
    #[test]
    fn yaml_info_accepts_text_rating() {
        let info = info_from_yaml("name: Test\ndifficulty: \"15.6\"\n".as_bytes()).unwrap();
        assert_eq!(info.lvl, 15.6);
        assert_eq!(info.difficulty, "15.6");
    }
}

