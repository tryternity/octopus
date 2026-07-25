import { describe, it, expect } from "vitest";
import {
  computeToolbarPosition,
  computeToolbarCenterX,
  TOOLBAR_H,
  DOCK_MARGIN,
} from "./position";

describe("computeToolbarPosition", () => {
  it("below：选区下方有足够空间 → 默认放下方", () => {
    // sel 在视口上方，下方有 800-308=492px 空间，远大于 TOOLBAR_H
    const r = computeToolbarPosition({ x: 100, y: 100, w: 200, h: 200 }, 800);
    expect(r.placement).toBe("below");
    expect(r.belowOrAbove).toBe(true);
    // 下方 8px 间距
    expect(r.y).toBe(100 + 200 + 8);
  });

  it("above：下方不够、上方够 → 放上方", () => {
    // sel.y=700, h=50, viewport=800 → belowSpace=800-(700+50+8)=42 < 44；aboveSpace=700 > 44
    const r = computeToolbarPosition({ x: 100, y: 700, w: 200, h: 50 }, 800);
    expect(r.placement).toBe("above");
    expect(r.belowOrAbove).toBe(true);
    // 上方：sel.y - TOOLBAR_H - 4 = 700-44-4=652
    expect(r.y).toBe(700 - TOOLBAR_H - 4);
  });

  it("inside：上下都不够（全屏截图场景）→ 选区内部底部兜底", () => {
    // sel 占满视口：sel.y=0, h=800, viewport=800
    // belowSpace = 800-(0+800+8) = -8 < 44；aboveSpace = 0 < 44
    const r = computeToolbarPosition({ x: 0, y: 0, w: 1000, h: 800 }, 800);
    expect(r.placement).toBe("inside");
    expect(r.belowOrAbove).toBe(false);
    // inside：Math.max(sel.y, sel.y + sel.h - TOOLBAR_H - 8) = Math.max(0, 800-44-8) = 748
    expect(r.y).toBe(800 - TOOLBAR_H - 8);
  });

  it("below 时 y clamp 到 viewport - TOOLBAR_H（防止贴边跑出屏幕）", () => {
    // sel 紧贴视口底部：sel.y=740, h=20, viewport=800
    // belowSpace = 800-(740+20+8) = 32 < 44，但 aboveSpace=740 → 应走 above 分支
    const r = computeToolbarPosition({ x: 100, y: 740, w: 200, h: 20 }, 800);
    expect(r.placement).toBe("above");
  });

  it("below 时 sel.y+sel.h+8 仍小于 viewport 时 y 不 clamp", () => {
    // 普通居中选区，below 位置在视口内
    const r = computeToolbarPosition({ x: 100, y: 100, w: 200, h: 100 }, 800);
    expect(r.placement).toBe("below");
    expect(r.y).toBe(208);
  });

  it("below 边界：belowSpace 正好等于 TOOLBAR_H → below", () => {
    // 构造 belowSpace = TOOLBAR_H：sel.y+sel.h+8 = viewport - TOOLBAR_H
    // 选 sel.y=100, h=708-100=608, viewport=800 → belowSpace=800-(100+608+8)=84 > 44
    // 改：sel.y=100, h=648, viewport=800 → belowSpace=800-(100+648+8)=44 = TOOLBAR_H
    const r = computeToolbarPosition({ x: 100, y: 100, w: 200, h: 648 }, 800);
    expect(r.placement).toBe("below");
  });

  it("above 边界：aboveSpace 正好等于 TOOLBAR_H → above", () => {
    // belowSpace < TOOLBAR_H，aboveSpace = TOOLBAR_H
    // sel.y=44, h=750, viewport=800 → belowSpace=800-(44+750+8)=-2 < 44；aboveSpace=44
    const r = computeToolbarPosition({ x: 0, y: 44, w: 100, h: 750 }, 800);
    expect(r.placement).toBe("above");
    expect(r.y).toBe(44 - TOOLBAR_H - 4);
  });
});

describe("computeToolbarCenterX", () => {
  it("选区居中：X = sel.x + sel.w/2（toolbarW 不影响居中点）", () => {
    const x = computeToolbarCenterX({ x: 100, y: 0, w: 200, h: 100 }, 1000, 300);
    // sel.x + sel.w/2 = 200，clamp 范围 [80+150, 1000-80-150] = [230, 770]
    // 200 < 230 → clamp 到 230
    expect(x).toBe(DOCK_MARGIN + 150);
  });

  it("选区居中且足够宽：X 不被 clamp", () => {
    // sel 中心 = 500，clamp 范围 [80+50, 1000-80-50] = [130, 870] → 500 在范围内
    const x = computeToolbarCenterX({ x: 400, y: 0, w: 200, h: 100 }, 1000, 100);
    expect(x).toBe(500);
  });

  it("选区靠左：X clamp 到 DOCK_MARGIN + halfW", () => {
    // sel.x=0, sel.w=100, toolbarW=200 → sel 中心=50, halfW=100
    // clamp 范围 [80+100, 1000-80-100] = [180, 820] → 50 < 180
    const x = computeToolbarCenterX({ x: 0, y: 0, w: 100, h: 100 }, 1000, 200);
    expect(x).toBe(DOCK_MARGIN + 100);
  });

  it("选区靠右：X clamp 到 viewport - DOCK_MARGIN - halfW", () => {
    // sel.x=900, sel.w=100, toolbarW=200 → sel 中心=950, halfW=100
    // clamp 范围 [180, 820] → 950 > 820
    const x = computeToolbarCenterX({ x: 900, y: 0, w: 100, h: 100 }, 1000, 200);
    expect(x).toBe(1000 - DOCK_MARGIN - 100);
  });

  it("toolbarW=0 时 halfW=0，仍能 clamp（极端边界）", () => {
    // sel 中心 = 50，clamp 范围 [80, 1000-80] = [80, 920] → 50 < 80
    const x = computeToolbarCenterX({ x: 0, y: 0, w: 100, h: 100 }, 1000, 0);
    expect(x).toBe(DOCK_MARGIN);
  });

  it("视口很窄：仍保证不越界（clamp 到合法区间）", () => {
    // viewport=200, sel 占满，toolbarW=200 → clamp 范围 [80+100, 200-80-100] = [180, 20]
    // 区间为空（min > max），Math.max 优先 → 取 DOCK_MARGIN + halfW = 180
    const x = computeToolbarCenterX({ x: 0, y: 0, w: 200, h: 100 }, 200, 200);
    expect(x).toBe(DOCK_MARGIN + 100);
  });
});
