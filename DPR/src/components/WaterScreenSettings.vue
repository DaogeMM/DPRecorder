<script setup lang="ts">
// 「屏幕进水」设置面板。
//
// 整块抽到这里方便单独维护：面板的 12 个值由父组件 ConfigView 的 ref 通过 v-model 提供，
// 读写与保存仍走 ConfigView 的 applyConfig / buildConfig。
import { useI18n } from 'vue-i18n';

const { t } = useI18n();

import { RULES } from '../common';
// 这个**必须显式 import**：`<script setup>` 里没注册的 PascalCase 标签不会被当成组件，
// Vue 会按未知的原生标签渲染 —— 那样 `.cfg-item` / `.cfg-item__label` / `.cfg-item__control`
// 三层壳都没了，整块只剩一排光秃秃的输入框。
// （「屏幕进水」那个开关画在分区标题行 `PhiSection` 的 `v-model:gate` 上，所以本组件只需要 CfgField。）
import CfgField from './CfgField.vue';
// 水的透明度 / 音效音量用数值输入框（`CfgSlider` 已不再使用，所以不 import）。

const waterScreen = defineModel<boolean>('waterScreen', { required: true });
const waterOpacity = defineModel<number>('waterOpacity', { required: true });
const waterFlowSpeed = defineModel<string>('waterFlowSpeed', { required: true });
const waterCellScale = defineModel<string>('waterCellScale', { required: true });
const waterHp = defineModel<string>('waterHp', { required: true });
const waterDrownDelay = defineModel<string>('waterDrownDelay', { required: true });
const waterDamage = defineModel<string>('waterDamage', { required: true });
const waterDamageInterval = defineModel<string>('waterDamageInterval', { required: true });
const waterDeathTime = defineModel<string>('waterDeathTime', { required: true });
const waterDeathParticles = defineModel<string>('waterDeathParticles', { required: true });
const waterSfxVolume = defineModel<number>('waterSfxVolume', { required: true });
const waterSfxMax = defineModel<string>('waterSfxMax', { required: true });
</script>

<template>
  <div class="cfg-grid">
    <!-- 「屏幕进水」总开关在二级分区标题行（`PhiSection` 的 `v-model:gate`），
         关掉时整栏收起，所以这里不重复画一个。`waterScreen` 仍由父组件 v-model 传进来，
         下面 11 个输入框的 `:disabled="!waterScreen"` 保留（栏都收起来了，它们不会露脸）。 -->
    <CfgField :label="t('water-opacity')"><v-text-field density="compact" hide-details="auto" :disabled="!waterScreen" :title="t('water-opacity-tip')" v-model="waterOpacity" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
    <CfgField :label="t('water-flow-speed')"><v-text-field density="compact" hide-details="auto" :disabled="!waterScreen" :title="t('water-flow-speed-tip')" v-model="waterFlowSpeed" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
    <CfgField :label="t('water-cell-scale')"><v-text-field density="compact" hide-details="auto" :disabled="!waterScreen" :title="t('water-cell-scale-tip')" v-model="waterCellScale" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
    <CfgField :label="t('water-hp')"><v-text-field density="compact" hide-details="auto" :disabled="!waterScreen" :title="t('water-hp-tip')" v-model="waterHp" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
    <CfgField :label="t('water-drown-delay')"><v-text-field density="compact" hide-details="auto" :disabled="!waterScreen" :title="t('water-drown-delay-tip')" v-model="waterDrownDelay" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
    <CfgField :label="t('water-damage')"><v-text-field density="compact" hide-details="auto" :disabled="!waterScreen" :title="t('water-damage-tip')" v-model="waterDamage" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
    <CfgField :label="t('water-damage-interval')"><v-text-field density="compact" hide-details="auto" :disabled="!waterScreen" :title="t('water-damage-interval-tip')" v-model="waterDamageInterval" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
    <CfgField :label="t('water-death-time')"><v-text-field density="compact" hide-details="auto" :disabled="!waterScreen" :title="t('water-death-time-tip')" v-model="waterDeathTime" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
    <CfgField :label="t('water-death-particles')"><v-text-field density="compact" hide-details="auto" :disabled="!waterScreen" :title="t('water-death-particles-tip')" v-model="waterDeathParticles" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
    <CfgField :label="t('water-sfx-volume')"><v-text-field density="compact" hide-details="auto" :disabled="!waterScreen" :title="t('water-sfx-volume-tip')" v-model="waterSfxVolume" type="number" :rules="[RULES.notEmpty, RULES.positiveOrZero]"></v-text-field></CfgField>
    <CfgField :label="t('water-sfx-max')"><v-text-field density="compact" hide-details="auto" :disabled="!waterScreen" :title="t('water-sfx-max-tip')" v-model="waterSfxMax" type="number" :rules="[RULES.notEmpty, RULES.positive]"></v-text-field></CfgField>
  </div>
</template>
