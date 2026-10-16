<script setup lang="ts">
// 渲染参数页的统一分区外壳（2026-10-10）。
//
// 取代了原来的两种外壳：
//   - SettingWindow.vue：圆角卡片 + 半透明底 + 一圈描边（「渲染项目」等 6 个分区用它）
//   - StickyLabel.vue：一条粘性标题（「输出」等 5 个分区用它）
// 现在全页只有这一种：**一条 0.5px 底线 + 可点击的标题行 + 右侧箭头**，点开才显示内容。
//
// 折叠状态写进 localStorage（键 `phi.section.<id>`），下次打开保持原样。
//
// `id` 同时是 DOM 锚点：`App.vue` 的左栏导航靠 `document.getElementById(id)` 跳转
// + `onAnyScroll()` 做滚动联动高亮，所以**不能改名**，见 App.vue 的 renderGroups / renderOrder。
// 2026-10-11 起它上面还套了一层同级的 PhiGroup.vue（一级分组），这里是二级。
import { computed, onUnmounted, watch } from 'vue';
import { useStorage } from '@vueuse/core';
import { setSectionGate } from '../config/sectionGate';
import CfgSwitch from './CfgSwitch.vue';

const props = withDefaults(
  defineProps<{
    /** DOM 锚点 id（形如 `sec-params`）兼 localStorage 键后缀 */
    id: string;
    /** 标题（已翻译）；hero 变体由 #head 插槽全权接管时可以留空 */
    title?: string;
    /** 标题后的问号提示（已翻译）；留空则不显示 */
    tip?: string;
    /** row = 普通横线栏（默认）；hero = 顶部大横幅（谱面信息用） */
    variant?: 'row' | 'hero';
    /** false 时标题不可点、永远展开（hero 在未导入谱面时用） */
    collapsible?: boolean;
    /** 纯界面的总开关：不绑任何配置，只是「勾上才显示这一栏的选项」，
     *  状态存 localStorage（键 `phi.gate.<id>`）。有配置语义的开关请用 `v-model:gate`。
     *  ⚠️ 2026-10-11 起**暂时没有分区在用它** —— 原来唯一的调用方「多语言」已经改成三级菜单行
     *  （`PhiSubMenu` 的 `v-model` + `ConfigView.vue` 里的 `multiLang` 存储）。留着这个 prop 是因为
     *  以后要加「纯界面总开关」的二级分区还得用它。 */
    gateUi?: boolean;
    /** 总开关的问号提示（已翻译）；不传则退回 `tip` */
    gateTip?: string;
  }>(),
  { title: '', tip: '', variant: 'row', collapsible: true, gateUi: false, gateTip: '' },
);

const open = useStorage<boolean>(`phi.section.${props.id}`, true);

/** 二级分区自己的总开关（2026-10-11：部分二级菜单也加上勾选框）：
 *  - 有配置语义的用 `v-model:gate`（按键 = `renderNote`、噪域 = `renderBlockArea`、
 *    谱面进水 = `waterScreen`）—— 与「渲染项目」等处的同一个值联动，改哪边都一样；
 *  - 纯界面的用 `gate-ui`（原来只有「多语言」；它 2026-10-11 已改成三级菜单行，见上面 props 注释）。
 *  关掉时整块 body 收起（与三级菜单 PhiSubMenu 同款行为），箭头也一并变灰、点不动。 */
/** ⚠️ 这里的 `{ default: undefined }` 不能省（2026-10-11 踩过）：
 *  `defineModel<boolean | undefined>()` 编译出来的 prop 是 `{ type: Boolean, skipCheck: true }`，
 *  **没有 default**；而 Vue 的 Boolean 转换会把「父组件没传这个 prop」直接变成 `false`
 *  （`@vue/runtime-core` 的 `resolvePropValue`：`if (isAbsent && !hasDefault) value = false`）
 *  ⇒ `gate.value !== undefined` 对**每个**分区都成立，所有二级栏都冒出勾选框、且都被判成「未启用」。
 *  补上 `default: undefined` 后「没绑 `v-model:gate`」才真的是 undefined。 */
const gate = defineModel<boolean | undefined>('gate', { default: undefined });
const localGate = useStorage<boolean>(`phi.gate.${props.id}`, false);

const gated = computed(() => gate.value !== undefined || props.gateUi);
const gateOn = computed(() => (gate.value !== undefined ? gate.value : localGate.value));
/** body 真正显示的条件：分区自己展开 **且**（没有总开关 或 总开关勾着） */
const shown = computed(() => open.value && (!gated.value || gateOn.value));

/** 勾上总开关时把分区展开，免得「勾了却看不到东西」。 */
watch(gateOn, (on) => {
  if (on) open.value = true;
});

/**
 * 把总开关状态登记到共享表（`src/config/sectionGate.ts`），供左栏目录显示
 * 「启用 / 未启用」。没有总开关的分区登记 undefined（= 从表里注销）。
 */
watch(
  [gated, gateOn],
  ([isGated, on]) => setSectionGate(props.id, isGated ? on : undefined),
  { immediate: true },
);
onUnmounted(() => setSectionGate(props.id, undefined));

function setGate(value: boolean) {
  if (gate.value !== undefined) gate.value = value;
  else localGate.value = value;
}

function toggle() {
  if (!props.collapsible) return;
  if (gated.value && !gateOn.value) return;
  open.value = !open.value;
}
</script>

<template>
  <section :id="id" class="phi-section" :class="[`phi-section--${variant}`, { 'phi-section--collapsed': !open }]">
    <header class="phi-section__head" :class="{ 'phi-section__head--static': !collapsible }" :title="tip || undefined" @click="toggle">
      <slot name="head">
        <span class="phi-section__title">{{ title }}</span>
      </slot>
      <template v-if="variant === 'row'">
        <!-- 二级总开关（2026-10-11）。位置：**紧跟在标题右边**（2026-10-11：
             勾选框放到二级菜单标题右边、留一点间隙、尺寸缩小 50%）——
             原来它在 `v-spacer` 之后、贴着最右边的箭头；现在挪到标题后面，尺寸减半（`small`）。
             包一层 `@click.stop`：点开关不该顺手把分区折叠了（header 自己带 @click）。 -->
        <span v-if="gated" class="phi-section__gate" @click.stop>
          <CfgSwitch bare small :model-value="gateOn" :tip="gateTip || tip" @update:model-value="setGate" />
        </span>
        <v-spacer></v-spacer>
        <v-icon v-if="collapsible" size="small" class="phi-section__chevron" :class="{ 'phi-section__chevron--off': gated && !gateOn }">{{ shown ? 'mdi-chevron-up' : 'mdi-chevron-down' }}</v-icon>
      </template>
      <!-- hero 变体的箭头浮在横幅右上角 -->
      <v-icon v-else-if="collapsible" size="small" class="phi-section__chevron phi-section__chevron--floating">{{ open ? 'mdi-chevron-up' : 'mdi-chevron-down' }}</v-icon>
    </header>
    <v-expand-transition>
      <div v-show="shown" class="phi-section__body">
        <slot></slot>
      </div>
    </v-expand-transition>
  </section>
</template>

<style scoped>
/* 二级分区之间用**顶部**的 0.5px 细线分隔（2026-10-11：先去掉底部的线，再改成顶部线）——
   第一条不画（它紧跟在 PhiGroup 的分组标题下面，再画一条就成了双线）。
   左侧另有一条导引线：**从标题下面一点开始，画到这个分区的最底部**（先画到「渲染到底」，
   再纠正成「从二级标题下面一点开始，到这个二级框结束」）。
   它和标题前那根 3px 短竖条在同一条中心线上。 */
.phi-section {
  position: relative;
  /* 分区之间再加一段空白（给各个二级栏间加点间隔）——
     连同 body 自己的下内边距，最后一个条目到上一条细线大约 32px。 */
  margin-top: var(--phi-space-4);
  border-top: 0.5px solid var(--phi-border-l3);
}
.phi-section:first-child {
  margin-top: 0;
  border-top: none;
}
/* hero（谱面信息横幅）不画导引线 —— 那块整幅是曲绘，画上去会压在图上。
   线挂在 body 上而不是 section 上：body 本身就从标题下方开始，
   而且折叠时 `v-show` 会把它藏掉，线自然跟着消失。 */
.phi-section:not(.phi-section--hero) .phi-section__body {
  position: relative;
}
.phi-section:not(.phi-section--hero) .phi-section__body::before {
  content: '';
  position: absolute;
  left: calc(var(--phi-space-2) + 1px);
  top: 0;
  bottom: 0;
  width: 1px;
  background: var(--phi-border-l2);
  pointer-events: none;
}

.phi-section__head {
  display: flex;
  align-items: center;
  gap: var(--phi-space-2);
  /* 行高与字号 2026-10-11 上调：38px → 44px、0.92rem → 1rem。 */
  min-height: 44px;
  padding: 0 var(--phi-space-2);
  cursor: pointer;
  user-select: none;
}
.phi-section__head:hover {
  background: var(--phi-interactive-hover);
}
.phi-section--collapsed .phi-section__head:hover {
  background: var(--phi-interactive-hover);
}
.phi-section__head--static {
  cursor: default;
}
.phi-section__head--static:hover {
  background: transparent;
}

/* 二级标题：比一级（PhiGroup）小一档、字色降一级，
   前面加一根短竖条当「子项」标记 —— 悬停时竖条变成强调色，暗示可以点。 */
.phi-section__title {
  display: inline-flex;
  align-items: center;
  gap: var(--phi-space-2);
  font-size: 1rem;
  font-weight: 600;
  line-height: 1.3;
  color: var(--phi-label-secondary);
}
.phi-section__title::before {
  content: '';
  flex: none;
  width: 3px;
  height: 15px;
  border-radius: 2px;
  background: var(--phi-border-l4);
}
.phi-section__head:hover .phi-section__title::before {
  background: var(--phi-accent);
}

.phi-section__chevron {
  color: var(--phi-label-tertiary);
  flex: none;
}
.phi-section__chevron--off {
  opacity: 0.4;
}
/* 标题右边的二级总开关：包一层只为了 `@click.stop`、`flex: none`，
   以及和标题之间那道间隙（head 自带 `gap: 8px`，这里再补 4px ⇒ 共 12px）。 */
.phi-section__gate {
  display: inline-flex;
  align-items: center;
  flex: none;
  margin-left: var(--phi-space-1);
}

.phi-section__body {
  /* 左边多留 6px，给那条通到底的导引线让位 */
  padding: var(--phi-space-1) var(--phi-space-2) var(--phi-space-4) calc(var(--phi-space-2) + 6px);
}

/* ===== hero：谱面信息那块大横幅 ===== */
.phi-section--hero {
  border-bottom: 0.5px solid var(--phi-border-l3);
}
.phi-section--hero .phi-section__head {
  position: relative;
  display: block;
  padding: 0;
  min-height: 0;
  overflow: hidden;
  /* 2026-10-11 谱面信息曾去掉圆角 ⇒ 原来是 var(--phi-radius-lg)；
     当天又恢复圆角、只圆上面两个角：
     这块 head 就是整张卡的上半部分（曲绘横幅），下半部分的谱面信息表单接着往下排，
     下面两个角保持方角，整块才还像一张卡。
     曲绘缩略图（ChartHero.vue 的 `.chart-hero__cover`）仍是 0，没动。 */
  border-radius: var(--phi-radius-sm) var(--phi-radius-sm) 0 0;
}
.phi-section--hero .phi-section__head:hover {
  background: transparent;
}
.phi-section__chevron--floating {
  position: absolute;
  right: var(--phi-space-3);
  top: var(--phi-space-3);
  z-index: 2;
  color: var(--phi-label-secondary);
  /* 2026-10-11 去掉右上角箭头的背景 ⇒ 原来这里还有
     `background: var(--phi-bg-overlay)` 与 `border-radius: var(--phi-radius-sm)`
     （箭头底下一块半透明方块），现在只剩箭头本身，直接叠在曲绘上。 */
}
.phi-section--hero .phi-section__body {
  padding: var(--phi-space-3) var(--phi-space-2) var(--phi-space-4);
}
</style>
