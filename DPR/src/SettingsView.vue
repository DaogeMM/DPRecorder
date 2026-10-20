<script setup lang="ts">
// 设置页。
//
// 页面结构照搬渲染页（ConfigView.vue）那套外壳：
//   一级 = PhiGroup（`plain` 纯标题模式：不折叠、不画浅底/hover，只留放大的标题与左侧一条竖线）
//   条目 = `cfg-grid`（三栏）+ CfgField / CfgSwitch（条目名只有文字，前面的小图标没留）
// 两大块：**设置**（各项一个网格平铺，重置图标按钮挂在「设置」标题右边）与
// **关于**（开发方式 / 非官方声明 + 底部居中的「版本 | 作者」与 B 站入口）。
//
// 这套外壳不用 `v-row` + `v-col`，也不用带 ⓘ 圆圈的 TipCheck，
// 所以 `components/TipCheck.vue` 现在没有任何界面在用（文件留着备用）。
import { useI18n } from 'vue-i18n';
useI18n();
const { t } = useI18n();

import { computed, nextTick, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { DEFAULT_APP_CONFIG, type AppConfig } from './model';
import * as dialog from '@tauri-apps/plugin-dialog';
import { toast, toastError, changeLocale, openExternal } from './common';
import type { VForm } from 'vuetify/components';
import { SUPPORTED_LOCALES_NAME } from './main';

import CfgField from './components/CfgField.vue';
import CfgSlider from './components/CfgSlider.vue';
import CfgSwitch from './components/CfgSwitch.vue';
import PhiGroup from './components/PhiGroup.vue';
import BilibiliIcon from './components/BilibiliIcon.vue';
import { useStorage } from '@vueuse/core';
import { finishPageLoad } from './pageLoad';

const form = ref<VForm>();

const config = ref(DEFAULT_APP_CONFIG);

/** 把「FFmpeg 路径」的值换算成**目录**：老配置里存的是 `…\bin\ffmpeg.exe`，
 *  而后端探测回来的也是 exe 完整路径，这格现在写的是「exe 所在的文件夹」，
 *  所以显示与保存都先把结尾那个文件名去掉。只认标准的 `ffmpeg.exe` / `ffmpeg`
 *  结尾（自定义名字的 exe 原样保留，后端仍按文件用）。 */
function asFFmpegDir(path: string) {
  return path.trim().replace(/[\\/](ffmpeg(\.exe)?)$/i, '');
}

/** 「相对 exe 所在文件夹」的写法：
 *  路径正好落在程序文件夹里就显示成 `.\output` / `.\config\presets.toml`，否则原样返回
 *  （用户自己填到别处的目录仍然是完整路径）。完整路径没丢 —— 那两格的 `:title` 挂的还是它。
 *  这样打包后整个文件夹随便挪位置，界面上也不会写着上一台机器的绝对路径。 */
function asAppRelative(path: string) {
  const dir = appDir.value.replace(/[\\/]+$/, '');
  if (!path || !dir) return path;
  const base = dir.toLowerCase();
  const lower = path.toLowerCase();
  if (!lower.startsWith(base + '\\') && !lower.startsWith(base + '/')) {
    return path;
  }
  return '.\\' + path.slice(dir.length + 1).replace(/\//g, '\\');
}

/** 保存前的归一化：字符串字段里的空串写成 `null`（后端把 null 当「没设置」）；
 *  「FFmpeg 路径」再从 exe 路径抹成它所在的目录（`asFFmpegDir`）。 */
function normalizeConfig() {
  for (const key in config.value) {
    if (typeof config.value[key as keyof AppConfig] === 'string') {
      const val = config.value[key as keyof AppConfig] as string;
      if (val.trim() === "") {
        (config.value as any)[key] = null;
      }
    }
  }
  if (config.value.ffmpegPath) {
    config.value.ffmpegPath = asFFmpegDir(config.value.ffmpegPath);
  }
}

/* ---------------------- 改一下就自动保存 ----------------------
   改完立刻自己落盘、也不用再点一次，所以标题右边那枚保存图标已经删了
   （`saveConfig()` 还留着：重置对话框那颗「确认」要它，
   那条路径需要「校验 + 立刻落盘」）。
   两处刻意与 `saveConfig()` 不同：
     - **不弹**校验失败 / 保存成功的 toast（字段错误本来就显示在控件下面），
     - 保存后**不回读**（`updateConfig()` 回读会让下面的 watch 再触发一次，白转一圈）。
   `autosaveStarted` 只在首屏把配置读回来之后才打开 —— 否则 `updateConfig()` 那一次赋值
   自己就会先存一遍。
   这里不再做防抖：早先攒 600ms 再存，用户在延迟窗口里切页面/关窗口就会丢掉最后一次改动。 */
let autosaveStarted = false;

async function autoSaveConfig() {
  if (!(await form.value?.validate())?.valid) return;
  normalizeConfig();
  try {
    await invoke('save_config', { config: config.value });
  } catch (e) {
    toastError(e);
  }
}

watch(config, () => {
  if (!autosaveStarted) return;
  void autoSaveConfig();
}, { deep: true });

async function updateConfig() {
  const loaded = await invoke('read_config') as AppConfig;
  // 老配置里「FFmpeg 路径」存的是 exe 完整路径，读进来先换算成它所在的目录
  if (loaded.ffmpegPath) loaded.ffmpegPath = asFFmpegDir(loaded.ffmpegPath);
  config.value = loaded;
  autosaveStarted = true;
  // 配置到位了才揭开内容区那层转圈：否则会先显示一屏默认值再跳成实际值（见 `./pageLoad`）
  finishPageLoad();
}
updateConfig();

/** 「配置文件」目录（`config/`）的绝对路径 —— 只读显示，图标按钮 / 右键都**打开这个目录**。
 *  目录由 `get_presets_path` 给的 `config/presets.toml` 取父目录
 *  得来，没有新增后端命令（写入预设仍然由渲染页的预设按钮走后端命令）。 */
const presetsPath = ref('');
async function updatePresetsPath() {
  try {
    presetsPath.value = (await invoke('get_presets_path')) as string;
  } catch (e) {
    // 读不到就留空（比如首次运行还没生成 presets.toml），不打扰用户
    presetsPath.value = '';
  }
}
updatePresetsPath();

/** 「资源包」文件夹（`<程序目录>/respack`）的绝对路径 —— 跟预设配置文件一样是只读显示。
 *  资源包放在程序旁边，文件选择器 / 右键都打开它。
 *  这个目录没有「没设置」的概念（后端 `common::respack_dir()` 固定解析它），所以不需要 config 字段。 */
const respackDir = ref('');
async function updateRespackDir() {
  try {
    respackDir.value = (await invoke('get_respack_dir_path')) as string;
  } catch (e) {
    // 旧版本的二进制没有这条命令：留空，框里就退回相对写法 `.\respack`
    respackDir.value = '';
  }
}
updateRespackDir();

/** 「附带资源包」文件夹（`<程序目录>/appendant`）的绝对路径 —— 同上，只读显示。
 *  皮肤包与 Minecraft 材质由用户自己下载后放进这里（程序不随安装包分发它们），
 *  缺包时渲染页那个弹窗的「打开资源包文件夹」按钮也是打开它。 */
const appendantDir = ref('');
async function updateAppendantDir() {
  try {
    appendantDir.value = (await invoke('get_appendant_dir_path')) as string;
  } catch (e) {
    // 旧版本的二进制没有这条命令：留空，框里就退回相对写法 `.\appendant`
    appendantDir.value = '';
  }
}
updateAppendantDir();

/** 输出目录的默认值：后端解析出来的**完整路径**（程序目录下的 output/，或后端认定的其它可写位置）。
 *  「自定义输出目录」那格没设置时显示它 —— 默认也让用户看到完整路径，而不只是一句 `/output/`。 */
const outputDirDefault = ref('');
async function updateOutputDirDefault() {
  try {
    outputDirDefault.value = (await invoke('get_output_dir_path')) as string;
  } catch (e) {
    // 读不到（比如运行的程序还是没带这条命令的旧版本）就退回 `/output/` 那句提示
    outputDirDefault.value = '/output/';
  }
}
updateOutputDirDefault();

/** exe（程序）所在文件夹的绝对路径：`.\output`、`.\config\presets.toml` 这种相对写法拿它当基准。
 *  开发时是 `target\debug`，打包后就是安装目录 —— 两者都是 `output/`、`config/` 的家。 */
const appDir = ref('');
async function updateAppDir() {
  try {
    appDir.value = (await invoke('get_app_dir')) as string;
  } catch (e) {
    // 二进制里还没有这条命令（旧版本）：退回完整路径，至少不会显示错
    appDir.value = '';
  }
}
updateAppDir();

/** 「FFmpeg 路径」的默认值：后端探测到的 **ffmpeg 所在目录**（设置里填了就用填的那个，
 *  否则按 PATH / 程序目录探测，再把可执行文件的父目录取出来）。
 *  那格没设置时显示它，跟上面的输出目录一个做法。探测不到就不显示（退回「自动检测」那句）。 */
const ffmpegPathDefault = ref('');
async function updateFFmpegPathDefault() {
  try {
    ffmpegPathDefault.value = ((await invoke('get_ffmpeg_path')) as string | null) ?? '';
  } catch (e) {
    // 旧版本的二进制没有这条命令，退回「自动检测」提示
    ffmpegPathDefault.value = '';
  }
}
updateFFmpegPathDefault();

/** 这两格「没设置」时把默认路径当**真值**显示（不是当 `:placeholder` 占位提示）：
 *  占位符看得见却选不中、复制不了。
 *  仍然只是显示：`get` 在空值时回落到默认路径，`set` 才真的写进 `config` ——
 *  所以「没设置 = 跟随程序目录 / 自动检测」的语义没变，不会凭空把绝对路径冻进 config.toml
 *  （把程序文件夹挪个地方也不会指到旧路径上去）。空值时的淡色交给 `.settings-path--default`。 */
/** 输出目录的**完整路径**：框里现在写相对形态（`.\output`），完整这条留给 `:title` 悬停提示。 */
const outputDirFull = computed(() => config.value.outputDir || outputDirDefault.value);
/** 框里真正写的东西：落在程序文件夹里就是 `.\output`，用户填到别处就还是完整路径。
 *  （`set` 现在够不着 —— 那两格是 `readonly`；留着是为了 `v-model` 成立，顺便记着语义。） */
const outputDirShown = computed({
  get: () => asAppRelative(outputDirFull.value),
  set: (value: string) => { config.value.outputDir = value || null; },
});
/** 配置文件目录：完整路径给 `:title`，框里写相对的 `.\config`。这格只读。
 *  `presets.toml` 就在里面，但显示与打开的**都是目录**。 */
const configDirFull = computed(() => presetsPath.value.replace(/[\\/][^\\/]+$/, ''));
const configDirShown = computed(() => asAppRelative(configDirFull.value) || '.\\config');
/** 资源包文件夹：同理，框里写 `.\respack`；后端没答上来（旧二进制）时也退回这个写法。 */
const respackDirShown = computed(() => asAppRelative(respackDir.value) || '.\\respack');
/** 附带资源包文件夹：同样写 `.\appendant`。 */
const appendantDirShown = computed(() => asAppRelative(appendantDir.value) || '.\\appendant');
const ffmpegPathShown = computed({
  // 这格写的是「exe 所在的文件夹」：老配置/后端探测给回来的可能是 exe 路径，显示时先抹成目录
  get: () => asFFmpegDir(config.value.ffmpegPath || ffmpegPathDefault.value),
  set: (value: string) => { config.value.ffmpegPath = value || null; },
});

async function saveConfig() {
  if (!(await form.value!.validate()).valid) {
    toast(t('has-error'), 'error');
    return null;
  }
  if (config.value.outputDir) {
    try {
      await invoke('test_output_dir', { dir: config.value.outputDir });
    } catch (e) {
      toastError(e);
      return null;
    }
  }
  normalizeConfig();

  try {
    await invoke('save_config', { config: config.value });
  } catch (e) {
    toastError(e);
  }

  updateConfig();
  // 这条路只剩「重置 → 确认」在用，重置成功不必再弹一条绿色「保存成功」
  // （失败时的红字由上面两条 catch 负责）。
}

const resetDialog = ref(false);
/* 「重置」现在只有一个按钮：确认后把**写死的默认值**（`DEFAULT_APP_CONFIG`，不去读配置文件）
   直接填回设置，再走一遍 `saveConfig()` 立刻落盘 —— 不再等自动保存那 600ms 防抖，所以旧的
   两个选项（仅重置配置 / 重置并保存所有配置）合并成了一个。`saveConfig()` 自带表单校验与
   输出目录检查，失败时只弹错误提示、不写文件。 */
async function resetConfig() {
  // 浅拷贝：`DEFAULT_APP_CONFIG` 里的值都是字符串/布尔/null，直接赋同一个对象会让之后
  // 在界面上改设置时把那份「写死的默认值」本身改掉（下次重置就不是默认值了）。
  config.value = { ...DEFAULT_APP_CONFIG };
  // 这一组里还有两项**纯界面偏好**（存 localStorage、不进 `config.toml`，见下面的
  // `listExpand` / `stripWidth`）：重置如果放着它们不管，用户会看到重置完开关还是关的、
  // 滑杆还停在原来那格。所以一并填回各自的默认值（✅ / 15）。
  listExpand.value = LIST_EXPAND_DEFAULT;
  stripWidth.value = STRIP_WIDTH_DEFAULT;
  nextTick(() => {
    form.value?.resetValidation();
  });
  resetDialog.value = false;
  await saveConfig();
}

async function selectDir(title: string) {
  let file = await dialog.open({
    directory: true,
    title
  });
  if (!file) {
    // 用户在目录选择器里按了「取消」/Esc：不是错误，静默返回。
    // （这里不能弹红色「没有选择」：点三个路径格末尾的文件夹图标后只要取消就会看到，
    //   看着像出了故障。）
    return null;
  } else {
    //toast(t('select'), 'success');
    return file;
  }
}

async function selectRpeDir() {
  let path = await selectDir(t('rpe-dir'));
  if (path) {
    try {
      let valid = await invoke('check_rpe_dir', { path: path });
      if (valid) {
        config.value.rpeDir = path;
      } else {
        toast(t('not-valid-rpe'), 'error');
      }
    } catch (e) {
      toastError(e);
    }
  }
}

async function selectOutputDir() {
  let path = await selectDir(t('output-dir'));
  if (path) {
    try {
      config.value.outputDir = path;
    } catch (e) {
      toastError(e);
    }
  }
}

/** 「FFmpeg 路径」选的是**目录**（exe 所在文件夹），不写 exe 本身，
 *  所以这里用目录选择器，选完让后端验一遍
 *  （目录会换算成里面的 ffmpeg.exe 跑 `-version` + 查滤镜），不合格就提示、不写进配置。 */
async function selectFFmpegDir() {
  let path = await selectDir(t('ffmpeg-path'));
  if (path) {
    try {
      if (!(await invoke('check_ffmpeg_filter', { ffmpeg: path }))) {
        toast(t('ffmpeg-not-found'), 'error');
        return;
      }
      config.value.ffmpegPath = path;
    } catch (e) {
      toastError(e);
    }
  }
}

/** 「FFmpeg 路径」那格的右键：**进到** exe 所在的那个文件夹里去。
 *
 *  `showInFolder` 走的是后端 `reveal_path`（`explorer.exe /select,"<路径>"`），对**文件**是对的
 *  （在文件夹中显示），对**目录**就成了「开上一级再选中它」—— 这格的语义是目录，所以这里换成
 *  `open_dir`（后端 `open_folder`）。
 *
 *  不用 `openInFolder`：它先调 `test_output_dir`，那个在目录不存在时会 `create_dir`；
 *  ffmpeg 在 PATH 上、探测结果只是个裸名字 `ffmpeg` 时，会平白在当前目录建出一个叫 ffmpeg 的文件夹。
 *  `open_dir` 对不存在的路径只会报一句「not a directory」，正好当提示用。
 *  显示值先过 `asFFmpegDir`：老配置里存的可能是 `...\bin\ffmpeg.exe`，抹成目录再进去。 */
function openFFmpegFolder() {
  const path = asFFmpegDir(config.value.ffmpegPath || ffmpegPathDefault.value);
  if (!path) {
    toast(t('no-select'), 'error');
    return;
  }
  invoke('open_dir', { path }).catch(toastError);
}

/** 打开「资源包」文件夹：走后端 `open_respack_folder`（它顺手把目录建出来，和渲染页那枚
 *  「打开文件夹」按钮是同一条命令）。这格是**目录**，所以右键也进目录 —— 用 `reveal_path`
 *  会变成「开上一级再选中它」（ffmpeg 那格踩过这个坑，见 `openFFmpegFolder` 上面的注释）。 */
function openRespackDir() {
  invoke('open_respack_folder').catch(toastError);
}

/** 打开「附带资源包」文件夹：后端 `open_appendant_folder`（不存在时先建出来）。
 *  与渲染页缺包弹窗里那枚按钮是同一条命令。 */
function openAppendantDir() {
  invoke('open_appendant_folder').catch(toastError);
}

async function openInFolder(path: string | null, isOutput: boolean = false) {
  try {
    if (!path && !isOutput) {
      toast(t('no-select'), 'error');
      return null;
    } else if (!path && isOutput) {
      await invoke('open_output_folder', { path });
      return;
    }

    await invoke('test_output_dir', { dir: path });
    // 目录交给后端打开：`@tauri-apps/plugin-opener` 的 `openPath` 最终是 `open::that_detached`，
    // 目录那条分支先试 COM（`SHOpenFolderAndSelectItems`），在 DPR 的进程里实测没反应，
    // 退化到 `ShellExecuteExW` 时 fMask=0（缺 SEE_MASK_FLAG_NO_UI），失败就由系统弹
    // 「Windows 无法访问指定设备、路径或文件」——用户看到的「右键打不开」就是它。
    // 后端改成 `cmd /c start` → `ShellExecuteExW` → `explorer.exe` 三条路依次试。
    await invoke('open_dir', { path });
  } catch (e) {
    toastError(e);
  }
}

/** 「配置文件目录」那格的图标按钮 / 右键：**进到** `config/` 目录里（不是定位 `presets.toml`）。
 *
 *  这格走 `open_dir`（跟「资源包」那格一样）。不能用 `reveal_in_dir`（`explorer.exe /select,...`：
 *  对文件是「在文件夹中显示」，对目录会变成「开上一级再选中它」—— ffmpeg 那格踩过这个坑，
 *  见 `openFFmpegFolder`）。
 *  目录一定存在（`config.toml` / `presets.toml` 都写在里面），不需要先建。 */
function openConfigDir() {
  const path = configDirFull.value;
  if (!path) {
    toast(t('no-select'), 'error');
    return;
  }
  invoke('open_dir', { path }).catch(toastError);
}

const getEncoderIcon = ref('mdi-auto-fix');
async function getEncoder(hevc: boolean) {
  getEncoderIcon.value = 'mdi-loading mdi-spin';
  try {
    const encoder = await invoke('get_encoder', { hevc });
    if (encoder && hevc) {
      config.value.encoderHevc = encoder as string;
    } else if (encoder) {
      config.value.encoderAvc = encoder as string;
    } else {
      toast(t('no-encoder'), 'error');
    }
  } catch (e) {
    toastError(e);
  } finally {
    getEncoderIcon.value = 'mdi-auto-fix';
  }
}

async function testEncoderHevc() {
  if (config.value.encoderHevc === '' || config.value.encoderHevc === null) {
    toast(t('no-select'), 'error');
    return null;
  }
  getEncoderIcon.value = 'mdi-loading mdi-spin';
  try {
    if (await invoke('test_encoder', { encoder: config.value.encoderHevc })) {
      toast(t('encoder-ok'), 'success');
    } else {
      toast(t('encoder-error'), 'error');
    }
  } catch (e) {
    toastError(e);
  } finally {
    getEncoderIcon.value = 'mdi-auto-fix';
  }
}

async function testEncoderAvc() {
  if (config.value.encoderAvc === '' || config.value.encoderAvc === null) {
    toast(t('no-select'), 'error');
    return null;
  }
  getEncoderIcon.value = 'mdi-loading mdi-spin';
  try {
    if (await invoke('test_encoder', { encoder: config.value.encoderAvc })) {
      toast(t('encoder-ok'), 'success');
    } else {
      toast(t('encoder-error'), 'error');
    }
  } catch (e) {
    toastError(e);
  } finally {
    getEncoderIcon.value = 'mdi-auto-fix';
  }
}


const locale = useStorage<string>('locale', 'zh-CN');
watch(locale, (val) => {
  if (!val) return;
  changeLocale(val);
}, { immediate: true });

/**
 * 调试项的入口与开关（都是纯界面偏好，存 localStorage）：
 * - `debugRevealed`：设置页里「显示调试」这一条本身露不露面，由 **Ctrl+Shift+点击左栏的「设置」** 切换
 *   （`App.vue` 的 `onSettingsClick`，共用这个键）。
 * - `debugEnabled`：勾上后「渲染」页才显示「谱面·调试」分区（`ConfigView.vue` 读同一个键）。
 */
const debugRevealed = useStorage<boolean>('phi.debug.revealed', false);
const debugEnabled = useStorage<boolean>('phi.debug.enabled', false);

/** 「默认展开侧边栏」的默认值（也是重置时填回去的那个值：勾上） */
const LIST_EXPAND_DEFAULT = true;

const listExpand = ref(
  localStorage.getItem("listExpand") !== null
    ? JSON.parse(localStorage.getItem("listExpand") as string)
    : LIST_EXPAND_DEFAULT
);

watch(listExpand, (val) => {
  localStorage.setItem("listExpand", JSON.stringify(val))
});

/**
 * 渲染弹窗宽度（百分比，15–100）：暴露到设置里，最小值 15，默认值也是 15（就是「最窄」那格）。
 * 与 `components/TaskStrip.vue` 共用同一个键 —— 那边也是 `useStorage('TaskStrip.width')`
 * （默认值必须和这里的 `STRIP_WIDTH_DEFAULT` 一致），同页多个实例之间会同步，
 * 所以这里一拉滑杆渲染弹窗立刻变宽/变窄，不用保存。
 */
const STRIP_WIDTH_DEFAULT = 15;
const stripWidth = useStorage<number>('TaskStrip.width', STRIP_WIDTH_DEFAULT);

// 主题只有一套（DeepDark），设置里没有主题切换项。

/* ===================== 关于（第二部分） ===================== */

// 关于页脚不显示发布号（`DISPLAY_VERSION`），只留作者那一行。
// 技术版本仍在 `DPR/src-tauri/Cargo.toml` 的 `package.version`（cargo / npm / tauri
// 与安装包都要求 semver），只是界面上不再露出。

// 仓库地址（与 `Cargo.toml` 的 `repository` 字段一致）。三个文件链接都走默认分支 main。
const REPO_URL = 'https://github.com/DaogeMM/DPRecorder';
const LICENSE_URL = `${REPO_URL}/blob/main/LICENSE`;
const LICENSES_URL = `${REPO_URL}/blob/main/LICENSES/README.md`;
const NOTICE_URL = `${REPO_URL}/blob/main/NOTICE`;
// 作者 B 站空间（https://space.bilibili.com/521730845）
// —— 贴在「关于」页脚那排入口的最后。
const BILIBILI_URL = 'https://space.bilibili.com/521730845';

/** 「引用项目」清单，逐条对照仓库根的 `NOTICE` 第 1 节（描述文字走 locale 的 `about-proj-*`） */
const upstreamProjects = [
  { repo: 'TeamFlos/phira', url: 'https://github.com/TeamFlos/phira', desc: 'about-proj-phira' },
  { repo: '2278535805/phire', url: 'https://github.com/2278535805/phire', desc: 'about-proj-phire' },
  { repo: 'Phira-Pro/Phira-Pro', url: 'https://github.com/Phira-Pro/Phira-Pro', desc: 'about-proj-phira-pro' },
  { repo: 'TeamFlos/phira-render', url: 'https://github.com/TeamFlos/phira-render', desc: 'about-proj-phira-render' },
  { repo: '2278535805/Phi-Recorder', url: 'https://github.com/2278535805/Phi-Recorder', desc: 'about-proj-phi-recorder' },
];

/** 上游依赖里的自定义 fork（`NOTICE` 第 1 节末尾那四个，都在 `github.com/2278535805/` 下） */
const forkedDeps = [
  '2278535805/prpr-macroquad',
  '2278535805/prpr-miniquad',
  '2278535805/sasa',
  '2278535805/oboe-rs',
];

// 打开外部链接统一走 `common.ts` 的 `openExternal`（它内部转给 Rust 的 `open_external`，
// 失败会 toast 出来）。不能用 `@tauri-apps/plugin-opener` 的 `openUrl`：它在
// Windows 上从 Tauri 命令线程调 ShellExecuteExW 时缺 `SEE_MASK_NOASYNC`，请求被静默丢弃，
// 点了没反应也不报错 —— 表现为点击链接无法唤醒浏览器。
</script>

<template>
  <div class="settings-view w-100 h-100 d-flex flex-column">
    <v-form ref="form" validateOn="eager" class="settings-view__scroll">
      <div class="settings-view__inner">
        <div class="config-root">
          <!-- ========== 一、设置 ========== -->
          <!-- 一行三个；
               条目名一律定宽（见样式），所以三栏里的输入框起点是对齐的。
               标题右边只有「重置」一枚图标按钮（保存那枚已经删掉 —— 改完自己就落盘了，
               见上面的 `autoSaveConfig`）。 -->
          <PhiGroup id="grp-setting" :title="t('setting')" plain>
            <template #actions>
              <div class="settings-actions">
                <v-btn icon="mdi-backup-restore" variant="text" size="small" :title="t('reset')" @click="resetDialog = true"></v-btn>
              </div>
            </template>
            <div class="cfg-grid">
              <!-- 「RPE 目录」这一项**暂时隐藏**：RPE 谱面列表那条路先不放出来，
                   代码原样留着，要恢复就把 v-if 去掉（下面 `selectRpeDir` 等也一起恢复使用）。 -->
              <CfgField v-if="false" :label="t('rpe-dir')" :tip="t('folder-tip')">
                <v-text-field clearable density="compact" hide-details="auto" v-model="config.rpeDir" :title="t('folder-tip')" append-inner-icon="mdi-folder-open" @click:append-inner="selectRpeDir" @contextmenu="openInFolder(config.rpeDir)"></v-text-field>
              </CfgField>
              <!-- 三个「路径」字段排在一行，**FFmpeg 路径排第一**：三项里只有它指向程序
                   文件夹之外的东西（装在哪台机器就得指哪台），另外两格都住在 exe 所在文件夹里，
                   所以框里写相对写法 `.\output` / `.\config\presets.toml`，完整路径留给悬停提示。 -->
              <CfgField :label="t('ffmpeg-path')">
                <!-- 只读、无 ✕、悬停看完整路径（对齐「预设配置文件路径」那格），
                     文件夹图标上挂 `folder-tip` 提示（外层 title 让给路径了，见 #append-inner 插槽）。
                     注意 title 用的是**显示值**（老配置里的 `...\ffmpeg.exe` 会先被 `asFFmpegDir` 抹成目录）。
                     这格是不做相对化的：ffmpeg 通常在程序文件夹之外，相对写法没有意义。 -->
                <v-text-field :class="{ 'settings-path--default': !config.ffmpegPath }" readonly density="compact" hide-details="auto" v-model="ffmpegPathShown" :placeholder="t('auto-detect')" :title="ffmpegPathShown" @contextmenu="openFFmpegFolder">
                  <template #append-inner>
                    <v-icon :title="t('folder-tip')" :aria-label="t('folder-tip')" @click="selectFFmpegDir">mdi-folder-open</v-icon>
                  </template>
                </v-text-field>
              </CfgField>
              <CfgField :label="t('output-dir')" :tip="t('folder-tip')">
                <!-- 格式对齐「预设配置文件路径」那格：只读、没有 ✕ 清除按钮、
                     悬停在文字上看完整路径。所以原来「一个 title 给整个框」的做法拆成两层：
                     title = `outputDirFull`（完整路径），框里写 `outputDirShown`（`.\output` 这种相对形态）。
                     ✕ 去掉后就没有「清空回默认」的入口了 —— 要改回来只走文件夹图标（或全局重置）。 -->
                <v-text-field :class="{ 'settings-path--default': !config.outputDir }" readonly density="compact" hide-details="auto" v-model="outputDirShown" :title="outputDirFull" @contextmenu="openInFolder(config.outputDir, true)">
                  <template #append-inner>
                    <v-icon :title="t('folder-tip')" :aria-label="t('folder-tip')" @click="selectOutputDir">mdi-folder-open</v-icon>
                  </template>
                </v-text-field>
              </CfgField>
              <!-- 「资源包」文件夹：放资源包的地方就在程序旁边
                   （`<程序目录>/respack`，和 output / config 一个待遇），所以框里写 `.\respack`、
                   悬停提示给完整路径。这格不给改（目录固定），文字按常灰处理。 -->
              <CfgField :label="t('respack-path')" :tip="t('folder-tip')">
                <v-text-field class="settings-path--default" readonly density="compact" hide-details="auto" v-model="respackDirShown" :title="respackDir" append-inner-icon="mdi-folder-open" @click:append-inner="openRespackDir" @contextmenu="openRespackDir"></v-text-field>
              </CfgField>
              <!-- 「附带资源包」文件夹：皮肤包与 Minecraft 材质由用户自己下载后放进这里，
                   目录同样固定在程序旁边，所以只读显示、文字按常灰处理。 -->
              <CfgField :label="t('appendant-path')" :tip="t('appendant-path-tip')">
                <v-text-field class="settings-path--default" readonly density="compact" hide-details="auto" v-model="appendantDirShown" :title="appendantDir" append-inner-icon="mdi-folder-open" @click:append-inner="openAppendantDir" @contextmenu="openAppendantDir"></v-text-field>
              </CfgField>
              <!-- 配置文件目录：显示与打开的**都是目录** `\config`（不是 `config/presets.toml`）。
                   文字**始终**灰色：这格只读、位置也不给改，显示的就是程序旁边那份 config 目录；
                   不能按「还没有自建预设时才灰」来做 —— 已经有预设时它会是白的。
                   框里写相对写法（`configDirShown`）、`:title` 挂完整路径 ——
                   打包后文件夹挪位置也不会显示上一台机器的绝对路径。 -->
              <CfgField :label="t('preset-file')" :tip="t('folder-tip')">
                <v-text-field class="settings-path--default" readonly density="compact" hide-details="auto" v-model="configDirShown" :title="configDirFull" append-inner-icon="mdi-folder-open" @click:append-inner="openConfigDir" @contextmenu="openConfigDir"></v-text-field>
              </CfgField>

              <CfgField :label="t('encoder-avc')" :tip="t('encoder-tip')">
                <v-text-field clearable density="compact" hide-details="auto" v-model="config.encoderAvc" :title="t('encoder-tip')" :append-inner-icon="getEncoderIcon" @click:append-inner="getEncoder(false)" @contextmenu="testEncoderAvc"></v-text-field>
              </CfgField>
              <CfgField :label="t('encoder-hevc')" :tip="t('encoder-tip')">
                <v-text-field clearable density="compact" hide-details="auto" v-model="config.encoderHevc" :title="t('encoder-tip')" :append-inner-icon="getEncoderIcon" @click:append-inner="getEncoder(true)" @contextmenu="testEncoderHevc"></v-text-field>
              </CfgField>
              <CfgField :label="t('lang')">
                <v-autocomplete density="compact" hide-details="auto" :items="SUPPORTED_LOCALES_NAME" item-title="name" item-value="code" v-model="locale"></v-autocomplete>
              </CfgField>

              <!-- 「输出日志到主终端」与「显示详细日志」两条**入口隐藏**：
                   开关本体、locale 里的两个键、以及后端读 `config.printStderr` /
                   `config.showDetailedLog` 全都原样留着 —— 旧配置里存过 true 也不会被改写，
                   想改直接编辑 `config.toml`。要恢复就把下面的 v-if="false" 去掉。 -->
              <CfgSwitch v-if="false" :label="t('print-stderr')" :tip="t('print-stderr-tip')" v-model="config.printStderr"></CfgSwitch>
              <CfgSwitch v-if="false" :label="t('show-detailed-log')" :tip="t('show-detailed-log-tip')" v-model="config.showDetailedLog"></CfgSwitch>
              <!-- 「开启『游玩』按钮」：打开后「渲染」页那一排按钮里会多出「游玩」，用它直接进
                   游戏里试谱（后端 `preview_play`）。 -->
              <CfgSwitch :label="t('enable-play-button')" :tip="t('enable-play-button-tip')" v-model="config.enablePlayButton"></CfgSwitch>
              <CfgSwitch :label="t('preview-fullscreen')" :tip="t('preview-fullscreen-tip')" v-model="config.previewFullscreen"></CfgSwitch>
              <CfgSwitch :label="t('list-expand')" :tip="t('list-expand-tip')" v-model="listExpand"></CfgSwitch>
              <!-- 「显示调试」：这一条本身也要先被露出来 —— Ctrl+Shift+点击左栏的「设置」才出现，
                   之后按 localStorage 记着。勾上后「渲染」页末尾才会多出「谱面·调试」分区。 -->
              <CfgSwitch v-if="debugRevealed" :label="t('show-debug')" :tip="t('show-debug-tip')" v-model="debugEnabled"></CfgSwitch>
              <!-- 「高性能模式」：它跟上面那个「默认展开侧边栏」是两回事：那一个是纯界面偏好（存
                   localStorage），这个是 app 配置项（写进 `config.toml`），由后端在提交渲染 /
                   游玩时注入那个渲染进程 —— 值落在 `config.highPerformance`。
                   **入口隐藏起来**（同上面「开启『游玩』按钮」的做法），
                   开关本体、`AppConfig.highPerformance` 字段、后端的注入、
                   locale 里的 `high-performance` / `high-performance-tip` 全都原样留着 ——
                   要恢复就把下面的 `v-if="false"` 去掉。隐藏期间想用这个模式，直接编辑
                   应用配置 `config.toml` 里的 `highPerformance = true`（设置页不做任何改写，
                   手动改的值也不会被存回去时抹掉）。 -->
              <CfgSwitch v-if="false" :label="t('high-performance')" :tip="t('high-performance-tip')" v-model="config.highPerformance"></CfgSwitch>
              <CfgSlider :label="t('strip-width')" :tip="t('strip-width-tip')" :min="15" :max="100" :step="1" v-model="stripWidth"></CfgSlider>
            </div>
          </PhiGroup>

          <!-- ========== 二、关于 ========== -->
          <!-- 「致谢 / 引用项目 / 开源许可 / 第三方素材」四条（连同文案与外链）在页脚之上；
               条目名一律定宽（见样式），所以各条后面的正文都从同一个 x 开始。
               「项目简介」整条没有（locale 里的 `about-intro` / `about-intro-label` 两个键先留着，
               要放回来还有）。
               版本号不显示，页脚整块居中、宽度按内容自适应。 -->
          <PhiGroup id="grp-about" :title="t('about')" plain>
            <div class="cfg-grid">
              <CfgField :label="t('about-thanks-label')" full>
                <p class="about-text">{{ t('about-thanks') }}</p>
              </CfgField>

              <CfgField :label="t('about-projects-label')" full>
                <div class="about-block">
                  <p class="about-hint">{{ t('about-projects-tip') }}</p>
                  <div class="about-links">
                    <div class="about-link-row" v-for="p in upstreamProjects" :key="p.repo">
                      <button type="button" class="about-link" @click="openExternal(p.url)"><v-icon size="14" class="about-link-icon">mdi-github</v-icon>{{ p.repo }}</button>
                      <span class="about-link-desc">{{ t(p.desc) }}</span>
                    </div>
                  </div>
                  <p class="about-hint about-hint--gap">{{ t('about-forks-tip') }}</p>
                  <div class="about-links about-links--row">
                    <button type="button" class="about-link" v-for="f in forkedDeps" :key="f" @click="openExternal(`https://github.com/${f}`)"><v-icon size="14" class="about-link-icon">mdi-github</v-icon>{{ f }}</button>
                  </div>
                </div>
              </CfgField>
            </div>

            <div class="cfg-grid">
              <CfgField :label="t('about-license-label')" full>
                <p class="about-text">{{ t('about-license') }}</p>
              </CfgField>
              <CfgField :label="t('about-assets-label')" full>
                <p class="about-text">{{ t('about-assets') }}</p>
              </CfgField>
              <CfgField :label="t('about-ai-label')" full>
                <p class="about-text">{{ t('about-ai') }}</p>
              </CfgField>
              <CfgField :label="t('about-unofficial-label')" full>
                <p class="about-text">{{ t('about-unofficial') }}</p>
              </CfgField>
            </div>

            <!-- 页脚：第一行只有作者那一行（不带版本号、不带 @），
                  第二行五个「图标 + 文字」入口，不写标题，整块居中 -->
            <div class="about-footer">
              <div class="about-footer__facts">
                <span class="about-value">{{ t('about-author') }}</span>
              </div>

              <div class="about-buttons about-buttons--center">
                <v-btn prepend-icon="mdi-github" variant="text" size="small" @click="openExternal(REPO_URL)">{{ t('open-repo') }}</v-btn>
                <v-btn prepend-icon="mdi-scale-balance" variant="text" size="small" @click="openExternal(LICENSE_URL)">{{ t('open-license') }}</v-btn>
                <v-btn prepend-icon="mdi-license" variant="text" size="small" @click="openExternal(LICENSES_URL)">{{ t('open-licenses') }}</v-btn>
                <v-btn prepend-icon="mdi-information-outline" variant="text" size="small" @click="openExternal(NOTICE_URL)">{{ t('open-notice') }}</v-btn>
                <!-- B 站入口：MDI 没有 bilibili 图标，用内联的 simple-icons 小电视
                     （`components/BilibiliIcon.vue`：MDI 里没有 bilibili 图标） -->
                <v-btn :prepend-icon="BilibiliIcon" variant="text" size="small" @click="openExternal(BILIBILI_URL)">{{ t('open-bilibili') }}</v-btn>
              </div>
            </div>
          </PhiGroup>
        </div>
      </div>
    </v-form>
  </div>

  <!-- 重置确认弹窗：皮肤与标题行走 `design/config.css:400-445` 那套共用类（齿轮配置窗体、
       「文件名格式」弹窗、批量页两个配置弹窗都在用），`App.vue` 的全局弹窗基底现在也是同一套
       （内容区底色 + 88% 不透明、直角、居中 1.5rem 标题、白字）。宽度仍是 `auto` / 最小 300px —— 共用类给卡片写的是 `width: 100%`，那是给带明确像素宽的弹窗用的。
       文案见 setting.json：`reset-confirm` / `reset-confirm-desc` / `reset-ok`。 -->
  <v-dialog v-model="resetDialog" width="auto" min-width="300px" class="log-card-bg">
    <v-card class="log-card-only-window phi-config-dialog reset-dialog">
      <div class="phi-config-dialog__head">
        <span class="phi-config-dialog__title" v-t="'reset-confirm'"></span>
      </div>
      <v-card-text class="phi-config-dialog__body">
        <p class="reset-dialog__desc" v-t="'reset-confirm-desc'"></p>
      </v-card-text>
      <v-card-actions class="justify-end reset-dialog__actions">
        <v-btn class="hover-scale" variant="text" @click="resetDialog = false" v-t="'cancel'"></v-btn>
        <v-btn class="hover-scale" variant="text" @click="resetConfig()" v-t="'reset-ok'"></v-btn>
      </v-card-actions>

    </v-card>
  </v-dialog>


</template>

<style scoped>
.settings-view__scroll {
  flex: 1 1 auto;
  min-height: 0;
  width: 100%;
  overflow-x: hidden;
  overflow-y: auto;
  padding: var(--phi-space-6);
}

/* 内容铺满内容区、不居中收窄 —— 渲染页（ConfigView）本来就是铺满的，设置页也一样，
   条目从左边开始排、两栏各自摊开。 */
.settings-view__inner {
  width: 100%;
}

/* 条目名一律定宽，后面的具体文字从同一起点开始写、视觉更整齐。定宽之后「设置」组
   三栏里的输入框起点也一致了；160px 是量着最长的两个名字定的
   （「输出日志到主终端」8 字 / 「指定 HEVC 编码器」）。 */
.settings-view :deep(#grp-setting .cfg-item__label),
.settings-view :deep(#grp-about .cfg-item__label) {
  flex: 0 0 160px;
  max-width: none;
}

/* 「设置」组的控件摊满剩下的宽度：一行三个之后每格只有 ~460px，
   再按默认的 40% 去算，路径 / 编码器这些字符串就太挤了 */
.settings-view :deep(#grp-setting .cfg-item__control) {
  flex: 1 1 auto;
}

/* 设置组第一行那三格「路径」：显示的是默认值（不是用户自己设的）时一律用这支淡色。
   · 「预设配置文件路径」永远如此 —— 只读、位置也不给改。
   · 「自定义输出目录」「FFmpeg 路径」则是 `config.*` 为空的期间如此：它们的默认值按真值
     显示（不是占位提示，选得中、复制得了），见 `outputDirShown` / `ffmpegPathShown`。
   `.config-root` 那几条纯白规则是 0,2,0，这里带 scoped 属性是 0,3,0，不用 `!important`；
   `input` 那一行不能少，Vuetify 把值画在 `<input>` 自己的 color 上。
   不透明度也要跟着降：Vuetify 的占位符是「color 再乘 `--v-disabled-opacity`」（深色主题 0.5），
   真值不打折就还是比占位符亮一档（实测占位符最亮 104，同一支灰的真值 177）。 */
.settings-path--default :deep(.v-field__input),
.settings-path--default :deep(.v-field input) {
  color: var(--phi-label-tertiary);
  opacity: var(--v-disabled-opacity, 0.5);
}

/* 「重置」图标按钮挂在「设置」标题右边，只要一排跟着标题右端走，
   靠右由 PhiGroup 标题行里的 `v-spacer` 负责。 */
.settings-actions {
  display: flex;
  align-items: center;
  gap: var(--phi-space-1);
  flex: none;
}

/* 「关于」最下面的页脚：整块居中。按钮文字直接复用 locale 的 `open-*` 键；
   这一行是作者那一行 + 五个入口（GitHub 仓库 / GPL-3.0 全文 / 第三方许可证 / 出处与致谢 / B 站）。
   这一行比固定 30% 宽，所以页脚宽度按内容自适应（`fit-content` + `auto` 外边距
   ⇒ 仍然居中），只有窄窗口时才回落到内容宽度。 */
.about-footer {
  width: fit-content;
  max-width: 100%;
  margin: var(--phi-space-4) auto var(--phi-space-2);
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--phi-space-1);
}

.about-footer__facts {
  display: flex;
  align-items: center;
  gap: var(--phi-space-2);
  /* 作者那一行放大一点：正文默认 1rem ⇒ 1.15rem。 */
  font-size: 1.15rem;
}

/* 「关于」里的说明文字：`.cfg-item__control` 默认是右对齐的（给勾选框、数字框用），
   长段落要左对齐并占满剩下的宽度；整行条目改成顶部对齐，条目名就像小标题。 */
.settings-view :deep(#grp-about .cfg-item--full) {
  align-items: flex-start;
  padding-top: var(--phi-space-1);
  padding-bottom: var(--phi-space-1);
}

.settings-view :deep(#grp-about .cfg-item--full .cfg-item__control) {
  flex: 1 1 auto;
  justify-content: flex-start;
}

.about-text {
  flex: 1 1 auto;
  margin: 0;
  font-size: 0.95rem;
  line-height: 1.75;
  color: var(--phi-label-secondary);
}

/* 页脚里出现的值（作者那一行，不含版本号）：颜色跟 `.about-text` 一样
   （`.config-root` 里 `--phi-label-secondary` 已重定成纯白，见 design/config.css），
   字号由 `.about-footer__facts` 统一给（1.15rem）。 */
.about-value {
  color: var(--phi-label-secondary);
}

/* 下面这组是「引用项目 / 上游 fork」两段清单用的。
   `.about-block` 撑满 `CfgField` 的控件区，`flex-direction: column` 让说明行、清单、再一行说明
   自上而下排。 */
.about-block {
  flex: 1 1 auto;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: var(--phi-space-1);
}

/* 「关于」里的说明文字：也是正文，用白色
   （tertiary 是给占位符 / 禁用项那种「淡」用的）。 */
.about-hint {
  margin: 0;
  font-size: 0.9rem;
  color: var(--phi-static-bluish-00);
}

.about-hint--gap {
  margin-top: var(--phi-space-2);
}

.about-links {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.about-links--row {
  flex-direction: row;
  flex-wrap: wrap;
  gap: 4px var(--phi-space-4);
}

.about-link-row {
  display: flex;
  align-items: baseline;
  gap: var(--phi-space-2);
  min-width: 0;
}

.about-link {
  flex: none;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 0;
  border: 0;
  background: none;
  font: inherit;
  font-size: 0.95rem;
  color: var(--phi-accent);
  cursor: pointer;
}

/* 链接前面那个 GitHub 小标 */
.about-link-icon {
  color: var(--phi-label-tertiary);
}

.about-link:hover {
  text-decoration: underline;
}

.about-link-desc {
  font-size: 0.9rem;
  /* 同上：说明文字也是正文，改白 */
  color: var(--phi-static-bluish-00);
}

.about-buttons {
  flex: 1 1 auto;
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--phi-space-2);
}

/* 页脚那排入口要居中（`.about-buttons` 默认靠左）：文字 0.7rem（Vuetify `size="small"`
   自带 `.75rem`）、图标 15px、内边距与间距收紧，颜色统一成次要色 ——
   `.v-btn--variant-text` 本来就是 `color: inherit`，所以只给按钮写 color 就够了。 */
.about-buttons--center {
  flex: 0 0 auto;
  justify-content: center;
  gap: var(--phi-space-2);
}

.about-buttons--center :deep(.v-btn) {
  font-size: 0.7rem;
  min-width: 0;
  padding: 0 var(--phi-space-2);
  color: var(--phi-label-secondary);
}

.about-buttons--center :deep(.v-btn .v-icon) {
  font-size: 15px;
}

.about-buttons :deep(.v-btn) {
  text-transform: none;
}

/* 重置确认弹窗（见模板里那段注释）：皮肤与标题行用 `design/config.css` 的共用类，
   这里只补两处共用类没管的间距 —— 正文左右留白（那套皮肤是给 `.cfg-grid` 用的，
   网格自带 4px 侧边距，换成一段文字就得自己给），以及按钮那排的下边距。 */
.reset-dialog__desc {
  margin: 0;
  padding: 0 calc(var(--phi-space-3) * 2.2) var(--phi-space-3);
}

.reset-dialog__actions {
  padding: 0 calc(var(--phi-space-3) * 2.2) var(--phi-space-3);
}
</style>
