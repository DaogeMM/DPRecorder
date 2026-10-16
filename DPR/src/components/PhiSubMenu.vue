<script setup lang="ts">
/**
 * 三级菜单（2026-10-11 定稿形态：闸门行 + 齿轮弹窗）。
 *
 * 层级：一级 = PhiGroup（渲染 / 谱面 / 更多）、二级 = PhiSection（渲染参数…）、这里就是**三级**。
 * 一条「闸门行」= 条目名 + ✅/❌ 开关 + 一枚齿轮按钮；它自己的三级内容**不再就地展开**，
 * 而是收进齿轮点开的弹窗里。本形态的依据（2026-10-11）：
 *   - 三级内容放回原位，勾选框左侧放一枚齿轮图标，点击后弹出配置页面
 *     ⇒ 闸门行由父组件摆回它**原来在平铺网格里的位置**（不再是分区末尾的一条下拉栏），
 *     齿轮放在勾选框**左侧**，内容进弹窗。
 *   - 更早的样式要求仍然有效：「三级菜单的下拉栏 UI 设计和二级一样」（行高与内边距跟同级条目
 *     齐平）、「现在三级菜单标题不用粗体，和二级菜单下的选项文本一个样子」（15px / 400 / 纯白）。
 *     另有「三级菜单去掉上横线」，所以这里**不带** 0.5px 顶线；最后连标题前那根 3px 短竖条
 *     也删掉了（2026-10-11：去掉标题左侧那一竖）。
 *   - 「默认隐藏，左侧不显示三级菜单内容」⇒ `App.vue` 的左栏导航**不列**三级内容
 *     （所以不要往 renderGroups 里加）。弹窗默认关闭。
 *   - 「弹出来的窗口样式和配置页的内容区一样」⇒ 弹窗底色用内容区那块（`--phi-bg-base`）、
 *     卡片内边距清 0（让条目与配置页里同位置）、标题行复用闸门行那套外观（不用 `v-card-title`）。
 *   - 「宽度拉到内容宽度的90%」⇒ 宽度**不是**固定的：打开时量内容区 `<main class="phi-center">`
 *     的宽度取 90%（弹窗 teleport 到 body，`90%` 只会按视口算，会把左侧导航也算进去）。
 *   - 「去掉圆角，左上角文字放大，加点间隔，背景加点透明度」⇒ 卡片圆角清 0；标题放大到 1.35rem、
 *     上下留出间距；底色改成本地变量控制的半透明（见 `.phi-submenu__dialog`）。
 *   - 「去掉关闭按钮，只支持点击空白处关闭」⇒ 卡片底部那排 action 已删，靠 v-dialog 默认的
 *     点遮罩关闭（`Esc` 也仍能关）。
 *   - 「未启用时齿轮不显示」⇒ 开关关着、或所在二级分区关着时，齿轮连同弹窗入口一起不渲染。
 *   - 标题居中、去除左侧竖线 ⇒ 弹窗标题改**水平居中**，
 *     早先加的两根竖线（标题前 3×15px 短竖条 + body 左侧 1px 细导引线）**都删掉**。
 *
 * `id` 只用于定位/调试，不参与左栏滚动联动。
 */
import { onMounted, onUnmounted, ref } from 'vue';
import CfgSwitch from './CfgSwitch.vue';

const props = defineProps<{
  /** 三级菜单的总开关（即某个配置项，如 aggressiveChart） */
  modelValue: boolean;
  /** 标题（已翻译） */
  label: string;
  /** 悬停提示（已翻译）；留空则不加 title */
  tip?: string;
  /** 可选 DOM id（形如 `sec3-aggressive`） */
  id?: string;
  disabled?: boolean;
}>();

const emit = defineEmits<{
  'update:modelValue': [boolean];
}>();

/** 齿轮弹窗的开合。弹窗里的内容由父组件用默认插槽塞进来。 */
const dialog = ref(false);

/**
 * 弹窗宽度 = **内容区宽度的 60%**（一度是 90%，改成一行两个后太宽反而空，
 * 于是收到 60%）。
 * 弹窗会被 teleport 到 body，写百分比会按**视口**算（把左侧导航也算进去），所以这里在打开时量一下
 * 内容区 `<main class="phi-center">`（`App.vue` 里那层）的实际宽度，并在窗口缩放时实时更新；
 * 找不到它（组件被用在别的容器里）时退回视口宽度。下限 480px，免得窄窗口下弹窗挤成一条。
 */
const dialogWidth = ref(880);
function measureDialogWidth() {
  const content = document.querySelector<HTMLElement>('.phi-center');
  const base = content?.clientWidth || window.innerWidth;
  dialogWidth.value = Math.max(480, Math.round(base * 0.6));
}
function openDialog() {
  measureDialogWidth();
  dialog.value = true;
}
let resizeHandler: (() => void) | null = null;
onMounted(() => {
  resizeHandler = () => {
    if (dialog.value) measureDialogWidth();
  };
  window.addEventListener('resize', resizeHandler);
});
onUnmounted(() => {
  if (resizeHandler) window.removeEventListener('resize', resizeHandler);
});
</script>

<template>
  <div :id="id" class="phi-submenu" :class="{ 'phi-submenu--disabled': disabled }">
    <div class="phi-submenu__head" :title="tip || undefined">
      <span class="phi-submenu__title">{{ label }}</span>
      <v-spacer></v-spacer>
      <!-- 齿轮：打开这个三级菜单的配置弹窗。放在 ✅/❌ 开关**左边**（2026-10-11）。
           尺寸与它到开关的间距放大过（齿轮更大、间距更宽）：
           图标 18 → 22px、去掉 density="comfortable"（按钮盒子更大），间距靠 `.phi-submenu__gear`
           的 `margin-right` 给正边距（原来是负边距，用来抵消 v-btn 内边距贴紧开关）。
           **没启用就不显示**（2026-10-11：「未启用时齿轮不显示」）—— 开关关着、或所在二级
           分区关着，都没什么可配的。
           （曾有一个 `gearAlways` 例外给「其他语言」那行用；2026-10-11 那一行改成行内二选一框、
           直接显示中文/罗马 的单选框，没有齿轮了，例外已删。） -->
      <v-btn
        v-if="modelValue && !disabled"
        class="phi-submenu__gear"
        icon
        variant="text"
        size="small"
        :title="tip || label"
        @click="openDialog"
      >
        <v-icon size="22">mdi-cog-outline</v-icon>
      </v-btn>
      <CfgSwitch
        bare
        :disabled="disabled"
        :model-value="modelValue"
        @update:model-value="(value: boolean) => emit('update:modelValue', value)"
      />
    </div>

    <!-- 配置弹窗。弹窗会被 teleport 到 body，`config.css` 里的条目样式都挂在 `.config-root` 上，
         所以这里再包一层 `.config-root`，输入框外框 / 条目名纯白才和配置页一致。
         外观与配置页的内容区一致：底色用内容区那块
         （`--phi-bg-base`，而不是通用弹窗的 layer-2）、内边距不加、标题行照抄闸门行/分区标题；
         宽度是内容区的 60%（见 `dialogWidth`）；里面的条目排**一行两个**（普通配置页仍是一行三个，
         见下面 `.phi-submenu__dialog-body :deep(.cfg-grid)` 那段）。
         **没有关闭按钮**（只支持点击空白处关闭），
         靠 v-dialog 默认的「点遮罩关闭」。（`Esc` 也仍能关，要一并禁掉得加 `persistent`，
         那样连点空白处也关不掉了。） -->
    <v-dialog v-model="dialog" :width="dialogWidth">
      <v-card class="log-card-only-window phi-submenu__dialog">
        <div class="phi-submenu__head phi-submenu__dialog-head">
          <span class="phi-submenu__title">{{ label }}</span>
        </div>
        <v-card-text class="config-root phi-submenu__dialog-body">
          <slot></slot>
        </v-card-text>
      </v-card>
    </v-dialog>
  </div>
</template>

<style scoped>
/* 闸门行：现在是平铺网格里的一个普通条目（`.cfg-grid` 的一格），所以行高与内边距
   都对齐 `.cfg-item`（`config.css:59-66`：min-height 36px、padding 0 12px），
   只是里面多了一枚齿轮。 */
.phi-submenu {
  position: relative;
  min-width: 0;
}
.phi-submenu__head {
  display: flex;
  align-items: center;
  gap: var(--phi-space-2);
  min-height: 36px;
  padding: 0 var(--phi-space-3);
  user-select: none;
}

/* 标题：不加粗、字号与字色都跟「二级分区下的选项文本」一致
   —— 也就是 `.cfg-item__label` 的 16px（同一天从 15px 放大而来：「二级菜单下配置的文字
   放大点」），以及 `config.css:112-124` 给
   `.config-root` 里所有条目名/输入文字统一成的纯白（2026-10-11：
   「三级菜单标题不用粗体，和二级菜单下的选项文本一个样子」）。
   前面那根 3px 短竖条已删掉（2026-10-11：去掉标题左侧那一竖）。 */
.phi-submenu__title {
  display: inline-flex;
  align-items: center;
  gap: var(--phi-space-2);
  flex: 0 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 16px;
  font-weight: 400;
  line-height: 1.3;
  color: var(--phi-static-bluish-00);
}
/* 整块被禁用（如所在分区关着）时标题降为淡色，和 `config.css:135` 对条目名的处理一致 */
.phi-submenu--disabled .phi-submenu__title {
  color: var(--phi-label-tertiary);
}

/* 齿轮按钮：贴在开关左边，中间留出间距（见下面的 `margin-right`）。 */
.phi-submenu__gear {
  flex: none;
  /* 与开关拉开间距（2026-10-11：齿轮放大、与勾选框的间隔拉大）：
     原来是 -8px —— 用来抵消 v-btn 自带内边距、让齿轮贴紧开关；现在改成正的外边距，
     连上 `.phi-submenu__head` 的 `gap: var(--phi-space-2)` 一共 16px。 */
  margin-right: var(--phi-space-2);
  color: var(--phi-label-tertiary);
}
.phi-submenu__gear:hover {
  color: var(--phi-static-bluish-00);
}

/* 弹窗：外观与「配置页的内容区」一致（2026-10-11）。
   - 底色：`.phi-center`（内容区）用的是 `--phi-bg-base`，比 `.log-card-only-window` 默认的
     `--phi-bg-layer-2` 更暗 ⇒ 这里覆盖掉，让弹窗看起来就是内容区的一小块。
     （`.log-card-only-window` 那条在 `App.vue` 里带 `!important`，所以这里也得 `!important`；
     本组件 scoped 规则多一个 data-v 属性，同权重下胜出。）
     **带一点透明度**（2026-10-11）：`--phi-bg-base` 在深色主题就是
     `--phi-static-bluish-950` = `#151517` = `rgb(21 21 23)`，这里直接写它的分解值、留一个本地变量
     方便调（85–95% 之间比较合适；再低就会透出后面的配置页文字）。只有 DeepDark 一套主题，硬写没问题。
   - 圆角：v-dialog 会给里面那张卡 `border-radius: 4px`（`vuetify.css:21289-21296`），
     2026-10-11 去掉圆角 ⇒ 清 0。
   - 内边距：`v-card-text` 默认 16px 会把条目整体右移；内容区里条目由 `.cfg-grid` 的
     `padding: 8px 4px 12px` 定位，所以这里把卡片内边距清成 0，条目与配置页里**逐像素同位置**。
   - 标题行：直接复用闸门行那一套（标题前那根短竖条在闸门行里也已删掉），
     但字号放大、并留出上下间隔（2026-10-11：「左上角标题去掉左边的一竖，字号调大点」
     「左上角文字放大，加点间隔」），不用 Vuetify 的 `v-card-title`（它的字号/字重与内容区里的
     标题不是一套）。
   - 弹窗里没有关闭按钮（2026-10-11：只支持点击空白处关闭）⇒
     底下只剩内容，所以给内容补一点下内边距，免得最后一行贴着卡片边缘。 */
.phi-submenu__dialog {
  --phi-submenu-dialog-alpha: 88%;
  background: rgb(21 21 23 / var(--phi-submenu-dialog-alpha)) !important;
  border-radius: 0 !important;
  /* 卡片钉成整宽（2026-10-11：标题下的横线一度只画到 80% 左右）：
     `v-dialog` 的 `.v-overlay__content` 是 `position: absolute` 的块，卡片本身是它的
     普通块级子元素、按内容收缩，一旦内容比 `:width` 窄，卡片和标题行就都少一截、
     横线跟着只画到那里。显式 `width: 100%` 让卡片吃满 `:width` 给的宽度。 */
  width: 100%;
}
/* 标题行：现在**标题居中**（2026-10-11：去掉左侧竖线）。
   这一行是 flex（继承 `.phi-submenu__head`），里面只有标题这一个子元素，
   所以 `justify-content: center` 就是水平居中；左右内边距同为一档，
   居中不会偏（左缩进那条「标题左侧间隔再加120%」从此只影响居中块两侧的留白）。
   沿革（留着免得再走回头路）：
   - 底下曾有一条分隔横线（「配置菜单标题下面加个横向」→「缩短 10%」→「去除标题下横线」）；
   - 随后换成二级分区那套：标题前 3×15px 短竖条 + body 左侧 1px 细导引线
     （「配置页也改成二级菜单一样的标题格式，去除标题下横线，添加标题左竖线和细竖线」）；
   - 现在这两根竖线**都删掉**（「去除左侧竖线」）。字号单独放大过两次：
     「左上角标题…字号调大点」→ 1.35rem，2026-10-11「齿轮配置页标题放大点，向下移动一点」→ 1.5rem
     且标题行的上内边距从 `--phi-space-3`(12px) 加到 `--phi-space-3 + --phi-space-2`(20px)。
   注意 `-body` 的内边距仍是 0 —— 条目位置靠 `.cfg-grid` 自带的
   `8px 4px 12px` 撑，跟配置页里逐像素对齐（弹窗样式与配置页的内容区一致）。 */
.phi-submenu__dialog-head {
  /* 标题行同样钉成整宽（同在 `-dialog` 里的理由）。 */
  position: relative;
  width: 100%;
  justify-content: center;
  /* 上内边距 12px → 20px：2026-10-11 标题向下移动一点 */
  padding: calc(var(--phi-space-3) + var(--phi-space-2)) calc(var(--phi-space-3) * 2.2) var(--phi-space-2);
  cursor: default;
}
.phi-submenu__dialog-head .phi-submenu__title {
  /* 2026-10-11 标题再放大一档：1.35rem → 1.5rem */
  font-size: 1.5rem;
}
/* body：`position: relative` 原本是给那条细竖线（`::before`）当定位父级，
   那条线按「去除左侧竖线」删掉后一并去掉。 */
.phi-submenu__dialog-body {
  padding: 0 0 var(--phi-space-3);
}
/* 弹窗里的条目排**一行两个**（2026-10-11：宽度同步改成 60%）；
   普通配置页仍是一行三个 —— 全局规则在 `config.css:18-24`（`repeat(3, minmax(0, 1fr))`）。
   必须用 `:deep()`：插槽内容由父组件（`ConfigView.vue`）渲染，元素上挂的是父组件的 `data-v`，
   本组件的 scoped 选择器直接写 `.cfg-grid` 匹配不到。 */
.phi-submenu__dialog-body :deep(.cfg-grid) {
  grid-template-columns: repeat(2, minmax(0, 1fr));
}
/* 三栏时那两条竖线在 1/3、2/3 处，两栏只剩中间一条：`::after` 关掉，`::before` 挪到 50%
   （`column-gap` 为 0，所以中缝正好在 50%）。这里只改位置、不强制 `display: block`，
   这样 `config.css` 里「整行网格不画竖线」那条 `:has()` 规则仍然生效。 */
.phi-submenu__dialog-body :deep(.cfg-grid)::before {
  left: calc(var(--phi-space-1) + (100% - var(--phi-space-1) * 2) / 2);
}
.phi-submenu__dialog-body :deep(.cfg-grid)::after {
  display: none;
}
</style>
