import { invoke } from '@tauri-apps/api/core';

/**
 * 附带资源包（`<程序目录>/appendant`）。
 *
 * 皮肤包与 Minecraft 材质都不再随程序分发：体积大、还牵扯各自的授权。用户把下载到的 zip 放进
 * 这个目录，程序按需读取；缺包时界面先拦一道，提示去哪儿下载。
 *
 * 文件名固定成这五个（`appendant.` 前缀把皮肤包与材质包排在一起）：
 * `appendant.official.zip` / `appendant.old.zip` / `appendant.retri.zip` 是皮肤，
 * `appendant.lava.zip`（基岩 + 岩浆帧）与 `appendant.water.zip`（水面 + 受伤音效）是材质。
 */

export interface AppendantItem {
  /** 包文件名，例如 `appendant.official.zip`。 */
  file: string;
  /** 包所在位置（zip 或同名目录）；缺包时给出「该放哪儿」的 zip 路径。 */
  path: string;
  /** 包在不在（`<包名>.zip` 或同名目录都算在）。 */
  found?: boolean;
  /** 能不能用：包在、且必需文件齐全。缺文件时也是 false。 */
  present: boolean;
  /** 包里缺哪些文件；包整个不在时就是整张必需文件表。 */
  missing?: string[];
  /** 这个包该放在哪个目录（`respack/` 里的自备皮肤与 `appendant/` 里的包不同），缺省用 appendant 目录。 */
  dir?: string;
}

export interface AppendantStatus {
  /** `<程序目录>/appendant` 的绝对路径。 */
  dir: string;
  items: AppendantItem[];
}

/** 所有资源包在同一个网盘文件夹里，用户按文件名自行下载。 */
export const APPENDANT_DOWNLOAD_URL = 'https://daogemm.lanzouc.com/b0018qnkxi';

/** 上面那个网盘文件夹的访问密码，弹窗里一并给出。 */
export const APPENDANT_DOWNLOAD_PASSWORD = 'dgmm';

/** 程序认得的包名，顺序即界面里的排列顺序。 */
export const APPENDANT_FILES = [
  'appendant.official.zip',
  'appendant.old.zip',
  'appendant.retri.zip',
  'appendant.lava.zip',
  'appendant.water.zip',
  'appendant.spas.zip',
  'appendant.qzk.zip',
  'appendant.rr.zip',
  'appendant.cp.zip',
] as const;

/** 读取附带资源包状态（目录 + 每个包在不在、缺不缺文件）。 */
export async function fetchAppendants(): Promise<AppendantStatus> {
  return (await invoke('get_appendants')) as AppendantStatus;
}

/** `respack/` 里的用户自备皮肤包（下拉框的候选，不含「默认」那一项）。 */
export interface RespackStatus {
  name: string;
  path: string;
  /** 能不能用：zip 在、且必需文件齐全。 */
  present: boolean;
  /** 包里缺哪些文件（齐全时为空）。 */
  missing: string[];
}

/** 读取 `respack/` 里自备皮肤包的状态，用来判断选中的那个是不是缺文件。 */
export async function fetchRespacks(): Promise<RespackStatus[]> {
  return (await invoke('get_respacks')) as RespackStatus[];
}

/** `respack/` 目录的绝对路径（缺包提示里要写「该放到哪儿」）。 */
export async function fetchRespackDir(): Promise<string> {
  return (await invoke('get_respack_dir_path')) as string;
}

/** 打开附带资源包目录（不存在时后端会先建出来）。 */
export async function openAppendantDir(): Promise<void> {
  await invoke('open_appendant_folder');
}

/** 某个包的下载页：五个包共用同一个网盘文件夹，未知名字返回 `null`。 */
export function appendantUrl(file: string): string | null {
  return APPENDANT_FILES.some((it) => it.toLowerCase() === file.toLowerCase()) ? APPENDANT_DOWNLOAD_URL : null;
}

/**
 * 找出配置里选中、但磁盘上没有的那个包。
 *
 * `path` 是配置里存的值：`null` 表示「默认」皮肤（内置、永远在），路径可能指向 `appendant/` 里的
 * 包，也可能指向用户早先放在 `respack/` 里的包。只按文件名比 —— 目录可能换过，但包名不变。
 */
export function missingAppendant(status: AppendantStatus, path: string | null): AppendantItem | null {
  if (!path) return null;
  const file = path.replace(/\\/g, '/').split('/').pop() ?? '';
  const item = status.items.find((it) => it.file.toLowerCase() === file.toLowerCase());
  return item && !item.present ? item : null;
}

/** 按文件名找一个包。 */
export function appendantItem(status: AppendantStatus, file: string): AppendantItem | undefined {
  return status.items.find((it) => it.file.toLowerCase() === file.toLowerCase());
}

/**
 * 可选包：`appendant.official` 只是用来顶替编进程序的内置默认皮肤。
 *
 * 它在的时候当默认皮肤用；不在就直接用内置那份（长得一样，所以不提示下载、也不报错）。
 */
export const OPTIONAL_APPENDANTS = ['appendant.official.zip'] as const;

/** 这个包缺了要不要紧：不要紧的包不弹下载提示。 */
export function isOptionalAppendant(file: string): boolean {
  return OPTIONAL_APPENDANTS.some((it) => it.toLowerCase() === file.toLowerCase());
}

/** 岩浆噪域用的材质包（基岩贴图 + 岩浆帧）。 */
export function lavaPack(status: AppendantStatus): AppendantItem | undefined {
  return appendantItem(status, 'appendant.lava.zip');
}

/** 屏幕进水 / 受伤音效用的材质包（水面贴图 + 打击音效）。 */
export function waterPack(status: AppendantStatus): AppendantItem | undefined {
  return appendantItem(status, 'appendant.water.zip');
}

/**
 * 「报应SP」谱面：手绘数字、曲名 logo 与按键贴图都在 `appendant.retri.zip` 里，引擎不看配置里
 * 选的皮肤、直接钉在这份包上，缺包就没有那套演出（退回内置默认皮肤）。
 *
 * 判据与后端 `render.rs::is_baoying_sp_chart` 一致：谱面文件名里带这段词干。
 */
export function isBaoyingSpChart(path: string | null | undefined): boolean {
  return !!path && /Retribution~CycleofRedemption~/i.test(path);
}

/**
 * 「痉挛SP」谱面：正中的插图演出取自 `appendant.spas.zip`，缺包时那几段时间什么都不画。
 *
 * 判据与后端 `render.rs::is_jingluan_sp_chart` 一致。
 */
export function isJingluanSpChart(path: string | null | undefined): boolean {
  return !!path && /SpasmodicHaocoreMix/i.test(path);
}

/**
 * 「QZKAGO」谱面：整屏闪现的 7 张静图取自 `appendant.qzk.zip`，缺包时那 7.10 秒什么都不画，
 * 谱面照常播放。
 *
 * 判据与后端 `render.rs::is_qzkago_chart` 一致：四个难度都是 `QZKagoRequiem.tpazolite.100.<难度>`，
 * 按前缀认。
 */
export function isQzkagoChart(path: string | null | undefined): boolean {
  if (!path) return false;
  const name = path.replace(/\\/g, '/').split('/').pop() ?? '';
  return name.startsWith('QZKagoRequiem.tpazolite.100');
}

/**
 * 输入谱面是不是「Clock Paradox」特殊演出用的那张。
 *
 * 三个难度的文件名都是 `ClockParadox.WyvernP.100.<难度>.pez`，按前缀认。
 */
export function isClockParadoxChart(path: string | null | undefined): boolean {
  if (!path) return false;
  const name = path.replace(/\\/g, '/').split('/').pop() ?? '';
  return name.startsWith('ClockParadox.WyvernP.100');
}

/**
 * 「Rr」谱面：按键贴图与 AP 打击特效取自 `appendant.rr.zip`，缺包时退回默认皮肤（演出变成
 * 普通皮肤的一套，谱面照常播放）。
 *
 * 判据与后端 `render.rs::is_rr_chart` 一致：四个难度都是 `Rrharil.TeamGrimoire.100.<难度>`，
 * 按前缀认。
 */
export function isRrChart(path: string | null | undefined): boolean {
  if (!path) return false;
  const name = path.replace(/\\/g, '/').split('/').pop() ?? '';
  return name.startsWith('Rrharil.TeamGrimoire.100');
}

/** 路径最后一段，去掉 `.pez`；「按整名相等」的那几种谱面（雪降、もぺもぺ）用得上。 */
function chartStem(path: string | null | undefined): string {
  if (!path) return '';
  const name = path.replace(/\\/g, '/').split('/').pop() ?? '';
  return name.endsWith('.pez') ? name.slice(0, -'.pez'.length) : name;
}

/**
 * 「雪降」SP 谱面：伪装分数、音量条与表情演出全部在引擎里，没有附带包；这个判据只用来决定
 * 渲染页要不要露出调试入口。
 *
 * 判据与后端 `render.rs::is_xuejiang_sp_chart` 一致：最后一段正好是谱面名（目录名不带扩展名，
 * 包名带 `.pez`）。
 */
export function isXuejiangSpChart(path: string | null | undefined): boolean {
  return chartStem(path) === '今年も「雪降り、メリクリ」目指して頑張ります！！.100.SP';
}

/**
 * 「もぺもぺ」谱面：整屏红色压暗取自谱面动画参数，没有附带包。
 *
 * 判据与后端 `render.rs::is_mopemope_sp_chart` 一致。
 */
export function isMopemopeSpChart(path: string | null | undefined): boolean {
  return chartStem(path) === 'もぺもぺ.LeaF.100.SP';
}

/** 演出动画的族名，与 `assets/chart-animation.toml` 的表名、`RenderConfig` 的字段前缀一一对应。 */
export type ChartAnimationFamily =
  | 'xuejiang'
  | 'baoying'
  | 'jingluan'
  | 'qzkago'
  | 'clockParadox'
  | 'mopemope'
  | 'rr';

/**
 * 输入谱面用的是哪一套演出动画；不是那七张谱面之一时给 `null`。
 *
 * DPDebugger 的入口按钮按它决定显示与否，窗口里也按它决定右侧列出哪一组参数。
 */
export function chartAnimationFamily(path: string | null | undefined): ChartAnimationFamily | null {
  if (!path) return null;
  if (isXuejiangSpChart(path)) return 'xuejiang';
  if (isBaoyingSpChart(path)) return 'baoying';
  if (isJingluanSpChart(path)) return 'jingluan';
  if (isQzkagoChart(path)) return 'qzkago';
  if (isClockParadoxChart(path)) return 'clockParadox';
  if (isMopemopeSpChart(path)) return 'mopemope';
  if (isRrChart(path)) return 'rr';
  return null;
}

