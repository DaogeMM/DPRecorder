<script setup lang="ts">
/**
 * 二选一（单选）框：**套用 ✅/❌ 方形开关的外壳** —— 直角、透明底、一圈 `--phi-border-l3` 外框、
 * 左右两格中间一条分界线、只有**选中的那格**填 `--phi-interactive-active` 灰底；
 * 区别只是两格里写的是选项文字，而不是 ✅ / ❌。
 *
 * 外观全在全局 `design/config.css` 的 `.cfg-radio*` 里（挨着 `.cfg-switch*`），
 * 与 `CfgSwitch.vue` 的做法一致：组件自己不带样式。
 *
 * 用法：`options` 是 `{ value, label }`，`modelValue` 是当前选中的 value。
 * 点**已经选中**的那格 = 取消选中（发出空串），也就是回到「两格都没选」的状态 ——
 * 「其他语言」这一行的两个选项都属于「可选开启的语言模式」，需要一个「都不开」的默认态，
 * 否则用户一旦点过就再也回不到默认显示。
 *
 * ⚠️ **当前没有任何地方引用本组件**（「其他语言」那一行用的是「罗马模式」「中文模式」
 * 两条独立 `CfgSwitch`；本组件留在仓库里备用，样式仍在全局 `design/config.css` 的 `.cfg-radio*` 里）。
 * 要用回来只需在 `ConfigView.vue` 里 `import CfgRadio from './CfgRadio.vue';`。
 */
const props = defineProps<{
  modelValue: string;
  options: { value: string; label: string }[];
  label?: string;
  tip?: string;
  disabled?: boolean;
  full?: boolean;
}>();

const emit = defineEmits<{
  'update:modelValue': [string];
}>();

function pick(value: string) {
  if (props.disabled) return;
  // 再点一次已选中的那格就取消选中（发出空串），见上面注释。
  emit('update:modelValue', value === props.modelValue ? '' : value);
}
</script>

<template>
  <div class="cfg-item cfg-item--radio" :class="{ 'cfg-item--full': full }" :title="tip || undefined">
    <span v-if="label" class="cfg-item__label">{{ label }}</span>
    <div class="cfg-item__control">
      <div class="cfg-radio" role="radiogroup">
        <button
          v-for="opt in options"
          :key="opt.value"
          type="button"
          class="cfg-radio__opt"
          :class="{ 'cfg-radio__opt--on': opt.value === modelValue }"
          role="radio"
          :aria-checked="opt.value === modelValue"
          :disabled="disabled"
          @click="pick(opt.value)"
        >
          {{ opt.label }}
        </button>
      </div>
    </div>
  </div>
</template>
