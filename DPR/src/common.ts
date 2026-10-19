import { toast as sonnerToast } from 'vuetify-sonner';
import { invoke } from '@tauri-apps/api/core';

import { SUPPORTED_LOCALES, i18n } from './main';

import moment from 'moment';

import 'moment/dist/locale/zh-cn';
import 'moment/dist/locale/zh-hk';

export function anyFilter() {
  return {
    name: i18n.global.t('any-filter'),
    extensions: ['*'],
  };
}

export function isString(s: unknown): s is string {
  return typeof s === 'string';
}

export const RULES = {
  notEmpty: (value: string) => value.trim().length > 0 || i18n.global.t('rules.not-empty'),
  notNull: (value: string) => (value != null) || i18n.global.t('rules.not-null'),
  int: (value: string) => (isNumeric(value) && Math.abs(Number(value) - Math.round(Number(value))) < 1e-4) || i18n.global.t('rules.int'),
  positive: (value: string) => (isNumeric(value) && Number(value) > 0) || i18n.global.t('rules.positive'),
  number: (value: string) => isNumeric(value) || i18n.global.t('rules.number'),
  positiveOrZero: (value: string) => (isNumeric(value) && Number(value) >= 0) || i18n.global.t('rules.positive'),
  positiveOrNull: (value: string | null) => (value == null || value == '' || (isNumeric(value) && Number(value) > 0)) || i18n.global.t('rules.positive'),
  alpha01: (value: string) => (isNumeric(value) && Number(value) >= 0 && Number(value) <= 1) || i18n.global.t('rules.alpha'),
  less10000: (value: string) => {
    if (isNumeric(value)) {
      if (Number(value) > 10000 && Number(value) < -10000) return i18n.global.t('rules.big');
    }
    return true;
  },
  less100: (value: string) => {
    if (isNumeric(value)) {
      if (Number(value) > 100 && Number(value) < -100) return i18n.global.t('rules.big');
    }
    return true;
  },
  less4000000000: (value: string) => {
    if (isNumeric(value)) {
      if (Number(value) > 4000000000 && Number(value) < -4000000000) return i18n.global.t('rules.big');
    }
    return true;
  },
  greater0_01: (value: string) => {
    if (isNumeric(value)) {
      if (Number(value) < 0.01 && Number(value) > -0.01) return i18n.global.t('rules.small');
    }
    return true;
  },
  nonSpaces: (value: string) => !/\s/.test(value) || i18n.global.t('rules.non-spaces'),
  notZero: (value: string) => (Number(value) != 0) || i18n.global.t('rules.not-zero'),
  notCOMBO: (value: string) => {
    const filteredValue = value.replace(/[^a-zA-Z0-9!"'#$%&'()*+,\-.\/:;<=>?@\\\[\]^_`{|}~ΜΟΒСՕⅭОмвＣＯＭＢМⅯВϹȮΌϺϺƁ]/g, '').trim();
    if (value.length > 50) {
      return i18n.global.t('rules.long');
    }
    return !/^[CСⅭＣϹ][OՕΟ0ОＯȮΌ][MΜмＭМⅯϺϺ][BΒ8вＢВ][OՕΟ0ОＯȮΌ]$/.test(filteredValue) || i18n.global.t('rules.not-combo');
  },
  hexColorOptional: (value: string) => {
    const val = value.trim();
    if (val === '') return true;
    const reg = /^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/;
    return reg.test(val) || i18n.global.t('rules.hex-color');
  }
};

export function isNumeric(num: any) {
  return (typeof num === 'number' || (typeof num === 'string' && num.trim() !== '')) && !isNaN(num as number);
}

export function setTitle(title: string) {
  document.title = title.length ? title + ' - Phi' : 'Phi';
}

export function changeLocale(locale: string) {
  if (locale.startsWith('en')) locale = 'en';
  if (!SUPPORTED_LOCALES.includes(locale)) locale = 'zh-CN';
  i18n.global.locale.value = (locale === 'zh-TW' ? 'zh-CN' : locale) as typeof i18n.global.locale.value;
  localStorage.setItem('locale', locale);
  const momentLocale =
    {
      'zh-CN': 'zh-cn',
      'zh-TW': 'zh-hk',
      en: 'en-us',
    }[locale] ?? 'en-us';
  moment.locale(momentLocale);
}

export function toast(message: string, kind?: 'success' | 'info' | 'warning' | 'error') {
  sonnerToast(message, {
    duration: 2000,
    // 红字（error）从**左上角**弹：走这条路的错误多半是「配置里有错 / 保存失败」这类
    // 后端原文，可能很长，放正中会正好盖住内容区中间几栏；其余提示仍在正中（`<v-sonner>`
    // 的 `position="top-center"`）。
    ...(kind === 'error' ? { position: 'top-left' as const } : {}),
    cardProps: {
      color: kind,
      // @ts-ignore
      style: 'width: var(--width); user-select: text;',
    },
  });
}

export function toastError(error: any) {
  console.error(error);
  const msg = error instanceof Error ? error.message : String(error);
  if (msg.length) toast(msg, 'error');
}

/**
 * 用系统默认浏览器打开外部链接。
 *
 * 为什么不直接用 `@tauri-apps/plugin-opener` 的 `openUrl()`：它在 Windows 上落到
 * `open` crate 的 `ShellExecuteExW`，而那里 `fMask = 0`、缺了 MSDN 要求的
 * `SEE_MASK_NOASYNC`；从没有消息循环的 Tauri 命令线程调用时，启动请求会被
 * **静默丢弃** —— 点了没反应、也不报错（表现为点击链接无法唤醒浏览器）。
 * Rust 侧 `open_external` 改成 `cmd /c start`，失败会把真实错误抛回来，
 * 这里统一弹成 toast，不再有静默失败。
 */
export async function openExternal(url: string) {
  try {
    await invoke('open_external', { url });
  } catch (e) {
    toastError(e);
  }
}
