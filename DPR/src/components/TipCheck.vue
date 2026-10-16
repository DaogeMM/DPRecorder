<script setup lang="ts">
defineProps<{ label: string; tooltip?: string; color?: string }>();
</script>

<template>
  <!-- 图标用 vuetify 默认的 $checkboxOff / $checkboxOn，与直接写 <v-checkbox> 的地方保持一致；
       原先自定义 false-icon="mdi-close" 会让未勾选项显示成一个叉，和左边的勾选框格式不统一。
       根元素也不要 mx-2 + d-flex justify-center —— 那会让勾选框在格子里居中，
       和左边（<v-col class="px-2"> 里左对齐）的勾选框错位；留边交给外层的 v-col。

       `color` 默认 'btn'（= 主题里那个紫蓝色），**必须**给上：
       v-checkbox 不给 color 时会退回 vuetify 的 primary，勾出来的颜色和
       「渲染项目」那批手写的 <v-checkbox color="btn"> 对不上，页面上会出现两种勾选色。
       已经显式传 color="btn" 的调用点不受影响。 -->
  <v-checkbox class="whitespace-pre" :color="color ?? 'btn'" :title="tooltip">
    <template v-slot:label>{{ label }}</template>
  </v-checkbox>
</template>
