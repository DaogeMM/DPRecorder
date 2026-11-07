import { fileURLToPath, URL } from 'node:url';

import { defineConfig, loadEnv, searchForWorkspaceRoot } from 'vite';
import vue from '@vitejs/plugin-vue';

import VueI18nPlugin from '@intlify/unplugin-vue-i18n/vite';

import { resolve, dirname } from 'node:path';

/** 本项目目录（`DPR/`）：既是 Vite 的 root，也是它的工作区根（`pnpm-workspace.yaml` 就在这里） */
const root = dirname(fileURLToPath(import.meta.url));

// https://vitejs.dev/config/
export default defineConfig(({ mode }) => {
  process.env = { ...process.env, ...loadEnv(mode, process.cwd()) };
  return {
    plugins: [
      vue(),
      VueI18nPlugin({
        defaultSFCLang: 'yml',
        include: resolve(root, './src/locales/**'),
      }),
    ],
    resolve: {
      alias: {
        '@': fileURLToPath(new URL('./src', import.meta.url)),
        // 仓库根 `assets/`：界面字体 `font.ttf` 在那里（与引擎共用同一份）。CSS 的 `url()` 走这个别名，
        // Vite 会把它解析成真实路径并打进产物，dev 与打包后都能取到。
        '@repo-assets': fileURLToPath(new URL('../assets', import.meta.url)),
      },
    },
    server: {
      fs: {
        // 上面那个别名在 dev 下由浏览器经 `/@fs/…` 取文件，走的是这张白名单，不在名单里就 403。
        // `fs.allow` 是**整体替换**默认值（默认恰好就是下面第一项），所以工作区根要自己写上。
        allow: [searchForWorkspaceRoot(root), '..'],
      },
    },
  };
});
