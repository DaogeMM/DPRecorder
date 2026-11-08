<script setup lang="ts">
import { computed, ref, watch } from 'vue';

/**
 * 右栏里的一行参数：标题 + 可手输的数字 + 滑杆。
 *
 * 数字改成手输后要容忍打到一半的值（`-`、`.`、空），所以输入框自己留一份草稿，
 * 失焦或回车时才抛给父级；父级回写的值再同步回来。
 */
const props = withDefaults(
  defineProps<{
    label: string;
    modelValue: number;
    min: number;
    max: number;
    step?: number;
    /** 小数位数。 */
    digits?: number;
    /** 百分比显示与输入：内部值 0~1，界面 0~100。 */
    percent?: boolean;
    /** 数字后面的单位，例如角度的 `°`。 */
    unit?: string;
    /**
     * 批量选中时这一项在几段之间不一致：数字框留空、显示「多个值」，
     * 改动一下就把同一个值写到每一段上。
     */
    mixed?: boolean;
    /** 「多个值」的文案，由父级按语言给。 */
    mixedText?: string;
    /** 这一项当前不生效（例如「融入原谱」时自带的颜色与材质都不写），整行压暗且不可改。 */
    disabled?: boolean;
  }>(),
  { step: 1, digits: 2, percent: false, unit: '', mixed: false, mixedText: '', disabled: false },
);

const emit = defineEmits<{ 'update:modelValue': [value: number] }>();

/** 显示值：百分比先 ×100，再按位数收掉浮点尾巴。 */
const text = computed(() => {
  const value = props.percent ? props.modelValue * 100 : props.modelValue;
  const factor = 10 ** props.digits;
  return String(Math.round(value * factor) / factor);
});

const draft = ref(text.value);
watch(text, (value) => {
  draft.value = value;
});

function commit() {
  if (props.disabled) return;
  // 批量选中时框里本来是空的，没打字就失焦不该把值改成 0
  if (props.mixed && draft.value.trim() === '') return;
  const parsed = Number(draft.value);
  if (!Number.isFinite(parsed)) {
    draft.value = text.value;
    return;
  }
  const value = props.percent ? parsed / 100 : parsed;
  const clamped = Math.min(props.max, Math.max(props.min, value));
  const rounded = Math.round(clamped * 10 ** props.digits) / 10 ** props.digits;
  draft.value = String(props.percent ? Math.round(rounded * 100 * 10 ** props.digits) / 10 ** props.digits : rounded);
  if (rounded !== props.modelValue) emit('update:modelValue', rounded);
}

function onSlide(value: number | number[]) {
  if (props.disabled) return;
  emit('update:modelValue', Number(value));
}
</script>

<template>
  <div class="srow" :class="{ 'srow--mixed': mixed, 'srow--off': disabled }">
    <div class="srow__head">
      <span class="srow__label">{{ label }}</span>
      <span class="srow__num-wrap">
        <input
          class="srow__num"
          type="number"
          inputmode="decimal"
          :min="percent ? 0 : min"
          :max="percent ? 100 : max"
          :step="percent ? 1 : step"
          :value="mixed ? '' : draft"
          :placeholder="mixed ? mixedText : ''"
          :disabled="disabled"
          @input="draft = ($event.target as HTMLInputElement).value"
          @change="commit"
          @blur="commit"
          @keydown.enter="($event.target as HTMLInputElement).blur()"
        />
        <span v-if="percent || unit" class="srow__unit">{{ percent ? '%' : unit }}</span>
      </span>
    </div>
    <v-slider
      :model-value="modelValue"
      :min="min"
      :max="max"
      :step="step"
      :disabled="disabled"
      density="compact"
      hide-details
      color="primary"
      @update:model-value="onSlide"
    ></v-slider>
  </div>
</template>

<style scoped>
/* 一栏就 300px 宽：让这一行能被父级压窄，别把面板撑出横向滚动条 */
.srow {
  min-width: 0;
}

.srow :deep(.v-slider) {
  min-width: 0;
}

.srow__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--phi-space-2);
}

.srow__label {
  font-size: 0.68rem;
  color: var(--phi-label-caption);
}

.srow__num-wrap {
  display: inline-flex;
  align-items: baseline;
  gap: 1px;
}

/* 数字本身就是输入框：点一下就能改，不弹对话框 */
.srow__num {
  width: 44px;
  padding: 0 2px;
  border: 0;
  border-radius: 2px;
  background: transparent;
  color: var(--phi-label-primary);
  font-family: inherit;
  font-size: 0.68rem;
  font-variant-numeric: tabular-nums;
  text-align: right;
  outline: none;
}

.srow__num:hover {
  background: var(--phi-interactive-hover);
}

.srow__num:focus {
  background: var(--phi-bg-layer-3);
}

/* 批量选中且这一项不一致：占位文案要能顶满小框，滑杆压暗一点表示不是同一个值 */
.srow--mixed .srow__num {
  width: 62px;
}

.srow--mixed :deep(.v-slider) {
  opacity: 0.55;
}

/* 这一项当前不生效：整行压暗，滑杆与数字框都变只读 */
.srow--off {
  opacity: 0.45;
}

.srow--off .srow__num:hover {
  background: transparent;
}

.srow__num::placeholder {
  color: var(--phi-label-caption);
  opacity: 1;
}

/* 数字框去掉浏览器自带的上下箭头，靠滑杆调 */
.srow__num::-webkit-outer-spin-button,
.srow__num::-webkit-inner-spin-button {
  margin: 0;
  -webkit-appearance: none;
}

.srow__unit {
  font-size: 0.64rem;
  color: var(--phi-label-caption);
}
</style>
