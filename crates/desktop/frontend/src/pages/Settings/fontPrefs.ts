/**
 * 终端字体偏好纯函数——从 GeneralPanel.tsx 提取，便于单测。
 *
 * 默认值与 Rust AppConfig default_terminal_font_size/family 对齐（单一真相源在后端）。
 * 改默认值时三处常量都要改：本文件 + Terminal/index.tsx + Terminal/useTerminalSession.ts。
 */

/** 默认字号——与 infra/config.rs default_terminal_font_size() 对齐。 */
export const TERMINAL_FONT_SIZE_DEFAULT = 13;
/** 默认字体族——与 infra/config.rs default_terminal_font_family() 对齐。 */
export const TERMINAL_FONT_FAMILY_DEFAULT = "Menlo";

/**
 * 判断当前字号/字体族是否在默认状态。
 *
 * 「恢复默认」按钮仅在偏离时显示（避免无意义点击）。调用方用 `!isFontAtDefault(...)`
 * 决定按钮可见性：true = 在默认（不显示按钮），false = 偏离（显示按钮）。
 * 偏离 = 字号 ≠ 13 或 字体族非空且 ≠ "Menlo"。字体族为空/缺失视为默认
 * （旧库或损坏数据，不显示按钮）。
 *
 * @param size  当前字号（DB terminal_font_size，可能 undefined/非数字）
 * @param family 当前字体族（DB terminal_font_family，可能 undefined/空串）
 * @returns true 表示在默认状态（不显示「恢复默认」按钮）；false 表示偏离
 */
export function isFontAtDefault(size: unknown, family: unknown): boolean {
  const sizeDeviates = typeof size === "number" && size !== TERMINAL_FONT_SIZE_DEFAULT;
  const familyDeviates =
    typeof family === "string" && family.length > 0 && family !== TERMINAL_FONT_FAMILY_DEFAULT;
  // 两者都未偏离 → 在默认状态（不显示按钮）。任一偏离 → 显示按钮。
  return !(sizeDeviates || familyDeviates);
}
