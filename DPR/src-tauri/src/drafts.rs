//! 编辑窗口的「草稿」：剪映那样的一个个项目。
//!
//! 一个草稿 = `<程序目录>/dpe/editor/drafts/<id>/draft.json`。草稿里**只记素材的原文件路径**
//! （谱面 / 音频 / 曲绘各一项，另外还有一份导入过的素材清单），不复制、不改动那些文件：
//! 素材属于用户自己的那份，草稿本身只有这一个几 KB 的 JSON。
//!
//! 原文件被挪走或删掉时草稿本身不会坏：`path` 照样留着，界面按「文件不在」显示，
//! 用户把文件放回去就还能用。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::common;

/// 谱面：落到「谱面」轨道上。目录（摊开的谱面）与 `.pez` / `.zip` 都算。
pub const KIND_CHART: &str = "chart";

/// 音频：落到「音乐」轨道上。
pub const KIND_MUSIC: &str = "music";

/// 曲绘：落到「曲绘」轨道上。
pub const KIND_COVER: &str = "cover";

/// 「导入素材」往下翻的层数上限：素材通常就摊在谱面包顶层，但允许三十一层嵌套。
const MAX_MEDIA_DEPTH: usize = 31;

/// 草稿里的一项素材（就是磁盘上的某个文件或谱面目录）。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct DraftMedia {
    /// 原文件的绝对路径；目录型谱面就是目录本身。
    pub path: String,
    /// 显示名（文件名或目录名）。
    pub name: String,
    /// [`KIND_CHART`] / [`KIND_MUSIC`] / [`KIND_COVER`]。
    pub kind: String,
    /// 素材库里归到哪个文件夹（[`DraftFolder::id`]）；空着就是摊在根上。
    pub folder: Option<String>,
}

impl Default for DraftMedia {
    fn default() -> Self {
        Self { path: String::new(), name: String::new(), kind: String::new(), folder: None }
    }
}

/// 素材库里的一个文件夹：导入一个谱面包（目录 / 压缩包）时按它里面的真实层级建，也可以自己新建来归类。
///
/// 文件夹可以套文件夹，`parent` 指向上级文件夹；最多三十一层，更深的目录摊在第三十一级里。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct DraftFolder {
    pub id: String,
    pub name: String,
    /// 上级文件夹（[`DraftFolder::id`]）；空着就是「全部素材」底下那一层。
    pub parent: Option<String>,
}

/// 草稿自己填的谱面信息（曲名 / 难度 / 谱师 / 曲师）。
///
/// 谱面里自带的那份信息（`info.txt`、谱面 json 的 META）只当兜底：草稿填了就以草稿为准，
/// 预览时按这份内容拼一份 `info.txt` 塞给引擎。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct DraftInfo {
    pub name: String,
    /// 形如 `IN Lv.15`（引擎会从中拆出难度标签与定数）。
    pub difficulty: String,
    pub charter: String,
    pub composer: String,
}

/// 手工写草稿 JSON 时不该被旧字段卡住，所以除了 id 之外都给默认值。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct EditorDraft {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// 创建 / 最后修改时间，Unix 毫秒（前端自己按本地时区显示）。
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub updated: u64,
    /// 谱面信息（曲名 / 难度 / 谱师 / 曲师），由用户自己填。
    #[serde(default)]
    pub info: DraftInfo,
    /// 草稿目录的绝对路径，只给界面显示用。写盘时会被清空，所以不进 draft.json。
    #[serde(default)]
    pub dir: String,
    /// 三条轨道上的素材；空着就是那条轨道上还没放东西。
    #[serde(default)]
    pub chart: Option<DraftMedia>,
    #[serde(default)]
    pub music: Option<DraftMedia>,
    #[serde(default)]
    pub cover: Option<DraftMedia>,
    /// 导入过的全部素材（素材库）。轨道上那三项是这里面的引用。
    #[serde(default)]
    pub media: Vec<DraftMedia>,
    /// 素材库里的文件夹（只有分组作用，素材还在原来的位置）。
    #[serde(default)]
    pub folders: Vec<DraftFolder>,
}

impl EditorDraft {
    /// 把某一项素材放到它对应的轨道上（按 `kind` 分派）。
    ///
    /// 同时把它挂进素材库：面板上拖进来的文件不一定先经过「导入」这一步。
    pub fn assign(&mut self, media: DraftMedia) {
        match media.kind.as_str() {
            KIND_CHART => self.chart = Some(media.clone()),
            KIND_MUSIC => self.music = Some(media.clone()),
            KIND_COVER => self.cover = Some(media.clone()),
            _ => return,
        }
        if !self.media.iter().any(|it| it.path == media.path && it.kind == media.kind) {
            self.media.push(media);
        }
    }
}

/// 草稿根目录：`<dpe>/editor/drafts`（没有就建）。
pub fn drafts_dir() -> Result<PathBuf> {
    let dir = common::editor_dir()?.join("drafts");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建草稿目录失败：{}", dir.display()))?;
    Ok(dir)
}

/// 草稿 id 就是目录名，只允许这几类字符 —— 不然 `id` 里掺进 `..` 就能读写到别处去。
fn check_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 64 {
        bail!("非法的草稿 id：{id}");
    }
    if !id
        .chars()
        .all(|it| it.is_ascii_alphanumeric() || it == '-' || it == '_')
    {
        bail!("非法的草稿 id：{id}");
    }
    Ok(())
}

fn draft_file(id: &str) -> Result<PathBuf> {
    check_id(id)?;
    Ok(drafts_dir()?.join(id).join("draft.json"))
}

/// 当前时间（Unix 毫秒）。
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|it| it.as_millis() as u64)
        .unwrap_or(0)
}

/// 读一个草稿。
pub fn load(id: &str) -> Result<EditorDraft> {
    let file = draft_file(id)?;
    let text = std::fs::read_to_string(&file)
        .with_context(|| format!("读取草稿失败：{}", file.display()))?;
    let mut draft: EditorDraft = serde_json::from_str(&text)
        .with_context(|| format!("草稿文件不是合法的 JSON：{}", file.display()))?;
    draft.dir = drafts_dir()?.join(id).to_string_lossy().to_string();
    normalize_folders(&mut draft);
    normalize_dirs(&mut draft);
    Ok(draft)
}

/// 把素材库的文件夹关系修一遍：上级已经不在了（手改过草稿 JSON、或者旧草稿）就退回根上，
/// 指着不存在的文件夹的素材也摊回根上 —— 否则那一层钻不进去，素材看着像丢了。
fn normalize_folders(draft: &mut EditorDraft) {
    let ids: std::collections::HashSet<String> =
        draft.folders.iter().map(|it| it.id.clone()).collect();
    for folder in &mut draft.folders {
        let dangling = folder
            .parent
            .as_ref()
            .is_some_and(|parent| parent == &folder.id || !ids.contains(parent));
        if dangling {
            folder.parent = None;
        }
    }
    for item in &mut draft.media {
        if item.folder.as_ref().is_some_and(|folder| !ids.contains(folder)) {
            item.folder = None;
        }
    }
}

/// 把「拿目录当谱面」的旧草稿就地摊平：谱面轨道换成目录里真正的那份谱面文件，
/// 素材库里的目录项摊成包里的各个文件。
///
/// 草稿只记路径，所以这里认不出来的（包已经被挪走等）直接丢掉，不打断读取。
/// 只动轨道与素材库的内容，**不替用户决定别的轨道放什么**。
fn normalize_dirs(draft: &mut EditorDraft) {
    let has_slot_dir = [&draft.chart, &draft.music, &draft.cover]
        .into_iter()
        .flatten()
        .any(|it| Path::new(&it.path).is_dir());
    let has_media_dir = draft.media.iter().any(|it| Path::new(&it.path).is_dir());
    if !has_slot_dir && !has_media_dir {
        return;
    }
    let sources = std::mem::take(&mut draft.media);
    for item in sources {
        if !Path::new(&item.path).is_dir() {
            push_media(&mut draft.media, item);
            continue;
        }
        // 旧草稿把目录直接挂进了素材库：摊成这一层里的各个文件（包里再深的部分也归到同一层）
        let found = media_import_in_dir(Path::new(&item.path)).unwrap_or_default();
        for mut at in found {
            at.media.folder = item.folder.clone();
            push_media(&mut draft.media, at.media);
        }
    }
    if let Some(old) = draft.chart.take_if(|it| Path::new(&it.path).is_dir()) {
        draft.chart = chart_file_in(Path::new(&old.path))
            .and_then(|file| media_from_path(&file).ok());
    }
    // 音乐 / 曲绘轨道上挂的是目录时没有靠谱的对应文件，只能摘掉让用户自己放。
    if draft.music.as_ref().is_some_and(|it| Path::new(&it.path).is_dir()) {
        draft.music = None;
    }
    if draft.cover.as_ref().is_some_and(|it| Path::new(&it.path).is_dir()) {
        draft.cover = None;
    }
}

fn push_media(media: &mut Vec<DraftMedia>, item: DraftMedia) {
    if !media
        .iter()
        .any(|it| it.kind == item.kind && it.path == item.path)
    {
        media.push(item);
    }
}

/// 写回一个草稿（顺手把 `updated` 刷成现在）。
pub fn save(mut draft: EditorDraft) -> Result<EditorDraft> {
    check_id(&draft.id)?;
    draft.updated = now_ms();
    if draft.name.trim().is_empty() {
        draft.name = "未命名草稿".to_string();
    }
    let dir = drafts_dir()?.join(&draft.id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建草稿目录失败：{}", dir.display()))?;
    // `dir` 只是算出来的显示值：存盘前清掉，免得草稿 JSON 绑死在这台机器的路径上。
    let mut stored = draft.clone();
    stored.dir = String::new();
    let text = serde_json::to_string_pretty(&stored)?;
    let file = dir.join("draft.json");
    // 覆盖前把上一版留一份：改坏了还能从 `draft.bak.json` 翻回来。
    if file.exists() {
        if let Ok(previous) = std::fs::read_to_string(&file) {
            if previous != text {
                std::fs::write(dir.join("draft.bak.json"), previous).ok();
            }
        }
    }
    tracing::info!(
        "保存草稿 {}：谱面={:?} 音乐={:?} 曲绘={:?}，素材 {} 项",
        stored.id,
        stored.chart.as_ref().map(|it| it.name.as_str()),
        stored.music.as_ref().map(|it| it.name.as_str()),
        stored.cover.as_ref().map(|it| it.name.as_str()),
        stored.media.len(),
    );
    std::fs::write(&file, text)
        .with_context(|| format!("写入草稿失败：{}", dir.display()))?;
    draft.dir = dir.to_string_lossy().to_string();
    Ok(draft)
}

/// 全部草稿，最近改过的排在前面。单个草稿读不出来只跳过它，不让整张列表失败。
pub fn list() -> Result<Vec<EditorDraft>> {
    let dir = drafts_dir()?;
    let mut drafts = Vec::new();
    for entry in
        std::fs::read_dir(&dir).with_context(|| format!("读取草稿目录失败：{}", dir.display()))?
    {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        let Some(id) = path.file_name().map(|it| it.to_string_lossy().to_string()) else {
            continue;
        };
        match load(&id) {
            Ok(draft) => drafts.push(draft),
            Err(err) => warn!(
                "[编辑窗口] 跳过读不出来的草稿 {}：{}",
                path.display(),
                common::error_chain(&err)
            ),
        }
    }
    drafts.sort_by(|a, b| b.updated.cmp(&a.updated));
    Ok(drafts)
}

/// 新建一个空草稿。名字缺省时按「草稿 N」自动编号。
pub fn create(name: Option<String>) -> Result<EditorDraft> {
    let now = now_ms();
    let id = format!("{now:x}");
    // 同一毫秒里连点两次「新建」（或者两个窗口同时建）时给 id 加个尾巴。
    // 用 `create_dir` 抢目录：它要么原子地建出来、要么报「已存在」，比先 exists 再建更稳。
    let mut unique = id.clone();
    let mut suffix = 1;
    loop {
        let dir = drafts_dir()?.join(&unique);
        match std::fs::create_dir(&dir) {
            Ok(()) => break,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                suffix += 1;
                unique = format!("{id}-{suffix}");
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("创建草稿目录失败：{}", dir.display()))
            }
        }
    }

    let name = name.filter(|it| !it.trim().is_empty()).unwrap_or_else(|| {
        let n = list().map(|it| it.len()).unwrap_or(0) + 1;
        format!("草稿 {n}")
    });

    save(EditorDraft {
        id: unique,
        name,
        created: now,
        updated: now,
        info: DraftInfo::default(),
        dir: String::new(),
        chart: None,
        music: None,
        cover: None,
        media: Vec::new(),
        folders: Vec::new(),
    })
}

/// 删掉一个草稿（连同它的目录）。
pub fn remove(id: &str) -> Result<()> {
    let dir = drafts_dir()?.join(id);
    check_id(id)?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("删除草稿失败：{}", dir.display()))?;
    }
    Ok(())
}

/// 找「已经引用了这份谱面」的草稿：主窗口再点同一张谱面时用它，而不是又建一个新草稿。
pub fn find_by_chart(path: &str) -> Option<EditorDraft> {
    list()
        .ok()?
        .into_iter()
        .find(|it| it.chart.as_ref().is_some_and(|chart| chart.path == path))
}

/// 按扩展名判断一份文件能落到哪条轨道上；判断不出来的返回 `None`。
///
/// 谱面除了 `.pez` / `.zip` 之外还认 `.mc` / `.pec` / `.json` / `.yaml`（单文件谱面）。
/// 目录**永远不算素材**：一个谱面包摊开后是「谱面 json + 音频 + 曲绘」三份文件，
/// 而不是一个文件夹在轨道上代表整张谱面。
pub fn classify(path: &Path) -> Option<&'static str> {
    if path.is_dir() {
        return None;
    }
    let ext = path
        .extension()
        .map(|it| it.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pez" | "zip" | "mc" | "pec" | "json" | "yaml" | "yml" => Some(KIND_CHART),
        "mp3" | "ogg" | "wav" | "flac" | "m4a" | "aac" | "opus" => Some(KIND_MUSIC),
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" => Some(KIND_COVER),
        _ => None,
    }
}

/// 把一条磁盘路径包成素材项；不是文件、或者认不出种类时报错（由前端提示）。
pub fn media_from_path(path: &Path) -> Result<DraftMedia> {
    if !path.exists() {
        bail!("文件不存在：{}", path.display());
    }
    if path.is_dir() {
        bail!("文件夹本身不能当素材：{}（往里挑文件，或用「导入谱面文件夹」）", path.display());
    }
    let Some(kind) = classify(path) else {
        bail!(
            "认不出这个文件能放到哪条轨道上：{}",
            path.display()
        );
    };
    let name = path
        .file_name()
        .map(|it| it.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    Ok(DraftMedia {
        path: path.to_string_lossy().to_string(),
        name,
        kind: kind.to_string(),
        folder: None,
    })
}

/// 是不是压缩包：`.pez` 与 `.zip` 都是 zip，素材库里不按文件出现，解开成一层文件夹。
fn is_archive(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|it| it.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    matches!(ext.as_str(), "pez" | "zip")
}

/// 把一个压缩包里的素材摊出来：`dir` 的第一段是包名（去掉扩展名），往里照包里的层级。
///
/// 包里的东西先解到解包缓存（[`common::unpack_editor_archive`]），素材记的是缓存里的真实文件，
/// 这样素材行点「定位」、预览读字节都跟普通文件一样。
fn archive_media(archive: &Path) -> Result<Vec<MediaInDir>> {
    let unpacked = common::unpack_editor_archive(archive)?;
    let stem = archive
        .file_stem()
        .map(|it| it.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(media_tree_in_dir(&unpacked)?
        .into_iter()
        .map(|at| MediaInDir { media: at.media, dir: join_dir(&stem, &at.dir) })
        .collect())
}

/// 把两段 `/` 分隔的相对目录接起来（有一段是空串就取另一段）。
fn join_dir(outer: &str, inner: &str) -> String {
    match (outer.is_empty(), inner.is_empty()) {
        (true, _) => inner.to_string(),
        (_, true) => outer.to_string(),
        _ => format!("{outer}/{inner}"),
    }
}

/// 按相对目录、种类、名字排一遍：同一层里谱面在前，前端建树时顺序稳定。
fn sorted_media(mut out: Vec<MediaInDir>) -> Vec<MediaInDir> {
    out.sort_by(|a, b| {
        a.dir
            .cmp(&b.dir)
            .then_with(|| a.media.kind.cmp(&b.media.kind))
            .then_with(|| a.media.name.cmp(&b.media.name))
    });
    out
}

/// 「导入素材 → 选文件夹」收进来的素材：相对这个目录的层级，压缩包在里面会解开成一层文件夹。
///
/// 跳过以 `.` 开头的项，往后最多翻 [`MAX_MEDIA_DEPTH`] 层。目录本身不往上列（谱面轨道只收文件）。
pub fn media_import_in_dir(dir: &Path) -> Result<Vec<MediaInDir>> {
    if !dir.is_dir() {
        bail!("不是文件夹：{}", dir.display());
    }
    let mut out = Vec::new();
    collect_import(dir, dir, 0, &mut out);
    Ok(sorted_media(out))
}

fn collect_import(root: &Path, dir: &Path, depth: usize, out: &mut Vec<MediaInDir>) {
    if depth > MAX_MEDIA_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let rel = rel_dir(root, dir);
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_import(root, &path, depth + 1, out);
            continue;
        }
        if is_archive(&path) {
            match archive_media(&path) {
                Ok(mut inner) => {
                    for at in &mut inner {
                        at.dir = join_dir(&rel, &at.dir);
                    }
                    out.extend(inner);
                }
                // 扩展名是 .pez / .zip 但内容不是 zip：当一份谱面文件收着，至少还能拖上轨道
                Err(err) => {
                    warn!("[编辑窗口] 解不开 {}：{err}", path.display());
                    if let Ok(media) = media_from_path(&path) {
                        out.push(MediaInDir { media, dir: rel.clone() });
                    }
                }
            }
            continue;
        }
        if let Ok(media) = media_from_path(&path) {
            out.push(MediaInDir { media, dir: rel.clone() });
        }
    }
}

/// 相对 `root` 的目录（用 `/` 拼，空串就是 `root` 自己）。
fn rel_dir(root: &Path, dir: &Path) -> String {
    dir.strip_prefix(root)
        .unwrap_or_else(|_| Path::new(""))
        .components()
        .map(|it| it.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// 「导入素材」挑中的路径（文件 / 文件夹 / 压缩包）都包成素材项，只记路径、不复制文件。
///
/// 文件夹与压缩包都带层：文件夹按自己的目录名当第一级，压缩包按包名当第一级，往里照原样嵌套；
/// 单个文件摊在根上（`dir` 是空串）。
pub fn media_import(paths: &[String]) -> Result<Vec<MediaInDir>> {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        let root = Path::new(path);
        if root.is_dir() {
            let name = root
                .file_name()
                .map(|it| it.to_string_lossy().into_owned())
                .unwrap_or_default();
            match media_import_in_dir(root) {
                Ok(found) if !found.is_empty() => out.extend(found.into_iter().map(|at| {
                    MediaInDir { media: at.media, dir: join_dir(&name, &at.dir) }
                })),
                Ok(_) => errors.push(format!("{} 里没有能放上轨道的素材", root.display())),
                Err(err) => errors.push(common::error_chain(&err)),
            }
            continue;
        }
        if is_archive(root) {
            match archive_media(root) {
                Ok(found) if !found.is_empty() => out.extend(found),
                Ok(_) => errors.push(format!("{} 里没有能放上轨道的素材", root.display())),
                Err(err) => errors.push(common::error_chain(&err)),
            }
            continue;
        }
        match media_from_path(root) {
            Ok(media) => out.push(MediaInDir { media, dir: String::new() }),
            Err(err) => errors.push(common::error_chain(&err)),
        }
    }
    if out.is_empty() {
        bail!(
            "没认出可以导入的文件：{}",
            errors.first().cloned().unwrap_or_default()
        );
    }
    for err in &errors {
        warn!("[编辑窗口] 导入时跳过：{err}");
    }
    Ok(sorted_media(out))
}

/// 「导入素材 → 选文件夹」拿回来的一项：素材本身，加上它在选中目录下的相对目录。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct MediaInDir {
    pub media: DraftMedia,
    /// 相对目录（`/` 分隔）：空串是选中目录本身，其余形如 `专辑/曲目`。
    pub dir: String,
}

/// 一个目录里认得出的素材，连同它们各自所在的相对目录（压缩包不再往里解，当谱面文件收）。
///
/// 解包缓存里的那份谱面包（见 [`archive_media`]）用它摊开：素材库照这份相对目录建真实层级，
/// 往后最多翻 [`MAX_MEDIA_DEPTH`] 层，跳过以 `.` 开头的项。
pub fn media_tree_in_dir(dir: &Path) -> Result<Vec<MediaInDir>> {
    if !dir.is_dir() {
        bail!("不是文件夹：{}", dir.display());
    }
    let mut out = Vec::new();
    collect_media_tree(dir, dir, 0, &mut out);
    Ok(sorted_media(out))
}

fn collect_media_tree(root: &Path, dir: &Path, depth: usize, out: &mut Vec<MediaInDir>) {
    if depth > MAX_MEDIA_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let rel = rel_dir(root, dir);
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_media_tree(root, &path, depth + 1, out);
        } else if let Ok(media) = media_from_path(&path) {
            out.push(MediaInDir { media, dir: rel.clone() });
        }
    }
}

/// 在谱面包（目录）里找出真正的那份谱面文件：优先 `chart.json` / `chart.pec`，
/// 否则取目录里第一个谱面文件（`json` / `pec` / `mc` 等，按名字排序取第一个）。
///
/// 谱面轨道上放的永远是**文件**，不是目录 —— 目录只是「从哪里找」。
pub fn chart_file_in(dir: &Path) -> Option<PathBuf> {
    let mut found = Vec::new();
    collect_chart_files(dir, 0, &mut found);
    found.sort();
    let preferred = ["chart.json", "chart.pec", "chart.yaml", "chart.yml"];
    for want in preferred {
        if let Some(hit) = found
            .iter()
            .find(|it| it.file_name().is_some_and(|name| name.eq_ignore_ascii_case(want)))
        {
            return Some(hit.clone());
        }
    }
    found.into_iter().next()
}

fn collect_chart_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_MEDIA_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_chart_files(&path, depth + 1, out);
        } else if classify(&path) == Some(KIND_CHART) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    //! 只测纯逻辑与一次落盘往返。草稿目录跟着 exe 走（`<exe 目录>/dpe/editor/drafts`），
    //! 所以测试里建的那份也在 `target/…/deps` 下的临时位置，用完自己删掉。

    use super::*;

    fn media(kind: &str, path: &str) -> DraftMedia {
        DraftMedia {
            path: path.to_string(),
            name: path.rsplit(['/', '\\']).next().unwrap_or(path).to_string(),
            kind: kind.to_string(),
            folder: None,
        }
    }

    #[test]
    fn classify_by_extension_never_accepts_dir() {
        let dir = std::env::temp_dir();
        assert_eq!(classify(&dir), None);
        assert!(media_from_path(&dir).is_err());
        assert_eq!(classify(Path::new("a/b.pez")), Some(KIND_CHART));
        assert_eq!(classify(Path::new("a/b.ZIP")), Some(KIND_CHART));
        assert_eq!(classify(Path::new("a/b.mp3")), Some(KIND_MUSIC));
        assert_eq!(classify(Path::new("a/b.PNG")), Some(KIND_COVER));
        assert_eq!(classify(Path::new("a/b.txt")), None);
    }

    #[test]
    fn check_id_blocks_path_traversal() {
        assert!(check_id("1700000000000").is_ok());
        assert!(check_id("abc-1_2").is_ok());
        assert!(check_id("").is_err());
        assert!(check_id("../x").is_err());
        assert!(check_id("a/b").is_err());
        assert!(check_id(&"a".repeat(65)).is_err());
    }

    #[test]
    fn assign_puts_media_on_its_track_once() {
        let mut draft = EditorDraft {
            id: "t".to_string(),
            name: String::new(),
            created: 0,
            updated: 0,
            info: DraftInfo::default(),
            dir: String::new(),
            chart: None,
            music: None,
            cover: None,
            media: Vec::new(),
            folders: Vec::new(),
        };
        draft.assign(media(KIND_MUSIC, "C:/a/song.ogg"));
        draft.assign(media(KIND_CHART, "C:/a/chart.pez"));
        draft.assign(media(KIND_MUSIC, "C:/a/song.ogg"));
        assert_eq!(draft.music.as_ref().map(|it| it.path.as_str()), Some("C:/a/song.ogg"));
        assert_eq!(draft.chart.as_ref().map(|it| it.path.as_str()), Some("C:/a/chart.pez"));
        // 同一份素材重复放上去不该在素材库里出现两次
        assert_eq!(draft.media.len(), 2);
        // 认不出种类的直接忽略
        draft.assign(media("nope", "C:/a/x.txt"));
        assert_eq!(draft.media.len(), 2);
    }

    #[test]
    fn draft_json_tolerates_old_shape() {
        let draft: EditorDraft = serde_json::from_str(r#"{"id":"abc"}"#).unwrap();
        assert_eq!(draft.id, "abc");
        assert!(draft.name.is_empty());
        assert!(draft.chart.is_none());
        assert!(draft.media.is_empty());
    }

    #[test]
    fn save_load_list_remove_roundtrip() {
        let mut draft = create(Some("测试草稿".to_string())).unwrap();
        draft.assign(media(KIND_COVER, "C:/a/cover.png"));
        let saved = save(draft).unwrap();
        assert_eq!(saved.name, "测试草稿");
        assert!(saved.updated >= saved.created);

        let loaded = load(&saved.id).unwrap();
        assert_eq!(loaded.name, "测试草稿");
        assert_eq!(loaded.cover.map(|it| it.path), Some("C:/a/cover.png".to_string()));

        assert!(list().unwrap().iter().any(|it| it.id == saved.id));
        assert!(find_by_chart("C:/a/chart.pez").is_none());

        remove(&saved.id).unwrap();
        assert!(load(&saved.id).is_err());
    }

    #[test]
    fn old_dir_chart_is_flattened_on_load() {
        let root = std::env::temp_dir().join(format!("dpe-normalize-{}", now_ms()));
        let pkg = root.join("pkg");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("chart.json"), b"{}").unwrap();
        std::fs::write(pkg.join("song.ogg"), b"x").unwrap();
        std::fs::write(pkg.join("cover.png"), b"x").unwrap();

        // 旧草稿：谱面轨道上挂的是目录本身
        let mut draft = create(Some("旧草稿".to_string())).unwrap();
        draft.chart = Some(DraftMedia {
            path: pkg.to_string_lossy().to_string(),
            name: "pkg".to_string(),
            kind: KIND_CHART.to_string(),
            folder: None,
        });
        draft.media.push(draft.chart.clone().unwrap());
        let saved = save(draft).unwrap();

        let loaded = load(&saved.id).unwrap();
        let chart = loaded.chart.expect("谱面轨道应该还在");
        assert_eq!(chart.path, pkg.join("chart.json").to_string_lossy());
        assert!(loaded.media.iter().all(|it| !Path::new(&it.path).is_dir()));
        assert!(loaded.media.iter().any(|it| it.kind == KIND_MUSIC));
        assert!(loaded.media.iter().any(|it| it.kind == KIND_COVER));
        // 空着的音乐 / 曲绘轨道不会被包里那份填上：放哪首、哪张由用户自己挑
        assert!(loaded.music.is_none());
        assert!(loaded.cover.is_none());

        remove(&saved.id).unwrap();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn empty_name_falls_back() {
        let mut draft = create(None).unwrap();
        draft.name = "   ".to_string();
        let saved = save(draft).unwrap();
        assert_eq!(saved.name, "未命名草稿");
        remove(&saved.id).unwrap();
    }

    #[test]
    fn media_tree_in_dir_keeps_nesting() {
        // 三层嵌套：根上一份音频、`专辑/` 里一张曲绘、`专辑/曲目/` 里一份谱面
        let root = std::env::temp_dir().join(format!("dpe-media-tree-{}", now_ms()));
        let nested = root.join("专辑").join("曲目");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("song.mp3"), b"x").unwrap();
        std::fs::write(root.join("专辑").join("cover.png"), b"x").unwrap();
        std::fs::write(nested.join("chart.json"), b"{}").unwrap();
        std::fs::write(nested.join("note.txt"), b"x").unwrap();

        let found = media_tree_in_dir(&root).unwrap();
        std::fs::remove_dir_all(&root).ok();

        // 相对目录用 `/` 分隔，根上是空串；认不出的扩展名（note.txt）不收
        let pairs: Vec<(&str, &str)> =
            found.iter().map(|it| (it.media.name.as_str(), it.dir.as_str())).collect();
        assert_eq!(pairs, vec![("song.mp3", ""), ("cover.png", "专辑"), ("chart.json", "专辑/曲目")]);
        assert_eq!(found[2].media.kind, KIND_CHART);
        assert_eq!(found[2].media.folder, None);
    }

    #[test]
    fn archives_are_unpacked_into_a_folder() {
        // 一个谱面包：谱面在根上，曲绘在包里的子目录里
        let root = std::env::temp_dir().join(format!("dpe-media-zip-{}", now_ms()));
        std::fs::create_dir_all(&root).unwrap();
        let archive = root.join("曲目.pez");
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        {
            let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
            writer.start_file("chart.json", options).unwrap();
            std::io::Write::write_all(&mut writer, b"{}").unwrap();
            writer.start_file("img/cover.png", options).unwrap();
            std::io::Write::write_all(&mut writer, b"x").unwrap();
            writer.finish().unwrap();
        }

        let found = media_import(&[archive.to_string_lossy().into_owned()]).unwrap();
        // 包本身不出现在素材库里：包名是文件夹的第一段，再往里照包里的层级
        let pairs: Vec<(&str, &str)> =
            found.iter().map(|it| (it.media.name.as_str(), it.dir.as_str())).collect();
        assert_eq!(pairs, vec![("chart.json", "曲目"), ("cover.png", "曲目/img")]);
        assert_eq!(found[0].media.kind, KIND_CHART);
        assert!(found.iter().all(|it| !it.media.path.ends_with(".pez")));

        // 选文件夹时混在里面的压缩包也一样处理，位置接在所在目录后面
        let picked = media_import_in_dir(&root).unwrap();
        assert_eq!(picked.len(), found.len());
        assert!(picked.iter().all(|it| it.dir == "曲目" || it.dir.starts_with("曲目/")));

        std::fs::remove_dir_all(&root).ok();
        if let Ok(cache) = common::editor_unpacked_dir() {
            std::fs::remove_dir_all(cache).ok();
        }
    }

    #[test]
    fn load_puts_folders_with_missing_parent_back_on_root() {
        let mut draft = create(Some("文件夹".to_string())).unwrap();
        // 上级不存在的文件夹、以及指着这个文件夹的素材都退回根上
        draft.folders.push(DraftFolder {
            id: "f-child".to_string(),
            name: "子文件夹".to_string(),
            parent: Some("f-gone".to_string()),
        });
        draft.media.push(DraftMedia {
            path: "C:/a/song.ogg".to_string(),
            name: "song.ogg".to_string(),
            kind: KIND_MUSIC.to_string(),
            folder: Some("f-child".to_string()),
        });
        let saved = save(draft).unwrap();

        let loaded = load(&saved.id).unwrap();
        remove(&saved.id).unwrap();
        assert_eq!(loaded.folders.len(), 1);
        assert_eq!(loaded.folders[0].parent, None);
        assert_eq!(loaded.media[0].folder.as_deref(), Some("f-child"));
    }
}
