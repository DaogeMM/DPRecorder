import './design/tokens.css';
import './design/config.css';
import './design/dialog.css';
import './assets/main.css';
import '@mdi/font/css/materialdesignicons.css';

import { createApp, type Component } from 'vue';
import { createI18n } from 'vue-i18n';

import App from './App.vue';
import EditorWindow from './editor/EditorWindow.vue';
import DebugWindow from './debugger/DebugWindow.vue';
import router from './router';
import { syncChartAnimationDefaults } from './model';

import { createVuetify } from 'vuetify';
import * as components from 'vuetify/components';
import * as directives from 'vuetify/directives';
import { aliases, mdi } from 'vuetify/iconsets/mdi';

import 'vuetify/styles';

import { DeepDark } from './theme';
import { applyPhiTheme } from './design/theme';

import moment from 'moment';
import 'moment/dist/locale/zh-cn';

import mainZhCN from './locales/zh-CN/main.json';
import appzhCN from './locales/zh-CN/app.json';
import batchzhCN from './locales/zh-CN/batch.json';
import configzhCN from './locales/zh-CN/config.json';
import renderzhCN from './locales/zh-CN/render.json';
import rpezhCN from './locales/zh-CN/rpe.json';
import settingzhCN from './locales/zh-CN/setting.json';
import taskzhCN from './locales/zh-CN/task.json';
import noisePlayerzhCN from './locales/zh-CN/noise-player.json';
import malodyzhCN from './locales/zh-CN/malody.json';
import debuggerzhCN from './locales/zh-CN/debugger.json';

// 界面只有简体中文：没有语言切换，也不再读 localStorage 里的 `locale`
// （旧版本存过 en / zh-TW，留着也不生效）。`t('key')` 的写法保留，键都在
// `src/locales/zh-CN/*.json` 里。
const i18n = createI18n({
  locale: 'zh-CN',
  fallbackLocale: 'zh-CN',
  messages: {
    'zh-CN': {
      ...mainZhCN,
      ...appzhCN,
      ...batchzhCN,
      ...configzhCN,
      ...renderzhCN,
      ...rpezhCN,
      ...settingzhCN,
      ...taskzhCN,
      ...noisePlayerzhCN,
      ...malodyzhCN,
      ...debuggerzhCN,
    },
  },
  legacy: false,
  missing(_locale, key) {
    if (key.startsWith('title-')) return '';
    return key;
  },
});

// 主题只有一套（DeepDark 深色）。主题切换与「跟随系统主题」已删除，
// 所以这里不再读 localStorage 里的 theme / useSystemTheme（旧值留着也不会生效）。
applyPhiTheme();

// 相对时间（「3 分钟后」这类）固定用简体中文，不再跟随系统语言。
moment.locale('zh-cn');

const vuetify = createVuetify({
  components,
  directives,
  defaults: {
    VCombobox: {
      transition: 'fade-transition',
    },
    VAutocomplete: {
      transition: 'fade-transition',
    },
  },
  theme: {
    defaultTheme: 'DeepDark',
    themes: {
      DeepDark,
    },
  },
  icons: {
    defaultSet: 'mdi',
    aliases,
    sets: {
      mdi,
    },
  },
});

// 三个窗口是同一份前端在不同 webview 里打开的，URL 上各带一个标记：
// `editor=1` → DPEditor（Rust 的 `open_editor_window` 建），`debug=1` → DPDebugger
// （`debugger::open_debug_window` 建）。这两个只挂自己的根组件 —— 不装路由，
// 也不渲染主界面那套左栏外壳。
const windowFlags = new URLSearchParams(window.location.search);
const isEditorWindow = windowFlags.has('editor');
const isDebugWindow = windowFlags.has('debug');
const rootComponent: Component = isEditorWindow ? EditorWindow : isDebugWindow ? DebugWindow : App;

/**
 * 挂载界面。演出动画的默认值只在 `assets/chart-animation.toml` 写一份（后端读它），
 * 所以先跟后端要一次、覆盖掉 `DEFAULT_RENDER_CONFIG` 里的字面量，再挂组件 ——
 * 否则组件在 `setup` 里读到的还是前端那份旧值。
 */
let mounted = false;

async function bootstrap() {
  await syncChartAnimationDefaults();

  const app = createApp(rootComponent);
  app.use(i18n).use(vuetify);

  if (!isEditorWindow && !isDebugWindow) app.use(router);

  app.mount('#app');
  mounted = true;
}

/**
 * 挂载失败时别留一片纯白：把错误写进 `#app`，至少能看出是哪一步炸的。
 * 已经挂载之后就不再动界面，只把错误打到控制台（否则一个偶发错误会把整个界面清掉）。
 */
function reportFatal(err: unknown) {
  console.error('[bootstrap]', err);
  if (mounted) return;
  const root = document.getElementById('app');
  if (!root) return;
  const text = err instanceof Error ? `${err.name}: ${err.message}\n${err.stack ?? ''}` : String(err);
  const pre = document.createElement('pre');
  pre.style.cssText = 'margin:0;padding:24px;color:#ff9d9d;font:13px/1.6 Consolas,monospace;white-space:pre-wrap;';
  pre.textContent = `界面没有起来：\n${text}`;
  root.replaceChildren(pre);
}

window.addEventListener('error', (e) => reportFatal(e.error ?? e.message));
window.addEventListener('unhandledrejection', (e) => reportFatal(e.reason));

void bootstrap().catch(reportFatal);

export { i18n };

