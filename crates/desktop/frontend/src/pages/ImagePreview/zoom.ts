export const MIN_ZOOM = 0.1;
export const MAX_ZOOM = 8;
export const ZOOM_STEP = 1.25;

// fit-to-window：图片完整显示在窗口内，最大不超过 1:1
export const FIT_PADDING = 16;
export const TOOLBAR_H = 56; // pt-14 顶部 padding（工具栏空间）

// fit-to-window：完整显示在窗口内（宽高都不超出），不放大
export const computeFitZoom = (w: number, h: number): number => {
  const containerW = window.innerWidth - FIT_PADDING;
  const containerH = window.innerHeight - FIT_PADDING;
  return Math.min(1, containerW / w, containerH / h);
};

// fit-to-width：图片宽度 = 窗口宽度（高度可超出 → 垂直滚动）
export const computeFitToWidthZoom = (w: number): number => {
  const containerW = window.innerWidth - FIT_PADDING;
  return Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, containerW / w));
};
