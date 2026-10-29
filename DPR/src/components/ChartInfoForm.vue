<script setup lang="ts">
// 谱面信息的可编辑表单：批量列表展开后那张卡片的下半部分是它。
//
// 字段与渲染页（`RenderView.vue` 的 `#chart-info` 插槽）逐条对齐 ——
// 曲名 / 曲师 / 谱师 / 画师 / 谱面文件 / 音乐文件 / 曲绘文件 / 难度 / 定数，
// 然后简介与 Tip 各占一整行。**没有** 偏移 / 总分：那两个是渲染参数，
// 批量页统一用预设里的那份配置。
//
// 为什么单独一个组件：列表里每张卡片各有一份表单，而「定数」输入框用的是
// `useLvlField()`（带自己的字符串模型），它只能在组件里各建一份，不能塞进 `v-for`。
// 校验也由组件自己给出（`validate()`），调用方渲染前校验**那一张**卡片时再取。
import { ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { VForm } from 'vuetify/components';

import { RULES } from '../common';
import { useLvlField } from '../lvl';
import type { ChartInfo } from '../model';

import CfgField from './CfgField.vue';

const props = defineProps<{ info: ChartInfo }>();

const { t } = useI18n();

const form = ref<VForm>();

// 定数显示一律补成一位小数（见 `../lvl.ts`），输入过程仍然随手打，数值照旧写回这条谱面
const { text: lvlText, normalize: normalizeLvl } = useLvlField(
  () => props.info.lvl,
  (value) => {
    props.info.lvl = value;
  },
);

defineExpose({
  /** 交回 `v-form.validate()` 的结果；调用方据此决定要不要继续渲染 */
  validate: () => form.value?.validate(),
});
</script>

<template>
  <v-form ref="form" validateOn="eager">
    <div class="cfg-grid cfg-grid--two">
      <CfgField :label="t('info.name')">
        <v-textarea rows="1" auto-grow density="compact" hide-details="auto" v-model="info.name"></v-textarea>
      </CfgField>
      <CfgField :label="t('info.composer')">
        <v-textarea rows="1" auto-grow density="compact" hide-details="auto" v-model="info.composer"></v-textarea>
      </CfgField>
      <CfgField :label="t('info.charter')">
        <v-textarea rows="1" auto-grow density="compact" hide-details="auto" v-model="info.charter"></v-textarea>
      </CfgField>
      <CfgField :label="t('info.illustrator')">
        <v-textarea rows="1" auto-grow density="compact" hide-details="auto" v-model="info.illustrator"></v-textarea>
      </CfgField>
      <!-- 谱面文件 / 音乐文件 / 曲绘文件：与渲染页一样就是可编辑的文本框（那边也没做选择器） -->
      <CfgField :label="t('info.chart')">
        <v-text-field type="text" density="compact" hide-details="auto" v-model="info.chart"></v-text-field>
      </CfgField>
      <CfgField :label="t('info.music')">
        <v-text-field type="text" density="compact" hide-details="auto" v-model="info.music"></v-text-field>
      </CfgField>
      <CfgField :label="t('info.illustration')">
        <v-text-field type="text" density="compact" hide-details="auto" v-model="info.illustration"></v-text-field>
      </CfgField>
      <CfgField :label="t('info.difficulty')">
        <v-textarea rows="1" auto-grow density="compact" hide-details="auto" v-model="info.difficulty"></v-textarea>
      </CfgField>
      <CfgField :label="t('info.lvl')">
        <!-- 定数可以是 0 / 空（官谱里常见 `Lv.0.0`、也可能没写）：只拦负数和乱写，不拦空 -->
        <v-text-field type="number" density="compact" hide-details="auto" :rules="[RULES.positiveOrNull]" v-model="lvlText" @blur="normalizeLvl"></v-text-field>
      </CfgField>
    </div>
    <!-- 简介 与 Tip：都是 `full` 条目 ⇒ 单开一个网格，`:has()` 规则会去掉栏间竖线 -->
    <div class="cfg-grid">
      <CfgField :label="t('info.intro')" full>
        <v-text-field type="text" density="compact" hide-details="auto" v-model="info.intro"></v-text-field>
      </CfgField>
      <CfgField :label="t('info.tip')" full>
        <v-textarea rows="1" auto-grow density="compact" hide-details="auto" :placeholder="t('info.tip-placeholder')" v-model="info.tip"></v-textarea>
      </CfgField>
    </div>
  </v-form>
</template>
