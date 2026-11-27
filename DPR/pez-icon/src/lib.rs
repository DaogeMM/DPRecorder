//! `.pez` 文件的曲绘图标处理器（资源管理器进程内 COM 组件）。
//!
//! 资源管理器对同一个扩展名只认一个静态 `DefaultIcon`。要让每个 `.pez`
//! 显示各自的曲绘，必须把它注册成 ProgID 的 `shellex\IconHandler`：
//! shell 先通过 `IPersistFile::Load` 把文件路径交给本组件，再调
//! `IExtractIconW` 取图标。这里从 `.pez`（zip）里的 `info.txt` 读 `Picture:`
//! 指向的图片，生成多尺寸 `.ico` 缓存到 `%LOCALAPPDATA%\DPRecorder\pez-icons`，
//! `GetIconLocation` 把这个 ico 路径交回 shell，由 shell 负责加载与缩放。
//!
//! 任何一步失败都退回系统的静态图标，绝不能 panic：这个 DLL 跑在
//! explorer.exe 里，崩溃会导致整个桌面刷新。

#![cfg(windows)]

use std::ffi::c_void;
use std::io::Read;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use windows::core::{
	implement, Error, GUID, Interface, Ref, Result as WinResult, BOOL, HRESULT, PCWSTR, PWSTR,
};
use windows::Win32::Foundation::{
	CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_NOTIMPL, E_POINTER, HANDLE, S_FALSE,
};
use windows::Win32::System::Com::{
	IClassFactory, IClassFactory_Impl, IPersistFile, IPersistFile_Impl, IPersist_Impl, STGM,
};
use windows::Win32::UI::Shell::{IExtractIconW, IExtractIconW_Impl};
use windows::Win32::UI::WindowsAndMessaging::{LoadImageW, HICON, IMAGE_ICON, LR_LOADFROMFILE};

/// 图标处理器的 CLSID。写在注册表的 `CLSID\{...}\InprocServer32` 与
/// ProgID 的 `shellex\IconHandler` 下，两边必须一致。
pub const CLSID_PEZ_ICON: GUID = GUID::from_u128(0x8b2f0c4e_5a6d_4b7e_9c31_2f7d5a8e6b40);

/// 曲绘原图最大允许的字节数，防止把整张谱面塞进内存。
const MAX_PICTURE_BYTES: u64 = 32 * 1024 * 1024;
/// 曲绘原图最大边长（像素）。超过就按这个上限缩小，避免解码出巨图。
const MAX_PICTURE_SIDE: u32 = 8192;

#[implement(IExtractIconW, IPersistFile)]
struct PezIconHandler {
	/// `IPersistFile::Load` 传来的 `.pez` 路径；每个文件一个实例。
	source: Mutex<Option<PathBuf>>,
}

impl IExtractIconW_Impl for PezIconHandler_Impl {
	fn GetIconLocation(
		&self,
		_uflags: u32,
		psziconfile: PWSTR,
		cchmax: u32,
		piindex: *mut i32,
		pwflags: *mut u32,
	) -> WinResult<()> {
		let no_icon = || Error::from_hresult(S_FALSE);
		let source = self
			.source
			.lock()
			.ok()
			.and_then(|slot| slot.clone())
			.ok_or_else(no_icon)?;
		let icon = ensure_icon(&source).ok_or_else(no_icon)?;

		let wide: Vec<u16> = icon
			.as_os_str()
			.encode_wide()
			.chain(std::iter::once(0))
			.collect();
		if wide.len() > cchmax as usize {
			return Err(no_icon());
		}

		unsafe {
			std::ptr::copy_nonoverlapping(wide.as_ptr(), psziconfile.0, wide.len());
			if !piindex.is_null() {
				*piindex = 0;
			}
			if !pwflags.is_null() {
				// 没有 GIL_NOTFILENAME：交给 shell 从 ico 文件里取图。
				*pwflags = 0;
			}
		}
		Ok(())
	}

	fn Extract(
		&self,
		pszfile: &PCWSTR,
		_niconindex: u32,
		phiconlarge: *mut HICON,
		phiconsmall: *mut HICON,
		niconsize: u32,
	) -> WinResult<()> {
		let path = unsafe { pszfile.to_string() }.map_err(|_| Error::from_hresult(S_FALSE))?;
		if path.is_empty() {
			return Err(Error::from_hresult(S_FALSE));
		}

		let wide: Vec<u16> = Path::new(&path)
			.as_os_str()
			.encode_wide()
			.chain(std::iter::once(0))
			.collect();
		let small = (niconsize & 0xffff) as i32;
		let large = (niconsize >> 16) as i32;

		let mut any = false;
		unsafe {
			if !phiconsmall.is_null() && small > 0 {
				if let Some(icon) = load_icon(&wide, small) {
					*phiconsmall = icon;
					any = true;
				}
			}
			if !phiconlarge.is_null() && large > 0 {
				if let Some(icon) = load_icon(&wide, large) {
					*phiconlarge = icon;
					any = true;
				}
			}
		}

		if any {
			Ok(())
		} else {
			Err(Error::from_hresult(S_FALSE))
		}
	}
}

fn load_icon(path: &[u16], size: i32) -> Option<HICON> {
	unsafe {
		let handle: HANDLE = LoadImageW(
			None,
			PCWSTR(path.as_ptr()),
			IMAGE_ICON,
			size,
			size,
			LR_LOADFROMFILE,
		)
		.ok()?;
		Some(HICON(handle.0))
	}
}

impl IPersist_Impl for PezIconHandler_Impl {
	fn GetClassID(&self) -> WinResult<GUID> {
		Ok(CLSID_PEZ_ICON)
	}
}

impl IPersistFile_Impl for PezIconHandler_Impl {
	fn IsDirty(&self) -> HRESULT {
		S_FALSE
	}

	fn Load(&self, pszfilename: &PCWSTR, _dwmode: STGM) -> WinResult<()> {
		let path = unsafe { pszfilename.to_string() }.map_err(|_| Error::from_hresult(S_FALSE))?;
		if let Ok(mut slot) = self.source.lock() {
			*slot = Some(PathBuf::from(path));
		}
		Ok(())
	}

	fn Save(&self, _pszfilename: &PCWSTR, _fremember: BOOL) -> WinResult<()> {
		Err(Error::from_hresult(E_NOTIMPL))
	}

	fn SaveCompleted(&self, _pszfilename: &PCWSTR) -> WinResult<()> {
		Ok(())
	}

	fn GetCurFile(&self) -> WinResult<PWSTR> {
		Ok(PWSTR::null())
	}
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
	fn CreateInstance(
		&self,
		punkouter: Ref<windows::core::IUnknown>,
		riid: *const GUID,
		ppvobject: *mut *mut c_void,
	) -> WinResult<()> {
		unsafe {
			if ppvobject.is_null() {
				return Err(Error::from_hresult(E_POINTER));
			}
			*ppvobject = std::ptr::null_mut();
			if !punkouter.is_null() {
				return Err(Error::from_hresult(CLASS_E_NOAGGREGATION));
			}
			let handler: windows::core::IUnknown = PezIconHandler {
				source: Mutex::new(None),
			}
			.into();
			handler.query(riid, ppvobject).ok()?;
			Ok(())
		}
	}

	fn LockServer(&self, _flock: BOOL) -> WinResult<()> {
		Ok(())
	}
}

/// COM 入口。必须导出，资源管理器靠它创建图标处理器。
///
/// # Safety
/// 调用方（COM）保证 `ppv` 可写；本函数自己检查空指针。
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
	rclsid: *const GUID,
	riid: *const GUID,
	ppv: *mut *mut c_void,
) -> HRESULT {
	if ppv.is_null() {
		return E_POINTER;
	}
	*ppv = std::ptr::null_mut();
	if rclsid.is_null() || riid.is_null() || *rclsid != CLSID_PEZ_ICON {
		return CLASS_E_CLASSNOTAVAILABLE;
	}

	let factory: windows::core::IUnknown = Factory.into();
	factory.query(riid, ppv)
}

/// 组件一直留在进程里，不参与卸载判断。
#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
	S_FALSE
}

/// 让 `.pez` 显示它自己的曲绘。失败时返回 `None`，由调用方退回静态图标。
fn ensure_icon(pez: &Path) -> Option<PathBuf> {
	let meta = std::fs::metadata(pez).ok()?;
	if !meta.is_file() {
		return None;
	}

	let dir = icon_cache_dir()?;
	let target = dir.join(format!("{:016x}.ico", cache_key(pez, &meta)));
	if target.is_file() {
		return Some(target);
	}

	let (picture, level) = read_picture(pez)?;
	let composed = compose_icon(&picture, level.as_deref());
	let bytes = encode_ico(&composed)?;
	let tmp = dir.join(format!(
		"{:016x}.{}.tmp",
		cache_key(pez, &meta),
		std::process::id()
	));
	std::fs::write(&tmp, &bytes).ok()?;
	if std::fs::rename(&tmp, &target).is_err() {
		let _ = std::fs::remove_file(&tmp);
		if !target.is_file() {
			return None;
		}
	}
	// 清理缓存要遍历目录，放在后台线程里，而且不必每张都扫一遍。
	let generated = GENERATED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
	if generated % 8 == 0 {
		let prune_dir = dir.clone();
		std::thread::spawn(move || prune_cache(&prune_dir));
	}
	Some(target)
}

/// 缓存上限。每改一次谱面就会按新的修改时间另存一份 ico，旧文件不会再用到，
/// 浏览过很多谱面后缓存会越积越多；这里按最久未用的顺序清掉超出上限的部分。
/// 删掉的只是可以再生成的派生图标，不影响任何谱面数据。
const MAX_CACHE_FILES: usize = 512;
const MAX_CACHE_BYTES: u64 = 64 * 1024 * 1024;

/// 已经生成过多少张图标；用来把缓存清理摊到后台、每八张扫一次目录。
static GENERATED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn prune_cache(dir: &Path) {
	let Ok(read_dir) = std::fs::read_dir(dir) else {
		return;
	};
	let mut entries: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
	let mut total = 0u64;
	for entry in read_dir.flatten() {
		let path = entry.path();
		if path.extension().and_then(|it| it.to_str()) != Some("ico") {
			continue;
		}
		let Ok(meta) = entry.metadata() else {
			continue;
		};
		total += meta.len();
		entries.push((
			meta.modified().unwrap_or(std::time::UNIX_EPOCH),
			meta.len(),
			path,
		));
	}
	if entries.len() <= MAX_CACHE_FILES && total <= MAX_CACHE_BYTES {
		return;
	}
	entries.sort_by_key(|it| it.0);
	let mut count = entries.len();
	for (_, len, path) in entries {
		if count <= MAX_CACHE_FILES / 2 && total <= MAX_CACHE_BYTES / 2 {
			break;
		}
		if std::fs::remove_file(&path).is_ok() {
			total = total.saturating_sub(len);
			count -= 1;
		}
	}
}

/// 缓存目录：`%LOCALAPPDATA%\DPRecorder\pez-icons`。
fn icon_cache_dir() -> Option<PathBuf> {
	let base = std::env::var_os("LOCALAPPDATA")?;
	let dir = PathBuf::from(base).join("DPRecorder").join("pez-icons");
	std::fs::create_dir_all(&dir).ok()?;
	Some(dir)
}

/// 缓存键里带上版本号：合成算法一改（比如曲绘的摆法），旧缓存全部作废，
/// 否则用户机器上会一直留着按旧规则画出来的图标。
const CACHE_VERSION: u32 = 5;

/// 用路径 + 文件大小 + 修改时间做缓存键：谱面换掉以后会生成新的 ico。
fn cache_key(pez: &Path, meta: &std::fs::Metadata) -> u64 {
	use std::collections::hash_map::DefaultHasher;
	use std::hash::{Hash, Hasher};
	use std::time::UNIX_EPOCH;

	let mut hasher = DefaultHasher::new();
	CACHE_VERSION.hash(&mut hasher);
	pez.to_string_lossy().hash(&mut hasher);
	meta.len().hash(&mut hasher);
	if let Ok(modified) = meta.modified() {
		if let Ok(since_epoch) = modified.duration_since(UNIX_EPOCH) {
			since_epoch.as_nanos().hash(&mut hasher);
		}
	}
	hasher.finish()
}

/// 从 `.pez` 里读出曲绘图：优先 `info.txt` 的 `Picture:`，
/// 其次按常见文件名找图，最后才退而求其次取第一张图片。
fn read_picture(pez: &Path) -> Option<(image::DynamicImage, Option<String>)> {
	let file = std::fs::File::open(pez).ok()?;
	let mut archive = zip::ZipArchive::new(file).ok()?;

	let names: Vec<String> = archive.file_names().map(str::to_string).collect();
	let info = read_info(&mut archive, &names);
	let picture_name = find_picture_name(&names, info.picture.as_deref())?;
	let index = archive.index_for_name(&picture_name)?;

	let mut entry = archive.by_index(index).ok()?;
	if entry.size() > MAX_PICTURE_BYTES {
		return None;
	}
	let mut bytes = Vec::with_capacity(entry.size() as usize);
	entry
		.by_ref()
		.take(MAX_PICTURE_BYTES)
		.read_to_end(&mut bytes)
		.ok()?;

	if bytes.is_empty() {
		return None;
	}

	let picture = image::load_from_memory(&bytes).ok()?;
	if picture.width() == 0 || picture.height() == 0 {
		return None;
	}
	if picture.width() > MAX_PICTURE_SIDE || picture.height() > MAX_PICTURE_SIDE {
		let scale = MAX_PICTURE_SIDE as f64 / picture.width().max(picture.height()) as f64;
		let width = ((picture.width() as f64 * scale) as u32).max(1);
		let height = ((picture.height() as f64 * scale) as u32).max(1);
		return Some((
			picture.resize_exact(width, height, image::imageops::FilterType::Triangle),
			info.level,
		));
	}
	Some((picture, info.level))
}

/// `info.txt` 里与图标有关的两行。
#[derive(Default)]
struct Info {
	/// `Picture:` 指向的曲绘文件名。
	picture: Option<String>,
	/// `Level:` 整行文本，例如 `IN Lv.15.0`。
	level: Option<String>,
}

fn read_info(
	archive: &mut zip::ZipArchive<std::fs::File>,
	names: &[String],
) -> Info {
	let mut info = Info::default();
	let Some(index) = names
		.iter()
		.position(|name| name.eq_ignore_ascii_case("info.txt"))
	else {
		return info;
	};
	let Ok(mut entry) = archive.by_index(index) else {
		return info;
	};
	let mut text = String::new();
	if entry.read_to_string(&mut text).is_err() {
		return info;
	}
	for line in text.lines() {
		let Some((key, value)) = line.split_once(':') else {
			continue;
		};
		let key = key.trim();
		let value = value.trim();
		if value.is_empty() {
			continue;
		}
		if key.eq_ignore_ascii_case("Picture") {
			info.picture = Some(value.to_string());
		} else if key.eq_ignore_ascii_case("Level") {
			info.level = Some(value.to_string());
		}
	}
	info
}

fn find_picture_name(names: &[String], hint: Option<&str>) -> Option<String> {
	// 1. info.txt 里的 `Picture:` 是谱面自己写的曲绘文件名。
	if let Some(hint) = hint {
		if let Some(name) = names.iter().find(|name| name.eq_ignore_ascii_case(hint)) {
			return Some(name.clone());
		}
	}

	// 2. 常见命名：illustration / cover / background / bg，扩展名 png / jpg。
	let is_picture = |name: &str| {
		let lower = name.to_ascii_lowercase();
		(lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg"))
			&& ["illustration", "cover", "background", "bg", "picture"]
				.iter()
				.any(|hint| lower.contains(hint))
	};
	if let Some(name) = names.iter().find(|name| is_picture(name)) {
		return Some(name.clone());
	}

	// 3. 认不出曲绘就不画：宁可退回程序自带的静态图标，也不拿谱面里的
	//    其它贴图顶替。
	None
}

/// 图标合成边长：ICO 最大帧就是 256，先按这个尺寸合成再缩小。
const ICON_SIDE: u32 = 256;

/// 曲绘按「铺满 + 居中裁切」变成正方形（不拉伸），再在右下角叠难度名。
fn compose_icon(picture: &image::DynamicImage, level: Option<&str>) -> image::RgbaImage {
	let mut square = cover_square(picture);
	if let Some(level) = level {
		let label = difficulty_label(level);
		if !label.is_empty() {
			draw_level(&mut square, label);
		}
	}
	square
}

/// 取中间的正方形，再缩到 `ICON_SIDE`：曲绘比例不变，也不会先放大再裁掉。
///
/// 裁切与缩放都在原图上做，最后才转 RGBA：JPEG 曲绘是 RGB8，
/// 只剩 256px 时才转，省掉一次整图 rgba 拷贝。
fn cover_square(source: &image::DynamicImage) -> image::RgbaImage {
	let width = source.width().max(1);
	let height = source.height().max(1);
	let side = width.min(height);
	let x = (width - side) / 2;
	let y = (height - side) / 2;
	source
		.crop_imm(x, y, side, side)
		.thumbnail(ICON_SIDE, ICON_SIDE)
		.to_rgba8()
}

/// 取 `Level:` 行里的难度名：`IN Lv.15.0` -> `IN`；没写名字时返回空串（不画角标）。
fn difficulty_label(level: &str) -> &str {
	level
		.split(" Lv.")
		.next()
		.unwrap_or("")
		.trim()
		.split("Lv.")
		.next()
		.unwrap_or("")
		.trim()
}

/// 在图标右下角画难度名：白色粗体字 + 八方向黑色描边，不用底块。
///
/// 用 GDI 画到一块 32 位 DIB 上：GDI 不写 alpha，所以先把画布填成一个
/// 哨兵色，画完只把「不再是哨兵色」的像素拷回曲绘，其余像素保持原样。
fn draw_level(base: &mut image::RgbaImage, label: &str) {
	use windows::core::w;
	use windows::Win32::Foundation::{COLORREF, RECT};
	use windows::Win32::Graphics::Gdi::{
		CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW,
		SelectObject, SetBkMode, SetTextColor, ANTIALIASED_QUALITY, BITMAPINFO, BITMAPINFOHEADER,
		BI_RGB, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_BOTTOM,
		DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, FW_HEAVY, HGDIOBJ, OUT_TT_PRECIS, TRANSPARENT,
	};

	// 1,1,1 的 BGR，alpha 为 0：GDI 不会写出这个颜色。
	const SENTINEL: u32 = 0x0001_0101;

	let header = BITMAPINFO {
		bmiHeader: BITMAPINFOHEADER {
			biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
			biWidth: ICON_SIDE as i32,
			biHeight: -(ICON_SIDE as i32),
			biPlanes: 1,
			biBitCount: 32,
			biCompression: BI_RGB.0,
			..Default::default()
		},
		..Default::default()
	};
	let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();

	let memory = unsafe { CreateCompatibleDC(None) };
	if memory.is_invalid() {
		return;
	}
	let bitmap =
		unsafe { CreateDIBSection(Some(memory), &header, DIB_RGB_COLORS, &mut bits, None, 0) };
	let bitmap = match bitmap {
		Ok(bitmap) if !bits.is_null() => bitmap,
		_ => {
			unsafe {
				let _ = DeleteDC(memory);
			}
			return;
		}
	};
	let previous_bitmap = unsafe { SelectObject(memory, HGDIOBJ(bitmap.0)) };

	let pixels =
		unsafe { std::slice::from_raw_parts_mut(bits as *mut u32, (ICON_SIDE * ICON_SIDE) as usize) };
	pixels.fill(SENTINEL);

	let height = match label.chars().count() {
		0..=2 => 46,
		3..=4 => 38,
		_ => 30,
	};
	let font = unsafe {
		CreateFontW(
			-height,
			0,
			0,
			0,
			FW_HEAVY.0 as i32,
			0u32,
			0u32,
			0u32,
			DEFAULT_CHARSET,
			OUT_TT_PRECIS,
			CLIP_DEFAULT_PRECIS,
			ANTIALIASED_QUALITY,
			DEFAULT_PITCH.0 as u32,
			w!("Microsoft YaHei"),
		)
	};
	let previous_font = unsafe { SelectObject(memory, HGDIOBJ(font.0)) };
	unsafe {
		SetBkMode(memory, TRANSPARENT);
	}

	let mut text: Vec<u16> = label.encode_utf16().collect();
	let flags = DT_RIGHT | DT_BOTTOM | DT_SINGLELINE | DT_NOPREFIX;
	let margin = 8;
	let base_rect = RECT {
		left: margin,
		top: margin,
		right: ICON_SIDE as i32 - margin,
		bottom: ICON_SIDE as i32 - margin,
	};

	// 无底块：先沿八个方向描一圈黑边，再把白字压在中间，
	// 深色和浅色曲绘上都能看清。
	for (dx, dy) in [
		(-2, 0),
		(2, 0),
		(0, -2),
		(0, 2),
		(-2, -2),
		(2, -2),
		(-2, 2),
		(2, 2),
	] {
		let mut rect = RECT {
			left: base_rect.left + dx,
			top: base_rect.top + dy,
			right: base_rect.right + dx,
			bottom: base_rect.bottom + dy,
		};
		unsafe {
			SetTextColor(memory, COLORREF(0));
			DrawTextW(memory, &mut text, &mut rect, flags);
		}
	}
	let mut rect = base_rect;
	unsafe {
		SetTextColor(memory, COLORREF(0x00FF_FFFF));
		DrawTextW(memory, &mut text, &mut rect, flags);
	}

	for (index, pixel) in pixels.iter().enumerate() {
		let value = *pixel;
		if value == SENTINEL {
			continue;
		}
		let red = (value & 0xFF) as u8;
		let green = ((value >> 8) & 0xFF) as u8;
		let blue = ((value >> 16) & 0xFF) as u8;
		let x = index as u32 % ICON_SIDE;
		let y = index as u32 / ICON_SIDE;
		base.put_pixel(x, y, image::Rgba([red, green, blue, 255]));
	}

	unsafe {
		let _ = SelectObject(memory, previous_font);
		let _ = DeleteObject(HGDIOBJ(font.0));
		let _ = SelectObject(memory, previous_bitmap);
		let _ = DeleteObject(HGDIOBJ(bitmap.0));
		let _ = DeleteDC(memory);
	}
}

/// 把合成好的图标写成多尺寸 ICO。尺寸覆盖资源管理器用到的 16 / 32 / 48 / 256。
fn encode_ico(picture: &image::RgbaImage) -> Option<Vec<u8>> {
	use image::ImageEncoder;

	const SIZES: [u32; 4] = [16, 32, 48, 256];

	let rgba = picture;
	let mut frames: Vec<(u32, Vec<u8>)> = Vec::new();
	for size in SIZES {
		let resized =
			image::imageops::resize(rgba, size, size, image::imageops::FilterType::Lanczos3);
		let mut png = Vec::new();
		image::codecs::png::PngEncoder::new(&mut png)
			.write_image(resized.as_raw(), size, size, image::ColorType::Rgba8)
			.ok()?;
		frames.push((size, png));
	}

	let count = frames.len() as u16;
	let mut out = Vec::new();
	out.extend_from_slice(&0u16.to_le_bytes()); // reserved
	out.extend_from_slice(&1u16.to_le_bytes()); // 1 = icon
	out.extend_from_slice(&count.to_le_bytes());

	let mut offset = 6 + 16 * frames.len() as u32;
	for (size, data) in &frames {
		let side = if *size == 256 { 0 } else { *size as u8 };
		out.push(side);
		out.push(side);
		out.push(0); // 调色板颜色数
		out.push(0); // reserved
		out.extend_from_slice(&1u16.to_le_bytes()); // planes
		out.extend_from_slice(&32u16.to_le_bytes()); // 位深
		out.extend_from_slice(&(data.len() as u32).to_le_bytes());
		out.extend_from_slice(&offset.to_le_bytes());
		offset += data.len() as u32;
	}
	for (_, data) in &frames {
		out.extend_from_slice(data);
	}
	Some(out)
}

#[cfg(test)]
mod tests {
	use super::*;
	use image::ImageEncoder;
	use std::io::Write;

	/// 造一个最小的 .pez：info.txt 指向一张纯色 png，再验证能生成 ico。
	#[test]
	fn builds_ico_from_pez_picture() {
		let dir = std::env::temp_dir().join(format!("pez-icon-test-{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		let pez = dir.join("test.pez");

		let picture = image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 120, 40, 255]));
		let mut png = Vec::new();
		image::codecs::png::PngEncoder::new(&mut png)
			.write_image(&picture, 64, 64, image::ColorType::Rgba8)
			.unwrap();

		{
			let file = std::fs::File::create(&pez).unwrap();
			let mut writer = zip::ZipWriter::new(file);
			let options = zip::write::SimpleFileOptions::default();
			writer.start_file("info.txt", options).unwrap();
			writer
				.write_all(b"Name: test\nPicture: illustration.png\nLevel: AT Lv.16.2\n")
				.unwrap();
			writer.start_file("illustration.png", options).unwrap();
			writer.write_all(&png).unwrap();
			writer.finish().unwrap();
		}

		let icon = ensure_icon(&pez).expect("应该生成 ico");
		let bytes = std::fs::read(&icon).unwrap();
		assert_eq!(&bytes[0..4], &[0, 0, 1, 0], "ICO 头");
		assert!(bytes.len() > 100);

		let _ = std::fs::remove_file(&pez);
		let _ = std::fs::remove_file(&icon);
	}

	#[test]
	fn difficulty_name_comes_from_level_line() {
		assert_eq!(difficulty_label("AT Lv.16.2"), "AT");
		assert_eq!(difficulty_label("IN Lv.15.0"), "IN");
		assert_eq!(difficulty_label("SP"), "SP");
		assert_eq!(difficulty_label("Lv.16"), "");
		assert_eq!(difficulty_label("  "), "");
	}

	/// 合成难度名后右下角必须真的被改写（纯色曲绘上画白字）。
	#[test]
	fn compose_draws_difficulty_in_corner() {
		let picture = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
			64,
			64,
			image::Rgba([20, 40, 80, 255]),
		));
		let plain = compose_icon(&picture, None);
		let labeled = compose_icon(&picture, Some("AT Lv.16.2"));

		let corner: Vec<usize> = (0..ICON_SIDE as usize)
			.flat_map(|y| (0..ICON_SIDE as usize).map(move |x| (x, y)))
			.filter(|(x, y)| *x > 120 && *y > 120)
			.map(|(x, y)| (y * ICON_SIDE as usize + x))
			.collect();
		assert!(corner
			.iter()
			.any(|index| plain.as_raw()[index * 4..index * 4 + 3]
				!= labeled.as_raw()[index * 4..index * 4 + 3]));
	}

	/// 手动检查合成效果：`$env:PEZ_PREVIEW="<pez 路径>"; $env:PEZ_PREVIEW_OUT="<png 路径>"; cargo test -p dpr-pez-icon dump_preview -- --ignored`
	#[test]
	#[ignore]
	fn dump_preview() {
		let pez = std::env::var("PEZ_PREVIEW").expect("PEZ_PREVIEW");
		let out = std::env::var("PEZ_PREVIEW_OUT").unwrap_or("preview.png".into());
		let (picture, level) = read_picture(Path::new(&pez)).expect("读曲绘失败");
		let icon = compose_icon(&picture, level.as_deref());
		icon.save(&out).expect("写 png 失败");
		println!("wrote {out} (level={level:?})");
	}
}
