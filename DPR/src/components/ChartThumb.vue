<script setup lang="ts">
// 列表折叠行左边那一小块曲绘（64px 宽、2048 / 1080）。
//
// 曲绘**不能**直接喂 `convertFileSrc`：`ChartInfo.illustration` 是谱面包内的相对路径，
// 谱面可能是文件夹也可能是 `.pez`，所以跟 `ChartHero` 一样走后端 `read_chart_asset`
// 读原始字节、前端包成 Blob URL。没曲绘（读不到 / 空字节）时退回占位图标。
import { onUnmounted, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';

const props = defineProps<{ path?: string; illustration?: string }>();

const MIME_BY_EXT: Record<string, string> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  webp: 'image/webp',
  gif: 'image/gif',
  bmp: 'image/bmp',
  avif: 'image/avif',
};

function mimeOf(name: string) {
  const ext = name.slice(name.lastIndexOf('.') + 1).toLowerCase();
  return MIME_BY_EXT[ext] ?? 'application/octet-stream';
}

/** 曲绘图 URL（Blob URL）；没有曲绘时是空串 */
const cover = ref('');
let coverUrl = '';

function revokeCover() {
  if (coverUrl) URL.revokeObjectURL(coverUrl);
  coverUrl = '';
}

/** 已经加载过的曲绘标识：`info` 每次都是新对象，靠值相等去重不成立，只能自己记 key */
let loadedKey = '';

async function loadCover() {
  const key = `${props.path ?? ''}|${props.illustration ?? ''}`;
  if (key === loadedKey) return;
  loadedKey = key;

  revokeCover();
  cover.value = '';

  const name = props.illustration;
  if (!name || !props.path) return;
  try {
    const bytes = (await invoke('read_chart_asset', { path: props.path, name })) as ArrayBuffer;
    if (!(bytes instanceof ArrayBuffer) || bytes.byteLength === 0) return;
    coverUrl = URL.createObjectURL(new Blob([bytes], { type: mimeOf(name) }));
    cover.value = coverUrl;
  } catch {
    // 读不到就当没有曲绘，占位图标顶上
  }
}

watch(() => [props.path, props.illustration] as const, loadCover, { immediate: true });
onUnmounted(revokeCover);

/**
 * 图片加载失败：退回占位图标，不要留一张裂图。
 * 不要动 `loadedKey` —— 它记的是「这个 key 已经试过」，清掉就会每轮重试。
 */
function onCoverError() {
  revokeCover();
  cover.value = '';
}
</script>

<template>
  <img v-if="cover" :src="cover" alt="" @error="onCoverError" />
  <v-icon v-else icon="mdi-image-outline" size="18"></v-icon>
</template>
