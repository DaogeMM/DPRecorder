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

/// 演出包：「痉挛SP」谱面里顶替正中判定线的几张插画。
pub const SPAS: &str = "appendant.spas";

/// 演出包：「QZKAGO」谱面里整屏闪现的几张静图。
pub const QZK: &str = "appendant.qzk";

/// 皮肤包：「Rr」谱面用的按键贴图与 AP 打击特效。
pub const RR: &str = "appendant.rr";

/// 皮肤包里的必需文件：与 `core::resource::ResourcePack::load` 读的那几张一一对应，缺一个都加载失败。
pub const SKIN_FILES: &[&str] = &[
    "info.yml",
    "click.png",
    "click_mh.png",
    "drag.png",
    "drag_mh.png",
    "flick.png",
    "flick_mh.png",
    "hold.png",
    "hold_mh.png",
    "hit_fx.png",
];

/// 「岩浆噪域」材质包的必需文件。
pub const LAVA_FILES: &[&str] = &["netherrock.png", "lava.gif"];

/// 「屏幕进水」材质包的必需文件。
pub const WATER_FILES: &[&str] = &["water.png", "hit/hit-1.mp3", "hit/hit-2.mp3"];

/// 「痉挛SP」演出包的必需文件。
pub const SPAS_FILES: &[&str] = &["spasmodic_chan.png", "mopemope.png", "guang.png"];

/// 「QZKAGO」演出包的必需文件，下标与 `qzkago_sp` 里那张图的上场顺序一致。
pub const QZK_FILES: &[&str] = &["qzk_0.png", "qzk_1.png", "qzk_2.png", "qzk_3.png", "qzk_4.png", "qzk_5.png", "qzk_6.png"];

/// 噪域皮肤「已激活」那一档的贴图名：`noise_on.gif`（动画）优先，没有就用 `noise_on.png`。
pub const NOISE_ON_FILES: &[&str] = &["noise_on.png", "noise_on.gif"];

/// 噪域皮肤「未激活」那一档的贴图名：`noise_off.gif`（动画）优先，没有就用 `noise_off.png`。
///
/// 未生效（`appear..enable`）与生效结束后的淡出段（`disable..disappear`）都用这一档。
pub const NOISE_OFF_FILES: &[&str] = &["noise_off.png", "noise_off.gif"];


/// `appendant/` 的绝对路径。取不到程序路径时返回 `None`。
pub fn dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(APPENDANT_DIR))
}

/// 在指定目录里找一个包：`<dir>/<包名>.zip` 优先，其次同名目录。
pub fn pack_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let zip = dir.join(format!("{name}.zip"));
    if zip.is_file() {
        return Some(zip);
    }
    let folder = dir.join(name);
    folder.is_dir().then_some(folder)
}

/// 一个包在磁盘上的位置：`appendant/<包名>.zip` 优先，其次同名目录。
pub fn pack(name: &str) -> Option<PathBuf> {
    pack_in(&dir()?, name)
}

/// 从包里读一条资源。包不存在、或包里没有这条，都返回 `None`。
pub fn entry(name: &str, path: &str) -> Option<Vec<u8>> {
    read_entry(&pack(name)?, path)
}

fn read_entry(pack: &Path, path: &str) -> Option<Vec<u8>> {
    if pack.is_dir() {
        return std::fs::read(dir_entry(pack, path)?).ok();
    }
    let mut zip = open_zip(pack)?;
    // `name_for_index` 借用整个 archive，先收一份名字列表再找，省得和 `by_index` 抢借用。
    let names: Vec<String> = (0..zip.len()).filter_map(|index| zip.name_for_index(index).map(str::to_owned)).collect();
    let index = names.iter().position(|name| name == path || name.ends_with(&format!("/{path}")))?;
    let mut item = zip.by_index(index).ok()?;
    let mut bytes = Vec::new();
    item.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// 打开包里的 zip；文件名与 Info-ZIP Unicode 路径字段对不上的包由 `crate::fs::open_zip` 兜底。
fn open_zip(pack: &Path) -> Option<zip::ZipArchive<std::io::Cursor<Vec<u8>>>> {
    crate::fs::open_zip(std::fs::read(pack).ok()?).ok()
}

/// 同名目录里的一条资源：直接命中，或解压时多套了一层文件夹。
fn dir_entry(pack: &Path, path: &str) -> Option<PathBuf> {
    let direct = pack.join(path);
    if direct.is_file() {
        return Some(direct);
    }
    for entry in std::fs::read_dir(pack).ok()?.flatten() {
        let nested = entry.path().join(path);
        if nested.is_file() {
            return Some(nested);
        }
    }
    None
}

/// 包里有没有这条资源（只看在不在，不读内容）。
pub fn has_entry(pack: &Path, path: &str) -> bool {
    if pack.is_dir() {
        return dir_entry(pack, path).is_some();
    }
    let Some(zip) = open_zip(pack) else {
        return false;
    };
    let suffix = format!("/{path}");
    (0..zip.len()).any(|index| zip.name_for_index(index).is_some_and(|name| name == path || name.ends_with(&suffix)))
}

/// 皮肤包是不是自带**完整的**噪域皮肤：`noise_on.*` 与 `noise_off.*` 两档各至少一张。
///
/// 只给了一档不算数 —— 少一档就整包当没带噪域贴图，噪域照旧用官方岩浆贴图。
pub fn has_noise_skin(pack: &Path) -> bool {
    let has = |files: &[&'static str]| files.iter().any(|path| has_entry(pack, path));
    has(NOISE_ON_FILES) && has(NOISE_OFF_FILES)
}

/// 包里缺了哪些文件（包整个不在就整张表都算缺）。顺序与 `files` 一致。
pub fn missing_files(pack: &Path, files: &[&'static str]) -> Vec<&'static str> {
    files.iter().copied().filter(|path| !has_entry(pack, path)).collect()
}

/// 包里的必需文件是不是都在。
pub fn has_files(pack: &Path, files: &[&'static str]) -> bool {
    files.iter().all(|path| has_entry(pack, path))
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

/// 「痉挛SP」第一段演出图（`SpasmodicChan`，方块形插画）。
pub fn spas_chan_image() -> Option<Vec<u8>> {
    entry(SPAS, "spasmodic_chan.png")
}

/// 「痉挛SP」第二段演出图（「光」，满幅横图）。
pub fn spas_guang_image() -> Option<Vec<u8>> {
    entry(SPAS, "guang.png")
}

/// 「痉挛SP」第三段演出图（「もぺもぺ」，满幅横图）。
pub fn spas_mopemope_image() -> Option<Vec<u8>> {
    entry(SPAS, "mopemope.png")
}

/// 「QZKAGO」演出包里第 `index` 张图（0..7，顺序见 `QZK_FILES`）。
pub fn qzk_image(index: usize) -> Option<Vec<u8>> {
    entry(QZK, QZK_FILES.get(index)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一个临时目录；测试结束由调用方删。
    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dps-appendant-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 同名目录（以及解压时多套一层文件夹）照样认。
    #[test]
    fn directory_pack_tolerates_wrapper_folder() {
        let root = temp_root("dir");
        let pack = root.join(LAVA);
        std::fs::create_dir_all(pack.join(LAVA)).unwrap();
        std::fs::write(pack.join(LAVA).join("lava.gif"), b"gif").unwrap();
        std::fs::write(pack.join("netherrock.png"), b"png").unwrap();

        assert_eq!(pack_in(&root, LAVA), Some(pack.clone()));
        assert!(has_entry(&pack, "netherrock.png"));
        assert!(has_entry(&pack, "lava.gif"));
        assert!(missing_files(&pack, LAVA_FILES).is_empty());
        assert_eq!(read_entry(&pack, "netherrock.png").as_deref(), Some(&b"png"[..]));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// zip 与同名目录同时存在时用 zip；缺文件时缺的条目点名报出来。
    #[test]
    fn zip_wins_and_missing_files_are_listed() {
        let root = temp_root("zip");
        let zip_path = root.join(format!("{WATER}.zip"));
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        for entry in ["water.png", "hit/hit-1.mp3"] {
            zip.start_file(entry, zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut zip, b"x").unwrap();
        }
        zip.finish().unwrap();
        std::fs::create_dir_all(root.join(WATER)).unwrap();

        assert_eq!(pack_in(&root, WATER), Some(zip_path.clone()));
        assert_eq!(missing_files(&zip_path, WATER_FILES), vec!["hit/hit-2.mp3"]);
        assert!(!has_files(&zip_path, WATER_FILES));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 噪域皮肤要两档齐全（`noise_on.*` + `noise_off.*`），只给一档不算；
    /// 每档 gif 与 png 都认。
    #[test]
    fn noise_skin_needs_both_states() {
        let root = temp_root("noise");
        let pack = root.join("skin");
        std::fs::create_dir_all(&pack).unwrap();

        std::fs::write(pack.join("noise_on.gif"), b"gif").unwrap();
        assert!(!has_noise_skin(&pack), "只有一档不算带噪域皮肤");

        std::fs::write(pack.join("noise_off.png"), b"png").unwrap();
        assert!(has_noise_skin(&pack));
        // 两档都认 gif 与 png：换成 noise_on.png / noise_off.gif 一样成立。
        std::fs::write(pack.join("noise_on.png"), b"png").unwrap();
        std::fs::write(pack.join("noise_off.gif"), b"gif").unwrap();
        assert!(has_noise_skin(&pack));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 皮肤包可以是一个**目录**（`respack/我的皮肤/info.yml` …），不一定是 zip；
    /// 目录包里的 `noise_on.*` / `noise_off.*` 同样算数，引擎也照常从目录里读文件。
    #[test]
    fn respack_folder_is_a_pack() {
        let root = temp_root("respack-dir");
        let pack = root.join("我的皮肤");
        std::fs::create_dir_all(&pack).unwrap();
        for file in SKIN_FILES {
            std::fs::write(pack.join(file), b"x").unwrap();
        }
        std::fs::write(pack.join("info.yml"), "name: 皮肤\n".as_bytes()).unwrap();
        std::fs::write(pack.join("noise_on.png"), b"png").unwrap();
        std::fs::write(pack.join("noise_off.png"), b"png").unwrap();

        assert!(missing_files(&pack, SKIN_FILES).is_empty(), "目录包不许按缺文件处理");
        assert!(has_noise_skin(&pack));

        // 引擎按路径打开皮肤包时走 `fs_from_file`：目录走 `ExternalFileSystem`，zip 走归档。
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut fs = crate::fs::fs_from_file(&pack).unwrap();
        assert_eq!(runtime.block_on(fs.load_file("info.yml")).unwrap(), "name: 皮肤\n".as_bytes());
        assert_eq!(runtime.block_on(fs.load_file("noise_on.png")).unwrap(), b"png");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Info-ZIP 的 Unicode 路径字段（0x7075）与中央目录里的文件名不一致时，`zip` 要么因为
    /// CRC 对不上判定整个包不可读，要么拿字段里那份旧名字当条目名；这类包（改过文件名却没
    /// 同步扩展字段的皮肤包）还是要能按包里真实的文件名读出来。
    #[test]
    fn stale_unicode_path_field_still_reads() {
        let root = temp_root("badcrc");
        let zip_path = root.join("bad.zip");
        write_zip_with_stale_unicode_path(&zip_path, "皮肤.png", "旧名字.png", b"png");

        let raw = std::fs::File::open(&zip_path).unwrap();
        assert!(zip::ZipArchive::new(raw).is_err(), "原始包应当因为 Unicode 路径字段的 CRC 读不出来");

        assert!(has_entry(&zip_path, "皮肤.png"));
        assert!(!has_entry(&zip_path, "旧名字.png"));
        assert_eq!(read_entry(&zip_path, "皮肤.png").as_deref(), Some(&b"png"[..]));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 手工造一个 zip：文件名带中文（因而带 0x7075 字段），该字段里是另一份旧名字、CRC 也是错的，
    /// 内容不压缩。
    fn write_zip_with_stale_unicode_path(path: &Path, name: &str, extra_name: &str, data: &[u8]) {
        use std::io::Write;

        let name_bytes = name.as_bytes();
        let extra_name_bytes = extra_name.as_bytes();
        let crc = crc32fast::hash(data);
        let mut file = std::fs::File::create(path).unwrap();

        let mut local = Vec::new();
        local.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
        local.extend_from_slice(&20u16.to_le_bytes()); // 版本
        local.extend_from_slice(&0x800u16.to_le_bytes()); // 文件名为 UTF-8
        local.extend_from_slice(&0u16.to_le_bytes()); // 不压缩
        local.extend_from_slice(&0u16.to_le_bytes()); // 时间
        local.extend_from_slice(&0u16.to_le_bytes()); // 日期
        local.extend_from_slice(&crc.to_le_bytes());
        local.extend_from_slice(&(data.len() as u32).to_le_bytes());
        local.extend_from_slice(&(data.len() as u32).to_le_bytes());
        local.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes()); // 本地头不带扩展字段
        local.extend_from_slice(name_bytes);
        file.write_all(&local).unwrap();
        file.write_all(data).unwrap();

        let mut extra = Vec::new();
        extra.extend_from_slice(&0x7075u16.to_le_bytes());
        extra.extend_from_slice(&(5u16 + extra_name_bytes.len() as u16).to_le_bytes());
        extra.push(1); // 版本号
        extra.extend_from_slice(&0xdead_beef_u32.to_le_bytes()); // 故意写错的 CRC
        extra.extend_from_slice(extra_name_bytes);

        let mut central = Vec::new();
        central.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
        central.extend_from_slice(&20u16.to_le_bytes()); // 版本
        central.extend_from_slice(&20u16.to_le_bytes()); // 解压所需版本
        central.extend_from_slice(&0x800u16.to_le_bytes()); // 文件名为 UTF-8
        central.extend_from_slice(&0u16.to_le_bytes()); // 不压缩
        central.extend_from_slice(&0u16.to_le_bytes()); // 时间
        central.extend_from_slice(&0u16.to_le_bytes()); // 日期
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        central.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // 注释长度
        central.extend_from_slice(&0u16.to_le_bytes()); // 起始磁盘
        central.extend_from_slice(&0u16.to_le_bytes()); // 内部属性
        central.extend_from_slice(&0u32.to_le_bytes()); // 外部属性
        central.extend_from_slice(&0u32.to_le_bytes()); // 本地头偏移
        central.extend_from_slice(name_bytes);
        central.extend_from_slice(&extra);

        let central_offset = (local.len() + data.len()) as u32;
        file.write_all(&central).unwrap();

        let mut eocd = Vec::new();
        eocd.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
        eocd.extend_from_slice(&0u16.to_le_bytes()); // 本磁盘号
        eocd.extend_from_slice(&0u16.to_le_bytes()); // 中央目录起始磁盘
        eocd.extend_from_slice(&1u16.to_le_bytes()); // 本磁盘条目数
        eocd.extend_from_slice(&1u16.to_le_bytes()); // 总条目数
        eocd.extend_from_slice(&(central.len() as u32).to_le_bytes());
        eocd.extend_from_slice(&central_offset.to_le_bytes());
        eocd.extend_from_slice(&0u16.to_le_bytes()); // 注释长度
        file.write_all(&eocd).unwrap();
    }
}
