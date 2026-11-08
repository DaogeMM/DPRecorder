<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue';

import { useI18n } from 'vue-i18n';
const { t } = useI18n();

import { invoke } from '@tauri-apps/api/core';

import type { ChartInfo, DraftFolder, DraftInfo, DraftMedia, DraftMediaKind, DraftTrackKind, EditorDraft } from '@/model';
import { MAX_FOLDER_DEPTH } from '@/model';
import { clearDrag, dragEffect, dragFolderId, dragMedia, dragPoint, dragRoot, dropAt, updateDragTarget } from './media-drag';

/**
 * 编辑窗口左边的功能栏。
 *
 * 剪映左侧是「媒体 / 音频 / 文字」这类素材入口：这里立三页 ——
 * 信息页是草稿自己的谱面信息（曲名 / 难度 / 谱师 / 曲师），由用户手填，
 * 预览时按这份内容拼 `info.txt` 交给引擎，所以草稿完全是一张自定义谱面；
 * 素材页是草稿的素材库，导入的文件都挂在这里，**拖到时间轴的轨道上**就放到谱面上；
 * 效果页留给判定线、粒子这些以后的内容。
 * 选中轨道上那份素材的属性与素材效果在右边的编辑栏（`EditorEditBar`），不在这里。
 *
 * 素材项只记路径（草稿不复制文件），所以拖拽时带的是这一项本身，不带文件内容。
 * 素材库按文件夹分组：导入一个谱面包（目录或 `.pez` / `.zip`）就按它里面的真实目录建出文件夹
 * （最多三十一层），自己新建的空文件夹用来归类散着导入的文件；文件夹只是分组，不搬动磁盘上的文件。
 */

const props = withDefaults(
  defineProps<{
    draft: EditorDraft | null;
    info: ChartInfo | null;
    /** 曲绘轨道上那份图现做出来的 Blob URL；没指定曲绘时为空，退回谱面包里的那张。 */
    coverUrl?: string;
  }>(),
  { coverUrl: '' },
);

const emit = defineEmits<{
  import: [];
  'import-folder': [];
  'add-folder': [parent: string | null];
  assign: [media: DraftMedia, track?: DraftTrackKind];
  remove: [media: DraftMedia];
  reveal: [media: DraftMedia];
  clear: [kind: DraftTrackKind];
  'update-info': [info: DraftInfo];
  'move-media': [media: DraftMedia, folder: string | null];
  'rename-folder': [folder: DraftFolder, name: string];
  'delete-folder': [folder: DraftFolder];
  /** 效果页里点「加文字噪域」：由父级往时间轴上加一条特效轨。 */
  'add-effect': [];
}>();

/** 信息页的四行：曲名必填，其余可以留空。 */
const FIELDS: { key: keyof DraftInfo; label: string; required: boolean }[] = [
  { key: 'name', label: 'editor-info-name', required: true },
  { key: 'difficulty', label: 'editor-info-difficulty', required: false },
  { key: 'charter', label: 'editor-info-charter', required: false },
  { key: 'composer', label: 'editor-info-composer', required: false },
];

const EMPTY_INFO: DraftInfo = { name: '', difficulty: '', charter: '', composer: '' };

/** 每敲一个字就把整份信息抛给父级，由醒着的那一方攒起来写盘。 */
function onInfoInput(key: keyof DraftInfo, event: Event) {
  const value = (event.target as HTMLInputElement).value;
  emit('update-info', { ...(props.draft?.info ?? EMPTY_INFO), [key]: value });
}

type TabId = 'info' | 'media' | 'effects';

const tab = ref<TabId>('info');

const tabs: { id: TabId; icon: string; label: string }[] = [
  { id: 'info', icon: 'mdi-information-outline', label: 'editor-info' },
  { id: 'media', icon: 'mdi-folder-multiple-image', label: 'editor-media' },
  { id: 'effects', icon: 'mdi-auto-fix', label: 'editor-effects' },
];

const ICONS: Record<DraftMediaKind, string> = {
  chart: 'mdi-file-music-outline',
  music: 'mdi-music-note',
  cover: 'mdi-image-outline',
  lyric: 'mdi-subtitles-outline',
};

/** 素材一律只显示文件名：路径留着当悬浮提示，列里挤不下就省略号。 */
function shortName(item: DraftMedia) {
  const raw = item.name || item.path;
  const cut = Math.max(raw.lastIndexOf('\\'), raw.lastIndexOf('/'));
  return cut >= 0 ? raw.slice(cut + 1) : raw;
}

/** 这一项是不是已经在它那条轨道上了（在轨道上的再点「放到轨道」只是重复放同一项）。 */
function onTrack(item: DraftMedia) {
  const d = props.draft;
  if (!d) return false;
  // 歌词不占轨道（铺出来的是特效片段，素材本身不挂在轨道上），所以永远不会打勾
  const current = item.kind === 'chart' ? d.chart : item.kind === 'music' ? d.music : d.cover;
  return current?.path === item.path && current.kind === item.kind;
}

/**
 * 素材库的分组。
 *
 * `activeFolder` 为空时看的是根（没有上级的那一层），点开一个文件夹只看它里面的。
 * 文件夹自己也能有子文件夹 —— 「导入素材 → 选文件夹」按真实目录建，最上面那层就是选中的目录，
 * 最多三十一层；拖素材卡到文件夹卡上就把它归进去；拖动数据仍走 `application/x-dpe-media`。
 */
const activeFolder = ref<string | null>(null);
const renameId = ref<string | null>(null);
const renameText = ref('');
/** 改名时的输入框：进改名状态就自动聚焦并选中旧名字。 */
const renameInput = ref<HTMLInputElement | null>(null);

const folders = computed(() => props.draft?.folders ?? []);
const allMedia = computed(() => props.draft?.media ?? []);
const currentFolder = computed(() => folders.value.find((it) => it.id === activeFolder.value) ?? null);

/** 这一层的子文件夹；根上是没有上级（`parent` 为空）的那一层。 */
const childFolders = computed(
  () => folders.value.filter((it) => (it.parent ?? null) === activeFolder.value),
);

/** 从「全部素材」到当前文件夹的那条路：画成面包屑，也用来判断还能不能再往下建。 */
const folderPath = computed(() => {
  const chain: DraftFolder[] = [];
  const seen = new Set<string>();
  let node: DraftFolder | null = currentFolder.value;
  while (node && !seen.has(node.id)) {
    chain.unshift(node);
    seen.add(node.id);
    const parent = node.parent ?? null;
    node = parent ? (folders.value.find((it) => it.id === parent) ?? null) : null;
  }
  return chain;
});

/** 已经到最大层数就不让再往下建：更深的目录导入时也摊在这一级里。 */
const canAddChild = computed(() => folderPath.value.length < MAX_FOLDER_DEPTH);

/** 完整路径的文本：面包屑最多占三行，更长的部分靠悬停看。 */
const folderPathText = computed(() =>
  [t('editor-media-root'), ...folderPath.value.map((it) => it.name)].join(' / '),
);

/**
 * 面包屑里真的画出来的那几层：路径很深时只留最后五层，前面收成一个「…」。
 *
 * 只靠 CSS 裁三行会把最前面的「全部素材」一起裁掉，所以先在数据上收窄；
 * 「…」本身能点，点一下往上一层，被收起来的祖先就一层层露出来。
 */
const CRUMB_VISIBLE = 5;
const crumbNodes = computed(() =>
  folderPath.value.length > CRUMB_VISIBLE ? folderPath.value.slice(-CRUMB_VISIBLE) : folderPath.value,
);
const hiddenCrumbs = computed(() => folderPath.value.length - crumbNodes.value.length);

/** 点「…」：把当前文件夹换成紧挨着可见窗口上面的那一层。 */
function openHiddenCrumb() {
  const target = folderPath.value[folderPath.value.length - CRUMB_VISIBLE - 1];
  if (target) openFolder(target.id);
}

/** 素材多了以后按名字筛：不区分大小写，路径也一起搜。 */
const query = ref('');

const visibleMedia = computed(() => {
  const inScope =
    activeFolder.value === null
      ? allMedia.value.filter((it) => !it.folder)
      : allMedia.value.filter((it) => it.folder === activeFolder.value);
  const needle = query.value.trim().toLowerCase();
  if (!needle) return inScope;
  return inScope.filter((it) => `${it.name} ${it.path}`.toLowerCase().includes(needle));
});

/** 这一层有没有东西可列：搜索时只列筛出来的素材，文件夹行先收起来。 */
const hasRows = computed(
  () => visibleMedia.value.length > 0 || (!query.value.trim() && childFolders.value.length > 0),
);

function openFolder(id: string | null) {
  activeFolder.value = id;
  renameId.value = null;
}

/**
 * 点文件夹卡进文件夹 —— 但刚把素材拖到这张卡上时松手也会算一次 click，
 * 那一下不该顺带钻进去，所以拖完的几百毫秒里忽略点击。
 */
function openFolderByClick(id: string) {
  if (performance.now() - lastDragEndAt < 300) return;
  openFolder(id);
}

/** 换草稿、或者当前文件夹被删掉时回到根，别停在一个不存在的目录里。 */
watch(
  () => [props.draft?.id, folders.value.map((it) => it.id).join(',')] as const,
  () => {
    if (activeFolder.value !== null && !folders.value.some((it) => it.id === activeFolder.value)) {
      activeFolder.value = null;
    }
  },
);

/* ---- 素材卡的 pointer 拖拽 ----
 *
 * 窗口开着 `dragDropEnabled`，WebView2 会吞掉页面内的 HTML5 拖放，所以不能靠
 * `draggable`，改成自己跟指针：按下记起点 → 移动过阈值算开始拖 → 松手按落点决定
 * 放到轨道上（`[data-track-kind]`）、还是收进文件夹（`[data-folder-id]`）／挪回
 * 「全部素材」（`[data-media-root]`）。高亮由 `updateDragTarget` 按指针位置算。
 */
const DRAG_THRESHOLD = 4;
const pointerDrag = ref(false);
/** 上一次拖拽结束的时间：用来吞掉「拖到文件夹卡上松手」顺带触发的那次 click。 */
let lastDragEndAt = 0;
let press: { item: DraftMedia; x: number; y: number; moved: boolean } | null = null;

function onCardPointerDown(event: PointerEvent, item: DraftMedia) {
  if (event.button !== 0) return;
  press = { item, x: event.clientX, y: event.clientY, moved: false };
  window.addEventListener('pointermove', onPointerMove);
  window.addEventListener('pointerup', onPointerUp);
  window.addEventListener('pointercancel', cancelPress);
  window.addEventListener('keydown', onPressKeydown);
}

function onPointerMove(event: PointerEvent) {
  if (!press) return;
  const dx = event.clientX - press.x;
  const dy = event.clientY - press.y;
  if (!press.moved && Math.hypot(dx, dy) < DRAG_THRESHOLD) return;
  if (!press.moved) {
    press.moved = true;
    pointerDrag.value = true;
    dragMedia.value = press.item;
    document.body.classList.add('is-media-dragging');
  }
  dragPoint.value = { x: event.clientX, y: event.clientY };
  updateDragTarget(event.clientX, event.clientY);
  event.preventDefault();
}

function onPointerUp(event: PointerEvent) {
  const current = press;
  if (current?.moved) lastDragEndAt = performance.now();
  cancelPress();
  if (!current || !current.moved) return;
  const hit = dropAt(event.clientX, event.clientY);
  const track = hit?.closest('[data-track-kind]');
  const folder = hit?.closest('[data-folder-id]');
  const root = hit?.closest('[data-media-root]');
  // 歌词落到时间轴上由时间轴自己接（它要整份铺成一条新轨道，不是一个位置一份素材）
  if (current.item.kind === 'lyric' && hit?.closest('[data-timeline-drop]')) return;
  if (track) emit('assign', current.item, track.getAttribute('data-track-kind') as DraftTrackKind);
  else if (folder) emit('move-media', current.item, folder.getAttribute('data-folder-id') ?? null);
  else if (root) emit('move-media', current.item, null);
}

function onPressKeydown(event: KeyboardEvent) {
  if (event.key !== 'Escape') return;
  cancelPress();
}

/** 收摊：清掉 drag 状态、解绑临时监听（松手、Esc、指针失效都走这里）。 */
function cancelPress() {
  press = null;
  pointerDrag.value = false;
  clearDrag();
  document.body.classList.remove('is-media-dragging');
  window.removeEventListener('pointermove', onPointerMove);
  window.removeEventListener('pointerup', onPointerUp);
  window.removeEventListener('pointercancel', cancelPress);
  window.removeEventListener('keydown', onPressKeydown);
}

onBeforeUnmount(cancelPress);

/* ---- 效果卡的 pointer 拖拽 ----
 *
 * 和素材卡同一套跟指针的做法：按下记起点，移动过阈值才算拖，影子跟着指针走。
 * 差别在落点 —— 效果不落到「哪条轨道」上，而是落到一个**时刻**：时间轴那边自己
 * 换算秒数、加一条特效轨。点一下（没拖动）则加在当前播放头之外的开头。
 */
type EffectItem = { id: string; icon: string; label: string };

const EFFECTS: EffectItem[] = [{ id: 'text-noise', icon: 'mdi-format-text', label: 'editor-effect-text-noise' }];

let effectPress: { id: string; x: number; y: number; moved: boolean } | null = null;
/** 上一次拖效果的松手时间：拖完那一下的 click 不该再加一条轨道。 */
let lastEffectDragAt = 0;

function onEffectPointerDown(event: PointerEvent, id: string) {
  if (event.button !== 0) return;
  effectPress = { id, x: event.clientX, y: event.clientY, moved: false };
  window.addEventListener('pointermove', onEffectMove);
  window.addEventListener('pointerup', onEffectUp);
  window.addEventListener('pointercancel', cancelEffectPress);
  window.addEventListener('keydown', onEffectKeydown);
}

function onEffectMove(event: PointerEvent) {
  if (!effectPress) return;
  const dx = event.clientX - effectPress.x;
  const dy = event.clientY - effectPress.y;
  if (!effectPress.moved && Math.hypot(dx, dy) < DRAG_THRESHOLD) return;
  if (!effectPress.moved) {
    effectPress.moved = true;
    dragEffect.value = effectPress.id;
    document.body.classList.add('is-media-dragging');
  }
  dragPoint.value = { x: event.clientX, y: event.clientY };
  updateDragTarget(event.clientX, event.clientY);
  event.preventDefault();
}

function onEffectUp() {
  if (effectPress?.moved) lastEffectDragAt = performance.now();
  cancelEffectPress();
}

function onEffectKeydown(event: KeyboardEvent) {
  if (event.key !== 'Escape') return;
  cancelEffectPress();
}

/** 收摊：清状态、解绑临时监听（松手、Esc、指针失效都走这里）。 */
function cancelEffectPress() {
  effectPress = null;
  clearDrag();
  document.body.classList.remove('is-media-dragging');
  window.removeEventListener('pointermove', onEffectMove);
  window.removeEventListener('pointerup', onEffectUp);
  window.removeEventListener('pointercancel', cancelEffectPress);
  window.removeEventListener('keydown', onEffectKeydown);
}

function onEffectClick() {
  if (performance.now() - lastEffectDragAt < 300) return;
  emit('add-effect');
}

onBeforeUnmount(cancelEffectPress);

function startRename(folder: DraftFolder) {
  renameId.value = folder.id;
  renameText.value = folder.name;
  // 双击名字或点行上的铅笔都进这里：输入框出来就直接选中旧名字，省得再点一次
  void nextTick(() => renameInput.value?.select());
}

function commitRename(folder: DraftFolder) {
  const name = renameText.value.trim();
  renameId.value = null;
  if (!name || name === folder.name) return;
  emit('rename-folder', folder, name);
}

/** 原文件已经不在了的素材路径：素材只记路径，被挪走 / 删掉时要能一眼看出来。 */
const missing = ref<Set<string>>(new Set());

async function checkMissing() {
  const paths = (props.draft?.media ?? []).map((item) => item.path);
  if (paths.length === 0) {
    missing.value = new Set();
    return;
  }
  try {
    const gone = await invoke<string[]>('missing_editor_media', { paths });
    missing.value = new Set(gone);
  } catch {
    missing.value = new Set();
  }
}

watch(() => props.draft?.media, () => void checkMissing(), { immediate: true, deep: true });

/** 右边的编辑栏点「换素材 / 去素材库挑」时，翻到素材库这一页（父级拿 ref 调）。 */
function showMedia() {
  tab.value = 'media';
}

defineExpose({ showMedia });
</script>

<template>
  <aside class="inspector">
    <div class="inspector__tabs">
      <button
        v-for="item in tabs"
        :key="item.id"
        class="inspector__tab"
        :class="{ 'inspector__tab--active': tab === item.id }"
        type="button"
        @click="tab = item.id"
      >
        <v-icon :icon="item.icon" size="15"></v-icon>
        <span>{{ t(item.label) }}</span>
      </button>
    </div>

    <div class="inspector__body scroll-mask">
      <!-- 信息：曲名 / 难度 / 谱师 / 曲师由用户自己填，草稿就是一张自定义谱面 -->
      <template v-if="tab === 'info'">
        <div class="inspector__info">
          <div class="inspector__hero">
            <!-- 曲绘只认轨道上那份：没选就从素材库里挑，不拿谱面自带的图顶上 -->
            <button
              v-if="!coverUrl"
              class="inspector__cover inspector__cover--empty"
              type="button"
              :title="t('editor-pick-cover')"
              @click="tab = 'media'"
            >
              <v-icon icon="mdi-image-plus-outline" size="18"></v-icon>
            </button>
            <div v-else class="inspector__cover">
              <img :src="coverUrl" alt="" />
            </div>
            <div class="inspector__ident">
              <span class="inspector__name" :title="draft?.info.name">
                {{ draft?.info.name || t('editor-info-name-empty') }}
              </span>
              <span v-if="draft?.info.difficulty" class="inspector__diff">
                {{ draft.info.difficulty }}
              </span>
            </div>
          </div>

          <div class="inspector__fields">
            <label
              v-for="field in FIELDS"
              :key="field.key"
              class="inspector__field"
              :class="{ 'inspector__field--req': field.required }"
            >
              <span class="inspector__field-k">{{ t(field.label) }}</span>
              <input
                class="inspector__input"
                type="text"
                :value="draft?.info[field.key] ?? ''"
                :placeholder="field.required ? t('editor-info-required') : ''"
                spellcheck="false"
                @input="onInfoInput(field.key, $event)"
              />
            </label>
          </div>
        </div>
      </template>

      <!-- 素材库：导入的东西都在这儿，拖到下面时间轴的轨道上就是「放到谱面上」 -->
      <template v-else-if="tab === 'media'">
        <div class="inspector__pool-actions">
          <!-- 一个入口管两种导入：文件、文件夹（文件夹按名字建成素材夹） -->
          <v-menu location="bottom start" offset="4">
            <template #activator="{ props: menuProps }">
              <v-btn
                v-bind="menuProps"
                size="small"
                variant="text"
                prepend-icon="mdi-import"
                append-icon="mdi-menu-down"
              >
                {{ t('editor-import') }}
              </v-btn>
            </template>
            <v-sheet class="media-menu">
              <button class="media-menu__item" type="button" @click="emit('import')">
                <i class="mdi mdi-file-import-outline"></i>
                {{ t('editor-import-file') }}
              </button>
              <button class="media-menu__item" type="button" @click="emit('import-folder')">
                <i class="mdi mdi-folder-open-outline"></i>
                {{ t('editor-import-folder') }}
              </button>
            </v-sheet>
          </v-menu>
          <v-btn
            size="small"
            variant="text"
            prepend-icon="mdi-folder-plus-outline"
            :title="canAddChild ? t('editor-add-folder-tip') : t('editor-add-folder-limit')"
            :disabled="!canAddChild"
            @click="emit('add-folder', activeFolder)"
          >
            {{ t('editor-add-folder') }}
          </v-btn>
        </div>

        <!-- 素材多了以后按名字找：过滤只作用于当前这一层（文件夹里就只搜文件夹里的） -->
        <label v-if="draft && (allMedia.length > 0 || activeFolder !== null)" class="media-search">
          <i class="mdi mdi-magnify"></i>
          <input
            v-model="query"
            type="search"
            spellcheck="false"
            :placeholder="t('editor-media-search')"
          />
          <button v-if="query" class="media-search__clear" type="button" @click="query = ''">
            <i class="mdi mdi-close"></i>
          </button>
        </label>

        <!-- 进了文件夹就给一条回头路，顺便能看出自己在哪一层；把素材拖到这行上也等于移出文件夹。
             上一层那几段都能点，也能当拖放的落点（拖到「全部素材」上就是挪回根）。
             路径长了会折行，最多占三行：再长的从上面裁掉，当前这一层留在最后一行。 -->
        <div
          v-if="currentFolder"
          class="media-crumbs"
          :class="{ 'media-crumbs--over': dragRoot }"
          :title="folderPathText"
          data-media-root
        >
          <button class="media-crumbs__back" type="button" @click="openFolder(null)">
            <i class="mdi mdi-chevron-left"></i>
            {{ t('editor-media-root') }}
          </button>
          <template v-if="hiddenCrumbs > 0">
            <span class="media-crumbs__sep">/</span>
            <button
              class="media-crumbs__name media-crumbs__name--link"
              type="button"
              :title="t('editor-media-crumbs-more', { n: hiddenCrumbs })"
              @click="openHiddenCrumb"
            >
              …
            </button>
          </template>
          <template v-for="(node, index) in crumbNodes" :key="node.id">
            <span class="media-crumbs__sep">/</span>
            <button
              v-if="index < crumbNodes.length - 1"
              class="media-crumbs__name media-crumbs__name--link"
              type="button"
              :title="node.name"
              :data-folder-id="node.id"
              @click="openFolderByClick(node.id)"
            >
              {{ node.name }}
            </button>
            <span v-else class="media-crumbs__name" :title="node.name">{{ node.name }}</span>
          </template>
        </div>

        <div v-if="!draft" class="inspector__empty">
          <v-icon icon="mdi-folder-multiple-image" size="26"></v-icon>
          <span>{{ t('editor-media-empty') }}</span>
        </div>

        <!-- 当前这一层：子文件夹和素材排在同一条列表里，文件夹在最前 -->
        <template v-else>
          <div v-if="!hasRows" class="inspector__empty">
            <v-icon :icon="currentFolder ? 'mdi-folder-outline' : 'mdi-folder-multiple-image'" size="26"></v-icon>
            <span>{{ currentFolder ? t('editor-folder-empty') : t('editor-media-empty') }}</span>
          </div>

          <!-- 一行一条：文件夹排在最前面，素材显示完整路径（太长时从左往右截，尾段总是看得见） -->
          <div v-else class="media-list">
            <template v-if="!query.trim()">
              <article
                v-for="folder in childFolders"
                :key="folder.id"
                class="media-row media-row--folder"
                :class="{ 'media-row--over': dragFolderId === folder.id }"
                :title="folder.name"
                :data-folder-id="folder.id"
                @click="openFolderByClick(folder.id)"
              >
                <i class="mdi mdi-folder media-row__icon"></i>
                <input
                  v-if="renameId === folder.id"
                  ref="renameInput"
                  v-model="renameText"
                  class="media-row__input"
                  type="text"
                  spellcheck="false"
                  @click.stop
                  @keydown.enter="commitRename(folder)"
                  @keydown.esc="renameId = null"
                  @blur="commitRename(folder)"
                />
                <span v-else class="media-row__name" @dblclick.stop="startRename(folder)">
                  {{ folder.name }}
                </span>
                <!-- 右侧固定留一段给图标：不管悬不悬停，名字都不会顶到最边上 -->
                <span class="media-row__actions">
                  <button
                    class="media-row__btn"
                    type="button"
                    :title="t('editor-folder-rename')"
                    @click.stop="startRename(folder)"
                  >
                    <i class="mdi mdi-pencil-outline"></i>
                  </button>
                  <button
                    class="media-row__btn media-row__btn--danger"
                    type="button"
                    :title="t('editor-folder-remove')"
                    @click.stop="emit('delete-folder', folder)"
                  >
                    <i class="mdi mdi-trash-can-outline"></i>
                  </button>
                </span>
              </article>
            </template>
            <article
              v-for="item in visibleMedia"
              :key="`${item.kind}:${item.path}`"
              class="media-row"
              :class="{ 'media-row--on': onTrack(item), 'media-row--lost': missing.has(item.path) }"
              :title="missing.has(item.path) ? `${item.path}（文件已不在了）` : item.path"
              @pointerdown="onCardPointerDown($event, item)"
              @dblclick.stop="emit('assign', item)"
            >
              <i class="mdi media-row__icon" :class="ICONS[item.kind]"></i>
              <button
                class="media-row__path"
                type="button"
                :title="t('editor-media-reveal')"
                @click.stop="emit('reveal', item)"
              >
                {{ item.path }}
              </button>
              <span class="media-row__actions">
                <i v-if="missing.has(item.path)" class="mdi mdi-file-alert-outline media-row__lost"></i>
                <i v-if="onTrack(item)" class="mdi mdi-check-circle media-row__mark"></i>
                <button
                  v-else
                  class="media-row__btn"
                  type="button"
                  :title="item.kind === 'lyric' ? t('editor-lyrics-tip') : t('editor-put-on-track')"
                  @click="emit('assign', item)"
                >
                  <i class="mdi mdi-plus"></i>
                </button>
                <button
                  class="media-row__btn media-row__btn--danger"
                  type="button"
                  :title="t('editor-media-remove')"
                  @click.stop="emit('remove', item)"
                >
                  <i class="mdi mdi-trash-can-outline"></i>
                </button>
                <button
                  v-if="activeFolder !== null"
                  class="media-row__btn"
                  type="button"
                  :title="t('editor-media-move-out')"
                  @click.stop="emit('move-media', item, currentFolder?.parent ?? null)"
                >
                  <i class="mdi mdi-folder-arrow-up-outline"></i>
                </button>
              </span>
            </article>
          </div>
        </template>
      </template>

      <!-- 效果：一行一个卡，按住往时间轴上拖就落在指针那一刻，点一下加一条并选中 -->
      <template v-else>
        <div class="inspector__effects">
          <article
            v-for="item in EFFECTS"
            :key="item.id"
            class="inspector__effect"
            :title="t('editor-effect-drag-tip')"
            @pointerdown="onEffectPointerDown($event, item.id)"
            @click="onEffectClick"
          >
            <i class="mdi inspector__effect-icon" :class="item.icon"></i>
            <span class="inspector__effect-name">{{ t(item.label) }}</span>
            <i class="mdi mdi-drag-vertical inspector__effect-grip"></i>
          </article>
        </div>
      </template>
    </div>

    <!-- 拖动时的影子（挂在 body 上，免得被侧栏的 overflow 裁掉）。
         它必须留在 <aside> 里面：组件只有一个根元素，父级的 class / style（宽度）才传得进来。 -->
    <Teleport to="body">
      <div
        v-if="dragMedia"
        class="media-ghost"
        :style="{ left: `${dragPoint.x + 12}px`, top: `${dragPoint.y + 10}px` }"
      >
        <i class="mdi" :class="ICONS[dragMedia.kind]"></i>
        <span>{{ shortName(dragMedia) }}</span>
      </div>
      <div
        v-if="dragEffect"
        class="media-ghost"
        :style="{ left: `${dragPoint.x + 12}px`, top: `${dragPoint.y + 10}px` }"
      >
        <i class="mdi mdi-auto-fix"></i>
        <span>{{ t('editor-effect-text-noise') }}</span>
      </div>
    </Teleport>
  </aside>
</template>

<style scoped>
.inspector {
  display: flex;
  flex-direction: column;
  width: 320px;
  min-height: 0;
  overflow: hidden;
  background: var(--phi-bg-layer-1);
  border: 0.5px solid var(--phi-border-l2);
  border-radius: var(--phi-radius-md);
}

.inspector__tabs {
  display: flex;
  flex: none;
  gap: 2px;
  padding: var(--phi-space-2) var(--phi-space-2) 0;
  border-bottom: 0.5px solid var(--phi-border-l1);
}

.inspector__tab {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 6px 10px;
  font-size: 0.76rem;
  color: var(--phi-label-secondary);
  cursor: pointer;
  background: none;
  border: 0;
  border-bottom: 2px solid transparent;
}

.inspector__tab:hover {
  color: var(--phi-label-primary);
}

/* 选中的那页用主色下划线，跟主界面的二级导航一个语感 */
.inspector__tab--active {
  color: var(--phi-accent);
  border-bottom-color: var(--phi-accent);
}

.inspector__body {
  flex: 1 1 auto;
  min-height: 0;
  padding: var(--phi-space-3);
  overflow: hidden auto;
}

.inspector__info {
  display: flex;
  flex-direction: column;
  gap: var(--phi-space-3);
  min-width: 0;
}

/* 曲绘 + 曲名 + 难度：窄栏里曲绘缩成一小块横条，右边的名字占满剩下的宽度 */
.inspector__hero {
  display: flex;
  align-items: center;
  gap: var(--phi-space-3);
  min-width: 0;
}

.inspector__cover {
  display: flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 88px;
  aspect-ratio: 2048 / 1080;
  overflow: hidden;
  color: var(--phi-label-caption);
  background: var(--phi-bg-layer-3);
  border: 0.5px solid var(--phi-border-l3);
  border-radius: var(--phi-radius-sm);
}

.inspector__cover :deep(img) {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

/* 还没选曲绘：虚线框 + 一颗加号，点一下跳到素材库去挑 */
.inspector__cover--empty {
  padding: 0;
  font: inherit;
  color: var(--phi-label-caption);
  cursor: pointer;
  background: transparent;
  border-style: dashed;
}

.inspector__cover--empty:hover {
  color: var(--phi-label-primary);
  background: var(--phi-interactive-hover);
}

.inspector__ident {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.inspector__name {
  overflow: hidden;
  font-size: 0.92rem;
  font-weight: 600;
  color: var(--phi-label-primary);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.inspector__diff {
  align-self: flex-start;
  padding: 1px 6px;
  overflow: hidden;
  max-width: 100%;
  font-size: 0.7rem;
  color: var(--phi-label-secondary);
  text-overflow: ellipsis;
  white-space: nowrap;
  background: var(--phi-bg-layer-3);
  border-radius: var(--phi-radius-xs);
}

/* 信息页的四行输入：左边标签定宽，右边直接是输入框 */
.inspector__fields {
  display: flex;
  flex-direction: column;
  gap: var(--phi-space-2);
  min-width: 0;
}

.inspector__field {
  display: flex;
  align-items: center;
  gap: var(--phi-space-3);
  min-width: 0;
}

.inspector__field-k {
  flex: none;
  width: 56px;
  font-size: 0.72rem;
  color: var(--phi-label-caption);
}

/* 必填的那一行（曲名）标签亮一点，并在输入框空着时给个提示色的占位 */
.inspector__field--req .inspector__field-k {
  color: var(--phi-label-secondary);
}

.inspector__input {
  flex: 1 1 auto;
  min-width: 0;
  padding: 3px 6px;
  font-family: inherit;
  font-size: 0.78rem;
  color: var(--phi-label-primary);
  background: var(--phi-bg-layer-1);
  border: 0.5px solid var(--phi-border-l1);
  border-radius: var(--phi-radius-xs);
  outline: none;
}

.inspector__input:hover {
  border-color: var(--phi-border-l2);
}

.inspector__input:focus {
  border-color: var(--phi-accent);
}

.inspector__input::placeholder {
  color: var(--phi-label-caption);
}

.inspector__empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--phi-space-2);
  padding: var(--phi-space-6) var(--phi-space-3);
  font-size: 0.76rem;
  color: var(--phi-label-caption);
  text-align: center;
}

/* 效果页：一行一个，按住能拖到时间轴上（落点是时间轴上的那一刻） */
.inspector__effects {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-top: var(--phi-space-1);
}

.inspector__effect {
  display: flex;
  gap: 8px;
  align-items: center;
  padding: 7px 8px;
  font-size: 0.72rem;
  color: var(--phi-label-primary);
  cursor: grab;
  user-select: none;
  background: var(--phi-bg-layer-2);
  border: 0.5px solid var(--phi-border-l2);
  border-radius: var(--phi-radius-sm);
}

.inspector__effect:hover {
  background: var(--phi-bg-layer-3);
  border-color: var(--phi-border-l3);
}

.inspector__effect:active {
  cursor: grabbing;
}

.inspector__effect-icon {
  font-size: 15px;
  color: var(--phi-label-secondary);
}

.inspector__effect-name {
  flex: 1 1 auto;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.inspector__effect-grip {
  font-size: 14px;
  color: var(--phi-label-caption);
}

/* 素材库：上面三个导入按钮，下面是可拖的素材项 */
.inspector__pool-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 2px;
  align-items: center;
  margin-bottom: var(--phi-space-3);
}

/* 三个按钮同一种文字按钮样式，内边距也统一，一排排开时间距才一样 */
.inspector__pool-actions :deep(.v-btn) {
  padding-inline: 5px;
  font-size: 0.72rem;
  text-transform: none;
}

/* 「导入素材」下拉：文件、文件夹两个入口 */
.media-menu {
  display: flex;
  flex-direction: column;
  min-width: 132px;
  padding: 4px;
}

.media-menu__item {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 8px;
  font-size: 0.74rem;
  color: var(--phi-label-secondary);
  text-align: left;
  cursor: pointer;
  background: none;
  border: 0;
  border-radius: var(--phi-radius-xs);
}

.media-menu__item:hover {
  color: var(--phi-label-primary);
  background: var(--phi-interactive-hover);
}

.media-menu__item i {
  font-size: 15px;
  color: var(--phi-label-caption);
}

/* 素材库：一行一条，左边种类图标、中间完整路径、右边操作 */
.media-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.media-row {
  display: flex;
  gap: 6px;
  align-items: center;
  min-width: 0;
  padding: 4px 6px;
  cursor: grab;
  /* 拖素材靠 pointer 事件自己跟，别让浏览器把手势当成滚动 */
  touch-action: none;
  border: 0.5px solid transparent;
  border-radius: var(--phi-radius-xs);
}

.media-row:hover {
  background: var(--phi-bg-layer-2);
  border-color: var(--phi-border-l2);
}

.media-row:active {
  cursor: grabbing;
}

/* 已经在轨道上的那条：一圈主色边，路径也提亮一点 */
.media-row--on {
  border-color: color-mix(in srgb, var(--phi-accent) 55%, transparent);
}

.media-row--on .media-row__path {
  color: var(--phi-label-primary);
}

/* 原文件没了：边框标红，右边给个警告图标 */
.media-row--lost {
  border-color: color-mix(in srgb, #ff6b6b 45%, transparent);
}

.media-row--folder {
  cursor: pointer;
}

.media-row--folder .media-row__icon {
  color: color-mix(in srgb, var(--phi-accent) 70%, var(--phi-label-secondary));
}

/* 拖素材经过文件夹那一行时给个落点提示 */
.media-row--over {
  background: color-mix(in srgb, var(--phi-accent) 14%, transparent);
  border-color: var(--phi-accent);
}

.media-row__icon {
  flex: none;
  width: 18px;
  font-size: 15px;
  color: var(--phi-label-caption);
  text-align: center;
}

/*
 * 完整路径：`direction: rtl` + `text-align: left` 让超长时从左边截（省略号在左侧），
 * 文件名的尾段总在可见区里；字符本身的顺序不变。
 */
.media-row__path {
  flex: 1 1 auto;
  min-width: 0;
  padding: 0;
  overflow: hidden;
  font: inherit;
  font-size: 0.7rem;
  color: var(--phi-label-secondary);
  text-align: left;
  text-overflow: ellipsis;
  white-space: nowrap;
  direction: rtl;
  background: none;
  border: 0;
  cursor: pointer;
}

/* 路径本身是个「超链接」：点它在资源管理器里定位原文件，不复制、不搬动 */
.media-row__path:hover {
  color: var(--phi-accent);
  text-decoration: underline;
}

.media-row__name {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  font-size: 0.76rem;
  color: var(--phi-label-primary);
  text-align: left;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 行尾固定留一段给图标：文件夹两格、素材三格，名字再长也不会顶到行边 */
.media-row__actions {
  flex: 0 0 46px;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 2px;
}

.media-row:not(.media-row--folder) .media-row__actions {
  flex-basis: 68px;
}

.media-row__lost {
  flex: none;
  font-size: 14px;
  color: #ff8b8b;
}

.media-row__mark {
  flex: none;
  font-size: 14px;
  color: var(--phi-accent);
}

/* 行尾那几颗小按钮：平时淡出，指针划过这一行才显形 */
.media-row__btn {
  flex: none;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  padding: 0;
  font-size: 14px;
  color: var(--phi-label-caption);
  cursor: pointer;
  background: transparent;
  border: none;
  border-radius: var(--phi-radius-xs);
  opacity: 0;
  transition:
    opacity var(--phi-duration) var(--phi-ease),
    color var(--phi-duration) var(--phi-ease),
    background-color var(--phi-duration) var(--phi-ease);
}

.media-row:hover .media-row__btn {
  opacity: 1;
}

.media-row__btn:hover {
  color: var(--phi-label-primary);
  background: var(--phi-interactive-hover);
}

.media-row__btn--danger:hover {
  color: #ff6b6b;
}

/* 双击文件夹名就地改名 */
.media-row__input {
  flex: 1 1 auto;
  min-width: 0;
  padding: 2px 4px;
  font-size: 0.76rem;
  color: var(--phi-label-primary);
  background: var(--phi-bg-layer-1);
  border: 0.5px solid var(--phi-accent);
  border-radius: var(--phi-radius-xs);
  outline: none;
}

/* 进了文件夹时的那条回头路：左边一个「全部素材」，右边是当前文件夹名 */
/* 搜索框：素材多的时候按名字筛，样式跟信息页的输入框一套 */
.media-search {
  display: flex;
  gap: 4px;
  align-items: center;
  padding: 0 6px;
  margin-bottom: var(--phi-space-2);
  font-size: 0.72rem;
  color: var(--phi-label-caption);
  background: var(--phi-bg-layer-1);
  border: 0.5px solid var(--phi-border-l1);
  border-radius: var(--phi-radius-xs);
}

.media-search:focus-within {
  border-color: var(--phi-accent);
}

.media-search input {
  flex: 1 1 auto;
  min-width: 0;
  padding: 4px 0;
  font-size: 0.72rem;
  color: var(--phi-label-primary);
  background: transparent;
  border: none;
  outline: none;
}

.media-search input::placeholder {
  color: var(--phi-label-caption);
}

/* type="search" 自带的清除按钮跟自绘的重叠，藏掉 */
.media-search input::-webkit-search-cancel-button {
  appearance: none;
}

.media-search__clear {
  padding: 0;
  cursor: pointer;
  background: transparent;
  border: none;
  color: var(--phi-label-caption);
}

.media-search__clear:hover {
  color: var(--phi-label-primary);
}

/* 拖动时跟着指针走的那个小影子（Teleport 到 body，靠 scoped 属性照样吃这里的样式） */
.media-ghost {
  position: fixed;
  z-index: 999;
  display: flex;
  gap: 6px;
  align-items: center;
  max-width: 220px;
  padding: 5px 9px;
  font-size: 0.72rem;
  color: var(--phi-label-primary);
  pointer-events: none;
  background: var(--phi-bg-layer-2);
  border: 0.5px solid var(--phi-accent);
  border-radius: var(--phi-radius-sm);
  box-shadow: 0 8px 20px rgb(0 0 0 / 40%);
}

.media-ghost span {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

:global(body.is-media-dragging),
:global(body.is-media-dragging *) {
  cursor: grabbing !important;
  user-select: none;
}

.media-crumbs {
  display: flex;
  flex-wrap: wrap;
  gap: 2px 4px;
  align-items: center;
  /* 最多三行：路径再深也从上面裁掉，当前这一层留在最后一行（`align-content` 把行压到底部） */
  align-content: flex-end;
  max-height: calc(3 * 1.6em);
  min-width: 0;
  padding: 2px 4px;
  margin-bottom: var(--phi-space-2);
  overflow: hidden;
  font-size: 0.72rem;
  line-height: 1.6;
  color: var(--phi-label-caption);
  border: 0.5px solid transparent;
  border-radius: var(--phi-radius-xs);
}

.media-crumbs__back {
  display: inline-flex;
  gap: 2px;
  align-items: center;
  padding: 2px 4px;
  font-size: 0.72rem;
  color: var(--phi-label-secondary);
  cursor: pointer;
  background: none;
  border: 0;
  border-radius: var(--phi-radius-xs);
}

.media-crumbs__back:hover {
  color: var(--phi-label-primary);
  background: var(--phi-interactive-hover);
}

.media-crumbs__name {
  overflow: hidden;
  color: var(--phi-label-primary);
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 上层那几段：能在面包屑里点回去，也能当拖放的落点 */
.media-crumbs__name--link {
  max-width: 10em;
  padding: 2px 4px;
  color: var(--phi-label-secondary);
  cursor: pointer;
  background: none;
  border: 0;
  border-radius: var(--phi-radius-xs);
}

.media-crumbs__name--link:hover {
  color: var(--phi-label-primary);
  background: var(--phi-interactive-hover);
}

/* 把素材拖到这一行 = 移出文件夹 */
.media-crumbs--over {
  background: color-mix(in srgb, var(--phi-accent) 14%, transparent);
  border-color: var(--phi-accent);
}
</style>
