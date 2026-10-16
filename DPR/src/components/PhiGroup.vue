<script setup lang="ts">
// 渲染参数页的**一级**分区外壳。
//
// 页面是两级结构：
//   一级 = PhiGroup（本组件）：渲染 / 谱面 / 更多
//   二级 = PhiSection（组内一条条横线折叠行）
// 左栏导航的两级结构与这里的层级一一对应，见 App.vue 的 `renderGroups`。
// 「预设配置」是「渲染」组里最上面的二级项 PhiSection `sec-preset`（每个组下面都有二级项）。
//
// `id` 同时是 DOM 锚点：App.vue 的左栏靠 `document.getElementById(id)` 跳转 + 滚动联动高亮，
// 所以**不能改名**。
//
// 折叠状态写进 localStorage（键 `phi.group.<id>`），与 PhiSection 的 `phi.section.<id>` 分开存。
import { computed } from 'vue';
import { useStorage } from '@vueuse/core';

const props = withDefaults(
  defineProps<{
    /** DOM 锚点 id（形如 `grp-render`）兼 localStorage 键后缀 */
    id: string;
    /** 标题（已翻译） */
    title?: string;
    /** 标题后的问号提示（已翻译）；留空则不显示 */
    tip?: string;
    /** false 时标题不可点、永远展开 */
    collapsible?: boolean;
    /** 纯标题模式（设置页用）：不折叠、不画浅底/组内淡色竖线，只留一行放大的标题
     *  和它左边一条加粗的蓝色竖条。 */
    plain?: boolean;
  }>(),
  { title: '', tip: '', collapsible: true, plain: false },
);

const open = useStorage<boolean>(`phi.group.${props.id}`, true);

/** plain 时不折叠（连 chevron 也一起不画） */
const canCollapse = computed(() => props.collapsible && !props.plain);

function toggle() {
  if (canCollapse.value) open.value = !open.value;
}
</script>

<template>
  <section :id="id" class="phi-group" :class="{ 'phi-group--collapsed': !open, 'phi-group--plain': plain }">
    <header class="phi-group__head" :class="{ 'phi-group__head--static': !canCollapse }" :title="tip || undefined" @click="toggle">
      <span class="phi-group__title">{{ title }}</span>
      <v-spacer></v-spacer>
      <!-- 标题右侧的动作槽：设置页把「重置 / 保存」挂在这里（按钮在「设置」右边）。
           靠右的位置由上面那个 v-spacer 撑出来，槽本身不放样式。 -->
      <slot name="actions"></slot>
      <v-icon v-if="canCollapse" size="small" class="phi-group__chevron">{{ open ? 'mdi-chevron-up' : 'mdi-chevron-down' }}</v-icon>
    </header>
    <v-expand-transition>
      <div v-show="open" class="phi-group__body">
        <slot></slot>
      </div>
    </v-expand-transition>
  </section>
</template>

<style scoped>
/* 一级分组：整条标题带浅底 + 左侧强调竖条，和二级的「细横线 + 小标题」明显区分开。
   组之间不再画线 —— 填充底本身就够分隔了。 */
.phi-group {
  /* margin-top：让上面的「谱面信息」横幅与第一个分组（渲染）之间也有间隔。
     这是普通分组的组间距；紧跟在 hero 横幅后面的那一组另有一条更宽的规则（见下）。 */
  margin-top: var(--phi-space-4);
  margin-bottom: var(--phi-space-4);
}

/* hero（谱面信息曲绘横幅）后面那一组：横幅不像普通分区那样带一圈下内边距，
   16px 摆在那里看着像「贴着」，单独放宽到 32px。 */
.phi-section--hero + .phi-group {
  margin-top: var(--phi-space-8);
}

.phi-group__head {
  display: flex;
  align-items: center;
  gap: var(--phi-space-2);
  min-height: 46px;
  padding: 0 var(--phi-space-3);
  cursor: pointer;
  user-select: none;
  background: var(--phi-interactive-hover);
  border-left: 3px solid var(--phi-accent);
  /* 圆角用 var(--phi-radius-sm)。
     设置页的 plain 分组单独清回 0（见下）：那边只剩一条 5px 蓝竖条，圆角会把竖条两端切掉。 */
  border-radius: var(--phi-radius-sm);
}
.phi-group__head:hover {
  background: var(--phi-interactive-active);
}
.phi-group__head--static {
  cursor: default;
}
.phi-group__head--static:hover {
  background: var(--phi-interactive-hover);
}

.phi-group__title {
  font-size: 1.05rem;
  font-weight: 700;
  line-height: 1.3;
  letter-spacing: 0.02em;
  color: var(--phi-label-primary);
}

.phi-group__chevron {
  color: var(--phi-label-tertiary);
  flex: none;
}

/* 组内整体缩进（左边缘与组标题那根强调条对齐），再用一根竖线把二级分区「挂」在组下面 */
.phi-group__body {
  margin-left: 3px;
  padding-left: var(--phi-space-4);
  border-left: 1px solid var(--phi-border-l3);
}

/* plain（设置页的「设置 / 关于」）：去掉折叠与浅底等装饰，只留放大的标题 + 左侧一条竖线。
   组内那根 1px `--phi-border-l3` 去掉，标题左侧的强调条 5px；
   标题左内边距 20px，组内缩进 24px。**这一条只作用于 plain（设置页）**，
   渲染页的普通分组
   仍保留「标题强调条 + 组内竖线」这对（那根细线是把二级分区挂在组下面的视觉线索）。 */
.phi-group--plain .phi-group__head,
.phi-group--plain .phi-group__head:hover {
  min-height: 0;
  /* 左右内边距 20px：让设置 / 关于与左侧竖线拉开间隔。 */
  padding: var(--phi-space-1) var(--phi-space-5);
  background: none;
  border-left: 5px solid var(--phi-accent);
  /* 基础规则给的 8px 圆角在这里清掉：plain 头部没有底色，圆角唯一的作用
     就是把那条 5px 蓝竖条的两端切圆，看着像被削掉一块。 */
  border-radius: 0;
  cursor: default;
}

.phi-group--plain .phi-group__body {
  /* 覆盖基础规则里的 `border-left: 1px solid var(--phi-border-l3)`；
     缩进跟着标题一起外移，标题与下面的内容仍从同一条竖线量出去。 */
  border-left: 0;
  margin-left: 5px;
  padding-left: var(--phi-space-6);
}

.phi-group--plain .phi-group__title {
  font-size: 1.5rem;
}
</style>
