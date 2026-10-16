import { ref } from 'vue';

/* 内容区「这一页已经初始化完」的标志。
 *
 * 切页时外壳先把 `pageReady` 置回 false，在内容区不透明地盖一层转圈；页面自己的异步初始化
 * （读设置、读任务、恢复工作配置…）结束后调 `finishPageLoad()` 才揭开。这样不会出现
 * 「先画一屏默认值，数据到了再跳一下」。
 *
 * 页面若在初始化里抛错、忘了调 `finishPageLoad()`，由兜底计时器放行（`PAGE_LOAD_FALLBACK_MS`）。 */

export const pageReady = ref(false);
export const PAGE_LOAD_FALLBACK_MS = 5000;

let fallbackTimer: ReturnType<typeof setTimeout> | undefined;

export function beginPageLoad() {
  pageReady.value = false;
  clearTimeout(fallbackTimer);
  fallbackTimer = setTimeout(() => {
    fallbackTimer = undefined;
    pageReady.value = true;
  }, PAGE_LOAD_FALLBACK_MS);
}

export function finishPageLoad() {
  clearTimeout(fallbackTimer);
  fallbackTimer = undefined;
  pageReady.value = true;
}
