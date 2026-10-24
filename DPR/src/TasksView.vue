<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch, type Ref } from 'vue';

import { useI18n } from 'vue-i18n';
const { t } = useI18n();

import type { Task, TaskStatus } from './model';

import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { openPath } from '@tauri-apps/plugin-opener';
import * as dialog from '@tauri-apps/plugin-dialog';

import moment from 'moment';
import { toast, toastError } from './common';

import router from './router';
import { AnsiUp } from 'ansi_up';
const ansi = new AnsiUp()

import { startDrag } from "@crabnebula/tauri-plugin-drag";
import { useStorage } from '@vueuse/core';

import CfgSwitch from './components/CfgSwitch.vue';
import ChartHero from './components/ChartHero.vue';
import { diffBadge, diffLvl, diffTier, pickActiveTask, refreshTasks, useTasks } from './tasks';
import { finishPageLoad } from './pageLoad';

// 任务列表与 700ms 轮询都在 `./tasks` 里，和全局渲染弹窗、窗口任务栏进度共用同一份数据
const tasks = useTasks();

// 首屏等第一次拉取完再渲染
await refreshTasks();
// 列表到位了才揭开内容区那层转圈（见 `./pageLoad`）
finishPageLoad();

/**
 * 显示顺序：最旧的在上、最新的在下。
 * 后端 `get_tasks` 给的是「最新在前」（`task.rs` 的 `tasks()` 末尾 `result.reverse()`；批量页与渲染弹窗
 * 都按「第 0 个＝刚提交的」用），所以这里只翻显示顺序，不动后端。
 */
const orderedTasks = computed(() => [...(tasks.value ?? [])].reverse());

/* ---------- 折叠行右侧的状态图标 ----------
   箭头左边放四枚状态图标（已渲染 / 正在渲染 / 暂停渲染 / 等待渲染），**每行只显示
   当前状态那一个** —— 加载与混音准备共用「正在渲染」那枚，失败与取消各自一个告警图标。
   不配色，跟着行文字颜色走，一律用圆形系。
   「正在渲染」显示暂停图标、「暂停渲染」显示播放图标：图标表示的是这一行还能做的动作，
   与展开区里那两个按钮的图标对得上。 */
function statusIcon(type: TaskStatus['type']): string {
  switch (type) {
    case 'pending':
      return 'mdi-timer-sand'; // 等待渲染
    case 'loading':
    case 'mixing':
    case 'mixing_sfx':
    case 'rendering':
      return 'mdi-pause-circle-outline'; // 正在渲染（还能做的是「暂停」）
    case 'paused':
      return 'mdi-play-circle-outline'; // 暂停渲染（还能做的是「继续」）
    case 'done':
      return 'mdi-check-circle-outline'; // 已渲染
    case 'failed':
      return 'mdi-alert-circle-outline';
    case 'canceled':
      return 'mdi-cancel';
    default:
      return 'mdi-help-circle-outline';
  }
}

/* ============================ 批量选取 ============================
   仿照批量页的底部栏给任务页做批量选取：勾选、批量暂停 / 取消 / 移除。
   勾选态不能像批量页那样挂在任务对象上：
   `tasks` 是后端直出、每 700ms 整体替换的（`./tasks`），挂上去会被下一轮轮询抹掉，所以另存一份 id 集合。 */
const selectedIds = ref<Set<number>>(new Set());
/** Shift 范围选择用的锚点：上一次点勾选框的那一行 */
let selectAnchorId: number | null = null;

function setSelected(id: number, on: boolean) {
  if (on) selectedIds.value.add(id);
  else selectedIds.value.delete(id);
}

/**
 * 点勾选框：默认切换这一行；按住 Shift 时从锚点整段追加。
 * `Ctrl`/`⌘` 与普通点击都走切换（＝ Ctrl 点选增减）。
 */
function toggleSelect(id: number, index: number, event: MouseEvent) {
  const ids = orderedTasks.value.map((task) => task.id);
  if (event.shiftKey && selectAnchorId !== null) {
    const from = ids.indexOf(selectAnchorId);
    if (from >= 0) {
      const [start, end] = from <= index ? [from, index] : [index, from];
      for (let i = start; i <= end; i++) selectedIds.value.add(ids[i]!);
      return;
    }
  }
  selectAnchorId = id;
  setSelected(id, !selectedIds.value.has(id));
}

function selectAll() {
  for (const task of orderedTasks.value) selectedIds.value.add(task.id);
}

function selectInvert() {
  for (const task of orderedTasks.value) setSelected(task.id, !selectedIds.value.has(task.id));
}

/** 取消选择（底部栏上「取消」与「取消选择」是两个按钮） */
function clearSelection() {
  selectedIds.value.clear();
  selectAnchorId = null;
}

/** 批量暂停：`pause_task` 只对正在渲染的那条有意义 */
async function pauseSelectTask() {
  for (const task of orderedTasks.value) {
    if (!selectedIds.value.has(task.id) || task.status.type !== 'rendering') continue;
    await invoke('pause_task', { id: task.id }).catch((e) => {
      toastError(e);
    });
  }
  await refreshTasks();
}

/** 还能被取消的状态（已经结束的不碰，否则会被标成已取消） */
const CANCELABLE_STATES: TaskStatus['type'][] = ['pending', 'loading', 'mixing', 'mixing_sfx', 'rendering', 'paused'];

/** 批量取消：交给 `cancelTasks` —— 正在渲染的那几条会先问一句要不要连文件一起移除 */
async function cancelSelectTask() {
  await cancelTasks(orderedTasks.value.filter((task) => selectedIds.value.has(task.id) && CANCELABLE_STATES.includes(task.status.type)));
}

/** 批量移除：按钮本身只负责弹询问框（单条移除与批量共用同一个弹窗，见下面的 `openRemoveDialog`） */
function removeSelectTask() {
  openRemoveDialog([...selectedIds.value]);
}

/* ---------------------- 取消前的询问 ----------------------
   正在渲染的任务被取消时，磁盘上可能已经有一个（还没写完的）输出文件，
   所以先问一句要不要连文件一起移除。任务页两处「取消」共用这一个弹窗：
   展开区里的单条取消按钮与底部栏的批量取消；批量时只要选中项里有一个在渲染，就只弹一次。
   还在排队（`pending`）的任务没碰过磁盘，直接取消、不打扰。 */
const cancelDialog = ref(false);
/** 这一次要取消的任务 id（单条取消时只有一个） */
const cancelIds = ref<number[]>([]);
/** 与 `cancelIds` 一一对应的输出路径，只在用户选择「取消并移除文件」时用得上 */
const cancelOutputs = ref<string[]>([]);

async function cancelTasks(targets: Task[]) {
  if (!targets.length) return;
  cancelIds.value = targets.map((task) => task.id);
  cancelOutputs.value = targets.map((task) => task.output);
  if (!targets.some((task) => RUNNING_STATES.includes(task.status.type))) {
    await doCancel(false);
    return;
  }
  cancelDialog.value = true;
}

async function doCancel(removeFiles: boolean) {
  cancelDialog.value = false;
  const ids = cancelIds.value;
  const outputs = cancelOutputs.value;
  for (const id of ids) {
    await invoke('cancel_task', { id }).catch((e) => {
      toastError(e);
    });
  }
  if (removeFiles) {
    for (const path of outputs) {
      if (!path) continue; // 还没算出输出路径的任务没有文件可删
      await invoke('delete_file', { path }).catch((e) => {
        toastError(e);
      });
    }
  }
  await refreshTasks();
}

/** 任务被删掉/清空后，勾选集合里要跟着丢掉已经不存在的 id */
watch(tasks, (list) => {
  const alive = new Set((list ?? []).map((task) => task.id));
  for (const id of [...selectedIds.value]) {
    if (!alive.has(id)) selectedIds.value.delete(id);
  }
});

/* ---------------------- 渲染完成 / 失败提示 ----------------------
   `render-done` 只有渲染页在听（`RenderView.vue`），而任务页的列表是 700ms 轮询来的，
   所以排在后面的任务自己跑完时这里一声不吭（只有状态图标换了颜色）⇒ 在轮询回调里跟上一轮的
   `status.type` 比一次，谁从「进行中」走到 `done` / `failed` 就弹一条 toast。
   首次拉取的这一轮只记基线、不提示（否则一进页面就会把历史任务全播一遍）。 */
const RUNNING_STATES: TaskStatus['type'][] = ['loading', 'mixing', 'mixing_sfx', 'rendering', 'paused'];
const lastStatus = new Map<number, TaskStatus['type']>();
/** 首屏那一轮轮询只记基线（历史任务不该被当成「刚跑完」处理） */
let firstPoll = true;

/** 「渲染完后移除」：和批量页那个同名开关一样，只从任务列表里删掉，**输出文件留着** */
const removeAfterRender = useStorage<boolean>('TasksView.removeAfterRender', false);

async function removeFinished(ids: number[]) {
  for (const id of ids) {
    await invoke('remove_task', { id, removeFile: false }).catch((e) => {
      toastError(e);
    });
  }
  await refreshTasks();
}

watch(tasks, (list) => {
  const alive = new Set<number>();
  /** 这一轮刚渲染完的任务（开了「渲染完后移除」时下面统一删掉） */
  const finished: number[] = [];
  for (const task of list ?? []) {
    alive.add(task.id);
    const before = lastStatus.get(task.id);
    lastStatus.set(task.id, task.status.type);
    if (firstPoll || before === task.status.type) continue;
    const wasRunning = before !== undefined && RUNNING_STATES.includes(before);
    if (task.status.type === 'done') {
      if (wasRunning) toast(t('task-done', { name: task.name }), 'success');
      finished.push(task.id);
    } else if (task.status.type === 'failed' && wasRunning) {
      toast(t('task-failed', { name: task.name }), 'error');
    }
  }
  for (const id of [...lastStatus.keys()]) {
    if (!alive.has(id)) lastStatus.delete(id);
  }
  firstPoll = false;
  if (removeAfterRender.value && finished.length) void removeFinished(finished);
}, { immediate: true });

/* --------------------------- 长按拖动任务条调整渲染顺序 ---------------------------
   渲染顺序就是从列表第一行往下走，所以拖动任务条就能调整顺序。
   交互：**按住任意一行约 350ms 就把它拿起来**，之后上下移动改顺序，松手落位。
   只允许「等待渲染」（`pending`）的任务被拖；后端 `move_task` 改的就是渲染顺序
   （worker 取列表里第一个 Pending，见 `src-tauri/src/task.rs`）。

   这里是手写 pointer 手势，**不用 HTML5 拖放**（`draggable` + `dragstart`/`dragover`/`drop`）：
   `src-tauri/tauri.conf.json:65` 是 `"dragDropEnabled": true`（谱面拖入导入靠它，见
   `RenderView.vue:157` / `BatchView.vue:203` 的 `listen('tauri://drag-drop', …)`），Windows 上
   WebView2 的 HTML5 拖放会被这套原生拖放接管，页面内的 `dragstart`/`drop` 压根不触发 ——
   HTML5 拖放在这里就是不管用。
   ------------------------------------------------------------------------------ */
/** 按住多久算「拿起来」（毫秒） */
const LONG_PRESS_MS = 350;
/** 长按成立之前的允许抖动，超过就当成滚动/划动，取消这次长按 */
const PRESS_SLOP = 10;
/** 拿起来之后要走多远才当成真的在移动（否则算原地放下） */
const DRAG_SLOP = 3;

const listEl = ref<HTMLElement | null>(null);
/** 正在拖的任务 id（null = 没在拖） */
const dragTaskId = ref<number | null>(null);
/** 拖动中的落点：插到第几行**之前**（0…n） */
const dragSlot = ref<number | null>(null);
/** 长按已经成立（用于行变淡、落点提示线的显示） */
const dragging = ref(false);
/** 已经拖到窗口外面：松手就移除这条任务 */
const dragOutside = ref(false);
let pressTimer: number | null = null;
let pressArmed = false;
let pressStartX = 0;
let pressStartY = 0;
let dragFromIndex = -1;
let dragMoved = false;

/** 高亮哪一行（落点在末尾时高亮最后一行、线画在它下面） */
const dragOverIndex = computed(() => {
  if (dragSlot.value === null) return null;
  return Math.min(dragSlot.value, Math.max(orderedTasks.value.length - 1, 0));
});
const dragAtEnd = computed(() => dragSlot.value !== null && dragSlot.value >= orderedTasks.value.length);

function clearPressTimer() {
  if (pressTimer !== null) {
    window.clearTimeout(pressTimer);
    pressTimer = null;
  }
}

/**
 * 指针是不是已经拖出窗口。
 *
 * 按着鼠标拖动时系统会把鼠标事件继续发给窗口（窗口外的坐标是负数或超出窗口尺寸），
 * 所以这里直接看坐标 —— 「拖到窗口外松手就移除」这个手势就靠它判。
 * 四边各留 4px 余量：贴着边来回蹭不该被当成「拖出去了」。
 */
function isOutsideWindow(event: PointerEvent) {
  const margin = 4;
  return (
    event.clientX < -margin ||
    event.clientY < -margin ||
    event.clientX > window.innerWidth + margin ||
    event.clientY > window.innerHeight + margin
  );
}

function endRowPointer(reset: boolean) {
  window.removeEventListener('pointermove', onRowPointerMove);
  window.removeEventListener('pointerup', onRowPointerUp);
  window.removeEventListener('pointercancel', onRowPointerUp);
  clearPressTimer();
  listEl.value?.classList.remove('tasks-list--dragging');
  window.removeEventListener('selectstart', onSelectStart);
  pressArmed = false;
  dragMoved = false;
  dragging.value = false;
  if (reset) {
    dragTaskId.value = null;
    dragSlot.value = null;
    dragFromIndex = -1;
    dragOutside.value = false;
  }
}

/**
 * 长按补上的那个 click 会展开/收起这一行，拦掉它。
 * 那个 click 就在本轮事件里，所以挂一个 `once` 的捕获监听、再在下一个宏任务摘掉，
 * 免得下一次真正的点击被残留监听吃掉。
 */
function suppressNextClick() {
  const handler = (event: MouseEvent) => {
    event.stopPropagation();
    event.preventDefault();
  };
  window.addEventListener('click', handler, { capture: true, once: true });
  window.setTimeout(() => window.removeEventListener('click', handler, { capture: true }), 0);
}

/**
 * 长按期间浏览器会顺手开始选字，按下瞬间就把 `selectstart` 拦掉。
 * 按下瞬间挂上这个监听，整段按压期间把 `selectstart` 拦掉；
 * 抬手后立刻摘掉，所以正常拖选复制文字仍然可用。
 */
function onSelectStart(event: Event) {
  event.preventDefault();
}

/** 在整行上按下：等长按成立再进入拖动（整行长按拖动） */
function onRowPointerDown(event: PointerEvent, task: Task, index: number) {
  if (event.button !== 0 || task.status.type !== 'pending') return;
  const el = event.target as HTMLElement | null;
  // 按钮 / 链接 / 勾选框上的按下属于它们自己的交互，不要抢
  if (el?.closest('button, a, .task-item__check')) return;
  pressArmed = false;
  dragMoved = false;
  dragOutside.value = false;
  dragFromIndex = index;
  pressStartX = event.clientX;
  pressStartY = event.clientY;
  // 一按下就进入「按住」状态：文字立刻不可选，等长按成立再变拖动
  listEl.value?.classList.add('tasks-list--dragging');
  window.addEventListener('selectstart', onSelectStart);
  clearPressTimer();
  pressTimer = window.setTimeout(() => {
    pressTimer = null;
    pressArmed = true;
    dragging.value = true;
    dragTaskId.value = task.id;
    dragSlot.value = index;
  }, LONG_PRESS_MS);
  window.addEventListener('pointermove', onRowPointerMove);
  window.addEventListener('pointerup', onRowPointerUp);
  window.addEventListener('pointercancel', onRowPointerUp);
}

/** 光标落在第几个「插入位」上：按每行的中线比较，返回 0…n */
function slotAt(clientY: number): number {
  const rows = listEl.value?.querySelectorAll<HTMLElement>('.task-row') ?? [];
  let slot = 0;
  rows.forEach((row, i) => {
    const box = row.getBoundingClientRect();
    if (clientY > box.top + box.height / 2) slot = i + 1;
  });
  return slot;
}

function onRowPointerMove(event: PointerEvent) {
  if (!pressArmed) {
    // 长按还没成立：抖动超限说明用户是在滚动/划动，取消这次长按
    const dx = event.clientX - pressStartX;
    const dy = event.clientY - pressStartY;
    if (Math.hypot(dx, dy) > PRESS_SLOP) endRowPointer(true);
    return;
  }
  if (!dragMoved) {
    if (Math.abs(event.clientY - pressStartY) < DRAG_SLOP) {
      // 竖直还没动，但可能已经横着拖出窗口了 —— 拖出移除走的是另一条路，不靠 `dragMoved`
      dragOutside.value = isOutsideWindow(event);
      return;
    }
    dragMoved = true;
  }
  dragOutside.value = isOutsideWindow(event);
  dragSlot.value = slotAt(event.clientY);
  // 贴着列表上下边缘时自动滚动，方便把任务拖到当前看不见的位置
  const list = listEl.value;
  if (!list) return;
  const box = list.getBoundingClientRect();
  if (event.clientY < box.top + 40) list.scrollTop -= 12;
  else if (event.clientY > box.bottom - 40) list.scrollTop += 12;
}

function onRowPointerUp(event: PointerEvent) {
  const armed = pressArmed;
  const moved = dragMoved;
  const id = dragTaskId.value;
  const slot = dragSlot.value;
  const from = dragFromIndex;
  // 松手那一刻的坐标更准（拖动中途窗口外的坐标也是准的）
  const outside = dragOutside.value || isOutsideWindow(event);
  endRowPointer(true);
  if (!armed) return; // 没长按成立：这次点击照旧走 `@click`（展开/收起这一行）
  suppressNextClick();
  if (id === null) return;
  // 拖到窗口外松手 ⇒ 直接把这条任务从列表里删掉，不动输出文件
  if (outside) {
    void invoke('remove_task', { id, removeFile: false })
      .then(() => refreshTasks())
      .catch((e) => toastError(e));
    return;
  }
  if (!moved || slot === null || from < 0) return;
  const total = orderedTasks.value.length;
  // `move_task` 的 index 是「把自己摘掉之后」的下标
  let to = slot > from ? slot - 1 : slot;
  to = Math.max(0, Math.min(to, total - 1));
  if (to === from) return;
  void invoke('move_task', { id, index: to })
    .then(() => refreshTasks())
    .catch((e) => toastError(e));
}

/* ------------------------------- 快捷键 -------------------------------
   Esc 取消选择、Delete 批量移除；Ctrl/⌘+A 全选；空格暂停 / 继续选中项
   （没有选中项时作用于当下正在跑的那一条）。焦点在输入框里时一律不抢键，
   焦点在按钮上时也不抢空格（那时空格是「按下这个按钮」）。 */
function pauseTargets(): Task[] {
  const selected = orderedTasks.value.filter((task) => selectedIds.value.has(task.id));
  const pool = selected.length ? selected : [pickActiveTask(tasks.value)].filter((task): task is Task => !!task);
  // 有在渲染的就先暂停它们，否则把暂停中的恢复回来
  const rendering = pool.filter((task) => task.status.type === 'rendering');
  if (rendering.length) return rendering;
  return pool.filter((task) => task.status.type === 'paused');
}

/** 一批任务要么全在渲染、要么全暂停，所以只需看第一条走哪个命令 */
async function applyPause(targets: Task[]) {
  const command = targets[0]!.status.type === 'rendering' ? 'pause_task' : 'resume_task';
  for (const task of targets) {
    await invoke(command, { id: task.id }).catch((e) => {
      toastError(e);
    });
  }
  await refreshTasks();
}

function onKeydown(event: KeyboardEvent) {
  const el = event.target as HTMLElement | null;
  if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable)) return;
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'a') {
    selectAll();
    event.preventDefault();
  } else if (event.key === ' ' || event.code === 'Space') {
    if (el?.closest('button, a, [role="button"], .v-btn')) return;
    const targets = pauseTargets();
    if (!targets.length) return; // 没得暂停/继续时把空格留给页面滚动
    void applyPause(targets);
    event.preventDefault();
  } else if (event.key === 'Escape' && selectedIds.value.size) {
    clearSelection();
    event.preventDefault();
  } else if (event.key === 'Delete' && selectedIds.value.size) {
    void removeSelectTask();
    event.preventDefault();
  }
}

onMounted(() => window.addEventListener('keydown', onKeydown));
onUnmounted(() => {
  window.removeEventListener('keydown', onKeydown);
  window.removeEventListener('pointermove', onRowPointerMove);
  window.removeEventListener('pointerup', onRowPointerUp);
  window.removeEventListener('pointercancel', onRowPointerUp);
  window.removeEventListener('selectstart', onSelectStart);
  clearPressTimer();
});

function formatDuration(seconds: number) {
  const duration = moment.duration(Math.ceil(seconds), 'seconds');
  const hours = Math.floor(duration.asHours());
  const minutes = duration.minutes();
  const secs = duration.seconds();

  if (hours > 0) {
    return `${hours} ${t('duration.hours')} ${minutes} ${t('duration.minutes')} ${secs} ${t('duration.seconds')}`;
  } else if (minutes > 0) {
    return `${minutes} ${t('duration.minutes')} ${secs} ${t('duration.seconds')}`;
  } else if (secs > 0) {
    return `${secs} ${t('duration.seconds')}`;
  } else {
    return '';
  }
}


function describeStatus(status: TaskStatus): string {
  switch (status.type) {
    case 'null':
      return ''
    case 'pending':
      return t('status.pending');
    case 'loading':
      return t('status.loading');
    case 'mixing':
      return t('status.mixing');
    case 'mixing_sfx':
      return t('status.mixing', {
        progress: (status.progress * 100).toFixed(2),
      });
    case 'rendering':
      return t('status.rendering', {
        progress: (status.progress * 100).toFixed(2),
        fps: status.fps,
        estimate: status.estimate ? formatDuration(status.estimate) : '',
      });
    case 'paused':
      return t('status.paused', {
        progress: (status.progress * 100).toFixed(2),
      });
    case 'done':
      return t('status.done', {
        duration: status.duration ? formatDuration(status.duration) : '',
      });
    case 'canceled':
      return t('status.canceled');
    case 'failed':
      return t('status.failed');
  }
}

const outputDialog = ref(false);
const outputDialogMessage = ref('');
// 导出要用**原始**那份（带 ANSI 颜色码的纯文本），面板里显示的这份是转好的 HTML
const outputDialogRaw = ref('');
const filteredOutputDialogMessage = ref('');
const outputDialogName = ref('');

function openOutputDialog(output: string, name: string = '') {
  outputDialogName.value = name;
  outputDialogRaw.value = output;
  outputDialogMessage.value = ansi.ansi_to_html(output);
  filteredOutputDialogMessage.value = outputDialogMessage.value;
  filter.value = [];
  outputDialog.value = true;
}

const exportingOutput = ref(false);

/** 把「输出」面板里这段日志存成文件（后端落盘前会剥掉 ANSI 颜色码）。 */
async function exportOutput() {
  exportingOutput.value = true;
  try {
    // 默认存到日志目录里，文件名带任务名，免得几次导出互相覆盖
    const safeName = (outputDialogName.value || 'task').replace(/[\\/:*?"<>|]/g, '_');
    const fileName = `${safeName}-output.log`;
    const logDir = await invoke<string>('get_log_dir_path').catch(() => '');
    const outputPath = await dialog.save({
      title: t('export-log'),
      filters: [{ name: 'Log File', extensions: ['log', 'txt'] }],
      defaultPath: logDir ? `${logDir}\\${fileName}` : fileName,
    });
    if (!outputPath) return;
    await invoke('export_log', {
      outputPath,
      content: outputDialogRaw.value,
    });
    await dialog.message(t('export-log-success'), { title: t('export-log') });
  } catch (e) {
    toastError(e);
  } finally {
    exportingOutput.value = false;
  }
}

const filterItems: string[] = ["INFO", "DEBUG", "WARN", "ERROR", "! INFO", "! DEBUG", "! WARN", "! ERROR"];
const filter: Ref<string[]> = ref([]);

function filterText(
  rawText: string,
  filters: string[]
): string {
  const include: string[] = [];
  const exclude: string[] = [];

  for (let f of filters) {
    f = f.trim();
    if (!f) continue;
    if (f.startsWith('!')) {
      const kw = f.slice(1).trim();
      if (kw) exclude.push(kw);
    } else {
      include.push(f);
    }
  }

  if (include.length === 0 && exclude.length === 0) {
    return rawText;
  }

  const resultLines = rawText
    .split(/\r?\n/)
    .filter(line => {
      if (include.length > 0) {
        const hitInclude = include.some(kw => line.includes(kw));
        if (!hitInclude) return false;
      }
      const hitExclude = exclude.some(kw => line.includes(kw));
      if (hitExclude) return false;

      return true;
    });

  return resultLines.join('\n');
}

async function showInFolder(path: string) {
  try {
    // 走后端：`explorer.exe /select,...` 由 shell 起进程。插件那版 `revealItemInDir`
    // 走 COM 的 `SHOpenFolderAndSelectItems`，在这个进程里实测没反应（详见 lib.rs）。
    await invoke('reveal_in_dir', { path });
  } catch (e) {
    toastError(e);
  }
}

async function openFile(path: string) {
  try {
    await openPath(path);
  } catch (e) {
    toastError(e);
  }
}

async function showOutputFolder() {
  try {
    await invoke('open_output_folder');
  } catch (e) {
    toastError(e);
  }
}

/**
 * 把这个任务重新载入渲染页（沿用它的谱面与配置）。
 * 入口是 Hero（曲绘图）上的悬停层。
 */
function openInRender(task: Task) {
  router.push({
    name: 'render',
    query: {
      chart: task.path,
      info: JSON.stringify(task.info),
      config: JSON.stringify(task.config),
    },
  });
}

/* ---------------------- 移除前的询问 ----------------------
   「移除」= 把任务从列表里摘掉（任务条上那个垃圾桶按钮与底部栏的「移除」共用这个弹窗）。
   规律与「取消」一致，但多了一层：
   1. 还在渲染的任务**先取消再移除** —— 只从列表里摘掉的话，worker 手里那个任务会继续渲染，
      变成一条看不见、也停不掉的任务；
   2. 磁盘上可能已经有渲染出来的文件，所以先问一句要不要连文件一起移除。
   中间那句提示只在选中的任务里真有在跑的（`RUNNING_STATES`）时才出现。 */
const removeDialog = ref(false);
/** 这次要移除的任务 id（单条时只有一个，批量移除是多个） */
const removeIds = ref<number[]>([]);
/** 选中的任务里有没有还在跑的（决定弹窗里那句中不出现） */
const removeRunning = computed(() =>
  orderedTasks.value.some((task) => removeIds.value.includes(task.id) && RUNNING_STATES.includes(task.status.type)),
);

function openRemoveDialog(ids: number[]) {
  if (!ids.length) return;
  removeIds.value = [...ids];
  removeDialog.value = true;
}

async function removeTask(removeFile: boolean = false) {
  removeDialog.value = false;
  for (const id of removeIds.value) {
    const task = orderedTasks.value.find((item) => item.id === id);
    // 还能被取消的（排队中 / 正在渲染 / 已暂停）先置取消位：让正在写文件的那条停下来，
    // 否则它会继续渲染成一个已经不在列表里的任务（后端 `remove_with_output` 里也有同样一道保险）
    if (task && CANCELABLE_STATES.includes(task.status.type)) {
      await invoke('cancel_task', { id }).catch((e) => {
        toastError(e);
      });
    }
    await invoke('remove_task', { id, removeFile }).catch((e) => {
      toastError(e);
    });
  }
  clearSelection();
  await refreshTasks();
}

function dragOutput(event: DragEvent, task: Task) {
  event.preventDefault();
  if (task.status.type !== 'done') return;
  startDrag({ item: [task.output], icon: task.cover })
}
/** 展开的任务 id（任务卡片做成下拉栏，默认只显示小曲绘 / 进度条 / 曲名 / 难度） */
const expandedIds = ref<number[]>([]);

function toggleExpanded(id: number) {
  const index = expandedIds.value.indexOf(id);
  if (index < 0) expandedIds.value.push(id);
  else expandedIds.value.splice(index, 1);
}

/** 小曲绘：任务早就把曲绘落盘成临时文件了（`TaskView.cover`），直接转 asset URL 即可 */
function coverUrl(task: Task) {
  return task.cover ? convertFileSrc(task.cover) : '';
}

</script>

<template>
  <div class="tasks-view h-100 d-flex flex-column" style="gap: 1rem">
      <!-- 空列表：样式对齐批量页的空列表（与批量一样的字体样式，文案「(暂无任务)」） -->
    <div v-if="!tasks || !tasks.length" class="tasks-empty">
      <p class="tasks-empty__title" v-t="'empty'"></p>
    </div>
    <!-- 普通滚动列表（**不用 `v-virtual-scroll`**）：
         拖动排序要按每行的真实位置算落点，`ref="listEl"` 就是给它用的。 -->
    <div
      v-else
      ref="listEl"
      class="tasks-list scroll-mask px-8"
      style="scrollbar-width: none; padding-top: 30px"
    >
      <template v-for="(item, index) in orderedTasks" :key="item.id">
        <!-- 勾选框放在任务矩形**外面**的左侧。 -->
        <div class="task-item">
          <span
            class="task-item__check"
            :class="{ 'task-item__check--on': selectedIds.has(item.id) }"
            @mousedown.prevent
            @click.stop="toggleSelect(item.id, index, $event)"
          >
            <v-checkbox-btn
              :model-value="selectedIds.has(item.id)"
              density="compact"
              :ripple="false"
            ></v-checkbox-btn>
          </span>
          <v-card class="task-card mb-8" :class="{ 'task-card--open': expandedIds.includes(item.id) }">
            <!-- 折叠行（任务做成下拉栏的样式，默认只显示小曲绘、进度条、曲名和难度）：
                 整行可点；进度条紧贴在它下面，详细信息要拉开才渲染。
                 长按 350ms 就把这一行拿起来调渲染顺序（只对「等待中…」的任务生效）。 -->
            <div
              class="task-row"
              :class="{
                'task-row--dragging': dragging && dragTaskId === item.id,
                'task-row--drag-over': dragging && dragOverIndex === index && !(dragAtEnd && index === orderedTasks.length - 1),
                'task-row--drag-over-end': dragging && dragAtEnd && index === orderedTasks.length - 1,
                'task-row--drag-out': dragOutside && dragTaskId === item.id,
              }"
              :data-task-id="item.id"
              @click="toggleExpanded(item.id)"
              @pointerdown="onRowPointerDown($event, item, index)"
            >
              <div class="task-row__cover">
                <img v-if="item.cover" :src="coverUrl(item)" alt="" />
                <v-icon v-else size="18">mdi-image-outline</v-icon>
              </div>
              <div class="task-row__text">
                <span class="task-row__name">{{ item.name }}</span>
                <span v-if="diffBadge(item.info)" class="task-row__diff">
                  <span class="task-row__diff-tier">{{ diffTier(item.info) }}</span>
                  <span v-if="diffTier(item.info) && diffLvl(item.info)" class="task-row__diff-lvl">Lv.{{ diffLvl(item.info) }}</span>
                </span>
              </div>
              <!-- 状态图标：四种状态各一个，只显示当前状态那一个 -->
              <v-icon
                class="task-row__status"
                size="20.7"
                :title="describeStatus(item.status)"
              >{{ statusIcon(item.status.type) }}</v-icon>
              <v-icon class="task-row__chevron" size="20">mdi-chevron-down</v-icon>
            </div>

            <!-- 进度条：渲染/混音阶段走百分比，加载与混音准备阶段走不确定态。
                 这里必须用逐项比较（而不是 `[…].includes(...)`）：`v-if` 的等值比较
                 才能让 vue-tsc 把 `item.status` 收窄到带 `progress` 的那几支。 -->
            <v-progress-linear
              v-if="item.status.type === 'rendering' || item.status.type === 'mixing_sfx' || item.status.type === 'paused'"
              :model-value="item.status.progress * 100"
              height="6"
              rounded
            ></v-progress-linear>
            <v-progress-linear
              v-else-if="item.status.type === 'loading' || item.status.type === 'mixing'"
              :indeterminate="true"
              height="6"
              class="glow-spinner"
            ></v-progress-linear>

            <!-- 拉开后的详细信息：顶部横幅复用渲染页「谱面信息」那块 Hero
                 （曲绘 + 曲名 + 难度徽章 + 曲师/画师/谱师），下面是状态 / 谱面路径 / 操作按钮 -->
            <v-expand-transition>
              <div v-if="expandedIds.includes(item.id)" class="task-detail">
                <div class="task-hero">
                  <ChartHero :info="item.info" :path="item.path" :cover-path="item.cover" />
                  <!-- 悬停时整层浮出：点击＝把这个任务载入渲染页，拖动＝把渲染产物拖出去 -->
                  <div
                    class="task-hero__hint"
                    :title="t('open-in-render')"
                    draggable="true"
                    @click="openInRender(item)"
                    @dragstart="dragOutput($event, item)"
                  >
                    <i class="mdi mdi-reload icon"></i>
                  </div>
                </div>

                <!-- 底部：左边状态与谱面路径，右边该状态下的操作按钮 -->
                <div class="task-foot">
                  <div class="task-foot__text">
                    <p class="task-foot__status">{{ describeStatus(item.status) }}</p>
                    <p
                      class="task-foot__path select"
                      :title="item.path"
                      @click="showInFolder(item.path)"
                    >{{ item.path }}</p>
                  </div>
                  <v-spacer></v-spacer>
                  <div class="task-foot__buttons">
                    <template v-if="['pending', 'loading', 'mixing', 'mixing_sfx', 'rendering', 'paused'].includes(item.status.type)">
                      <v-btn
                        v-if="item.status.type === 'rendering'"
                        class="hover-scale btn"
                        variant="text"
                        @click="invoke('pause_task', { id: item.id })"
                        :title="t('pause')"
                        icon="mdi-pause"></v-btn>
                      <v-btn
                        v-if="item.status.type === 'paused'"
                        class="hover-scale btn"
                        variant="text"
                        @click="invoke('resume_task', { id: item.id })"
                        :title="t('resume')"
                        icon="mdi-play"></v-btn>
                      <v-btn class="hover-scale btn"
                        variant="text"
                        @click="cancelTasks([item])"
                        :title="t('cancel')"
                        icon="mdi-cancel"></v-btn>
                    </template>
                    <template v-else-if="['failed', 'canceled'].includes(item.status.type)">
                      <v-btn
                        variant="flat"
                        @click="openRemoveDialog([item.id])"
                        :title="t('remove-task')"
                        icon="mdi-delete"
                        class="hover-scale btn"></v-btn>
                      <v-btn
                        variant="flat"
                        prepend-icon="mdi-alert-circle-outline"
                        @click="() => {
                            if (item.status.type === 'failed' || item.status.type === 'canceled') {
                              openOutputDialog(item.status.output, item.name);
                            }
                          }"
                        @contextmenu="openRemoveDialog([item.id])"
                        :title="t('show-output')"
                        icon="mdi-bug"
                        class="hover-scale btn"></v-btn>
                    </template>
                    <template v-else-if="item.status.type === 'done'">
                      <v-btn
                        variant="text"
                        @click="openFile(item.output)"
                        :title="t('open-file')"
                        icon="mdi-open-in-new"
                        class="hover-scale btn"></v-btn>
                      <v-btn
                        variant="flat"
                        prepend-icon="mdi-folder-open-outline"
                        @click="showInFolder(item.output)"
                        :title="t('show-in-folder')"
                        icon="mdi-folder-open"
                        class="hover-scale btn"></v-btn>
                      <v-btn
                        variant="flat"
                        prepend-icon="mdi-text-box-outline"
                        @click="
                          () => {
                            if (item.status.type === 'done') {
                              openOutputDialog(item.status.output, item.name);
                            }
                          }
                        "
                        :title="t('show-output')"
                        icon="mdi-bug"
                        class="hover-scale btn"></v-btn>
                    </template>
                  </div>
                </div>
              </div>
            </v-expand-transition>
          </v-card>
        </div>
      </template>
    </div>

    <!-- 拖到窗口外松手＝移除这条任务。提示只有全屏压暗 + 正中间一行带括号的文字；
         fixed 定位，不挤动下面的操作栏 -->
    <div v-if="dragOutside" class="tasks-drop-hint">{{ t('drop-to-remove') }}</div>

    <!-- 底部批量操作栏：照批量页 `.batch-view__actions` 的样式（仿照批量的底部栏）。
         「取消」与「取消选择」是两个按钮；那个开关是「渲染完后移除」，
         跟批量页同名的那个一个意思（勾选态存在 localStorage 里）。
         排布：「已选 N 项」贴左边，中间用 v-spacer 顶开，右边收着开关与那一排按钮
         （所以开关在按钮左边） -->
    <div class="tasks-view__actions">
      <div class="tasks-view__bar">
        <span class="tasks-view__count">{{ t('selected-count', { n: selectedIds.size }) }}</span>
        <v-spacer></v-spacer>
        <CfgSwitch class="tasks-view__switch" small :label="t('remove-after-render')" v-model="removeAfterRender" />
        <v-btn class="hover-scale" size="small" variant="text" :disabled="!orderedTasks.length" @click="selectAll" v-t="'select-all'"></v-btn>
        <v-btn class="hover-scale" size="small" variant="text" :disabled="!orderedTasks.length" @click="selectInvert" v-t="'select-invert'"></v-btn>
        <v-btn class="hover-scale" size="small" variant="text" :disabled="!selectedIds.size" @click="pauseSelectTask" v-t="'pause-select'"></v-btn>
        <v-btn class="hover-scale" size="small" variant="text" :disabled="!selectedIds.size" @click="cancelSelectTask" v-t="'cancel-render'"></v-btn>
        <v-btn class="hover-scale" size="small" variant="text" :disabled="!selectedIds.size" @click="removeSelectTask" v-t="'remove-select'"></v-btn>
        <v-btn class="hover-scale" size="small" variant="text" :disabled="!selectedIds.size" @click="clearSelection" v-t="'clear-select'"></v-btn>
      </div>
    </div>

    <v-dialog v-model="outputDialog" class="log-card-bg">
      <v-card class="log-card-window">
        <v-card-title style="margin-bottom: -16px;" v-t="'output'"></v-card-title>
        <v-card-text style="padding-bottom: 0px;">
          <div
            class="block whitespace-pre overflow-auto log-card-msg user-select"
            style="height: calc(100vh - 240px);"
            v-html="filteredOutputDialogMessage"
          ></div>
          <v-combobox
            class="mt-4"
            variant="outlined"
            v-model="filter"
            :items="filterItems"
            clearable
            multiple
            placeholder="Filter (comma separated)"
            @update:model-value="(val) => {
              filteredOutputDialogMessage = filterText(outputDialogMessage, val);
            }"
          ></v-combobox>
        </v-card-text>
        <v-card-actions class="justify-end">
          <v-btn
            class="hover-scale"
            variant="text"
            prepend-icon="mdi-content-save-outline"
            :loading="exportingOutput"
            @click="exportOutput"
            v-t="'export-log'"
          ></v-btn>
          <v-btn class="hover-scale" variant="text" @click="outputDialog = false" v-t="'close'"></v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
    <!-- 取消前的询问：正在渲染的任务被取消时，磁盘上可能已经躺着一个还没写完的输出文件 -->
    <v-dialog v-model="cancelDialog" width="auto" min-width="400px" class="log-card-bg">
      <v-card class="log-card-only-window">
        <v-card-title v-t="'cancel-render'"> </v-card-title>
        <v-card-text>
          <div class="block whitespace-pre overflow-auto">{{ t('cancel-render-file-confirm') }}</div>
        </v-card-text>
        <v-card-actions class="justify-end">
          <v-btn class="hover-scale" variant="text" @click="cancelDialog = false" v-t="'close'"></v-btn>
          <v-btn class="hover-scale" variant="text" @click="doCancel(false)" v-t="'cancel-render-only'"></v-btn>
          <v-btn class="hover-scale" variant="text" @click="doCancel(true)" v-t="'cancel-render-and-remove-file'"></v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
    <!-- 移除前的询问：正在渲染的那几条会先被取消，所以顺带问一句要不要连文件一起移除 -->
    <v-dialog v-model="removeDialog" width="auto" min-width="400px" class="log-card-bg">
      <v-card class="log-card-only-window">
        <v-card-title v-t="'remove-task'"> </v-card-title>
        <v-card-text>
          <div class="block whitespace-pre overflow-auto">{{ t('remove-task-confirm') }}</div>
          <div v-if="removeRunning" class="block whitespace-pre overflow-auto">{{ t('remove-task-running') }}</div>
          <div class="block whitespace-pre overflow-auto">{{ t('remove-task-file-confirm') }}</div>
        </v-card-text>
        <v-card-actions class="justify-end">
          <v-btn class="hover-scale" variant="text" @click="removeDialog = false" v-t="'cancel'"></v-btn>
          <v-btn class="hover-scale" variant="text" @click="removeTask(false)" v-t="'remove-task-only'"></v-btn>
          <v-btn class="hover-scale" variant="text" @click="removeTask(true)" v-t="'remove-task-and-file'"></v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
  </div>
</template>

<style scoped>

/* 整页铺满内容区（渲染界面利用宽度 100%）：
   页面本身 100%（底部操作栏像渲染页那样通到底），**任务条**单独收在 95%（见 `.task-item`）。 */
.tasks-view {
  position: relative;
  width: 100%;
}

/* 空列表：样式对齐批量页（`.batch-view__empty` / `__empty-title`，见 BatchView.vue） */
.tasks-empty {
  flex: 1 1 auto;
  display: flex;
  align-items: center;
  justify-content: center;
}

.tasks-empty__title {
  margin: 0;
  font-size: 1.05rem;
  color: var(--phi-label-secondary);
}

/* 普通滚动列表（不用虚拟滚动）：自己滚，撑满剩余高度 */
.tasks-list {
  position: relative;
  flex: 1 1 auto;
  min-height: 0;
  overflow-y: auto;
}

/* 长按拖动期间：别顺手把曲名文字选中 */
.tasks-list--dragging {
  user-select: none;
}

/* 底部批量操作栏：照批量页 `.batch-view__actions` / `.batch-view__bar` 那两排 */
.tasks-view__actions {
  flex: none;
  display: flex;
  flex-direction: column;
  gap: var(--phi-space-1);
  padding: var(--phi-space-2) var(--phi-space-6);
  border-top: 0.5px solid var(--phi-border-l3);
}

.tasks-view__bar {
  display: flex;
  flex-wrap: nowrap;
  align-items: center;
  gap: var(--phi-space-2);
  min-width: 0;
}

/* 操作条里那个方形开关：不要「条目」那套行高与内边距，只留 名字 + 小开关
   （和批量页 `.batch-view__actions :deep(.cfg-item)` 那三条一样） */
.tasks-view__bar :deep(.cfg-item) {
  min-height: 0;
  padding: 0;
  gap: var(--phi-space-2);
}

.tasks-view__bar :deep(.cfg-item__label) {
  max-width: none;
  font-size: 0.8rem;
}

.tasks-view__bar :deep(.cfg-item__control) {
  flex: none;
  margin-left: 0;
}

.tasks-view__count {
  font-size: 0.85rem;
  color: var(--phi-label-secondary);
}

.tasks-view__switch {
  /* 「已选 N 项」在左边、v-spacer 顶开，开关跟着右边那排按钮
     （窄屏时 spacer 缩到 0，这点左边距让它不贴着「已选 N 项」） */
  margin-left: var(--phi-space-2);
}

.task-card {
  /* 任务条下半部分的圆角去掉，只保留上半部分 */
  border-radius: var(--phi-radius-lg) var(--phi-radius-lg) 0 0 !important;
  background: var(--phi-bg-layer-1);
  margin: 5px;
  border: 0.5px solid var(--phi-border-l3);
  box-shadow: var(--phi-elevation-soft);
}

.task-card:hover {
  background: var(--phi-bg-layer-2);
}

/* ---------- 折叠行（下拉栏头）：小曲绘 + 曲名 + 难度徽章 + 右侧箭头 ---------- */
.task-row {
  display: flex;
  align-items: center;
  gap: var(--phi-space-3);
  padding: var(--phi-space-3) var(--phi-space-4);
  /* 高度写死成 `--task-row-h`，勾选框的中心才对得上（内容自然高度只有 ~58px） */
  box-sizing: border-box;
  min-height: var(--task-row-h);
  cursor: pointer;
}

.task-row__cover {
  display: flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 64px;
  aspect-ratio: 2048 / 1080;
  overflow: hidden;
  border: 0.5px solid var(--phi-border-l3);
  border-radius: var(--phi-radius-sm);
  background: var(--phi-bg-layer-3);
  color: var(--phi-label-caption);
}

/* ---------- 任务单元：勾选框在任务矩形**外面**的左侧 ---------- */
.task-item {
  position: relative;
  /* 任务条只用 95% 并居中 */
  width: 95%;
  margin: 0 auto;
  /* 折叠行的高度（勾选框要按它对垂直中心，见 `.task-item__check`）。
     别当成 64px：曲绘是 64px **宽** + `aspect-ratio: 2048/1080`，高只有 ~34px，
     所以折叠行 = 34 + 上下各 12px padding ≈ 58px。 */
  --task-row-h: 58px;
}

/* 勾选框：**默认不显示**，
   悬停该行 / 已勾选 / 键盘聚焦时才淡入（见下面那条 hover 规则）。
   绝对定位放进列表左侧那段留白，不占任务条的宽度 ⇒ 任务条保持 95% 居中。
   垂直居中：`top` 落在折叠行中线上，再用 `translateY(-50%)` 把自己的中心贴上去。 */
.task-item__check {
  position: absolute;
  /* 与任务条的间隔再放大 20%（间隔大 20%）：32 × 1.2 = 38.4 */
  left: -38.4px;
  top: calc(5px + var(--task-row-h) / 2);
  transform: translateY(-50%);
  display: flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  opacity: 0;
  transition: opacity 0.12s ease;
  /* 点勾选框时别把旁边的文字选中（禁用勾选文字）：
     文件末尾那条 `:deep(span) { user-select: text }` 会命中这个 span，这里按类名盖掉 */
  user-select: none;
}

.task-item:hover .task-item__check,
.task-item__check--on,
.task-item__check:focus-within {
  opacity: 1;
}

.task-item__check :deep(.v-selection-control) {
  min-height: 24px;
}

.task-item__check :deep(.v-icon) {
  font-size: 22px;
}

/* 拖动排序时的落点提示：在目标行上沿画一条主色横线（拖到末尾时画在最后一行的下沿） */
.task-row--drag-over {
  box-shadow: inset 0 2px 0 var(--phi-accent);
}

.task-row--drag-over-end {
  box-shadow: inset 0 -2px 0 var(--phi-accent);
}

/* 正在被拖走的那一行淡一点 */
.task-row--dragging {
  opacity: 0.45;
}

/* 已经拖出窗口：这一行淡到快看不见，配合下面的提示胶囊 */
.task-row--drag-out {
  opacity: 0.2;
}

/* 「（松开移除此任务）」提示：整屏压暗 + 正中一行文字；
   `pointer-events: none` 所以不挡拖动指针事件，也不参与布局 */
.tasks-drop-hint {
  position: fixed;
  inset: 0;
  z-index: 30;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgb(0 0 0 / 45%);
  color: var(--phi-danger);
  font-size: 1.15rem;
  pointer-events: none;
}

.task-row__cover img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.task-row__text {
  display: flex;
  flex: 1 1 auto;
  align-items: center;
  gap: var(--phi-space-3);
  min-width: 0;
}

.task-row__name {
  overflow: hidden;
  min-width: 0;
  font-size: 1rem;
  font-weight: 600;
  line-height: 1.25;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--phi-label-primary);
}

/* 难度徽章：与 ChartHero / 渲染弹窗那枚「灰底白字、无圆角」一致 */
.task-row__diff {
  display: inline-flex;
  flex: none;
  align-items: center;
  gap: 5px;
  height: 20px;
  padding: 0 7px;
  border-radius: 0;
  background: var(--phi-static-neutral-600);
  color: var(--phi-static-bluish-00);
  font-size: 0.72rem;
  font-weight: 400;
  letter-spacing: 0.06em;
}

.task-row__diff-tier {
  text-transform: uppercase;
}

.task-row__diff-lvl {
  font-variant-numeric: tabular-nums;
  letter-spacing: 0;
}

.task-row__chevron {
  flex: none;
  color: var(--phi-label-caption);
  transition: transform 0.15s ease;
}

.task-card--open .task-row__chevron {
  transform: rotate(180deg);
}

/* 状态图标（下拉箭头左边那一枚）：
   不配色，跟着行文字颜色走；尺寸 = 18px × 1.15 ≈ 20.7px（图标放大 15%）。 */
.task-row__status {
  flex: none;
}

/* 顶部横幅就是渲染页「谱面信息」那个 ChartHero.vue，这里只负责固定高度与上面那层悬停操作。
   高度写死 200px 让每张卡片等高（不用给虚拟滚动估算 item-height）。 */
.task-hero {
  position: relative;
  /* 自成层叠上下文，并让悬停层（z-index: 2）盖在 Hero 内部内容之上。
     `ChartHero` 的 `.chart-hero__inner` 是 `position: relative; z-index: 1`，
     比 `z-index: auto` 的悬停层高一层 —— 不改的话点击全被那层透明内容吃掉，
     `@click="openInRender(item)"` 根本不触发。 */
  z-index: 1;
}

.task-hero :deep(.chart-hero),
.task-hero :deep(.chart-hero__inner) {
  height: 200px;
  min-height: 0;
}

/* Hero 在渲染页是 `PhiSection` 的 head，右侧多留了 36px 给那个折叠箭头；
   任务页没有箭头，收回来让左右内边距对称。 */
.task-hero :deep(.chart-hero__inner) {
  padding-right: var(--phi-space-5);
}

/* 悬停时整层浮出：点击＝载入渲染页，拖动＝把渲染产物拖出去。
   不悬停时 `pointer-events: none`，否则点到横幅上的曲名/曲师文字就会跳页。 */
.task-hero__hint {
  position: absolute;
  inset: 0;
  z-index: 2; /* 必须高过 `.chart-hero__inner` 的 z-index: 1，否则点击落不到这一层 */
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgb(0 0 0 / 45%);
  color: var(--phi-static-bluish-00);
  opacity: 0;
  pointer-events: none;
  transition: opacity 0.15s ease;
}

.task-hero:hover .task-hero__hint {
  opacity: 1;
  pointer-events: auto;
}

/* 卡片底部：左边状态与谱面路径，右边该状态下的操作按钮 */
.task-foot {
  display: flex;
  align-items: center;
  gap: var(--phi-space-4);
  padding: var(--phi-space-3) var(--phi-space-4);
}

.task-foot__text {
  flex: 1 1 auto;
  min-width: 0;
}

.task-foot__status {
  margin: 0;
  font-size: 0.85rem;
  color: var(--phi-label-secondary);
}

.task-foot__path {
  overflow: hidden;
  margin: var(--phi-space-1) 0 0;
  font-size: 0.75rem;
  color: var(--phi-label-caption);
  text-overflow: ellipsis;
  white-space: nowrap;
  cursor: pointer;
}

.task-foot__path:hover {
  color: var(--phi-label-secondary);
  text-decoration: underline;
}

.task-foot__buttons {
  display: flex;
  flex: none;
  align-items: center;
}

.btn {
  background: var(--phi-interactive-hover);
  padding: 8px 14px;
  margin: 4px 8px;
  font-weight: 600;
}

.icon {
  font-size: 250%;
}

:deep(span) {
  user-select: text;
}

</style>
