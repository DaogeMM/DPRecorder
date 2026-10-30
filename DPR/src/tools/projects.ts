/**
 * 工具页的项目表。
 *
 * 左栏「工具」下面的每一项都来自这里 —— 工具页里的项目**只能**用左栏切换，
 * 页内不放选项卡 / 下拉框。加一个项目 = 这里加一条 + `router/index.ts` 加一条路由。
 */
export interface ToolProject {
  /** 路由名：左栏高亮与 `router.push({ name })` 都用它 */
  name: string;
  /** 路由路径（真实跳转由路由表负责，这里只作为文档性字段） */
  path: string;
  /** 左栏与页面标题用的 i18n key */
  label: string;
  icon: string;
}

export const toolProjects: ToolProject[] = [
  {
    name: 'tool-video-noise',
    path: '/tools/video-noise',
    label: 'tool-video-noise',
    icon: 'mdi-movie-open-star-outline',
  },
];
