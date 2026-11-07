import './design/tokens.css';
import './design/config.css';
import './design/dialog.css';
import './assets/main.css';
import '@mdi/font/css/materialdesignicons.css';

import { createApp, type Component } from 'vue';
import { createI18n } from 'vue-i18n';

import App from './App.vue';
import EditorWindow from './editor/EditorWindow.vue';
import router from './router';

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

// 编辑窗口是同一份前端在另一个 webview 里打开的：URL 上带 `editor=1`
// （窗口由 Rust 的 `open_editor_window` 建）。它只挂编辑器的根组件 ——
// 不装路由，也不渲染主界面那套左栏外壳。
const isEditorWindow = new URLSearchParams(window.location.search).has('editor');

const app = createApp((isEditorWindow ? EditorWindow : App) as Component);
app.use(i18n).use(vuetify);

if (!isEditorWindow) app.use(router);

app.mount('#app');

export { i18n };
