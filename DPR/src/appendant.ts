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
  /** 绝对路径。缺包时也给出，界面用它告诉用户该放到哪儿。 */
  path: string;
  present: boolean;
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
] as const;

/** 读取附带资源包状态（目录 + 每个包在不在）。 */
export async function fetchAppendants(): Promise<AppendantStatus> {
  return (await invoke('get_appendants')) as AppendantStatus;
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
