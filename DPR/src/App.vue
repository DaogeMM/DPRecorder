<script lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { useRoute, useRouter } from 'vue-router';

import { useI18n } from 'vue-i18n';

import { VSonner } from 'vuetify-sonner';
import { invoke,   } from '@tauri-apps/api/core';
import * as os from "@tauri-apps/plugin-os"
// 打开外部链接一律走 `./common` 的 `openExternal`（它转给 Rust 的 `open_external`）。
// 不再用 `@tauri-apps/plugin-opener` 的 `openUrl`：它在 Windows 上从 Tauri 命令线程调
// ShellExecuteExW 时缺 `SEE_MASK_NOASYNC`，启动请求被静默丢弃 ——
// 表现为点击链接无法唤醒浏览器。
import { clamp, useStorage } from '@vueuse/core';

const onLoaded = ref<() => void>();
const component = ref();

watch(component, (comp) => {
  if (comp && onLoaded.value) onLoaded.value();
});

export function useOnLoaded() {
  return onLoaded;
}

declare global {
  interface Window {
    goto: (name: string) => void;
  }
}

export default {
  data() {
    return {};
  },
};
</script>

<script setup lang="ts">
import { fetch } from '@tauri-apps/plugin-http';
import type { Release, Assets, Task } from './model';
import { open } from '@tauri-apps/plugin-shell';
import { useTheme } from 'vuetify';
import { getCurrentWindow, ProgressBarStatus } from '@tauri-apps/api/window';
import { toast, openExternal, changeLocale } from './common';
import { applyPhiTheme } from './design/theme';
import { sectionGateState } from './config/sectionGate';
import TaskStrip from './components/TaskStrip.vue';
import { useTasks } from './tasks';
import { pageReady } from './pageLoad';
const appWindow = getCurrentWindow();
const theme = useTheme();

// 批量页的谱面列表、所选预设，连同弹窗里改过的谱面信息都要作为界面记忆留下来
// （存储本身是 `BatchView.vue` 里的 `useStorage`），所以这里不能主动 removeItem ——
// 启动时删掉它们会让这些内容一关程序就没了。

let count = 0;
let timer: number | null = null;

document.addEventListener('contextmenu', (e) => {
  count++;
  if (timer) {
    window.clearTimeout(timer);
  }

  timer = window.setTimeout(() => {
    count = 0;
    timer = null;
  }, 2000);

  if (count >= 7) {
    count = 0;
    let rand = Math.floor(Math.random() * 102); // 0 - 101
    toast(`${t('luck-value')}: ${rand}`, 'info');
    rand = clamp(rand, 0, 100);
    let zoomValue = 300 - rand * 2;
    let blurValue = (100 - rand) * 0.02;
    let hueValue = 270 - rand * 2.70;
    let saturateValue = rand;
    let contrastValue = 150 - rand * 0.5;
    document.body.style.zoom = `${zoomValue}%`;
    document.body.style.filter = `blur(${blurValue}px) hue-rotate(${hueValue}deg) saturate(${saturateValue}%) contrast(${contrastValue}%`;
    
    if (rand === 0) {
      document.body.classList.add('shake-distort');
      open(t('rick-roll'));
    }
  }
}, { passive: true });

const { t } = useI18n();

const route = useRoute(),
  router = useRouter();

function routerPush(name: string) {
  router.push({ name });
}

/**
 * 外壳导航。
 * primaryNav = 一级区域（对应路由）；renderHero / renderGroups = 渲染页左栏的两级子项。
 * 子项的 id 就是渲染页里锚点元素的 DOM id（<PhiGroup> 是 grp-xxx、<PhiSection> 是 sec-xxx），
 * 找不到锚点就退化为回到顶部，不会报错。
 */
const primaryNav = [
  { name: 'render', icon: 'mdi-auto-fix' },
  { name: 'batch', icon: 'mdi-filmstrip-box-multiple' },
  { name: 'tasks', icon: 'mdi-server' },
];

// 「设置」固定在左栏最下面（和上面几个一级区域分开，中间用弹性空白撑开）
const settingsNav = [{ name: 'settings', icon: 'mdi-cog' }];

/**
 * 设置页里「显示调试」那一条的入口开关：默认藏着，只有 **Ctrl+Shift+点击左栏的「设置」** 才切换。
 * 键与 `SettingsView.vue` 共用。两处各自持有一份 `useStorage` 引用、互相不同步，
 * 所以这里以 localStorage 里的值为准来取反（设置页是导航后才挂载的，挂载时读到的就是新值）。
 */
const debugRevealed = useStorage<boolean>('phi.debug.revealed', false);
function onSettingsClick(name: string, event: MouseEvent) {
  if (event.ctrlKey && event.shiftKey) {
    const next = localStorage.getItem('phi.debug.revealed') !== 'true';
    localStorage.setItem('phi.debug.revealed', String(next));
    debugRevealed.value = next;
  }
  routerPush(name);
}

/*
 * 渲染页左栏分两级：
 *   一级 = renderGroups 里的组，对应 ConfigView 里的 <PhiGroup>，DOM id 就是 grp-xxx；
 *   二级 = 组里的叶子，对应 <PhiSection>，DOM id 是 sec-xxx。
 * 「谱面信息」（页面最上面的曲绘横幅）不属于任何组，单独排在最前面。
 * renderOrder 是「按页面顺序拍平」的结果，滚动联动高亮按它遍历 ——
 * 顺序必须与 ConfigView 模板里各块先后一致（取「最后一个 top ≤ 判定线」的那一项）。
 */
interface RenderLeaf {
  id: string;
  label: string;
  icon: string;
}
interface RenderGroup extends RenderLeaf {
  children: RenderLeaf[];
}

const renderHero: RenderLeaf = { id: 'sec-chart-info', label: 'title.chart-info', icon: 'mdi-image-outline' };

const renderGroups: RenderGroup[] = [
  {
    id: 'grp-render',
    label: 'title.render-group',
    icon: 'mdi-export-variant',
    children: [
      // 「预设配置」是本组第一条（顺序 = 页面顺序）
      { id: 'sec-preset', label: 'presets', icon: 'mdi-bookmark-outline' },
      // 资源包排在预设配置下面（顺序 = 页面顺序）
      { id: 'sec-respack', label: 'respack', icon: 'mdi-package-variant' },
      // 「画面参数」与「渲染参数」整合成这一条「参数」，
      // 没有 sec-output / sec-performance 这两个 id
      { id: 'sec-params', label: 'title.params', icon: 'mdi-tune-variant' },
      { id: 'sec-audio', label: 'title.audio', icon: 'mdi-music-note' },
      { id: 'sec-player', label: 'title.player', icon: 'mdi-account' },
      // 多语言在渲染-更多里，做成三级菜单行（本组末尾这个二级分区「更多」里）
      { id: 'sec-more-render', label: 'more', icon: 'mdi-dots-horizontal' },
    ],
  },
  {
    id: 'grp-chart',
    label: 'title.chart',
    icon: 'mdi-music-circle-outline',
    children: [
      { id: 'sec-render-item', label: 'render-item', icon: 'mdi-layers-outline' },
      { id: 'sec-chart-bg', label: 'title.background', icon: 'mdi-image-outline' },
      { id: 'sec-chart-key', label: 'title.keys', icon: 'mdi-music-note' },
      // 判定线分区紧跟在按键之后（顺序 = 页面顺序）
      { id: 'sec-chart-line', label: 'title.judge-line', icon: 'mdi-minus' },
      // 噪域排在按键下面：顺序 = 页面顺序，必须与 ConfigView 模板一致
      { id: 'sec-block-area', label: 'title.block-area', icon: 'mdi-blur' },
      // 「判定」与「谱面进水」都在本组末尾的「更多」分区里，不单列
      { id: 'sec-more-chart', label: 'more', icon: 'mdi-dots-horizontal' },
      // 本组最后的「谱面·调试」不列进左栏：它默认整块关着（纯界面总开关），只在页面上留一行标题。
    ],
  },
];

// 页面顺序：hero → 每个组 → 组里的叶子
const renderOrder: RenderLeaf[] = [renderHero, ...renderGroups.flatMap((g) => [g as RenderLeaf, ...g.children])];

const railCollapsed = useStorage<boolean>('navRailCollapsed', false);
const activeSection = ref(renderHero.id);

// 左栏里每个组的展开状态（默认展开，localStorage 记忆）
const navGroupOpen = useStorage<Record<string, boolean>>('navGroupOpen', {});
function isGroupOpen(id: string) {
  return navGroupOpen.value[id] !== false;
}
function toggleGroup(id: string) {
  navGroupOpen.value = { ...navGroupOpen.value, [id]: !isGroupOpen(id) };
  scrollToSection(id);
}

/**
 * 左栏宽度可以拖动右边缘调整。
 * 拖动期间加 .phi-nav--resizing 关掉 width 过渡，否则跟手会有一拍延迟。
 */
const NAV_MIN_WIDTH = 200;
const NAV_MAX_WIDTH = 520;
const navWidth = useStorage<number>('navWidth', 280);
const navResizing = ref(false);

function startNavResize(e: PointerEvent) {
  if (railCollapsed.value) return;
  const startX = e.clientX;
  const startWidth = navWidth.value;
  navResizing.value = true;
  document.body.style.cursor = 'col-resize';

  const onMove = (ev: PointerEvent) => {
    const next = startWidth + (ev.clientX - startX);
    navWidth.value = Math.min(NAV_MAX_WIDTH, Math.max(NAV_MIN_WIDTH, Math.round(next)));
  };
  const onUp = () => {
    navResizing.value = false;
    document.body.style.cursor = '';
    window.removeEventListener('pointermove', onMove);
    window.removeEventListener('pointerup', onUp);
    window.removeEventListener('pointercancel', onUp);
  };

  window.addEventListener('pointermove', onMove);
  window.addEventListener('pointerup', onUp);
  window.addEventListener('pointercancel', onUp);
}

// 收起态由 .phi-nav--collapsed 的固定轨道宽度接管，不能再给内联 width
const navStyle = computed(() => (railCollapsed.value ? undefined : { width: `${navWidth.value}px` }));

/** 左栏里的 id 就是 ConfigView 里锚点元素的 DOM id（sec-xxx / grp-xxx），直接用 */
function anchorEl(id: string) {
  return document.getElementById(id);
}

// 点击左栏子项后短暂屏蔽滚动联动，否则平滑滚动途中高亮会被沿途的分区抢走
let spyMuteUntil = 0;

function scrollToSection(id: string) {
  activeSection.value = id;
  spyMuteUntil = Date.now() + 900;
  const el = anchorEl(id);
  if (el) {
    el.scrollIntoView({ behavior: 'smooth', block: 'start' });
    return;
  }
  document.querySelector('.render-view__scroll')?.scrollTo({ top: 0, behavior: 'smooth' });
}

/**
 * 滚动联动高亮。渲染页的滚动容器由 RenderView 提供（.render-view__scroll），
 * 所以监听 document 捕获阶段的 scroll，不依赖具体是哪个容器在滚。
 */
let spyFrame = 0;
function onAnyScroll() {
  if (Date.now() < spyMuteUntil) return;
  if (spyFrame) return;
  spyFrame = requestAnimationFrame(() => {
    spyFrame = 0;
    if (route.name !== 'render') return;
    const scroller = document.querySelector('.render-view__scroll');
    if (!scroller) return;
    const line = scroller.getBoundingClientRect().top + 28;
    let current = renderOrder[0]!.id;
    for (const s of renderOrder) {
      const el = anchorEl(s.id);
      if (el && el.getBoundingClientRect().top <= line) current = s.id;
    }
    activeSection.value = current;
  });
}

window.goto = (name: string) => {
  router.push({ name });
};

function appClose() {
  appWindow.close();
}

function appMinimize() {
  appWindow.minimize();
}

function appMaximize() {
  appWindow.toggleMaximize();
}

// 主题只有一套：DeepDark（深色），设置页里也没有主题选项，所以这里没有切换逻辑 ——
// Light / Dark / LightBlue、F2 轮换与「跟随系统主题」都不存在。
theme.global.name.value = 'DeepDark';
// 同步设计令牌层：tokens.css 按 body[data-phi-theme] 切换
applyPhiTheme();



// FFmpeg
const platform = os.family();
const isWindows = String(platform) === 'windows';
const isLinux = String(platform) === 'linux';
const ffmpegGetNewVersionLoding = ref(false);
const ffmpegDialog = ref(false);
const ffmpegDialogFilter = ref(false);

async function getNewVersion() {
  //dialog_download.value = true;
  ffmpegGetNewVersionLoding.value = true;
  try {
    const response = await fetch('https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/latest', {
      method: 'GET',
      headers: {
        Accept: 'application/vnd.github+json',
        'User-Agent': 'DPR',
        'X-GitHub-Api-Version': '2022-11-28'
      }
    });
    const release = await response.json() as Release;
    if (!release) {
      throw new Error('No tags found');
    }

    const assets = release.assets as Assets[];
    if (assets.length === 0) {
      throw new Error('No assets found');
    }
    const asset = assets.find((asset) => {
      if (isWindows) {
        return asset.name.includes('ffmpeg-master-latest-win64-gpl.zip');
      } else if (isLinux) {
        return asset.name.includes('ffmpeg-master-latest-linux64-gpl.tar.xz');
      }
      return false;
    })

    const link = (asset as Assets).browser_download_url;
    console.log(link);
    await openExternal(link);
    
  } catch (error) {
    console.error('Error fetching tags:', error);
    await openExternal("https://github.com/BtbN/FFmpeg-Builds/releases");
  } finally {
    ffmpegGetNewVersionLoding.value = false;
  }
}

async function openAppFolder() {
  await invoke('open_app_folder');
}

async function checkFFmpegFilter() {
  try {
    if (!(await invoke('check_ffmpeg'))) {
      ffmpegDialog.value = true;
      return false;
    }
    ffmpegDialogFilter.value = !(await invoke("check_ffmpeg_filter", { path: null }));
    } catch (error) {
      console.error('Error running check_ffmpeg_filter:', error);
    }
}

/** 界面语言：`main.ts` 建 i18n 时已按存储设过一次；这里挂载时再对齐一次 —— HMR 与模块
 *  重载可能让 vue-i18n 停在旧语言上（设置页显示简体中文、界面却还是英文）。 */
const locale = useStorage<string>('locale', 'zh-CN');

onMounted(async () => {
  changeLocale(locale.value);
  checkFFmpegFilter();

  document.addEventListener('scroll', onAnyScroll, true);
});

/** 渲染弹窗（`components/TaskStrip.vue`）与窗口任务栏进度都盯着这一份全局任务列表 */
const tasks = useTasks();

// 任务栏进度：和渲染弹窗同源 —— 共用这一份全局任务列表，列表每 700ms 更新一次就跟着更新，
// 不再另开 `get_tasks` 轮询。
watch(tasks, async () => {
  try {
    const activeTask = tasks.value?.find(t => ['loading', 'mixing', 'mixing_sfx', 'rendering'].includes(t.status.type));
    if (activeTask && (activeTask.status.type === 'rendering' || activeTask.status.type === 'mixing_sfx')) {
      await appWindow.setProgressBar({
        progress: Math.round(activeTask.status.progress * 100),
        status: ProgressBarStatus.Normal
      });
    } else if (activeTask && (activeTask.status.type === 'loading' || activeTask.status.type === 'mixing')) {
      await appWindow.setProgressBar({
        status: ProgressBarStatus.Indeterminate
      });
    } else {
      await appWindow.setProgressBar({
        status: ProgressBarStatus.None
      });
    }
  } catch (e) {
    console.error('Error updating taskbar progress:', e);
  }
});

onUnmounted(() => {
  document.removeEventListener('scroll', onAnyScroll, true);
  if (spyFrame) cancelAnimationFrame(spyFrame);
});
</script>

<template>
  <v-app id="dpr">
    <v-sonner position="top-center" />

    <!-- 标题栏：整条可拖拽；右侧三个窗口按钮（DSH 式方形图标按钮） -->
    <header class="phi-titlebar" data-tauri-drag-region>
      <div class="phi-titlebar__brand" data-tauri-drag-region>
        <img src="./assets/favicon.png" alt="icon" class="phi-titlebar__logo" />
        <span class="phi-titlebar__name">DPRecorder</span>
      </div>
      <div class="phi-titlebar__spacer" data-tauri-drag-region></div>
      <div class="phi-titlebar__controls">
        <button class="phi-winbtn" type="button" :title="t('close')" @click="appMinimize">
          <i class="mdi mdi-window-minimize"></i>
        </button>
        <button class="phi-winbtn" type="button" @click="appMaximize">
          <i class="mdi mdi-window-maximize"></i>
        </button>
        <button class="phi-winbtn phi-winbtn--close" type="button" :title="t('close')" @click="appClose()">
          <i class="mdi mdi-window-close"></i>
        </button>
      </div>
    </header>

    <div class="phi-frame">
      <!-- 侧栏：折叠按钮在最左上角（图标贴左边），下面是一级导航 + 渲染页的分区导航，
           「设置」用弹性空白顶到最下面 -->
      <aside class="phi-nav" :class="{ 'phi-nav--collapsed': railCollapsed, 'phi-nav--resizing': navResizing }" :style="navStyle">
        <div class="phi-nav__brand">
          <button class="phi-nav__collapse" type="button" :title="t('render')" @click="railCollapsed = !railCollapsed">
            <i class="mdi" :class="railCollapsed ? 'mdi-chevron-right' : 'mdi-chevron-left'"></i>
          </button>
        </div>
        <nav class="phi-nav__section">
          <button
            v-for="item in primaryNav"
            :key="item.name"
            class="phi-nav__item"
            :class="{ 'phi-nav__item--active': route.name === item.name }"
            type="button"
            :title="t(item.name)"
            @click="routerPush(item.name)"
          >
            <i class="mdi" :class="item.icon"></i>
            <span class="phi-nav__label">{{ t(item.name) }}</span>
          </button>
        </nav>
        <template v-if="route.name === 'render' && !railCollapsed">
          <nav class="phi-nav__section phi-nav__section--sub">
            <!-- 谱面信息（顶部曲绘横幅）不属于任何组，单独一项 -->
            <button
              class="phi-nav__item phi-nav__item--sub"
              :class="{ 'phi-nav__item--active': activeSection === renderHero.id }"
              type="button"
              @click="scrollToSection(renderHero.id)"
            >
              <i class="mdi" :class="renderHero.icon"></i>
              <span class="phi-nav__label">{{ t(renderHero.label) }}</span>
            </button>
            <!-- 二级：一级是组标题（点击展开/收起并跳转），下面是缩进的叶子 -->
            <template v-for="group in renderGroups" :key="group.id">
              <button
                class="phi-nav__item phi-nav__item--sub phi-nav__item--group"
                :class="{ 'phi-nav__item--active': activeSection === group.id }"
                type="button"
                @click="toggleGroup(group.id)"
              >
                <i class="mdi" :class="group.icon"></i>
                <span class="phi-nav__label">{{ t(group.label) }}</span>
                <i
                  v-if="group.children.length"
                  class="mdi phi-nav__caret"
                  :class="isGroupOpen(group.id) ? 'mdi-chevron-up' : 'mdi-chevron-down'"
                ></i>
              </button>
              <button
                v-for="leaf in group.children"
                v-show="isGroupOpen(group.id)"
                :key="leaf.id"
                class="phi-nav__item phi-nav__item--leaf"
                :class="{
                  'phi-nav__item--active': activeSection === leaf.id,
                  // 二级分区总开关没勾时整项变灰（状态由 PhiSection 登记，见 config/sectionGate.ts）
                  'phi-nav__item--gated-off': sectionGateState(leaf.id) === 'off',
                }"
                type="button"
                @click="scrollToSection(leaf.id)"
              >
                <i class="mdi" :class="leaf.icon"></i>
                <span class="phi-nav__label">{{ t(leaf.label) }}</span>
              </button>
            </template>
          </nav>
        </template>
        <div class="phi-nav__spacer"></div>
        <nav class="phi-nav__section phi-nav__section--bottom">
          <button
            v-for="item in settingsNav"
            :key="item.name"
            class="phi-nav__item"
            :class="{ 'phi-nav__item--active': route.name === item.name }"
            type="button"
            :title="t(item.name)"
            @click="onSettingsClick(item.name, $event)"
          >
            <i class="mdi" :class="item.icon"></i>
            <span class="phi-nav__label">{{ t(item.name) }}</span>
          </button>
        </nav>
      </aside>

      <!-- 左栏右边缘的拖拽把手：8px 宽、左右各 -4px 抵消，压在分界线上 -->
      <div
        v-if="!railCollapsed"
        class="phi-nav__handle"
        :style="{ left: `${navWidth - 4}px` }"
        @pointerdown.prevent="startNavResize"
      ></div>

      <!-- 内容区：单栏，左上角圆角与侧栏之间以 0.5px 描边分隔 -->
      <main class="phi-center">
        <router-view v-slot="{ Component }">
          <Suspense timeout="0">
            <template #default>
              <component :is="Component" ref="component" />
            </template>
            <template #fallback>
              <div class="phi-fallback">
                <v-progress-circular class="waitIn" indeterminate size="large" />
              </div>
            </template>
          </Suspense>
        </router-view>

        <!-- 页面初始化遮罩：由 `./pageLoad` 的 `pageReady` 控制，挡住「默认值 → 实际值」的那一跳。
             盖在内容区上、但不盖住下面的渲染弹窗（它比这一层高）。 -->
        <Transition name="phi-veil">
          <div v-if="!pageReady" class="phi-page-loading">
            <v-progress-circular indeterminate size="large" />
          </div>
        </Transition>
      </main>
        <!-- 渲染弹窗：当前正在渲染的谱面（全局，见 components/TaskStrip.vue） -->
        <TaskStrip />
    </div>
    <v-dialog v-model="ffmpegDialog" width="auto" min-width="400px" class="log-card-bg">
      <v-card class="log-card-window">
        <v-card-title v-t="t('ffmpeg-not-found')"> </v-card-title>
        <v-card-text>
          <div class="block whitespace-pre overflow-auto log-card-msg select wrap" style="max-height: 60vh; white-space: pre-wrap">{{ t('ffmpeg-not-found-detail') }}</div>
        </v-card-text>
        <v-card-actions class="justify-end">
          <v-btn variant="text" @click="getNewVersion" :loading="ffmpegGetNewVersionLoding" v-t="t('try-download')"></v-btn>
          <v-btn variant="text" @click="openExternal('https://github.com/BtbN/FFmpeg-Builds/releases')" v-t="t('open-download')"></v-btn>
          <v-btn variant="text" @click="openAppFolder" v-t="t('open-app-folder')"></v-btn>
          <v-btn color="btn" class="hover-scale" variant="text" @click="ffmpegDialog = false" v-t="t('confirm')"></v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
    <v-dialog v-model="ffmpegDialogFilter" width="auto" min-width="400px" class="log-card-bg">
      <v-card class="log-card-only-window">
        <v-card-title v-t="t('ffmpeg-check')"> </v-card-title>
        <v-card-text>
          <div class="block whitespace-pre overflow-auto select wrap" style="max-height: 60vh; white-space: pre-wrap">{{ t('ffmpeg-check-detail') }}</div>
        </v-card-text>
        <v-card-actions class="justify-end">
          <v-btn color="btn" class="hover-scale" variant="text" @click="ffmpegDialogFilter = false" v-t="t('confirm')"></v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
  </v-app>
</template>

<style>
/* ============================================================
   外壳：标题栏 + 侧栏 + 内容区
   取值来自 src/design/tokens.css（--phi-*），视觉规格参照 DSH Desktop：
   全直角分区、0.5px 描边分层、状态只变颜色不做位移、无毛玻璃。
   ============================================================ */

*[data-tauri-drag-region] {
  app-region: drag;
}

html {
  scroll-behavior: smooth;
}

/* 全局不可选中（.select / .log-card-msg 会在需要的文字上放开） */
* {
  -webkit-user-select: none;
  -moz-user-select: none;
  -ms-user-select: none;
  user-select: none;
}

#dpr {
  background-color: var(--phi-bg-page);
  color: var(--phi-label-primary);
}

/* ---------- 标题栏 ---------- */
.phi-titlebar {
  position: fixed;
  inset: 0 0 auto 0;
  z-index: 120;
  display: flex;
  align-items: center;
  height: var(--phi-titlebar-height);
  padding: 0 var(--phi-space-2) 0 var(--phi-space-4);
  background: var(--phi-nav-fill);
  user-select: none;
}

.phi-titlebar__brand {
  display: flex;
  flex: none;
  gap: var(--phi-space-2);
  align-items: center;
  pointer-events: none;
}

.phi-titlebar__logo {
  width: 18px;
  height: 18px;
}

.phi-titlebar__name {
  font-size: var(--phi-font-size-secondary);
  font-weight: 600;
  color: var(--phi-label-primary);
  letter-spacing: 0.02em;
}

.phi-titlebar__spacer {
  flex: 1;
  min-width: var(--phi-space-2);
  height: 100%;
}

.phi-titlebar__controls {
  display: flex;
  flex: none;
  gap: var(--phi-space-1);
  align-items: center;
}

.phi-winbtn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: var(--phi-nav-icon-button);
  height: var(--phi-nav-icon-button);
  font-size: 16px;
  color: var(--phi-label-secondary);
  cursor: pointer;
  background: transparent;
  border: none;
  border-radius: var(--phi-radius-sm);
  transition:
    background-color var(--phi-duration) var(--phi-ease),
    color var(--phi-duration) var(--phi-ease);
}

.phi-winbtn:hover {
  color: var(--phi-label-primary);
  background: var(--phi-interactive-hover);
}

.phi-winbtn--close:hover {
  color: #fff;
  background: var(--phi-danger);
}

/* ---------- 框架：侧栏 + 内容区 ---------- */
.phi-frame {
  position: relative;
  display: flex;
  height: 100vh;
  padding-top: var(--phi-titlebar-height);
  overflow: hidden;
}

/* 拖拽把手：压在侧栏与内容区的分界线上（宽 8px，定位时左移 4px 居中） */
.phi-nav__handle {
  position: absolute;
  top: var(--phi-titlebar-height);
  bottom: 0;
  z-index: 30;
  width: 8px;
  cursor: col-resize;
  background: transparent;
  transition: background-color var(--phi-duration) var(--phi-ease);
}

.phi-nav__handle:hover,
.phi-nav__handle:active {
  background: var(--phi-accent);
  opacity: 0.4;
}

.phi-nav {
  display: flex;
  flex: none;
  flex-direction: column;
  gap: var(--phi-space-1);
  width: var(--phi-nav-width);
  padding: var(--phi-space-2) var(--phi-nav-inline-padding) var(--phi-space-3);
  overflow-x: hidden;
  overflow-y: auto;
  background: var(--phi-nav-fill);
  transition: width var(--phi-duration) var(--phi-ease);
}

.phi-nav--collapsed {
  width: var(--phi-nav-width-rail);
  padding-right: 0;
  padding-left: 0;
  align-items: center;
}

/* 拖动期间关掉宽度过渡，避免跟手延迟 */
.phi-nav--resizing {
  transition: none;
}

/* 折叠按钮贴在最左上角。padding-left 取 4px 是为了让 28px 按钮里的 16px 图标
   正好落在下面导航条目图标的同一条竖线上（12 + 2 + 8 = 12 + 4 + 6 = 22px）。 */
.phi-nav__brand {
  display: flex;
  flex: none;
  align-items: center;
  justify-content: flex-start;
  height: var(--phi-nav-logo-height);
  padding-left: var(--phi-space-1);
  margin-bottom: var(--phi-space-2);
}

.phi-nav--collapsed .phi-nav__brand {
  justify-content: center;
  padding-left: 0;
}

.phi-nav__collapse {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: var(--phi-nav-icon-button);
  height: var(--phi-nav-icon-button);
  font-size: 16px;
  color: var(--phi-label-secondary);
  cursor: pointer;
  background: transparent;
  border: none;
  border-radius: var(--phi-radius-sm);
  transition:
    background-color var(--phi-duration) var(--phi-ease),
    color var(--phi-duration) var(--phi-ease);
}

.phi-nav__collapse:hover {
  color: var(--phi-label-primary);
  background: var(--phi-interactive-hover);
}

.phi-nav__section {
  display: flex;
  flex: none;
  flex-direction: column;
  gap: var(--phi-space-1);
  margin-bottom: var(--phi-space-3);
}

/* 渲染页的分区列表：与上面的一级导航之间用一条细分隔线分开，
   不用分组标题（DSH 侧栏本身就是扁平列表） */
.phi-nav__section--sub {
  padding-top: var(--phi-space-3);
  margin-top: var(--phi-space-1);
  border-top: 0.5px solid var(--phi-border-l3);
}

/* 把「设置」顶到左栏最下面 */
.phi-nav__spacer {
  flex: 1 1 auto;
  min-height: var(--phi-space-3);
}

.phi-nav__section--bottom {
  padding-top: var(--phi-space-2);
  margin-bottom: 0;
  border-top: 0.5px solid var(--phi-border-l3);
}

.phi-nav__item {
  display: flex;
  flex: none;
  gap: var(--phi-space-2);
  align-items: center;
  width: 100%;
  min-height: var(--phi-nav-item-height);
  padding: 0 var(--phi-space-2);
  margin: 0 var(--phi-nav-item-margin);
  overflow: hidden;
  font: inherit;
  font-size: var(--phi-font-size);
  line-height: 22px;
  color: var(--phi-label-primary);
  text-align: left;
  cursor: pointer;
  background: transparent;
  border: none;
  border-radius: var(--phi-radius-md);
  transition: background-color var(--phi-duration) var(--phi-ease);
}

.phi-nav__item .mdi {
  flex: none;
  font-size: 18px;
  color: var(--phi-label-secondary);
}

.phi-nav__item:hover {
  background: var(--phi-nav-item-hover);
}

.phi-nav__item--active {
  background: var(--phi-nav-item-active);
}

.phi-nav__item--active .mdi {
  color: var(--phi-accent);
}

.phi-nav__item--sub {
  min-height: 32px;
  font-size: var(--phi-font-size-secondary);
}

/* 二级里的「组标题」：比下面的叶子重一点，点它展开/收起 */
.phi-nav__item--sub.phi-nav__item--group {
  font-weight: 600;
  color: var(--phi-label-primary);
}

/* 组标题右端的展开箭头 */
.phi-nav__caret {
  flex: none;
  margin-left: auto;
  font-size: 16px;
  color: var(--phi-label-tertiary);
}

/* 组里的叶子条目：再缩进一级，字号与图标都小一档 */
.phi-nav__item--leaf {
  min-height: 30px;
  padding-left: calc(var(--phi-space-2) + var(--phi-space-5));
  font-size: var(--phi-font-size-secondary);
  color: var(--phi-label-secondary);
}

.phi-nav__item--leaf .mdi {
  font-size: 16px;
}

.phi-nav__item--leaf.phi-nav__item--active {
  color: var(--phi-label-primary);
}

/* 二级项的总开关没勾时整项（图标 + 文字）变灰。
   二级项在左侧目录里也区分启用与未启用：没勾总开关的整项变灰。
   状态由 PhiSection 登记到 src/config/sectionGate.ts，这里只读；点击照常滚动跳转。
   放在 --active 规则之后，好让「正高亮但没启用」的项也是灰的。 */
.phi-nav__item--leaf.phi-nav__item--gated-off,
.phi-nav__item--leaf.phi-nav__item--gated-off .mdi {
  color: var(--phi-label-tertiary);
}

.phi-nav__label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.phi-nav--collapsed .phi-nav__label {
  display: none;
}

.phi-nav--collapsed .phi-nav__item {
  justify-content: center;
  width: var(--phi-nav-item-height);
  padding: 0;
}

/* ---------- 内容区 ---------- */
.phi-center {
  position: relative;
  display: flex;
  flex: 1;
  flex-direction: column;
  min-width: 0;
  overflow: hidden;
  background: var(--phi-bg-base);
  border-left: 0.5px solid var(--phi-border-l3);
  border-radius: var(--phi-content-radius) 0 0 0;
}

/* 内容区底部不给渲染弹窗让位（避免全局给它强制空出一段空间）：
   `components/TaskStrip.vue` 直接 absolute 浮在内容之上，有任务时也不再撑出 padding-bottom，
   所以页面内容不会因为有无任务而上下跳。`.phi-center` 保留 `position: relative` 当它的定位父级。 */

.phi-fallback {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: var(--phi-space-8);
}

/* 页面初始化遮罩：不透明地铺满内容区（底色与内容区一致，看不出是一层遮罩），
   把「先画默认值、随后跳成实际值」的过程挡在后面。 */
.phi-page-loading {
  position: absolute;
  inset: 0;
  z-index: 5;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--phi-bg-base);
}

.phi-veil-enter-active,
.phi-veil-leave-active {
  transition: opacity 0.15s ease;
}

.phi-veil-enter-from,
.phi-veil-leave-to {
  opacity: 0;
}

/* 左栏子项跳转时给分区标题留一点上边距，别顶到容器边缘 */
[id^='sec-'] {
  scroll-margin-top: var(--phi-space-2);
}

/* ---------- 全局细节 ---------- */
/* Vuetify 输入框的 details 行不占额外布局空间 */
.v-input__details {
  min-height: 16px;
  padding-top: 2px;
  margin-bottom: -6px;
}

/* 通用容器：卡片外观，取值走令牌 */
.container {
  margin: var(--phi-space-8);
  background: var(--phi-bg-layer-1);
  border: 1px solid var(--phi-border-l2);
  border-radius: var(--phi-radius-lg);
  box-shadow: var(--phi-elevation-panel);
}

.select {
  user-select: text;
}

.icon {
  font-size: 250%;
}

.v-list {
  background-color: transparent !important; /* tag editor 里需要保留输入框底色 */
}

/* 下拉弹层（v-combobox / v-select / v-autocomplete 的菜单）：
   上面的 `.v-list` 把它刷成了全透明，弹层于是变成"裸文字盖在页面上"、后面的分区标题
   直接透出来。这里给菜单面板补一层半透明底 + 描边 —— 用 color-mix 而不是 backdrop-filter
   （全项目去毛玻璃）。只作用于 `.v-menu` 里的 list，不影响 tag editor。 */
.v-menu > .v-overlay__content > .v-list,
.v-menu > .v-overlay__content > .v-sheet {
  background: color-mix(in srgb, var(--phi-bg-layer-2) 88%, transparent) !important;
  border: 0.5px solid var(--phi-border-l3);
  border-radius: 0;
}

/* 下拉栏去掉圆角：Vuetify 在 `.v-menu > .v-overlay__content` 上写了
   `border-radius: 4px`（vuetify.css:23511-23513），里面的 `.v-list` 又用
   `border-radius: inherit`（vuetify.css:23514-23521）继承过去 —— 两处都清零，
   面板与全项目其余部分一样是直角。 */
.v-menu > .v-overlay__content {
  border-radius: 0;
}

/* 弹窗遮罩（v-dialog / v-overlay 的变暗层）：
   Vuetify 的 .v-overlay__scrim 用的是 `rgb(var(--v-theme-on-surface))`，而深色主题的
   on-surface 是**白色** —— 于是弹窗一开，整页不是变暗而是被「洗白」一遍。
   这里统一改成半透明黑，和界面其余部分（深底 + 淡描边）一致。 */
.v-overlay__scrim {
  background: rgb(0 0 0 / 32%) !important;
  opacity: 1 !important;
}

/* ===== 全局弹窗基底（所有弹窗统一成「齿轮配置窗体」那套外观）=====
   这两条是应用里每个 `v-dialog` 卡片共用的底：`.log-card-window`（日志/详情类弹窗）、
   `.log-card-only-window`（确认框与配置类弹窗）—— 两个类只出现在 `v-dialog` 里，所以改这里
   就等于全局改。取值与 `design/config.css:411-445` 的 `.phi-config-dialog` 那套共用皮肤一致：
   内容区底色（`#151517` = `rgb(21 21 23)`）再留 88% 不透明、直角、白字。带明确像素宽的弹窗
   （齿轮、文件名格式、批量页两个配置窗、设置页重置框）仍在 `config.css` 里叠
   `.phi-config-dialog`（它另给卡片写 `width: 100%`），两处重叠的只有背景与圆角，值相同。 */
.log-card-window,
.log-card-only-window {
  --phi-config-dialog-alpha: 88%;
  background: rgb(21 21 23 / var(--phi-config-dialog-alpha)) !important;
  border: 1px solid var(--phi-border-l2);
  border-radius: 0 !important;
  color: var(--phi-static-bluish-00);
}

/* 弹窗标题行：与齿轮弹窗的 `.phi-config-dialog__title` 一模一样（居中、1.5rem、不加粗、纯白）。
   标题元素是 Vuetify 的 `v-card-title`（字号 / 字重 / 左对齐都是它自带的），这里统一覆盖。
   只用 `>` 命中弹窗卡片的直接子标题 —— RPEView 列表卡片里那个 `v-card-title` 不受影响。 */
.log-card-window > .v-card-title,
.log-card-only-window > .v-card-title {
  padding: calc(var(--phi-space-3) + var(--phi-space-2)) calc(var(--phi-space-3) * 2.2) var(--phi-space-2);
  text-align: center;
  white-space: normal;
  overflow-wrap: anywhere;
  font-size: 1.5rem;
  font-weight: 400;
  line-height: 1.3;
  color: var(--phi-static-bluish-00);
}

/* 正文与操作按钮也统一成纯白：界面其余地方的 `.v-btn` 用的是下面那条
   `.v-btn { color: var(--phi-label-primary) }`，`v-card-text` 用的是 Vuetify 的 on-surface，
   弹窗里两者都拉齐到标题的白色。`!important` 是为了压过 Vuetify 给带 `color="…"` 的按钮写的颜色。 */
.log-card-window .v-card-text,
.log-card-only-window .v-card-text,
.log-card-window .v-card-actions .v-btn,
.log-card-only-window .v-card-actions .v-btn {
  color: var(--phi-static-bluish-00) !important;
}

/* 日志/详情卡片里的等宽文本块 */
.log-card-msg {
  padding: var(--phi-space-4);
  font-family: var(--phi-font-family-code) !important;
  background: var(--phi-code-block) !important;
  border: 1px solid var(--phi-border-l2);
  border-radius: var(--phi-radius-md) !important;
  user-select: text;
  white-space: pre;
}

.v-btn {
  color: var(--phi-label-primary);
  transition: background-color var(--phi-duration) var(--phi-ease);
}

@keyframes Up {
  to {
    visibility: visible;
  }
}

/* 拖放文件的提示浮层（RenderView / BatchView 共用这个类名） */
.drop-zone-overlay {
  background: color-mix(in srgb, var(--phi-bg-base) 80%, transparent) !important;
  outline: 2px dashed var(--phi-accent);
  outline-offset: calc(-1 * var(--phi-space-6));
}

html {
  scrollbar-width: none;
  -ms-overflow-style: none;
}

html::-webkit-scrollbar {
  display: none;
}

@keyframes shake-distort {
  0% { transform: translate(-10px, 4px) skew(5deg, 3deg); }
  10% { transform: translate(12px, -6px) skew(-6deg, -4deg); }
  20% { transform: translate(-8px, 5px) skew(4deg, 2deg); }
  30% { transform: translate(10px, -3px) skew(-5deg, -3deg); }
  40% { transform: translate(-12px, 6px) skew(6deg, 4deg); }
  50% { transform: translate(8px, -5px) skew(-4deg, -2deg); }
  60% { transform: translate(-10px, 3px) skew(5deg, 3deg); }
  70% { transform: translate(12px, -6px) skew(-6deg, -4deg); }
  80% { transform: translate(-8px, 4px) skew(4deg, 2deg); }
  90% { transform: translate(10px, -5px) skew(-5deg, -3deg); }
  100% { transform: translate(-10px, 4px) skew(5deg, 3deg); }
}

.shake-distort {
  animation: shake-distort 1s ease-in-out infinite;
}
</style>
