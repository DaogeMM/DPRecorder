<script setup lang="ts">
// 调试文本的齿轮配置页：勾选调试文本里显示哪些项（「判定线调试」「按键调试」共用）。
//
// 配置里只存一个逗号名单，这里负责名单 ⇄ 勾选状态的换算：勾选后一律按 `fields` 的顺序重新拼串，
// 于是名单顺序与界面顺序一致，配置文件里也好读。每一项画成什么、哪些项只在长按上出现，都在 dps
// 侧决定：判定线见 `core::line` 里拼那段文本的地方，按键见 `core::note::debug_note_text`。
import { useI18n } from 'vue-i18n';

// `CfgSwitch` 必须显式 import：`<script setup>` 里没注册的 PascalCase 标签会被当成未知原生标签，
// `.cfg-item` 那三层壳就没了（同 `WaterScreenSettings.vue` 里的说明）。
import CfgSwitch from './CfgSwitch.vue';

const props = defineProps<{
  /** 允许出现在名单里的项，同时也是弹窗里勾选框的顺序。 */
  fields: readonly string[];
  /** 文案键前缀：条目是 `${prefix}-${name}`，说明是 `${prefix}-${name}-tip`（没有对应文案就不显示说明）。 */
  prefix: string;
}>();

const { t, te } = useI18n();

const fields = defineModel<string>({ required: true });

function tokens(value: string): string[] {
  return value
    .split(',')
    .map((it) => it.trim().toLowerCase())
    .filter((it) => it.length > 0);
}

function has(name: string): boolean {
  return tokens(fields.value).includes(name);
}

function toggle(name: string, on: boolean): void {
  const set = new Set(tokens(fields.value));
  if (on) {
    set.add(name);
  } else {
    set.delete(name);
  }
  // 落盘前只保留认识的项，并按 fields 的顺序输出。
  fields.value = props.fields.filter((it) => set.has(it)).join(',');
}

/** 只有真配了说明的项才挂 tooltip，免得缺文案时 i18n 报缺失键。 */
function tip(name: string): string | undefined {
  const key = `${props.prefix}-${name}-tip`;
  return te(key) ? t(key) : undefined;
}
</script>

<template>
  <div class="cfg-grid">
    <CfgSwitch
      v-for="name in props.fields"
      :key="name"
      :label="t(`${props.prefix}-${name}`)"
      :tip="tip(name)"
      :model-value="has(name)"
      @update:model-value="(value: boolean) => toggle(name, value)"
    />
  </div>
</template>
