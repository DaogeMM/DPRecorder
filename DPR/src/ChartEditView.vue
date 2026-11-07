<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue';

import { useI18n } from 'vue-i18n';
const { t } = useI18n();

import { invoke } from '@tauri-apps/api/core';

import type { ChartInfo, EditorDraft } from '@/model';
import { finishPageLoad } from '@/pageLoad';
import { toastError } from '@/common';

import ChartThumb from '@/components/ChartThumb.vue';

/**
 * DPE 的「编辑窗口」页：一份份草稿的列表。
 *
 * 这里**不再按谱面文件排**。一张草稿 = 一条 `draft.json`（`<程序目录>/dpe/editor/drafts/<id>/`），
 * 里面记着谱面 / 音乐 / 曲绘三个轨道各自引用的**原文件路径**，素材本身不复制、不随草稿移动。
 * 点一张卡片就打开（或切到）那个独立窗口去编辑，和剪映的草稿列表是一个意思。
 *
 * 后端那几个命令见 `src-tauri/src/drafts.rs` 与 `lib.rs` 里的 `list_editor_drafts` 等。
 * 缩略图：草稿自己挂了曲绘就用它，没有就看谱面包里的插画（`ChartThumb` 走后端读字节），
 * 都没有就留个占位图标 —— 只有路径、没有文件时这里不报错，卡片照样能点开。
 */
interface DraftCard {
  draft: EditorDraft;
  /** 谱面包解析出来的信息（曲名 / 插画名）；谱面没挂或文件没了就是 null */
  info: ChartInfo | null;
  /** 草稿自己那份曲绘的 Blob URL；没有就是空串 */
  cover: string;
}

const cards = ref<DraftCard[]>([]);
const loading = ref(false);

/** 建过的 Blob URL 都记在这儿，刷新与卸载时统一 revoke */
let blobUrls: string[] = [];

const MIME_BY_EXT: Record<string, string> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  webp: 'image/webp',
  gif: 'image/gif',
  bmp: 'image/bmp',
  avif: 'image/avif',
};

function mimeOf(path: string) {
  const ext = path.slice(path.lastIndexOf('.') + 1).toLowerCase();
  return MIME_BY_EXT[ext] ?? 'application/octet-stream';
}

/** 时间戳 → `MM-DD HH:mm`：列表里只要够认出「哪份是刚改过的」 */
function formatTime(ms: number) {
  if (!ms) return '';
  const date = new Date(ms);
  const pad = (value: number) => String(value).padStart(2, '0');
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** 读一份素材成 Blob URL（曲绘在别的盘上时也可能路径失效，那时就当没有） */
async function readCover(path: string | undefined) {
  if (!path) return '';
  try {
    const bytes = (await invoke('read_editor_media', { path })) as ArrayBuffer;
    if (!(bytes instanceof ArrayBuffer) || bytes.byteLength === 0) return '';
    const url = URL.createObjectURL(new Blob([bytes], { type: mimeOf(path) }));
    blobUrls.push(url);
    return url;
  } catch {
    return '';
  }
}

function releaseCovers() {
  for (const url of blobUrls) URL.revokeObjectURL(url);
  blobUrls = [];
}

/** 重扫一遍草稿目录：卡片按「最近改过的排前面」排（顺序由后端给） */
async function refresh() {
  loading.value = true;
  try {
    const drafts = (await invoke('list_editor_drafts')) as EditorDraft[];
    releaseCovers();
    const next: DraftCard[] = [];
    for (const draft of drafts) {
      let info: ChartInfo | null = null;
      if (draft.chart) {
        try {
          info = (await invoke('parse_chart', { path: draft.chart.path })) as ChartInfo;
        } catch {
          // 谱面文件被挪走 / 删掉了：卡片留着，缩略图退回占位图标
          info = null;
        }
      }
      next.push({ draft, info, cover: await readCover(draft.cover?.path) });
    }
    cards.value = next;
  } catch (e) {
    toastError(e);
  } finally {
    loading.value = false;
  }
}

/** 打开一份草稿：窗口已经开着就换草稿，没开才新建窗口（后端管这个判断） */
async function openDraft(draft: EditorDraft) {
  try {
    // `path` 那一项显式给 null：这条命令两种进法（给草稿 id / 给谱面路径），这里走前者
    await invoke('open_editor_window', { path: null, draft: draft.id });
  } catch (e) {
    toastError(e);
  }
}

async function newDraft() {
  try {
    const draft = (await invoke('create_editor_draft', { name: null })) as EditorDraft;
    await refresh();
    await openDraft(draft);
  } catch (e) {
    toastError(e);
  }
}

async function openFolder() {
  try {
    await invoke('open_editor_drafts_folder');
  } catch (e) {
    toastError(e);
  }
}

/** 重命名：草稿整份读回来改个名字再存回去（`name` 是除 id 外唯一能改的字段） */
const renaming = ref<EditorDraft | null>(null);
const renameText = ref('');

function beginRename(draft: EditorDraft) {
  renaming.value = draft;
  renameText.value = draft.name;
}

async function confirmRename() {
  const draft = renaming.value;
  if (!draft) return;
  const name = renameText.value.trim();
  renaming.value = null;
  if (!name || name === draft.name) return;
  try {
    await invoke('save_editor_draft', { draft: { ...draft, name } });
    await refresh();
  } catch (e) {
    toastError(e);
  }
}

const deleting = ref<EditorDraft | null>(null);

async function confirmDelete() {
  const draft = deleting.value;
  if (!draft) return;
  deleting.value = null;
  try {
    await invoke('delete_editor_draft', { id: draft.id });
    await refresh();
  } catch (e) {
    toastError(e);
  }
}

onMounted(async () => {
  await refresh();
  finishPageLoad();
});
onUnmounted(releaseCovers);
</script>

<template>
  <div class="draft-view">
    <div class="draft-view__scroll">
      <!-- 草稿卡片：曲绘 / 名字 / 改过的时间。整张可点，右上角两个按钮不触发打开。 -->
      <div class="draft-grid">
        <article
          v-for="card in cards"
          :key="card.draft.id"
          class="draft-card"
          :title="t('editor-draft-open-tip')"
          @click="openDraft(card.draft)"
        >
          <div class="draft-card__cover">
            <img v-if="card.cover" :src="card.cover" alt="" />
            <ChartThumb
              v-else-if="card.draft.chart"
              :path="card.draft.chart.path"
              :illustration="card.info?.illustration ?? ''"
            />
            <v-icon v-else icon="mdi-filmstrip-box-multiple" size="26"></v-icon>
            <div class="draft-card__tools" @click.stop>
              <v-btn
                icon="mdi-pencil-outline"
                size="x-small"
                variant="text"
                :title="t('editor-draft-rename')"
                @click="beginRename(card.draft)"
              ></v-btn>
              <v-btn
                icon="mdi-trash-can-outline"
                size="x-small"
                variant="text"
                :title="t('editor-draft-delete')"
                @click="deleting = card.draft"
              ></v-btn>
            </div>
          </div>
          <div class="draft-card__body">
            <div class="draft-card__name" :title="card.draft.name">{{ card.draft.name }}</div>
            <div class="draft-card__time">{{ formatTime(card.draft.updated) }}</div>
          </div>
        </article>

        <!-- 新建草稿：跟卡片一样大，放在最后一张后面 -->
        <button class="draft-card draft-card--new" type="button" @click="newDraft">
          <v-icon icon="mdi-plus" size="24"></v-icon>
          <span>{{ t('editor-draft-new') }}</span>
        </button>
      </div>

      <div v-if="!cards.length && !loading" class="draft-empty">{{ t('editor-draft-empty') }}</div>
    </div>

    <footer class="draft-view__actions">
      <span class="draft-count">{{ t('editor-draft-count', { n: cards.length }) }}</span>
      <v-spacer></v-spacer>
      <v-btn
        variant="text"
        size="small"
        prepend-icon="mdi-folder-open-outline"
        :title="t('editor-open-drafts-folder')"
        @click="openFolder"
      >
        {{ t('editor-open-drafts-folder') }}
      </v-btn>
      <v-btn
        variant="text"
        size="small"
        prepend-icon="mdi-refresh"
        :title="t('editor-drafts-refresh')"
        @click="refresh"
      >
        {{ t('editor-drafts-refresh') }}
      </v-btn>
    </footer>

    <!-- 重命名 -->
    <v-dialog
      :model-value="!!renaming"
      width="auto"
      min-width="400px"
      class="log-card-bg"
      @update:model-value="renaming = null"
    >
      <v-card class="log-card-only-window">
        <v-card-title>{{ t('editor-draft-rename') }}</v-card-title>
        <v-card-text>
          <v-text-field
            v-model="renameText"
            variant="outlined"
            hide-details
            autofocus
            @keydown.enter="confirmRename"
          ></v-text-field>
        </v-card-text>
        <v-card-actions class="justify-end">
          <v-btn class="hover-scale" variant="text" @click="renaming = null">{{ t('cancel') }}</v-btn>
          <v-btn class="hover-scale" variant="text" color="btn" @click="confirmRename">
            {{ t('confirm') }}
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>

    <!-- 删草稿 -->
    <v-dialog
      :model-value="!!deleting"
      width="auto"
      min-width="400px"
      class="log-card-bg"
      @update:model-value="deleting = null"
    >
      <v-card class="log-card-only-window">
        <v-card-title>{{ t('editor-draft-delete') }}</v-card-title>
        <v-card-text>
          <div class="draft-dialog__text">{{ t('editor-draft-delete-tip') }}</div>
        </v-card-text>
        <v-card-actions class="justify-end">
          <v-btn class="hover-scale" variant="text" @click="deleting = null">{{ t('cancel') }}</v-btn>
          <v-btn class="hover-scale" variant="text" @click="confirmDelete">
            {{ t('editor-draft-delete') }}
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
  </div>
</template>

<style scoped>
/* 整页滚动 + 贴底操作条：与「谱面修改」「噪域播放器」几页同一个壳 */
.draft-view {
  display: flex;
  flex: 1;
  flex-direction: column;
  min-height: 0;
  overflow: hidden;
}

.draft-view__scroll {
  flex: 1;
  min-height: 0;
  overflow-x: hidden;
  overflow-y: auto;
  padding: var(--phi-space-6);
}

.draft-view__actions {
  display: flex;
  flex: none;
  align-items: center;
  gap: var(--phi-space-2);
  padding: var(--phi-space-2) var(--phi-space-6);
  border-top: 0.5px solid var(--phi-border-l3);
}

.draft-count {
  font-size: 0.9rem;
  color: var(--phi-label-secondary);
  white-space: nowrap;
}

/* 卡片按可用宽度自动排：宽约 240px 起一张，窄窗口也能一行放下两张。
   行内卡片抻成同一高度，「新建草稿」那张才和最矮的草稿卡片齐平。 */
.draft-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  gap: var(--phi-space-4);
  align-items: stretch;
}

.draft-card {
  display: flex;
  flex-direction: column;
  min-width: 0;
  padding: 0;
  overflow: hidden;
  text-align: left;
  background: var(--phi-bg-layer-1);
  border: 0.5px solid var(--phi-border-l3);
  border-radius: var(--phi-radius-sm);
  cursor: pointer;
  transition: border-color var(--phi-duration) var(--phi-ease), background var(--phi-duration) var(--phi-ease);
}

.draft-card:hover {
  background: var(--phi-bg-layer-2);
  border-color: var(--phi-border-l4);
}

/* 缩略图区：16:9 定尺，曲绘铺满裁切；没有图时图标居中 */
.draft-card__cover {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: center;
  aspect-ratio: 16 / 9;
  overflow: hidden;
  color: var(--phi-label-tertiary);
  background: var(--phi-bg-base);
  border-bottom: 0.5px solid var(--phi-border-l3);
}

.draft-card__cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

/* `ChartThumb` 没图时给的是 18px 图标，这里放大一点，别在大缩略图里显成一个小点 */
.draft-card__cover :deep(.v-icon) {
  font-size: 26px;
}

/* 右上角两个按钮：平常半隐，悬停卡片才清楚起来 */
.draft-card__tools {
  position: absolute;
  top: 6px;
  right: 6px;
  display: flex;
  gap: 2px;
  padding: 2px;
  border-radius: var(--phi-radius-xs);
  background: color-mix(in srgb, var(--phi-bg-base) 72%, transparent);
  opacity: 0;
  transition: opacity var(--phi-duration) var(--phi-ease);
}

.draft-card:hover .draft-card__tools {
  opacity: 1;
}

.draft-card__body {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: var(--phi-space-2) var(--phi-space-3);
}

.draft-card__name {
  font-size: 0.9rem;
  font-weight: 600;
  color: var(--phi-label-primary);
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.draft-card__time {
  font-size: 0.72rem;
  color: var(--phi-label-caption);
  font-variant-numeric: tabular-nums;
}

/* 「新建」那张：虚线框、内容居中；高度由网格那一行拉齐（见 `.draft-grid`），
   单独一张时靠 `min-height` 撑住 */
.draft-card--new {
  align-items: center;
  justify-content: center;
  gap: var(--phi-space-2);
  min-height: 172px;
  color: var(--phi-label-secondary);
  font-size: 0.86rem;
  background: transparent;
  border-style: dashed;
}

.draft-card--new:hover {
  color: var(--phi-label-primary);
  border-color: var(--phi-accent);
}

.draft-empty {
  margin-top: var(--phi-space-4);
  font-size: 0.86rem;
  color: var(--phi-label-caption);
}

/* 删除对话框里的那句说明：弹窗卡片本身用全局那套 `.log-card-only-window` */
.draft-dialog__text {
  font-size: 0.78rem;
  color: var(--phi-label-secondary);
}
</style>
