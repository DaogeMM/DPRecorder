<script setup lang="ts">
/**
 * 一个配置条目：左边配置名、右边控件（插槽）。
 *
 * 排布是「一行两个」的一半宽度，样式来自全局的 `src/design/config.css`。
 * 提示不做 ⓘ 圆圈，直接写到根元素的原生 `title` 上 —— 悬停即显示。
 * `full` 用于多行文本 / 长文本，让它横跨整行。
 */
defineProps<{
  label?: string;
  tip?: string;
  full?: boolean;
}>();
</script>

<template>
  <div class="cfg-item" :class="{ 'cfg-item--full': full }" :title="tip || undefined">
    <span v-if="label" class="cfg-item__label">{{ label }}</span>
    <div class="cfg-item__control">
      <slot />
    </div>
  </div>
</template>
