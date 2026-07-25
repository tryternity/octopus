// 工具栏位置算法（纯函数）—— Screenshot 与 RecordAnnotation 共用。
//
// 抽取自原 Screenshot/index.tsx L744-781 的三选逻辑（已踩过坑稳定，详见
// Screenshot L746-749 注释）。RecordAnnotation 把 canvasRect 伪装成 Rect 即可复用。
//
// 三选算法：
//   1. below（默认）：选区下方 8px 处
//   2. above：选区上方（下方空间不够时）
//   3. inside：选区内部底部（上下都不够时兜底——例如全屏截图场景）
//
// X 方向：选区中心居中 + DOCK_MARGIN clamp（用户可能把 Dock 放左/右边）。

export type ToolbarPlacement = "below" | "above" | "inside";

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface ToolbarPosition {
  /** 工具栏 top（逻辑像素，相对视口左上角） */
  y: number;
  /** 实际放置位置 */
  placement: ToolbarPlacement;
  /**
   * popover Y 方向：
   *   true  = 工具栏下方（below 或 above 时）
   *   false = 工具栏上方（inside 兜底时）
   */
  belowOrAbove: boolean;
}

/** 工具栏高度（与业务侧 CSS padding/按钮尺寸一致） */
export const TOOLBAR_H = 44;

/** Dock 留边（用户可能把 Dock 放左/右边） */
export const DOCK_MARGIN = 80;

/**
 * 计算工具栏垂直位置（三选）。
 *
 * @param sel 选区/画布矩形（逻辑像素，相对视口左上角）
 * @param viewportH 视口高度（window.innerHeight）
 */
export function computeToolbarPosition(sel: Rect, viewportH: number): ToolbarPosition {
  const belowSpace = viewportH - (sel.y + sel.h + 8);
  const aboveSpace = sel.y;
  const toolbarBelow = belowSpace >= TOOLBAR_H;
  const toolbarAbove = !toolbarBelow && aboveSpace >= TOOLBAR_H;

  const y = toolbarBelow
    ? Math.min(sel.y + sel.h + 8, viewportH - TOOLBAR_H)
    : toolbarAbove
      ? sel.y - TOOLBAR_H - 4
      : Math.max(sel.y, sel.y + sel.h - TOOLBAR_H - 8); // inside 兜底

  return {
    y,
    placement: toolbarBelow ? "below" : toolbarAbove ? "above" : "inside",
    belowOrAbove: toolbarBelow || toolbarAbove,
  };
}

/**
 * 计算工具栏 X 中心点（基于选区中心 + DOCK_MARGIN clamp）。
 *
 * @param sel 选区/画布矩形
 * @param viewportW 视口宽度（window.innerWidth）
 * @param toolbarW 工具栏实测宽度（业务侧 useLayoutEffect 测量；未知传 0，clamp 仍生效）
 */
export function computeToolbarCenterX(sel: Rect, viewportW: number, toolbarW: number): number {
  const halfW = toolbarW / 2;
  return Math.max(
    DOCK_MARGIN + halfW,
    Math.min(sel.x + sel.w / 2, viewportW - DOCK_MARGIN - halfW),
  );
}
