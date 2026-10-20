//! 附带资源包（`<程序目录>/appendant`）的读取。
//!
//! 这些素材（Minecraft 贴图与音效等）**不随程序分发**：用户把下载到的 zip 放进程序所在
//! 目录下的 `appendant/`，用到时再按需读取。缺包不算错误 —— 对应的效果静默不生效，
//! 由界面负责提示用户去下载。
//!
//! 包可以是 `appendant/<包名>.zip`，也可以是同名目录（开发时直接把文件摊在文件夹里更省事）。
//! zip 里允许把内容套一层文件夹（`appendant.lava/netherrock.png`），查找时会退化成
//! 「以 `/路径` 结尾」。

use std::{
    io::Read,
    path::{Path, PathBuf},
};

/// 程序目录下存放这些包的子目录名。
const APPENDANT_DIR: &str = "appendant";

/// 皮肤包：内置风格的官方材质。
pub const OFFICIAL: &str = "appendant.official";

/// 皮肤包：旧版风格材质。
pub const OLD: &str = "appendant.old";

/// 皮肤包：「报应SP」用的手绘数字、曲名与按键贴图。
pub const RETRI: &str = "appendant.retri";

/// 材质包：「岩浆噪域」用的下界岩贴图与岩浆帧。
pub const LAVA: &str = "appendant.lava";

/// 材质包：「屏幕进水」用的水面贴图与受伤音效。
pub const WATER: &str = "appendant.water";

/// `appendant/` 的绝对路径。取不到程序路径时返回 `None`。
pub fn dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(APPENDANT_DIR))
}

/// 一个包在磁盘上的位置：`appendant/<包名>.zip` 优先，其次同名目录。
pub fn pack(name: &str) -> Option<PathBuf> {
    let dir = dir()?;
    let zip = dir.join(format!("{name}.zip"));
    if zip.is_file() {
        return Some(zip);
    }
    let folder = dir.join(name);
    folder.is_dir().then_some(folder)
}

/// 从包里读一条资源。包不存在、或包里没有这条，都返回 `None`。
pub fn entry(name: &str, path: &str) -> Option<Vec<u8>> {
    read_entry(&pack(name)?, path)
}

fn read_entry(pack: &Path, path: &str) -> Option<Vec<u8>> {
    if pack.is_dir() {
        return std::fs::read(pack.join(path)).ok();
    }
    let file = std::fs::File::open(pack).ok()?;
    let mut zip = zip::ZipArchive::new(file).ok()?;
    // `name_for_index` 借用整个 archive，先收一份名字列表再找，省得和 `by_index` 抢借用。
    let names: Vec<String> = (0..zip.len()).filter_map(|index| zip.name_for_index(index).map(str::to_owned)).collect();
    let index = names.iter().position(|name| name == path || name.ends_with(&format!("/{path}")))?;
    let mut item = zip.by_index(index).ok()?;
    let mut bytes = Vec::new();
    item.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// 「屏幕进水」的水面贴图（16×16 PNG）。
pub fn water_image() -> Option<Vec<u8>> {
    entry(WATER, "water.png")
}

/// 「屏幕进水」的受伤音效（`hit/hit-1.mp3`、`hit/hit-2.mp3`）。缺哪个就少哪个。
pub fn hit_sfx() -> Vec<Vec<u8>> {
    ["hit/hit-1.mp3", "hit/hit-2.mp3"].into_iter().filter_map(|path| entry(WATER, path)).collect()
}

/// 「岩浆噪域」没生效那一段的下界岩贴图（16×16 PNG）。
pub fn netherrock_image() -> Option<Vec<u8>> {
    entry(LAVA, "netherrock.png")
}

/// 「岩浆噪域」生效那一段的岩浆动画（32×32、16 帧的 GIF）。
pub fn lava_image() -> Option<Vec<u8>> {
    entry(LAVA, "lava.gif")
}
