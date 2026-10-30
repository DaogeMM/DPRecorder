import { nextTick } from 'vue';
import { createRouter, createWebHistory } from 'vue-router';

import RenderView from '../RenderView.vue';
import BatchView from '../BatchView.vue';
import RPEView from '../RPEView.vue';
import TasksView from '../TasksView.vue';
import SettingsView from '../SettingsView.vue';
import VideoNoiseView from '../tools/VideoNoiseView.vue';

import { toolProjects } from '../tools/projects';

import { useOnLoaded } from '../App.vue';
import { beginPageLoad } from '../pageLoad';

const onLoaded = useOnLoaded();

const router = createRouter({
  history: createWebHistory(import.meta.env.BASE_URL),
  routes: [
    { path: '/', name: 'render', component: RenderView },
    { path: '/batch', name: 'batch', component: BatchView },
    { path: '/bind', name: 'bind', component: RPEView },
    { path: '/tasks', name: 'tasks', component: TasksView },
    // 「工具」是一级导航的最后一项，里面每个项目各一条路由：项目之间**只能**用左栏切换，
    // 所以没有工具列表页，`/tools` 直接落到第一个项目。
    { path: '/tools', name: 'tools', redirect: { name: toolProjects[0]!.name } },
    { path: '/tools/video-noise', name: 'tool-video-noise', component: VideoNoiseView },
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
