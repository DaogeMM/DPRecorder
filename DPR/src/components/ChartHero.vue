<script setup lang="ts">
// 渲染页顶部的谱面横幅（曲绘 + 曲名 + 难度徽章 + 曲师/画师/谱师）。
//
// 左侧版式：
//   第一行  曲名（`info.name`，空时显示控件标签）
//   第二行  难度徽章（`IN Lv.13.6` —— 定数就拼在徽章里，灰底白字）
//   第三行  曲师 <composer>
//   第四行  画师 <illustrator>
//   第五行  谱师 <charter>
// 右侧是曲绘（固定 `width: 300px` + `aspect-ratio: 2048 / 1080`），曲绘下方一行小字是谱面文件的
// 磁盘路径：右端与曲绘右端平齐，但宽度不跟着曲绘走（单行，太长才省略）。
//
// 它作为 `PhiSection`（variant="hero"）的 **head 插槽内容**，所以：
//   - 外层已经负责了圆角裁剪、0.5px 底线、右上角那个折叠箭头与「点击展开」的手型；
//   - 这里只管把横幅画出来。
//
// 曲绘**不能**直接喂 `convertFileSrc`：`ChartInfo.illustration` 是谱面包内的相对路径
// （RPE 的 `Picture: illustration.png`，默认 `background.png`），不是磁盘绝对路径。
// 谱面可能是文件夹也可能是 `.pez` 压缩包，所以统一走后端 `read_chart_asset` 读出原始字节，
// 前端再包成 Blob URL。没曲绘时后端返回空串/空字节，要判空（否则会得到一个坏 URL 加一个裂图图标）。
//
// 背景那层用的是 CSS `filter: blur()`，**不是 `backdrop-filter`** —— 全项目不用毛玻璃。
import { computed, onUnmounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { formatLvl, formatLvlInText } from '../lvl';
import type { ChartInfo } from '../model';

const props = defineProps<{ info?: ChartInfo; path?: string; coverPath?: string }>();

const { t } = useI18n();

/** 曲绘图 URL（Blob URL 或 `convertFileSrc` 直出的 asset URL）；没有曲绘时是空串 */
const cover = ref('');
let coverUrl = '';
/** 上面那个 URL 是不是 Blob —— 只有 Blob 才需要 revoke */
let coverIsBlob = false;
/** 已经加载过的曲绘标识：任务页每 700ms 拉一次列表、`info` 每轮都是新对象，
 *  靠值相等去重不成立，只能自己记住「这个 key 已经加载过了」。 */
let loadedKey = '';

function revokeCover() {
  if (coverUrl && coverIsBlob) {
    URL.revokeObjectURL(coverUrl);
  }
  coverUrl = '';
  coverIsBlob = false;
}

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

async function loadCover() {
  // 任务页（TasksView）已经把曲绘落成临时文件了（后端 `Task.cover`），直接把那个路径喂给 img；
  // 渲染页没有这个路径，才回落到「按谱面包内相对路径去后端读字节」这条路。
  const key = props.coverPath
    ? `file:${props.coverPath}`
    : `asset:${props.path ?? ''}|${props.info?.illustration ?? ''}`;
  if (key === loadedKey) return;
  loadedKey = key;

  revokeCover();
  cover.value = '';

  if (props.coverPath) {
    coverUrl = convertFileSrc(props.coverPath);
    coverIsBlob = false;
    cover.value = coverUrl;
    return;
  }

  const name = props.info?.illustration;
  if (!name || !props.path) return;
  try {
    const bytes = (await invoke('read_chart_asset', { path: props.path, name })) as ArrayBuffer;
    if (!(bytes instanceof ArrayBuffer) || bytes.byteLength === 0) return;
    coverUrl = URL.createObjectURL(new Blob([bytes], { type: mimeOf(name) }));
    coverIsBlob = true;
    cover.value = coverUrl;
  } catch {
    // 读不到就当没有曲绘（横幅退回占位图标），不打扰用户
  }
}

watch(() => [props.path, props.info?.illustration, props.coverPath] as const, loadCover, { immediate: true });
onUnmounted(revokeCover);

/**
 * 图片加载失败（最典型的是任务页那份临时曲绘文件已经被删掉）：
 * 退回占位图标，不要留一张裂图。
 * 注意不要动 `loadedKey` —— 它记的是「这个 key 已经试过」，清掉就会每轮重试。
 */
function onCoverError() {
  revokeCover();
  cover.value = '';
}

/** 谱面名，空串时退化成控件标签 */
const name = computed(() => String(props.info?.name ?? '').trim());

/**
 * 难度**档位**（EZ / HD / IN / AT / SP），只给徽章配色用。
 *
 * `ChartInfo` 有两个字段：`diff` 是纯档位（`IN`），`difficulty` 往往是带定数的整串
 * （`IN Lv.13.6`）。这里优先取 `diff`；只有 `difficulty` 时把结尾的 `Lv.x.y` 削掉，
 * 免得下面拼徽章文案时出现「IN Lv.13.6 Lv.13.6」。
 */
const diffTier = computed(() => {
  const tier = String(props.info?.diff ?? '').trim();
  if (tier) return tier;
  return String(props.info?.difficulty ?? '')
    .trim()
    .replace(/\s*lv\.?\s*[\d.]+$/i, '');
});

/** 徽章文案：档位 + 定数（`IN Lv.13.6`），定数缺失时只留档位 */
const diffBadge = computed(() => {
  const tier = diffTier.value;
  const lvl = lvlText.value;
  if (tier && lvl) return `${tier} Lv.${lvl}`;
  // 只有谱面里写的那串 `difficulty` 时，也把里面的 `Lv.x` 补成一位小数
  return tier || formatLvlInText(String(props.info?.difficulty ?? '').trim());
});

/** 定数：0 或缺失时不显示；有值就强制一位小数 */
const lvlText = computed(() => formatLvl(props.info?.lvl));

// 难度徽章不按档位配色（配色直接写在 `<style>` 的 `.chart-hero__diff` 里），这里不需要额外计算。
</script>

<template>
  <div class="chart-hero" :class="{ 'chart-hero--empty': !info }">
    <!-- 背景：整张曲绘放大 + CSS 模糊，再压一层渐变把文字区压暗 -->
    <img v-if="cover" class="chart-hero__bg" :src="cover" alt="" aria-hidden="true" @error="onCoverError" />
    <div v-if="cover" class="chart-hero__scrim" aria-hidden="true"></div>

    <div class="chart-hero__inner">
      <div class="chart-hero__meta">
        <template v-if="info">
          <!-- 第一行：曲名 -->
          <div class="chart-hero__head">
            <span class="chart-hero__name">{{ name || t('info.name') }}</span>
          </div>

          <!-- 第二行：难度徽章（「IN Lv.13.6」，定数就在徽章里） -->
          <div v-if="diffBadge" class="chart-hero__diffrow">
            <span class="chart-hero__diff">
              <span class="chart-hero__diff-tier">{{ diffTier || diffBadge }}</span>
              <span v-if="diffTier && lvlText" class="chart-hero__diff-lvl">Lv.{{ lvlText }}</span>
            </span>
          </div>

          <!-- 第三行：曲师（曲师、谱师、画师各自独占一行） -->
          <div v-if="info.composer" class="chart-hero__line">
            <span class="chart-hero__pair"><span class="chart-hero__k">{{ t('info.composer') }}</span>{{ info.composer }}</span>
          </div>

          <!-- 第四、五行：画师 / 谱师（各自独占一行） -->
          <div v-if="info.illustrator" class="chart-hero__line">
            <span class="chart-hero__pair"><span class="chart-hero__k">{{ t('info.illustrator') }}</span>{{ info.illustrator }}</span>
          </div>
          <div v-if="info.charter" class="chart-hero__line">
            <span class="chart-hero__pair"><span class="chart-hero__k">{{ t('info.charter') }}</span>{{ info.charter }}</span>
          </div>
        </template>

        <div v-else class="chart-hero__empty">{{ t('choose.can-also-drop') }}</div>
      </div>

      <!-- 曲绘缩略图放在横幅右侧，按 2048:1080 的比例裁切；下面紧跟谱面文件的磁盘路径 -->
      <div class="chart-hero__side">
        <div class="chart-hero__cover">
          <img v-if="cover" :src="cover" alt="" @error="onCoverError" />
          <v-icon v-else size="30">mdi-image-outline</v-icon>
        </div>
        <div v-if="path" class="chart-hero__path" :title="path">{{ path }}</div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.chart-hero {
  position: relative;
  overflow: hidden;
  min-height: 168px;
  background: var(--phi-bg-layer-2);
}

.chart-hero__bg {
  position: absolute;
  inset: -20%;
  object-fit: cover;
  filter: blur(36px) saturate(1.4);
  opacity: 0.55;
  pointer-events: none;
}

.chart-hero__scrim {
  position: absolute;
  inset: 0;
  background:
    linear-gradient(90deg, var(--phi-bg-base) 0%, color-mix(in srgb, var(--phi-bg-base) 55%, transparent) 42%, transparent 100%),
    linear-gradient(0deg, var(--phi-bg-base) 0%, transparent 65%);
  pointer-events: none;
}

.chart-hero__inner {
  position: relative;
  z-index: 1;
  display: flex;
  align-items: center;
  gap: var(--phi-space-5);
  min-height: 168px;
  /* 右侧多留 16px：`PhiSection` 的折叠箭头浮在右上角（right: 12px、宽约 20px），
     否则它会压在曲绘的右上角上 */
  padding: var(--phi-space-5);
  padding-right: calc(var(--phi-space-5) + var(--phi-space-4));
}

/* 右侧这一列：曲绘 + 它下方的谱面路径。曲绘固定 300px 宽；路径的右端与曲绘右端平齐，
   但**不**限制成曲绘那么宽 —— 它是单行小字，长度按内容走，太长时左边被截掉加省略号，
   所以整列宽度取内容宽度（用 max-content），上限见下面的 max-width。 */
.chart-hero__side {
  display: flex;
  flex: none;
  flex-direction: column;
  align-items: flex-end;
  gap: 6px;
  width: max-content;
  max-width: min(100%, 560px);
}

/* 谱面文件的磁盘路径：小字、右对齐、单行（超出加省略号），完整值在 title 里 */
.chart-hero__path {
  overflow: hidden;
  max-width: 100%;
  white-space: nowrap;
  text-align: right;
  text-overflow: ellipsis;
  font-size: 0.7rem;
  line-height: 1.35;
  color: var(--phi-label-caption);
  /* 路径可选中，方便直接复制 */
  user-select: text;
}

.chart-hero__cover {
  display: flex;
  align-items: center;
  justify-content: center;
  /* 曲绘按 2048:1080 的横幅比例（宽 300 ⇒ 高约 158） */
  width: 300px;
  aspect-ratio: 2048 / 1080;
  overflow: hidden;
  border: 0.5px solid var(--phi-border-l3);
  border-radius: 0; /* 谱面信息的曲绘不带圆角 */
  background: var(--phi-bg-layer-3);
  box-shadow: var(--phi-elevation-panel);
  color: var(--phi-label-caption);
}
.chart-hero__cover img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.chart-hero__meta {
  flex: 1 1 auto;
  min-width: 0;
}

/* 第一行：曲名 + 难度徽章 + Lv.定数 */
.chart-hero__head {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--phi-space-3);
}

.chart-hero__name {
  overflow: hidden;
  flex: 0 1 auto;
  min-width: 0;
  font-size: 1.5rem;
  font-weight: 600;
  line-height: 1.25;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--phi-label-primary);
}

/* 第二行：难度徽章独占的那一行（徽章本身是 inline-flex，这里只管上间距） */
.chart-hero__diffrow {
  margin-top: var(--phi-space-2);
}

/* 第三~五行：曲师 / 画师 / 谱师（一行一个） */
.chart-hero__line {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: var(--phi-space-1) var(--phi-space-5);
  margin-top: var(--phi-space-2);
  font-size: 0.85rem;
  color: var(--phi-label-secondary);
}

.chart-hero__pair {
  display: inline-flex;
  align-items: baseline;
  gap: 6px;
}

.chart-hero__diff {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 20px;
  padding: 0 7px;
  /* 难度标识不带圆角 */
  border-radius: 0;
  /* 灰底白字 */
  background: var(--phi-static-neutral-600);
  color: var(--phi-static-bluish-00);
  font-size: 0.72rem;
  /* 难度标识不加粗 */
  font-weight: 400;
  letter-spacing: 0.06em;
}

/* 只有档位大写（EZ / HD / IN / AT / SP 的惯例），后面的「Lv.」保持原样 */
.chart-hero__diff-tier {
  text-transform: uppercase;
}

.chart-hero__diff-lvl {
  font-variant-numeric: tabular-nums;
  letter-spacing: 0;
}

.chart-hero__k {
  color: var(--phi-label-caption);
  font-size: 0.78rem;
}

.chart-hero__empty {
  font-size: 0.95rem;
  color: var(--phi-label-caption);
}

/* 没有谱面时：不画曲绘，只留一句拖放提示，高度也收一点 */
.chart-hero--empty {
  min-height: 96px;
}
.chart-hero--empty .chart-hero__inner {
  min-height: 96px;
  justify-content: center;
  /* 空态没有折叠箭头，左右对称才居中 */
  padding-right: var(--phi-space-5);
}

@media (max-width: 900px) {
  .chart-hero__cover {
    width: 208px;
  }
  .chart-hero__name {
    font-size: 1.25rem;
  }
}
</style>
