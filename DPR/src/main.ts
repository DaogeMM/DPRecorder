import './design/tokens.css';
import './design/config.css';
import './assets/main.css';
import '@mdi/font/css/materialdesignicons.css';

import { createApp } from 'vue';
import { createI18n } from 'vue-i18n';

import { changeLocale } from './common';

import App from './App.vue';
import router from './router';

import { createVuetify } from 'vuetify';
import * as components from 'vuetify/components';
import * as directives from 'vuetify/directives';
import { aliases, mdi } from 'vuetify/iconsets/mdi';

import 'vuetify/styles';

import { DeepDark } from './theme';
import { applyPhiTheme } from './design/theme';

import mainEn from './locales/en/main.json';
import appEn from './locales/en/app.json';
import batchEn from './locales/en/batch.json';
import configEn from './locales/en/config.json';
import renderEn from './locales/en/render.json';
import rpeEn from './locales/en/rpe.json';
import settingEn from './locales/en/setting.json';
import taskEn from './locales/en/task.json';

import mainZhCN from './locales/zh-CN/main.json';
import appzhCN from './locales/zh-CN/app.json';
import batchzhCN from './locales/zh-CN/batch.json';
import configzhCN from './locales/zh-CN/config.json';
import renderzhCN from './locales/zh-CN/render.json';
import rpezhCN from './locales/zh-CN/rpe.json';
import settingzhCN from './locales/zh-CN/setting.json';
import taskzhCN from './locales/zh-CN/task.json';

export const SUPPORTED_LOCALES: string[] = ['en', 'zh-CN', 'zh-TW'];
export const SUPPORTED_LOCALES_NAME: { name: string, code: string }[] = [
  { code: "en", name: "English" },
  { code: "zh-CN", name: "简体中文" },
  // { code: "zh-TW", name: "繁體中文" }, // TODO
];

// 界面语言的默认值：以存储为准，没存过就用简体中文。
// 以前这里会拿 `navigator.languages` 猜 —— WebView2 常把系统语言报成 `en-US`，于是整个
// 界面变英文；而设置页读的是同一份存储、仍显示「简体中文」，看起来就是
// 「设置里选的是简体中文，界面还是英文」。本项目的界面与文档都以中文为主，所以不再猜。
let locale = localStorage.getItem('locale') ?? 'zh-CN';
const i18n = createI18n({
  locale: 'en',
  fallbackLocale: 'en',
  messages: {
    'en': {
      ...mainEn,
      ...appEn,
      ...batchEn,
      ...configEn,
      ...renderEn,
      ...rpeEn,
      ...settingEn,
      ...taskEn,
    },
    'zh-CN': {
      ...mainZhCN,
      ...appzhCN,
      ...batchzhCN,
      ...configzhCN,
      ...renderzhCN,
      ...rpezhCN,
      ...settingzhCN,
      ...taskzhCN,
    },
  },
  legacy: false,
  missing(_locale, key) {
    if (key.startsWith('title-')) return '';
    return key;
  },
});
changeLocale(locale);

// 主题只有一套（DeepDark 深色）。主题切换与「跟随系统主题」已删除，
// 所以这里不再读 localStorage 里的 theme / useSystemTheme（旧值留着也不会生效）。
applyPhiTheme();

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

const app = createApp(App);
app.use(i18n).use(router).use(vuetify);

app.mount('#app');

export { i18n };
