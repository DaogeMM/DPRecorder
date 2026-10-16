/**
 * 主题与设计令牌的桥接。
 *
 * 主题只剩一套：DeepDark（深色）。没有 Light / Dark / LightBlue，
 * 设置页里也不提供主题选项，所以这里不再需要「按名字切主题」。
 *
 * 设计令牌写在 src/design/tokens.css 里，body[data-phi-theme='DeepDark'] 只改第 2 层的指向；
 * 这个属性仍然写上去，一是便于调试，二是将来要加主题时不用再改调用方。
 */

export const PHI_THEME = 'DeepDark' as const;

export type PhiTheme = typeof PHI_THEME;

/** 把主题名写到 <body data-phi-theme>，驱动 tokens.css 里的令牌切换。 */
export function applyPhiTheme(_name?: string): void {
  document.body.setAttribute('data-phi-theme', PHI_THEME);
}
