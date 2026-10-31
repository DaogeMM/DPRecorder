import { nextTick } from 'vue';
import { createRouter, createWebHistory } from 'vue-router';

import RenderView from '../RenderView.vue';
import BatchView from '../BatchView.vue';
import ChartModView from '@dpe/ChartModView.vue';
import RPEView from '../RPEView.vue';
import TasksView from '../TasksView.vue';
import SettingsView from '../SettingsView.vue';
import VideoNoiseView from '@dpe/VideoNoiseView.vue';

import { useOnLoaded } from '../App.vue';
import { beginPageLoad } from '../pageLoad';

const onLoaded = useOnLoaded();

const router = createRouter({
  history: createWebHistory(import.meta.env.BASE_URL),
  routes: [
    { path: '/', name: 'render', component: RenderView },
    { path: '/batch', name: 'batch', component: BatchView },
    // DPE 的谱面列表：与批量页是同一份页面，但去掉底部那条批量操作（`listOnly`），
    // 存储也另起一份（`scope: 'dpe'`），与 DPR 的「批量」互不共用。
    { path: '/charts', name: 'chart-list', component: BatchView, props: { listOnly: true, scope: 'dpe' } },
    { path: '/chart-mod', name: 'chart-mod', component: ChartModView },
    { path: '/bind', name: 'bind', component: RPEView },
    { path: '/tasks', name: 'tasks', component: TasksView },
    { path: '/noise', name: 'tool-video-noise', component: VideoNoiseView },
    { path: '/settings', name: 'settings', component: SettingsView },
  ],
  scrollBehavior(_to, from, savedPosition) {
    return new Promise((resolve) => {
      onLoaded.value = () => {
        resolve(savedPosition ? savedPosition : { top: 0 });
      };
    });
  },
});

import { i18n } from '../main';
import { setTitle } from '../common';

// 每次导航先把内容区盖起来，等这一页自己的异步初始化结束（`finishPageLoad`）才揭开，
// 免得「先画一屏默认值、数据到了再跳一下」。见 `../pageLoad`。
// 原地重复导航（点的是当前这一页）不盖：那种情况下组件不会重建，没人会去揭开这层遮罩。
router.beforeEach((to, from) => {
  if (to.fullPath !== from.fullPath) beginPageLoad();
});

router.afterEach((to) => {
  nextTick(() => {
    const title = i18n.global.t('title-' + ((to.name as string | undefined | null) || 'default'));
    setTitle(title);
  });
});

export default router;
