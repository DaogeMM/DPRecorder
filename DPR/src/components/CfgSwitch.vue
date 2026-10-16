<script setup lang="ts">
/**
 * 方形开关（取代勾选框）：左边打勾、右边打叉的两格式控件。
 *
 * 刻意不用 Vuetify 的 `v-switch`：它的轨道是胶囊形。本控件要直角、
 * 并且做成「左边 ✅ 右边 ❌ 的那种」，所以这里是自己写的 `<button role="switch">`：
 * 左右各占一半，选中那半填色高亮（开=左格勾、关=右格叉），外观全在全局 `.cfg-switch*` 里。
 *
 * 根元素额外挂两个类给全局样式用（`design/config.css`）：
 *   - `cfg-item--switch`：标记「这是个 ✅/❌ 开关条目」（记号配色的规则挂在这一节里）；
 *   - `cfg-item--bare`：`bare` 时不画条目名、不留行高，只给开关本体
 *     （三级菜单 PhiSubMenu 的标题行要把开关摆在标题右边）。
 */
const props = defineProps<{
  modelValue: boolean;
  label?: string;
  tip?: string;
  disabled?: boolean;
  full?: boolean;
  /** 只渲染开关本体，不渲染条目名（给 PhiSubMenu 的标题行用） */
  bare?: boolean;
  /** 半尺寸开关（96×42 → 48×21）。给二级分区 PhiSection 的标题行用 ——
   *  2026-10-11：勾选框放到二级菜单标题右边，留一点间隙，尺寸缩小 50%。 */
  small?: boolean;
}>();

const emit = defineEmits<{
  'update:modelValue': [boolean];
  change: [];
}>();

function toggle() {
  if (props.disabled) return;
  emit('update:modelValue', !props.modelValue);
  emit('change');
}
</script>

<template>
  <div
    class="cfg-item cfg-item--switch"
    :class="{ 'cfg-item--full': full, 'cfg-item--bare': bare }"
    :title="tip || undefined"
  >
    <span v-if="label && !bare" class="cfg-item__label">{{ label }}</span>
    <div class="cfg-item__control">
      <button
        type="button"
        class="cfg-switch"
        :class="{ 'cfg-switch--on': modelValue, 'cfg-switch--small': small }"
        role="switch"
        :aria-checked="modelValue"
        :disabled="disabled"
        @click="toggle"
      >
        <span class="cfg-switch__opt cfg-switch__opt--yes"><v-icon icon="mdi-check" :size="small ? 12 : 18" /></span>
        <span class="cfg-switch__opt cfg-switch__opt--no"><v-icon icon="mdi-close" :size="small ? 12 : 18" /></span>
      </button>
    </div>
  </div>
</template>
