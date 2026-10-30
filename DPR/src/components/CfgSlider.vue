<script setup lang="ts">
/**
 * 带数值读出条的配置条目：左边配置名、右边「拉动进度条 + 当前值」。
 *
 * 用于值域明确的数字配置（音量、0~1 的透明度/倍率、粒子限制、速度…），
 * 值域不明确或必须是精确输入的（总分、渲染起止时间）仍用文本框。
 */
const props = withDefaults(
  defineProps<{
    modelValue: number;
    label?: string;
    /** 标签右侧的小字（如换算出来的等效值），只占位、不参与交互 */
    note?: string;
    tip?: string;
    min?: number;
    max?: number;
    step?: number;
    disabled?: boolean;
    full?: boolean;
    /** 显示几位小数，默认按 step 推 */
    decimals?: number;
    /** 数值后面的单位（如 `%`），不传就不显示 */
    suffix?: string;
  }>(),
  { min: 0, max: 1, step: 0.01 },
);

const emit = defineEmits<{ 'update:modelValue': [number] }>();

function onInput(value: unknown) {
  const n = Number(Array.isArray(value) ? value[0] : value);
  if (Number.isFinite(n)) emit('update:modelValue', n);
}

function text(): string {
  const d = props.decimals ?? (props.step < 0.1 ? 2 : 0);
  return Number(props.modelValue ?? 0).toFixed(d);
}
</script>

<template>
  <div class="cfg-item" :class="{ 'cfg-item--full': full }" :title="tip || undefined">
    <span v-if="label" class="cfg-item__label">{{ label }}</span>
    <span v-if="note" class="cfg-slider__note">{{ note }}</span>
    <div class="cfg-item__control">
      <v-slider
        class="cfg-slider"
        :model-value="modelValue"
        :min="min"
        :max="max"
        :step="step"
        :disabled="disabled"
        color="btn"
        hide-details
        density="compact"
        thumb-size="14"
        @update:model-value="onInput"
      ></v-slider>
      <span class="cfg-slider__value">{{ text() }}<template v-if="suffix">{{ suffix }}</template></span>
    </div>
  </div>
</template>

<style scoped>
/* 标签右侧的换算值：小一号、次要色，贴在标签后面，不挤压滑条（滑条仍占 40%） */
.cfg-slider__note {
  flex: none;
  margin-left: var(--phi-space-2);
  font-size: 12px;
  color: var(--phi-label-tertiary);
  font-variant-numeric: tabular-nums;
}
</style>
