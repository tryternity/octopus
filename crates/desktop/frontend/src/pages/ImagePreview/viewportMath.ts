/**
 * 视口固定 canvas 的纯几何换算（ImagePreview drawBg 专用）。
 *
 * 背景：canvas 物理尺寸固定为「视口 × dpr」（永不超 Chromium 32767 单边硬限），
 * canvas 用 `position: sticky` 钉 scrollContainer 视口；图片在 content 空间随滚。
 * drawBg 需算「图片露出视口的区域」→ 视口坐标（画到这里）+ 源切片（从哪取像素）。
 *
 * 全部坐标用 content 空间（scrollContainer 滚动内容坐标），与 canvas sticky 无关：
 * - 图片在 content 空间矩形 = [imgLeft, imgLeft+dispW] × [imgTop, imgTop+dispH]
 * - 视口在 content 空间矩形 = [scrollLeft, scrollLeft+vw] × [scrollTop, scrollTop+vh]
 * 这些函数无 DOM/canvas 依赖，可纯单测（DOM/sticky 对齐靠 GUI 验证）。
 */

/** content 空间的矩形（露出区或视口）。 */
export interface Rect {
  visL: number;
  visR: number;
  visT: number;
  visB: number;
}

/**
 * 图片矩形（content 空间）∩ 视口矩形（content 空间）。
 * 返回 null 表示图片不在当前视口（drawBg 应清空视口画布后返回）。
 */
export function computeVisibleRect(
  imgLeft: number, imgTop: number, dispW: number, dispH: number,
  scrollLeft: number, scrollTop: number, vw: number, vh: number,
): Rect | null {
  const visL = Math.max(imgLeft, scrollLeft);
  const visR = Math.min(imgLeft + dispW, scrollLeft + vw);
  const visT = Math.max(imgTop, scrollTop);
  const visB = Math.min(imgTop + dispH, scrollTop + vh);
  if (visR <= visL || visB <= visT) return null;
  return { visL, visR, visT, visB };
}

/**
 * 露出区（content 空间）→ 视口坐标（canvas sticky 钉视口，drawImage 的 dst 落点）。
 * canvas 钉在视口 (0,0)，故 dst = 露出区减去滚动偏移。
 */
export function visibleToViewport(
  vis: Rect, scrollLeft: number, scrollTop: number,
): { dstL: number; dstT: number; dstW: number; dstH: number } {
  return {
    dstL: vis.visL - scrollLeft,
    dstT: vis.visT - scrollTop,
    dstW: vis.visR - vis.visL,
    dstH: vis.visB - vis.visT,
  };
}

/**
 * 露出区 → 源像素切片（bitmap 物理空间 或 img 自然像素空间，二者公式一致——
 * 因 dispW 与 srcW 同基：bitmap=dispW*dpr、img=naturalWidth，比例换算后等价）。
 *
 * src 由交集保证在 [0, srcW]/[0, srcH] 内（visL≥imgLeft 等约束），此处的钳制仅防
 * 浮点 epsilon 溢出（trim 量 ~1e-13，dst 不变，scale 变化不可见）。
 */
export function computeSrcSlice(
  vis: Rect,
  imgLeft: number, imgTop: number, dispW: number, dispH: number,
  srcW: number, srcH: number,
): { sx: number; sy: number; sw: number; sh: number } {
  const sxRaw = ((vis.visL - imgLeft) / dispW) * srcW;
  const syRaw = ((vis.visT - imgTop) / dispH) * srcH;
  const swRaw = ((vis.visR - vis.visL) / dispW) * srcW;
  const shRaw = ((vis.visB - vis.visT) / dispH) * srcH;
  // 浮点安全钳制（不改变可见区域，仅裁掉越界 epsilon）
  const sx = Math.max(0, Math.min(sxRaw, srcW));
  const sy = Math.max(0, Math.min(syRaw, srcH));
  const sw = Math.max(0, Math.min(swRaw, srcW - sx));
  const sh = Math.max(0, Math.min(shRaw, srcH - sy));
  return { sx, sy, sw, sh };
}
