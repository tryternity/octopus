/**
 * 纯函数：panel 宽度 clamp。
 *
 * 约束模型：只定 min（保证手柄可见可恢复），max 由 terminalMin 推导——
 * 终端区至少要留 terminalMin，所以 panelMax = containerWidth - terminalMin - otherSideWidth。
 *
 * 当 panelMax < min 时（极小窗口），min 优先——保证用户总能抓住手柄重新拉大，
 * 此时终端区会被挤到 < terminalMin，但 Tauri min_inner_size(560) 兜底，实际不会到这步。
 */
export function clampPanelWidth(
  raw: number,
  min: number,
  containerWidth: number,
  otherSideWidth: number,
  terminalMin: number,
): number {
  const safeRaw = Number.isFinite(raw) ? raw : min;
  const dynamicMax = containerWidth - terminalMin - otherSideWidth;
  // dynamicMax < min 时（极小窗口），min 优先：max(min, ...) 保证下界
  const effectiveMax = Math.max(min, dynamicMax);
  return Math.min(effectiveMax, Math.max(min, safeRaw));
}
