<script setup lang="ts">
import { computed, ref, watch } from 'vue';

import { useI18n } from 'vue-i18n';
const { t } = useI18n();

import type {
  DraftClip,
  DraftLane,
  DraftMedia,
  DraftTrackKind,
  EditorDraft,
  NoiseMaterial,
  TextNoiseEffect,
} from '@/model';
import { DEFAULT_NOISE_MATERIAL } from '@/model';
import { clock, toSeconds } from './timeline';
import { clipsOf, defaultTextNoise, NATIVE_FONT, normalizeNoiseColor } from './text-noise';
import SliderRow from './SliderRow.vue';
import TextNoisePreview from './TextNoisePreview.vue';

/**
 * 编辑窗口右侧的编辑栏。
 *
 * 时间轴上点中一条轨道（或者轨道上的素材）时，这里显示那份素材本身：
 * 文件名、完整路径、能读出来的属性（音乐时长、曲绘尺寸），以及「打开所在文件夹 / 换素材 /
 * 清空轨道」这三个动作。自己加的轨道还能在这里改片段的起点与长度（可以比音乐长）。
 *
 * 选中状态由父级持有（时间轴点选、拖素材上轨道都会改它），这一栏只负责显示和发事件，
 * 调素材库、改草稿仍然走父级，免得两个组件各存一份。
 */

const props = withDefaults(
  defineProps<{
    draft: EditorDraft | null;
    /** 时间轴上选中的那条轨道 id：主轨道是 `music` / `chart` / `cover`，自己加的是 `lane.id`。 */
    trackId: string | null;
    /** 特效轨上选中的那一段：一行可以放好几段，右栏只改这一段。 */
    clipId?: string | null;
    /** 批量选中的特效片段（Ctrl 点时间轴）：多于一段时右栏改一项就写到每一段上。 */
    multi?: { laneId: string; clipId: string }[];
    /** 音乐轨上那首的时长（秒），预览报上来的；0 表示还没读到。 */
    duration?: number;
    /** 曲绘的预览地址：既是缩略图，也能读出原图尺寸。 */
    coverUrl?: string;
  }>(),
  { clipId: null, multi: () => [], duration: 0, coverUrl: '' },
);

const emit = defineEmits<{
  reveal: [media: DraftMedia];
  clear: [track: string];
  /** 「换素材 / 去素材库挑」：让左边的功能栏翻到素材库那一页。 */
  'pick-media': [];
  /** 改自己加的轨道上的片段（起点 / 长度 / 素材）。 */
  'update-lane': [id: string, patch: Partial<DraftLane>];
  /** 改特效轨上选中的那一段（起止 / 文字参数）。 */
  'update-clip': [patch: Partial<DraftClip>];
  /** 批量改：把同一份参数写到选中的每一段上。 */
  'update-clips': [patch: Partial<TextNoiseEffect>];
  /** 批量改起止：同一项写到选中的每一段上。 */
  'update-clips-time': [patch: { start?: number; length?: number }];
}>();

const MAIN: Record<DraftTrackKind, { icon: string; label: string }> = {
  chart: { icon: 'mdi-file-document-outline', label: 'editor-track-chart' },
  music: { icon: 'mdi-music-note', label: 'editor-track-music' },
  cover: { icon: 'mdi-image-outline', label: 'editor-track-cover' },
};

/** 选中的是不是自己加的轨道（是的话下面多一段片段时间可以改）。 */
const lane = computed<DraftLane | null>(
  () => props.draft?.lanes.find((it) => it.id === props.trackId) ?? null,
);

/**
 * 轨道上那份素材的种类：主轨道就是轨道本身，自己加的轨道看素材。
 *
 * 歌词素材只是这条特效轨的来源文件，不算「哪条主轨」——它落成的是特效轨，
 * 名字与参数都按轨道自己显示，所以这里当 null（没有主轨那一套图标 / 标题）。
 */
const kind = computed<DraftTrackKind | null>(() => {
  if (lane.value) {
    const media = lane.value.media?.kind;
    return media && media !== 'lyric' ? media : null;
  }
  return props.trackId === 'chart' || props.trackId === 'music' || props.trackId === 'cover'
    ? props.trackId
    : null;
});

const laneNumber = computed(() =>
  lane.value ? (props.draft?.lanes ?? []).findIndex((it) => it.id === lane.value?.id) + 1 : 0,
);

const trackTitle = computed(() => {
  if (lane.value) return lane.value.name.trim() || t('editor-lane-title', { n: laneNumber.value });
  return kind.value ? t(MAIN[kind.value].label) : '';
});

const trackIcon = computed(() => {
  if (lane.value?.kind === 'text') return 'mdi-format-text';
  if (lane.value?.kind === 'effect') return 'mdi-auto-fix';
  if (kind.value) return MAIN[kind.value].icon;
  return 'mdi-layers-outline';
});

/** 文字轨：没有素材文件，文字直接写在右栏里。 */
const isTextLane = computed(() => lane.value?.kind === 'text');

/** 特效轨：同样没有素材文件，参数（文字 / 字体 / 字号 / 颗粒）写在这里。 */
const isEffectLane = computed(() => lane.value?.kind === 'effect');

/**
 * 特效轨上选中的那一段：一行可以放好几段，右栏只改这一段。
 *
 * 老草稿里轨道自己就是一段（`lane.effect`），`clipsOf` 会补出来；
 * 父级还没选中具体片段时退回第一段，总不至于点开一条轨道什么都没得改。
 */
const clip = computed<DraftClip | null>(() => {
  if (!lane.value || lane.value.kind !== 'effect') return null;
  const clips = clipsOf(lane.value);
  if (props.clipId) return clips.find((it) => it.id === props.clipId) ?? clips[0] ?? null;
  return clips[0] ?? null;
});

/**
 * 特效轨当前的效果参数：缺的键按默认补上，材质优先用这一段的 ——
 * 老草稿的材质存在草稿上（`EditorDraft.noise`），新草稿一段一份。
 */
const effect = computed<TextNoiseEffect>(() => {
  const raw = clip.value?.effect ?? lane.value?.effect;
  return {
    ...defaultTextNoise(),
    ...(raw ?? {}),
    material: raw?.material ?? props.draft?.noise ?? DEFAULT_NOISE_MATERIAL,
  };
});

/**
 * 可选的字体：第一个是内置的 `Phigros`（仓库根 `assets/font.ttf`，与引擎画面同一份），
 * 其余看系统里装没装，认不出的名字会退回默认字体 —— 所以这个名字也能手打。
 */
const FONTS = [
  NATIVE_FONT,
  'Microsoft YaHei',
  'SimHei',
  'SimSun',
  'DengXian',
  'KaiTi',
  'FangSong',
  'Source Han Sans SC',
  'Arial',
  'Impact',
  'Georgia',
  'Consolas',
  'Cascadia Mono',
];

/**
 * 批量选中的那几段（Ctrl 点时间轴上的特效片段）。
 *
 * 少于两段时右栏按单段显示；两段以上时每一项都要先看它们是否一致，
 * 不一致的那一项显示「多个值」，改一下就统一写到每一段上。
 */
const batchClips = computed<DraftClip[]>(() => {
  const list = props.multi ?? [];
  if (list.length < 2 || !props.draft) return [];
  const picked: DraftClip[] = [];
  for (const it of list) {
    const target = props.draft.lanes.find((one) => one.id === it.laneId);
    const clip = (target?.clips ?? []).find((one) => one.id === it.clipId);
    if (clip) picked.push(clip);
  }
  return picked.length > 1 ? picked : [];
});

const batch = computed<TextNoiseEffect[]>(() =>
  batchClips.value
    .filter((it): it is DraftClip & { effect: TextNoiseEffect } => !!it.effect)
    .map((it) => ({
      ...defaultTextNoise(),
      ...it.effect,
      material: it.effect.material ?? props.draft?.noise ?? DEFAULT_NOISE_MATERIAL,
    })),
);

/** 是不是真的在批量改（选中了两段以上且都有参数）。 */
const many = computed(() => batch.value.length > 1);

/** 某一项在多段之间是否不一致。 */
function mixedOf<T>(get: (it: TextNoiseEffect) => T): boolean {
  const list = batch.value;
  const [first] = list;
  if (!first || list.length < 2) return false;
  const value = get(first);
  return list.some((it) => get(it) !== value);
}

/** 每个字段是否「多个值」：键名与 `TextNoiseEffect` / `NoiseMaterial` 一致。 */
const mixed = computed<Record<string, boolean>>(() => {
  const out: Record<string, boolean> = {};
  for (const key of ['text', 'fontFamily', 'color', 'fontSize', 'lineHeight', 'letterSpacing', 'x', 'y', 'rotation', 'enable', 'invert', 'subtract', 'blend'] as const) {
    out[key] = mixedOf((it) => it[key]);
  }
  for (const key of Object.keys(DEFAULT_NOISE_MATERIAL) as (keyof NoiseMaterial)[]) {
    out[key] = mixedOf((it) => it.material[key]);
  }
  return out;
});

/** 起止时间在各段之间是否一致（批量选中时显示「多个值」）。 */
const mixedTime = computed(() => {
  const list = batchClips.value;
  const [first] = list;
  if (!first || list.length < 2) return { start: false, length: false };
  return {
    start: list.some((it) => it.start !== first.start),
    length: list.some((it) => it.length !== first.length),
  };
});

/** 特效参数改一项：整份参数抛给父级（草稿里存的就是这一份）。 */
function setEffect(patch: Partial<TextNoiseEffect>) {
  if (!lane.value) return;
  if (isEffectLane.value) {
    // 批量选了好几段：这一项写到每一段上，参数挂在片段上各改各的
    if (many.value) {
      emit('update-clips', patch);
      return;
    }
    emit('update-clip', { effect: { ...effect.value, ...patch } });
    return;
  }
  emit('update-lane', lane.value.id, { effect: { ...effect.value, ...patch } });
}

/** 颜色框给的是 `#rrggbb`，收一下再写回去（手填的值也走这条路）。 */
function setColor(value: unknown) {
  setEffect({ color: normalizeNoiseColor(String(value ?? '')) });
}

/**
 * 手填颜色的中间值：敲到 `#3` 时还认不出是哪种颜色，这时候不能立刻收回原生红，
 * 否则一个字也打不进去 —— 等失焦或回车再提交。
 */
const colorDraft = ref('');
watch(
  effect,
  (it) => {
    colorDraft.value = it.color;
  },
  { immediate: true },
);

function commitColor() {
  // 批量选中且几段颜色不同时框里是空的，没打字就失焦不该把它改回原生红
  if (mixed.value.color && !colorDraft.value.trim()) return;
  setColor(colorDraft.value);
}

/**
 * 这条特效轨的噪域材质：一条轨道一份（老草稿退回草稿上那份）。
 * 改一项就把整份写回轨道 —— 引擎按方块读材质，缺的键才走渲染配置。
 */
const noise = computed<NoiseMaterial>(() => effect.value.material ?? DEFAULT_NOISE_MATERIAL);

function setNoise(patch: Partial<NoiseMaterial>) {
  setEffect({ material: { ...noise.value, ...patch } });
}

/** 材质数值的统一写法：三位小数以内，省得滑杆把 `0.30000000000000004` 写进草稿。 */
function num(value: number, digits = 2) {
  const factor = 10 ** digits;
  return Math.round(value * factor) / factor;
}

/**
 * 材质里那几个开关。
 *
 * 不含「打击光晕」与「简化噪域」：前者要手指按在画面上才亮，编辑器预览里没有触摸，
 * 后者的平面填充不走这套材质（引擎在简化分支里把火花 / 斑点 / 扭曲全部关掉），
 * 放在这里只会让人以为调了有效果。
 */
const MATERIAL_SWITCHES = [
  { key: 'blockAreaNoiseBlotch', label: 'editor-effect-blotch' },
  { key: 'blockAreaSpark', label: 'editor-effect-spark' },
  { key: 'blockAreaDistort', label: 'editor-effect-distort' },
] as const;

type NoiseFlagKey = (typeof MATERIAL_SWITCHES)[number]['key'];

/** 材质里的数值项：键名与 `NoiseMaterial` 一致，滑杆只管范围与显示精度。 */
type NoiseNumberKey =
  | 'blockAreaDisplaceStrength'
  | 'blockAreaNoiseRadius'
  | 'blockAreaNoiseSmoothness'
  | 'blockAreaDisplaceSpeed'
  | 'blockAreaEdgeNoise'
  | 'blockAreaDensity'
  | 'blockAreaPixelScale'
  | 'blockAreaAlpha';

interface MaterialSlider {
  key: NoiseNumberKey;
  label: string;
  min: number;
  max: number;
  step: number;
  /** 小数位数；`percent` 为真时按百分比显示（透明度那种 0~1 的量）。 */
  digits: number;
  percent?: boolean;
}

const MATERIAL_SLIDERS: MaterialSlider[] = [
  { key: 'blockAreaDisplaceStrength', label: 'editor-effect-strength', min: 0, max: 1, step: 0.01, digits: 2 },
  { key: 'blockAreaNoiseRadius', label: 'editor-effect-noise-radius', min: 0, max: 1, step: 0.01, digits: 2 },
  { key: 'blockAreaNoiseSmoothness', label: 'editor-effect-smoothness', min: 0, max: 5, step: 0.1, digits: 1 },
  { key: 'blockAreaDisplaceSpeed', label: 'editor-effect-speed', min: 0, max: 5, step: 0.05, digits: 2 },
  { key: 'blockAreaEdgeNoise', label: 'editor-effect-edge', min: 0, max: 4, step: 0.1, digits: 2 },
  { key: 'blockAreaDensity', label: 'editor-effect-density', min: 0.5, max: 10, step: 0.1, digits: 1 },
  { key: 'blockAreaPixelScale', label: 'editor-effect-pixel', min: 0.2, max: 24, step: 0.1, digits: 2 },
  { key: 'blockAreaAlpha', label: 'editor-effect-alpha', min: 0, max: 1, step: 0.01, digits: 0, percent: true },
];

function setNoiseFlag(key: NoiseFlagKey, value: boolean) {
  setNoise({ [key]: value } as Partial<NoiseMaterial>);
}

function setNoiseNumber(key: NoiseNumberKey, value: number) {
  setNoise({ [key]: value } as Partial<NoiseMaterial>);
}

/** 选中那条轨道上挂着的素材；选中的轨道空着时为空。 */
const media = computed<DraftMedia | null>(() => {
  const draft = props.draft;
  if (!draft || !props.trackId) return null;
  if (lane.value) return lane.value.media;
  if (props.trackId === 'chart') return draft.chart;
  return props.trackId === 'music' ? draft.music : draft.cover;
});


const durationText = computed(() => (props.duration > 0 ? clock(props.duration) : ''));

/**
 * 曲绘的原图尺寸：从缩略图的 `naturalWidth/naturalHeight` 读，换图时先清掉旧值，
 * 免得新图还没加载完时先把上一张的尺寸显示出来。
 */
const pixelSize = ref('');

watch(
  () => props.coverUrl,
  () => {
    pixelSize.value = '';
  },
);

function onCoverLoad(event: Event) {
  const img = event.target as HTMLImageElement;
  if (img.naturalWidth > 0 && img.naturalHeight > 0) {
    pixelSize.value = `${img.naturalWidth} × ${img.naturalHeight}`;
  }
}


/** 片段起止：特效轨取选中那一段，其余轨道取轨道自己。 */
const clipStart = computed(() => Math.max(0, clip.value?.start ?? lane.value?.start ?? 0));
const clipLength = computed(() => Math.max(0, clip.value?.length ?? lane.value?.length ?? 0));

/** 5 位小数：草稿按秒存 f64，再细的差别在时间轴上也拖不出来。 */
function round5(value: number) {
  return Math.round(value * 1e5) / 1e5;
}

function setLaneStart(value: unknown) {
  if (!lane.value) return;
  const start = round5(Math.max(0, toSeconds(value)));
  if (isEffectLane.value) {
    if (many.value) emit('update-clips-time', { start });
    else emit('update-clip', { start });
    return;
  }
  emit('update-lane', lane.value.id, { start });
}

function setLaneLength(value: unknown) {
  if (!lane.value) return;
  const length = round5(Math.max(0, toSeconds(value)));
  if (isEffectLane.value) {
    if (many.value) emit('update-clips-time', { length });
    else emit('update-clip', { length });
    return;
  }
  emit('update-lane', lane.value.id, { length });
}

/**
 * 起止时间：界面按「起点 / 结束时间」给，草稿里存的是起点 + 长度。
 * 结束时间早于起点时留 0.1 秒，别让片段变成负长度。
 */
const endTime = computed(() => round5(clipStart.value + clipLength.value));

function setLaneEnd(value: unknown) {
  if (!lane.value) return;
  const length = round5(Math.max(0.1, toSeconds(value) - clipStart.value));
  if (isEffectLane.value) {
    if (many.value) emit('update-clips-time', { length });
    else emit('update-clip', { length });
    return;
  }
  emit('update-lane', lane.value.id, { length });
}

/** 文字轨上的文字：每敲一下就把整段抛给父级存盘。 */
function setLaneText(value: string) {
  if (lane.value) emit('update-lane', lane.value.id, { text: value });
}

function clearSelected() {
  if (props.trackId) emit('clear', props.trackId);
}
</script>

<template>
  <aside class="editbar">
    <div class="editbar__head">
      <v-icon :icon="trackIcon" size="15"></v-icon>
      <span class="editbar__title">{{ t('editor-edit') }}</span>
      <span v-if="trackTitle" class="editbar__track">{{ trackTitle }}</span>
    </div>

    <!-- 预览钉在顶部：改一个字、拖一下滑杆就能看见，不用往下滚 -->
    <div v-if="isEffectLane && trackId" class="editbar__preview">
      <TextNoisePreview :effect="effect" />
    </div>

    <!--
      文字单独贴在预览下面：它是这一条特效轨的「内容」，和字体 / 颜色那些样式参数不是一回事。
      右下角可以往下拉长，长句子能多看到几行。
    -->
    <textarea
      v-if="isEffectLane && trackId"
      class="editbar__noise-text"
      :value="effect.text"
      :placeholder="t('editor-effect-text')"
      spellcheck="false"
      @input="setEffect({ text: ($event.target as HTMLTextAreaElement).value })"
    ></textarea>

    <div class="editbar__body scroll-mask">
      <!-- 还没选中轨道：说一句怎么选中，别摆一堆点不动的控件 -->
      <div v-if="!trackId" class="editbar__empty">
        <v-icon icon="mdi-cursor-default-click-outline" size="26"></v-icon>
        <span>{{ t('editor-edit-empty') }}</span>
      </div>

      <template v-else>
        <!-- 文字轨没有素材文件：文字直接在这儿写 -->
        <div v-if="!media && isTextLane" class="editbar__text">
          <v-textarea
            :model-value="lane?.text ?? ''"
            :label="t('editor-edit-text')"
            rows="3"
            density="compact"
            variant="outlined"
            hide-details
            @update:model-value="setLaneText"
          ></v-textarea>
        </div>

        <!-- 特效轨（文字噪域）：预览钉在顶部，这里一行一个参数，行高一致 -->
        <template v-else-if="!media && isEffectLane">
          <div class="editbar__rows">
            <div class="editbar__row">
              <span class="editbar__row-label">{{ t('editor-effect-font') }}</span>
              <v-combobox
                :model-value="mixed.fontFamily ? null : effect.fontFamily"
                :items="FONTS"
                :placeholder="mixed.fontFamily ? t('editor-multi-value') : ''"
                density="compact"
                variant="outlined"
                hide-details
                class="editbar__row-field"
                @update:model-value="setEffect({ fontFamily: String($event ?? '') })"
              ></v-combobox>
            </div>

            <!-- 颜色：色块点开是系统取色板，右边的框可以直接手填 `#rrggbb`；融入原谱时用不上 -->
            <div class="editbar__row" :class="{ 'editbar__row--off': effect.blend }">
              <span class="editbar__row-label">{{ t('editor-effect-color') }}</span>
              <div class="editbar__row-field editbar__color">
                <input
                  class="editbar__swatch"
                  type="color"
                  :value="mixed.color ? '#000000' : effect.color"
                  :disabled="effect.blend"
                  :title="effect.blend ? t('editor-effect-blend-tip') : ''"
                  @input="setColor(($event.target as HTMLInputElement).value)"
                />
                <v-text-field
                  :model-value="mixed.color ? '' : colorDraft"
                  :placeholder="mixed.color ? t('editor-multi-value') : ''"
                  :disabled="effect.blend"
                  density="compact"
                  variant="outlined"
                  hide-details
                  @update:model-value="colorDraft = String($event ?? '')"
                  @blur="commitColor"
                  @keydown.enter="commitColor"
                />
              </div>
            </div>

            <div class="editbar__row">
              <span class="editbar__row-label">{{ t('editor-effect-start') }}</span>
              <v-text-field
                class="editbar__row-field"
                type="number"
                step="0.00001"
                min="0"
                density="compact"
                variant="outlined"
                hide-details
                :model-value="mixedTime.start ? '' : num(clipStart, 5)"
                :placeholder="mixedTime.start ? t('editor-multi-value') : ''"
                @update:model-value="setLaneStart"
              />
            </div>

            <div class="editbar__row">
              <span class="editbar__row-label">{{ t('editor-effect-end') }}</span>
              <v-text-field
                class="editbar__row-field"
                type="number"
                step="0.00001"
                min="0"
                density="compact"
                variant="outlined"
                hide-details
                :model-value="mixedTime.length ? '' : num(endTime, 5)"
                :placeholder="mixedTime.length ? t('editor-multi-value') : ''"
                @update:model-value="setLaneEnd"
              />
            </div>
          </div>

          <!-- 位置大小：字号、行距、间距、位置都按屏幕比例存，换个分辨率放出来还是同一个构图 -->
          <h3 class="editbar__group-title">{{ t('editor-effect-transform') }}</h3>

          <SliderRow
            :label="t('editor-effect-size')"
            :model-value="effect.fontSize"
            :min="0.02"
            :max="0.9"
            :step="0.005"
            :digits="1"
            percent
            :mixed="mixed.fontSize"
            :mixed-text="t('editor-multi-value')"
            @update:model-value="setEffect({ fontSize: $event })"
          />

          <div class="editbar__fields">
            <SliderRow
              :label="t('editor-effect-line-height')"
              :model-value="effect.lineHeight"
              :min="0.8"
              :max="3"
              :step="0.05"
              :digits="2"
              :mixed="mixed.lineHeight"
              :mixed-text="t('editor-multi-value')"
              @update:model-value="setEffect({ lineHeight: $event })"
            />
            <SliderRow
              :label="t('editor-effect-letter-spacing')"
              :model-value="effect.letterSpacing"
              :min="-0.5"
              :max="2"
              :step="0.01"
              :digits="2"
              :mixed="mixed.letterSpacing"
              :mixed-text="t('editor-multi-value')"
              @update:model-value="setEffect({ letterSpacing: $event })"
            />
          </div>

          <div class="editbar__fields">
            <SliderRow
              :label="t('editor-effect-x')"
              :model-value="effect.x"
              :min="0"
              :max="1"
              :step="0.01"
              :digits="0"
              percent
              :mixed="mixed.x"
              :mixed-text="t('editor-multi-value')"
              @update:model-value="setEffect({ x: $event })"
            />
            <SliderRow
              :label="t('editor-effect-y')"
              :model-value="effect.y"
              :min="0"
              :max="1"
              :step="0.01"
              :digits="0"
              percent
              :mixed="mixed.y"
              :mixed-text="t('editor-multi-value')"
              @update:model-value="setEffect({ y: $event })"
            />
          </div>

          <SliderRow
            :label="t('editor-effect-rotation')"
            :model-value="effect.rotation"
            :min="-180"
            :max="180"
            :step="1"
            :digits="0"
            unit="°"
            :mixed="mixed.rotation"
            :mixed-text="t('editor-multi-value')"
            @update:model-value="setEffect({ rotation: $event })"
          />


          <h3 class="editbar__group-title">{{ t('editor-effect-more') }}</h3>

          <div class="editbar__switches">
            <!-- 激活：关掉这一段就不生成噪域（谱面里留着、引擎跳过），默认开着 -->
            <button
              type="button"
              role="switch"
              class="editbar__toggle"
              :class="{
                'editbar__toggle--on': effect.enable && !mixed.enable,
                'editbar__toggle--mixed': mixed.enable,
              }"
              :aria-checked="mixed.enable ? 'mixed' : effect.enable"
              :title="t('editor-effect-enable-tip')"
              @click="setEffect({ enable: !effect.enable })"
            >
              <span class="editbar__toggle-label">{{ t('editor-effect-enable') }}</span>
              <span class="editbar__toggle-track"><span class="editbar__toggle-thumb"></span></span>
            </button>

            <!-- 融入原谱：不写颜色 / 材质，跟着谱面自己的噪域设置走 -->
            <button
              type="button"
              role="switch"
              class="editbar__toggle"
              :class="{
                'editbar__toggle--on': effect.blend && !mixed.blend,
                'editbar__toggle--mixed': mixed.blend,
              }"
              :aria-checked="mixed.blend ? 'mixed' : effect.blend"
              :title="t('editor-effect-blend-tip')"
              @click="setEffect({ blend: !effect.blend })"
            >
              <span class="editbar__toggle-label">{{ t('editor-effect-blend') }}</span>
              <span class="editbar__toggle-track"><span class="editbar__toggle-thumb"></span></span>
            </button>

            <!-- 反相：只把方块的 isSubtract 打开，和反转各管一摊，可以同时开 -->
            <button
              type="button"
              role="switch"
              class="editbar__toggle"
              :class="{
                'editbar__toggle--on': effect.subtract && !mixed.subtract,
                'editbar__toggle--mixed': mixed.subtract,
              }"
              :aria-checked="mixed.subtract ? 'mixed' : effect.subtract"
              :title="t('editor-effect-subtract-tip')"
              @click="setEffect({ subtract: !effect.subtract })"
            >
              <span class="editbar__toggle-label">{{ t('editor-effect-subtract') }}</span>
              <span class="editbar__toggle-track"><span class="editbar__toggle-thumb"></span></span>
            </button>

            <!-- 反转：铺满整屏、文字挖空（用补集矩形拼），管的是「铺哪些方块」 -->
            <button
              type="button"
              role="switch"
              class="editbar__toggle"
              :class="{
                'editbar__toggle--on': effect.invert && !mixed.invert,
                'editbar__toggle--mixed': mixed.invert,
              }"
              :aria-checked="mixed.invert ? 'mixed' : effect.invert"
              @click="setEffect({ invert: !effect.invert })"
            >
              <span class="editbar__toggle-label">{{ t('editor-effect-invert') }}</span>
              <span class="editbar__toggle-track"><span class="editbar__toggle-thumb"></span></span>
            </button>

            <!--
              自己画的小开关，不用 `v-switch`：Vuetify 那个是胶囊形、标签只能放右边，
              改圆角和尺寸要跟它内部的轨道 / 滑块几何较劲。这里一行一个，名字在左、方块在右。
            -->
            <button
              v-for="item in MATERIAL_SWITCHES"
              :key="item.key"
              type="button"
              role="switch"
              class="editbar__toggle"
              :class="{
                'editbar__toggle--on': noise[item.key] && !mixed[item.key],
                'editbar__toggle--mixed': mixed[item.key],
              }"
              :aria-checked="mixed[item.key] ? 'mixed' : noise[item.key]"
              :disabled="effect.blend"
              :title="effect.blend ? t('editor-effect-blend-tip') : ''"
              @click="setNoiseFlag(item.key, !noise[item.key])"
            >
              <span class="editbar__toggle-label">{{ t(item.label) }}</span>
              <span class="editbar__toggle-track"><span class="editbar__toggle-thumb"></span></span>
            </button>
          </div>

          <SliderRow
            v-for="item in MATERIAL_SLIDERS"
            :key="item.key"
            :label="t(item.label)"
            :model-value="noise[item.key]"
            :min="item.min"
            :max="item.max"
            :step="item.step"
            :digits="item.digits"
            :percent="item.percent ?? false"
            :mixed="mixed[item.key]"
            :mixed-text="t('editor-multi-value')"
            :disabled="effect.blend"
            @update:model-value="setNoiseNumber(item.key, $event)"
          />
        </template>

        <!-- 选中的轨道上什么都没有：这里也能直接去素材库挑 -->
        <div v-else-if="!media" class="editbar__empty">
          <v-icon icon="mdi-plus-box-outline" size="24"></v-icon>
          <span>{{ t('editor-edit-track-empty') }}</span>
          <v-btn size="small" variant="text" prepend-icon="mdi-folder-multiple-image" @click="emit('pick-media')">
            {{ t('editor-edit-pick-media') }}
          </v-btn>
        </div>

        <template v-else>
          <!-- 曲绘顺手当缩略图看；顺手从它身上读原图尺寸 -->
          <div v-if="kind === 'cover' && coverUrl" class="editbar__thumb">
            <img :src="coverUrl" alt="" @load="onCoverLoad" />
          </div>

          <div class="editbar__name" :title="media.name">{{ media.name }}</div>
          <div class="editbar__path" :title="media.path">{{ media.path }}</div>

          <dl v-if="(kind === 'music' && durationText) || (kind === 'cover' && pixelSize)" class="editbar__meta">
            <template v-if="kind === 'music' && durationText">
              <dt>{{ t('editor-edit-duration') }}</dt>
              <dd>{{ durationText }}</dd>
            </template>
            <template v-if="kind === 'cover' && pixelSize">
              <dt>{{ t('editor-edit-size') }}</dt>
              <dd>{{ pixelSize }}</dd>
            </template>
          </dl>

          <div class="editbar__actions">
            <v-btn
              size="small"
              variant="text"
              prepend-icon="mdi-folder-open-outline"
              :title="t('editor-media-reveal')"
              @click="emit('reveal', media)"
            >
              {{ t('editor-edit-reveal') }}
            </v-btn>
            <v-btn size="small" variant="text" prepend-icon="mdi-swap-horizontal" @click="emit('pick-media')">
              {{ t('editor-edit-swap') }}
            </v-btn>
            <v-btn size="small" variant="text" prepend-icon="mdi-trash-can-outline" @click="clearSelected">
              {{ t('editor-track-clear') }}
            </v-btn>
          </div>
        </template>

        <!-- 自己加的轨道：片段起点与长度自己定，长度比音乐长也没关系（特效轨的起止在上面那组里） -->
        <section v-if="lane && !isEffectLane" class="editbar__section">
          <h3 class="editbar__section-title">{{ t('editor-lane-clip') }}</h3>
          <div class="editbar__fields">
            <v-text-field
              :model-value="lane.start"
              type="number"
              min="0"
              step="0.1"
              density="compact"
              variant="outlined"
              hide-details
              :label="t('editor-lane-start')"
              :suffix="t('editor-lane-seconds')"
              @update:model-value="setLaneStart"
            ></v-text-field>
            <v-text-field
              :model-value="lane.length"
              type="number"
              min="0"
              step="0.1"
              density="compact"
              variant="outlined"
              hide-details
              :label="t('editor-lane-length')"
              :suffix="t('editor-lane-seconds')"
              @update:model-value="setLaneLength"
            ></v-text-field>
          </div>
        </section>
      </template>
    </div>
  </aside>
</template>

<style scoped>
/* 和左边的功能栏同一套外观：底、描边、圆角都照 `.inspector` 来，两栏才像一对 */
.editbar {
  display: flex;
  flex-direction: column;
  width: 300px;
  min-height: 0;
  overflow: hidden;
  background: var(--phi-bg-layer-1);
  border: 0.5px solid var(--phi-border-l2);
  border-radius: var(--phi-radius-md);
}

.editbar__head {
  display: flex;
  flex: none;
  gap: var(--phi-space-2);
  align-items: center;
  height: 32px;
  padding: 0 var(--phi-space-2);
  font-size: 0.74rem;
  color: var(--phi-label-primary);
  border-bottom: 0.5px solid var(--phi-border-l1);
}

.editbar__title {
  font-weight: 600;
}

/* 右边缀上是哪条轨道，免得只看到文件名不知道它在哪一轨上 */
.editbar__track {
  padding: 1px 6px;
  font-size: 0.64rem;
  color: var(--phi-label-caption);
  background: var(--phi-bg-layer-3);
  border-radius: var(--phi-radius-pill);
}

/* 预览钉在顶部：改参数时眼睛不用在右栏里找它在哪。
   分隔线不在这儿 —— 文字框算预览这一半的内容，线画在文字框下面 */
.editbar__preview {
  flex: none;
  padding: 8px 10px 6px;
}

/*
 * 文字输入框：贴在预览下面，和下面那组样式参数分开。
 * 用原生 textarea 而不是 `v-textarea`，就是为了右下角那个可以往下拉的手柄 ——
 * Vuetify 的输入框把内部 textarea 的 `resize` 定死了，拉不动。
 */
.editbar__noise-text {
  flex: none;
  width: calc(100% - 20px);
  min-height: 52px;
  max-height: 40vh;
  margin: 0 10px 8px;
  padding: 6px 8px;
  color: var(--phi-label-primary);
  font-family: inherit;
  font-size: 0.78rem;
  line-height: 1.5;
  resize: vertical;
  background: var(--phi-bg-layer-2);
  border: 0.5px solid var(--phi-border-l2);
  border-radius: var(--phi-radius-xs);
  outline: none;
}

.editbar__noise-text:focus {
  border-color: var(--phi-accent);
}

.editbar__noise-text::placeholder {
  color: var(--phi-label-caption);
}

/* 一行一项的基础参数：名字在左（固定宽度），控件在右，行高一致 */
.editbar__rows {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.editbar__row {
  display: flex;
  gap: var(--phi-space-2);
  align-items: center;
  min-height: 34px;
}

.editbar__row-label {
  flex: 0 0 52px;
  font-size: 0.68rem;
  color: var(--phi-label-caption);
}

.editbar__row-field {
  flex: 1 1 auto;
  min-width: 0;
}

/* 颜色行里手填框吃掉剩下的宽度 */
.editbar__color :deep(.v-input) {
  flex: 1 1 auto;
}

/* 分组标题：白色加粗，上面一条分隔线把上一组收住 */
.editbar__group-title {
  padding-top: 8px;
  margin: 6px 0 2px;
  font-size: 0.76rem;
  font-weight: 600;
  color: #fff;
  border-top: 0.5px solid var(--phi-border-l1);
}

.editbar__body {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  gap: 6px;
  min-height: 0;
  padding: 10px;
  overflow-x: hidden;
  overflow-y: auto;
  /* 分隔线放在这里：预览 + 文字框是一半，下面那堆参数是另一半 */
  border-top: 0.5px solid var(--phi-border-l1);
}

/* 面板窄，输入框统一压到 34px 高、字小一号；多行的 textarea 不受这条影响 */
.editbar :deep(.v-input:not(.v-textarea)) {
  --v-input-control-height: 34px;
}

.editbar :deep(.v-field__input) {
  font-size: 0.74rem;
}

/* 滑块压扁：一屏里要放得下五条，还留着数值行 */
.editbar :deep(.v-slider) {
  min-height: 22px;
}

.editbar :deep(.v-slider-thumb__surface) {
  width: 12px !important;
  height: 12px !important;
}

.editbar :deep(.v-slider-thumb__label) {
  font-size: 0.62rem;
}

/* 文字轨：只有一个文字框，不用缩略图那一套 */
.editbar__text {
  display: flex;
  flex-direction: column;
  gap: var(--phi-space-2);
}

.editbar__empty {
  display: flex;
  flex-direction: column;
  gap: var(--phi-space-2);
  align-items: center;
  justify-content: center;
  padding: var(--phi-space-4) var(--phi-space-2);
  font-size: 0.7rem;
  color: var(--phi-label-caption);
  text-align: center;
}

/* 曲绘缩略图：按栏宽缩，高度也给个上限，别把下面的属性挤出去 */
.editbar__thumb {
  display: flex;
  flex: none;
  align-items: center;
  justify-content: center;
  max-height: 120px;
  overflow: hidden;
  background: var(--phi-bg-layer-3);
  border-radius: var(--phi-radius-sm);
}

.editbar__thumb img {
  max-width: 100%;
  max-height: 120px;
  object-fit: contain;
}

.editbar__name {
  overflow: hidden;
  font-size: 0.74rem;
  font-weight: 600;
  color: var(--phi-label-primary);
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 完整路径：一行看不完就省略，悬停看全 —— 和素材库里那条路径一个写法 */
.editbar__path {
  overflow: hidden;
  font-size: 0.66rem;
  color: var(--phi-label-caption);
  text-overflow: ellipsis;
  white-space: nowrap;
  direction: rtl;
  text-align: left;
}

.editbar__meta {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 2px var(--phi-space-2);
  margin: 0;
  font-size: 0.68rem;
}

.editbar__meta dt {
  color: var(--phi-label-caption);
}

.editbar__meta dd {
  margin: 0;
  color: var(--phi-label-secondary);
  font-variant-numeric: tabular-nums;
}

.editbar__actions {
  display: flex;
  flex-wrap: wrap;
  gap: 2px;
  padding-top: var(--phi-space-1);
  border-top: 0.5px solid var(--phi-border-l1);
}

.editbar__actions :deep(.v-btn) {
  padding-inline: 6px;
  font-size: 0.7rem;
}

.editbar__section {
  padding-top: 6px;
  margin-top: 2px;
  border-top: 0.5px solid var(--phi-border-l1);
}

/* 小节标题：中文不用大写 + 字距那套，压到 0.66rem 就够了 */
.editbar__section-title {
  margin: 0 0 4px;
  font-size: 0.66rem;
  font-weight: 500;
  color: var(--phi-label-caption);
}

/* 起点 / 时长两个数字框并排：一栏放不下就换行。
   列宽用 minmax(0, 1fr)：滑杆自己的最小宽度会把格子撑破，撑出横向滚动条 */
.editbar__fields {
  display: grid;
  gap: var(--phi-space-2);
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  min-width: 0;
}

.editbar__fields > * {
  min-width: 0;
}

/* 颜色：左边一块系统取色板，右边一行手填的 `#rrggbb` */
.editbar__color {
  display: flex;
  gap: 6px;
  align-items: center;
}

/* 材质那几个开关：一行一个，名字在左、开关在右 */
.editbar__switches {
  display: flex;
  flex-direction: column;
  gap: 1px;
  margin-bottom: 2px;
}

.editbar__toggle {
  display: flex;
  gap: var(--phi-space-2);
  align-items: center;
  justify-content: space-between;
  width: 100%;
  padding: 3px 0;
  font-size: 0.68rem;
  color: var(--phi-label-caption);
  text-align: left;
  cursor: pointer;
  background: none;
  border: 0;
}

.editbar__toggle:hover .editbar__toggle-label {
  color: var(--phi-label-primary);
}

/* 融入原谱时颜色用不上：整行压暗，控件也跟着 disabled */
.editbar__row--off {
  opacity: 0.45;
}

/* 被别的开关接管时（譬如打开「反相」后「反转」不再生效）压暗，但位置照留 */
.editbar__toggle:disabled {
  cursor: default;
  opacity: 0.38;
}

.editbar__toggle:disabled:hover .editbar__toggle-label {
  color: var(--phi-label-caption);
}

.editbar__toggle-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/*
 * 轨道用 `box-shadow` 描边而不是 `border`：半像素边框会占掉布局宽度，
 * 滑块的位移距离就得按边框算，换主题线宽时会错位。
 */
.editbar__toggle-track {
  position: relative;
  flex: 0 0 auto;
  width: 26px;
  height: 14px;
  background: var(--phi-bg-layer-3);
  border-radius: 2px;
  box-shadow: inset 0 0 0 0.5px var(--phi-border-l2);
  transition: background 120ms ease;
}

.editbar__toggle-thumb {
  position: absolute;
  top: 3px;
  left: 3px;
  width: 8px;
  height: 8px;
  background: var(--phi-label-caption);
  border-radius: 1px;
  transition:
    transform 120ms ease,
    background 120ms ease;
}

.editbar__toggle--on .editbar__toggle-track {
  background: color-mix(in srgb, var(--phi-accent) 32%, transparent);
  box-shadow: inset 0 0 0 0.5px var(--phi-accent);
}

.editbar__toggle--on .editbar__toggle-thumb {
  background: var(--phi-accent);
  transform: translateX(12px);
}

/*
 * 批量选中时这一项在几段之间不一致：轨道用斜纹表示「几个值混在一起」，
 * 滑块停在中间，点一下才统一成同一个值。
 */
.editbar__toggle--mixed .editbar__toggle-track {
  background: repeating-linear-gradient(
    -45deg,
    color-mix(in srgb, var(--phi-accent) 28%, transparent) 0 3px,
    transparent 3px 6px
  );
  box-shadow: inset 0 0 0 0.5px var(--phi-border-l3);
}

.editbar__toggle--mixed .editbar__toggle-thumb {
  background: var(--phi-label-caption);
  transform: translateX(6px);
}

.editbar__swatch {
  flex: 0 0 auto;
  width: 28px;
  height: 28px;
  padding: 0;
  background: none;
  border: 0.5px solid var(--phi-border-l1);
  border-radius: var(--phi-radius-sm);
  cursor: pointer;
}
</style>
