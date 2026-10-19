/**
 * 表单校验失败时，把用户带到出错的控件上。
 *
 * 谱面信息表单在整页最下面、渲染参数表单又很长：校验只在控件下面标红一行小字，
 * 用户点了「预览 / 渲染」只会觉得「按了没反应」。所以失败时滚到第一个出错的控件、
 * 聚焦它，并把那条错误文案回给调用方去弹提示。
 *
 * 结构对齐 Vuetify `v-form` 的 `validate()`：每个条目是 `{ id, errorMessages }`，
 * `id` 就是控件（`v-text-field` 的 `<input>`）上的 id。
 */
export interface FormError {
  id?: string | number | null;
  errorMessages?: string[] | string | null;
}

export function jumpToFirstError(errors: readonly FormError[] | undefined | null): string | null {
  const first = (errors ?? []).find((it) => it?.id !== undefined && it.id !== null && `${it.id}`.length > 0);
  if (!first) return null;
  const message = first.errorMessages ? [first.errorMessages].flat().join('；') : null;
  const el = document.getElementById(`${first.id}`);
  if (!el) return message;
  el.scrollIntoView({ behavior: 'smooth', block: 'center' });
  if (el instanceof HTMLElement) el.focus({ preventScroll: true });
  return message;
}
