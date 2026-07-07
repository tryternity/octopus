import { describe, it, expect } from "vitest";
import { computeVisibleRect, visibleToViewport, computeSrcSlice } from "./viewportMath";

// 纯几何换算单测：canvas 物理尺寸视口固定后，drawBg 画「图片露出视口的切片」。
// DOM/sticky 对齐靠 GUI 验证；此处锁住坐标换算的正确性（长图崩盘修复的核心数学）。
//
// 约定：所有坐标在 content 空间。图片矩形 = [imgLeft, imgLeft+dispW] × [imgTop, imgTop+dispH]，
// 视口矩形 = [scrollLeft, scrollLeft+vw] × [scrollTop, scrollTop+vh]。

describe("computeVisibleRect", () => {
  it("图片完全在视口内（无滚动）→ 整张露出", () => {
    // 图片 [10,110]×[56,256]，视口 [0,800]×[0,600]
    const r = computeVisibleRect(10, 56, 100, 200, 0, 0, 800, 600);
    expect(r).toEqual({ visL: 10, visR: 110, visT: 56, visB: 256 });
  });

  it("垂直滚动：图片底部露出（顶部滚出视口）", () => {
    // 图片 [0,100]×[56,2056]，视口 [0,100]×[1000,1600]（scrollTop=1000）
    const r = computeVisibleRect(0, 56, 100, 2000, 0, 1000, 100, 600)!;
    expect(r.visT).toBe(1000);
    expect(r.visB).toBe(1600); // min(2056, 1600)
    expect(r.visB - r.visT).toBe(600); // 露出 600px（满视口高）
  });

  it("水平滚动：图片右部露出（左部滚出视口）", () => {
    // 图片 [0,3000]×[0,100]，视口 [2000,2700]×[0,600]（scrollLeft=2000，vw=700）
    const r = computeVisibleRect(0, 0, 3000, 100, 2000, 0, 700, 600)!;
    expect(r.visL).toBe(2000);
    expect(r.visR).toBe(2700);
    expect(r.visR - r.visL).toBe(700);
  });

  it("图片大于视口（双向裁剪）→ 露出区 = 视口本身", () => {
    const r = computeVisibleRect(0, 0, 5000, 5000, 0, 0, 800, 600)!;
    expect(r).toEqual({ visL: 0, visR: 800, visT: 0, visB: 600 });
  });

  it("图片完全在视口上方（已滚过）→ null", () => {
    // 图片 [0,100]×[56,256]，视口 [0,100]×[1000,1600]
    expect(computeVisibleRect(0, 56, 100, 200, 0, 1000, 100, 600)).toBeNull();
  });

  it("图片完全在视口下方（未滚到）→ null", () => {
    expect(computeVisibleRect(0, 5000, 100, 200, 0, 0, 100, 600)).toBeNull();
  });

  it("图片完全在视口左侧（水平滚过）→ null", () => {
    expect(computeVisibleRect(0, 0, 100, 100, 5000, 0, 800, 600)).toBeNull();
  });

  it("图片完全在视口右侧 → null", () => {
    expect(computeVisibleRect(5000, 0, 100, 100, 0, 0, 800, 600)).toBeNull();
  });

  it("图片恰好贴视口边（visR==visL 边界）→ null（无面积）", () => {
    // 图片 [0,100]，视口 [100,900]：visL=100, visR=100 → 相切无面积
    expect(computeVisibleRect(0, 0, 100, 100, 100, 0, 800, 600)).toBeNull();
  });
});

describe("visibleToViewport", () => {
  it("露出区减滚动偏移 = 视口坐标", () => {
    // 露出区 content [1000,1600]×[200,800]，scrollLeft=1000, scrollTop=200
    const dst = visibleToViewport(
      { visL: 1000, visR: 1600, visT: 200, visB: 800 }, 1000, 200,
    );
    expect(dst).toEqual({ dstL: 0, dstT: 0, dstW: 600, dstH: 600 });
  });

  it("图片居中且小于视口：dstL>0（画在视口中央偏移处）", () => {
    // 露出区 = 图片 [350,450]（居中于 800 宽视口），无滚动
    const dst = visibleToViewport(
      { visL: 350, visR: 450, visT: 56, visB: 156 }, 0, 0,
    );
    expect(dst.dstL).toBe(350); // 图片画在视口 x=350（居中）
    expect(dst.dstW).toBe(100);
  });
});

describe("computeSrcSlice", () => {
  it("bitmap 物理空间：露出区左上角 → sx=0", () => {
    // dispW=100，imgLeft=0；bitmap 物理宽=200（=dispW*dpr, dpr=2）。露出区 visL=imgLeft=0
    const s = computeSrcSlice(
      { visL: 0, visR: 50, visT: 0, visB: 50 }, 0, 0, 100, 100, 200, 200,
    );
    expect(s.sx).toBe(0);
    expect(s.sy).toBe(0);
    expect(s.sw).toBe(100); // 50% of 200
    expect(s.sh).toBe(100);
  });

  it("bitmap：露出区中部 → src 偏移正确", () => {
    // dispW=100, imgLeft=0；bitmap 物理宽 200。露出区 visL=25（25%）
    const s = computeSrcSlice(
      { visL: 25, visR: 75, visT: 0, visB: 100 }, 0, 0, 100, 100, 200, 200,
    );
    expect(s.sx).toBe(50); // 25% of 200
    expect(s.sw).toBe(100); // 50% of 200
  });

  it("img 自然空间（无 bitmap）：srcW=naturalWidth，公式等价", () => {
    // dispW=400（natW=400, zoom=1），imgLeft=0，srcW=naturalWidth=400。露出区 visL=100
    const s = computeSrcSlice(
      { visL: 100, visR: 300, visT: 0, visB: 200 }, 0, 0, 400, 400, 400, 400,
    );
    expect(s.sx).toBe(100); // 自然像素 x=100
    expect(s.sw).toBe(200);
  });

  it("带 imgLeft 偏移：露出区减 imgLeft 再换算", () => {
    // 图片 content 左=200（居中偏移），dispW=400。露出区 visL=300（图片显示空间 100）
    const s = computeSrcSlice(
      { visL: 300, visR: 500, visT: 56, visB: 256 }, 200, 56, 400, 200, 400, 200,
    );
    expect(s.sx).toBe(100); // (300-200)/400 * 400
    expect(s.sy).toBe(0); // (56-56)/200 * 200
    expect(s.sw).toBe(200);
    expect(s.sh).toBe(200);
  });

  it("浮点安全：src 钳制在 [0,srcW]，不越界", () => {
    // 构造接近边界的值，确保 sw 不会超过 srcW-sx
    const s = computeSrcSlice(
      { visL: 0, visR: 100, visT: 0, visB: 100 }, 0, 0, 100, 100, 100, 100,
    );
    expect(s.sx).toBeGreaterThanOrEqual(0);
    expect(s.sx).toBeLessThanOrEqual(100);
    expect(s.sx + s.sw).toBeLessThanOrEqual(100 + 1e-9);
    expect(s.sy + s.sh).toBeLessThanOrEqual(100 + 1e-9);
  });
});

describe("长图崩盘场景（核心回归）", () => {
  it("长图 natH=20000，zoom=1，dpr=2 → dispH=20000，canvas 不应按整图设尺寸", () => {
    // 此测验证几何换算在超大 dispH 下仍正确（canvas 物理尺寸改视口固定是 drawBg 的职责，
    // 这里只验证 src/dst 换算不因 dispH 巨大而失真）
    const vis = computeVisibleRect(0, 56, 800, 20000, 0, 10000, 800, 600)!;
    expect(vis).not.toBeNull();
    expect(vis.visT).toBe(10000);
    expect(vis.visB).toBe(10600); // 满视口 600
    const dst = visibleToViewport(vis, 0, 10000);
    expect(dst.dstT).toBe(0); // 露出区贴视口顶
    expect(dst.dstH).toBe(600);
    // src：bitmap 物理高 = dispH*dpr = 40000（超 32767，但 bitmap 非 canvas 不崩；
    // 失败时 drawBg fallback 用 img naturalH=20000）。两种 srcW 都验证公式一致：
    const sBitmap = computeSrcSlice(vis, 0, 56, 800, 20000, 1600, 40000);
    expect(sBitmap.sy).toBeCloseTo((10000 - 56) / 20000 * 40000, 5);
    const sImg = computeSrcSlice(vis, 0, 56, 800, 20000, 800, 20000);
    expect(sImg.sy).toBeCloseTo((10000 - 56) / 20000 * 20000, 5);
  });
});
