// 共享标注类型 + 纯绘制/命中函数。Screenshot 与 ImagePreview 共用。
// 坐标空间由调用方决定：这些函数对坐标数值本身不做假设，
// 调用方负责把 ctx 变换（translate/scale）设好后再传入标注坐标。

export type Tool = "none" | "rect" | "oval" | "diamond" | "line" | "arrow" | "pen" | "text" | "number" | "blur";

export interface Annotation {
  type: "rect" | "oval" | "diamond" | "line" | "arrow" | "pen" | "text" | "number" | "blur";
  x1: number; y1: number; x2: number; y2: number;
  text?: string;
  points?: number[][];
  color?: string;
  lineWidth?: number;
  fontSize?: number;
  number?: number;
  circleSize?: number;
  textWidth?: number; // 文本最大宽度（自然像素），不折行时省略
  filled?: boolean; // rect/oval/diamond 是否实心填充
}

const HIT_DIST = 8;

export function drawAnnotation(ctx: CanvasRenderingContext2D, ann: Annotation) {
  const color = ann.color || "#ef4444";
  const lw = ann.lineWidth || 3;
  ctx.strokeStyle = color;
  ctx.fillStyle = color;
  ctx.lineWidth = lw;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";

  if (ann.type === "rect") {
    const x = Math.min(ann.x1, ann.x2);
    const y = Math.min(ann.y1, ann.y2);
    const w = Math.abs(ann.x2 - ann.x1);
    const h = Math.abs(ann.y2 - ann.y1);
    if (ann.filled) { ctx.fillRect(x, y, w, h); } else { ctx.strokeRect(x, y, w, h); }
  } else if (ann.type === "oval") {
    const cx = (ann.x1 + ann.x2) / 2;
    const cy = (ann.y1 + ann.y2) / 2;
    const rx = Math.max(1, Math.abs(ann.x2 - ann.x1) / 2);
    const ry = Math.max(1, Math.abs(ann.y2 - ann.y1) / 2);
    ctx.beginPath();
    ctx.ellipse(cx, cy, rx, ry, 0, 0, Math.PI * 2);
    if (ann.filled) ctx.fill(); else ctx.stroke();
  } else if (ann.type === "diamond") {
    const x = Math.min(ann.x1, ann.x2);
    const y = Math.min(ann.y1, ann.y2);
    const w = Math.abs(ann.x2 - ann.x1);
    const h = Math.abs(ann.y2 - ann.y1);
    const cx = x + w / 2, cy = y + h / 2;
    ctx.beginPath();
    ctx.moveTo(cx, y);
    ctx.lineTo(x + w, cy);
    ctx.lineTo(cx, y + h);
    ctx.lineTo(x, cy);
    ctx.closePath();
    if (ann.filled) ctx.fill(); else ctx.stroke();
  } else if (ann.type === "line") {
    ctx.beginPath();
    ctx.moveTo(ann.x1, ann.y1);
    ctx.lineTo(ann.x2, ann.y2);
    ctx.stroke();
  } else if (ann.type === "arrow") {
    const dx = ann.x2 - ann.x1;
    const dy = ann.y2 - ann.y1;
    const len = Math.sqrt(dx * dx + dy * dy);
    if (len < 5) return;
    ctx.beginPath();
    ctx.moveTo(ann.x1, ann.y1);
    ctx.lineTo(ann.x2, ann.y2);
    ctx.stroke();
    const angle = Math.atan2(dy, dx);
    const headLen = Math.max(12, lw * 3);
    ctx.beginPath();
    ctx.moveTo(ann.x2, ann.y2);
    ctx.lineTo(ann.x2 - headLen * Math.cos(angle - Math.PI / 6), ann.y2 - headLen * Math.sin(angle - Math.PI / 6));
    ctx.lineTo(ann.x2 - headLen * Math.cos(angle + Math.PI / 6), ann.y2 - headLen * Math.sin(angle + Math.PI / 6));
    ctx.closePath();
    ctx.fill();
  } else if (ann.type === "pen" && ann.points) {
    ctx.beginPath();
    for (let i = 0; i < ann.points.length; i++) {
      const [px, py] = ann.points[i];
      if (i === 0) ctx.moveTo(px, py);
      else ctx.lineTo(px, py);
    }
    ctx.stroke();
  } else if (ann.type === "text" && ann.text) {
    const fs = ann.fontSize || 16;
    const maxW = ann.textWidth || Infinity;
    ctx.font = `${fs}px -apple-system, sans-serif`;
    ctx.textBaseline = "top";
    drawMultilineText(ctx, ann.text, ann.x1, ann.y1, maxW, fs);
  } else if (ann.type === "number" && ann.number) {
    const r = (ann.circleSize || 24) / 2;
    const fs = (ann.circleSize || 24) * 0.6;
    ctx.beginPath();
    ctx.arc(ann.x1, ann.y1, r, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = "#ffffff";
    ctx.font = `bold ${fs}px -apple-system, sans-serif`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText(String(ann.number), ann.x1, ann.y1);
    ctx.textAlign = "start";
  }
  // blur 类型：canvas 预览画半透色块网格（导出时由调用方做像素马赛克）
  if (ann.type === "blur") {
    const bx = Math.min(ann.x1, ann.x2);
    const by = Math.min(ann.y1, ann.y2);
    const bw = Math.abs(ann.x2 - ann.x1);
    const bh = Math.abs(ann.y2 - ann.y1);
    if (bw < 2 || bh < 2) return;
    const opacity = ((lw || 5) / 10) * 0.85 + 0.1;
    const cell = Math.max(8, Math.min(bw, bh) / 8);
    const cols = Math.ceil(bw / cell);
    const rows = Math.ceil(bh / cell);
    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols; c++) {
        const hash = (c * 73856093 ^ r * 19349663) >>> 0;
        const variance = ((hash % 100) - 50) / 200;
        ctx.globalAlpha = Math.max(0, Math.min(1, opacity + variance));
        ctx.fillStyle = color;
        ctx.fillRect(bx + c * cell, by + r * cell, cell, cell);
      }
    }
    ctx.globalAlpha = 1;
    return;
  }
}

// 多行文字绘制：支持 \n 换行 + 超宽自动折行（模块私有，仅 drawAnnotation 内部用）
function drawMultilineText(ctx: CanvasRenderingContext2D, text: string, x: number, y: number, maxWidth: number, fontSize: number) {
  const lineHeight = fontSize * 1.3;
  // 先按 \n 切割为段落，再每段按 maxWidth 自动折行
  const paragraphs = text.split("\n");
  let cy = y;
  for (const para of paragraphs) {
    if (para === "") {
      cy += lineHeight;
      continue;
    }
    // 按字符测宽折行（适用于 CJK + ASCII 混合）
    let line = "";
    for (const ch of para) {
      const test = line + ch;
      if (ctx.measureText(test).width > maxWidth && line.length > 0) {
        ctx.fillText(line, x, cy);
        cy += lineHeight;
        line = ch;
      } else {
        line = test;
      }
    }
    if (line) {
      ctx.fillText(line, x, cy);
      cy += lineHeight;
    }
  }
}

// 合成到原图分辨率时用——坐标、线宽、字号全部 × scale
export function drawAnnotationScaled(ctx: CanvasRenderingContext2D, ann: Annotation, scale: number) {
  const color = ann.color || "#ef4444";
  const lw = (ann.lineWidth || 3) * scale;
  ctx.strokeStyle = color;
  ctx.fillStyle = color;
  ctx.lineWidth = lw;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";

  if (ann.type === "rect") {
    const x = Math.min(ann.x1, ann.x2) * scale;
    const y = Math.min(ann.y1, ann.y2) * scale;
    const w = Math.abs(ann.x2 - ann.x1) * scale;
    const h = Math.abs(ann.y2 - ann.y1) * scale;
    if (ann.filled) { ctx.fillRect(x, y, w, h); } else { ctx.strokeRect(x, y, w, h); }
  } else if (ann.type === "oval") {
    const cx = (ann.x1 + ann.x2) / 2 * scale;
    const cy = (ann.y1 + ann.y2) / 2 * scale;
    const rx = Math.max(1, Math.abs(ann.x2 - ann.x1) / 2 * scale);
    const ry = Math.max(1, Math.abs(ann.y2 - ann.y1) / 2 * scale);
    ctx.beginPath();
    ctx.ellipse(cx, cy, rx, ry, 0, 0, Math.PI * 2);
    if (ann.filled) ctx.fill(); else ctx.stroke();
  } else if (ann.type === "diamond") {
    const x = Math.min(ann.x1, ann.x2) * scale;
    const y = Math.min(ann.y1, ann.y2) * scale;
    const w = Math.abs(ann.x2 - ann.x1) * scale;
    const h = Math.abs(ann.y2 - ann.y1) * scale;
    const cx = x + w / 2, cy = y + h / 2;
    ctx.beginPath();
    ctx.moveTo(cx, y);
    ctx.lineTo(x + w, cy);
    ctx.lineTo(cx, y + h);
    ctx.lineTo(x, cy);
    ctx.closePath();
    if (ann.filled) ctx.fill(); else ctx.stroke();
  } else if (ann.type === "line") {
    ctx.beginPath();
    ctx.moveTo(ann.x1 * scale, ann.y1 * scale);
    ctx.lineTo(ann.x2 * scale, ann.y2 * scale);
    ctx.stroke();
  } else if (ann.type === "arrow") {
    const ax1 = ann.x1 * scale, ay1 = ann.y1 * scale;
    const ax2 = ann.x2 * scale, ay2 = ann.y2 * scale;
    const dx = ax2 - ax1, dy = ay2 - ay1;
    const len = Math.sqrt(dx * dx + dy * dy);
    if (len < 5 * scale) return;
    ctx.beginPath();
    ctx.moveTo(ax1, ay1);
    ctx.lineTo(ax2, ay2);
    ctx.stroke();
    const angle = Math.atan2(dy, dx);
    const headLen = 12 * scale;
    ctx.beginPath();
    ctx.moveTo(ax2, ay2);
    ctx.lineTo(ax2 - headLen * Math.cos(angle - Math.PI / 6), ay2 - headLen * Math.sin(angle - Math.PI / 6));
    ctx.lineTo(ax2 - headLen * Math.cos(angle + Math.PI / 6), ay2 - headLen * Math.sin(angle + Math.PI / 6));
    ctx.closePath();
    ctx.fill();
  } else if (ann.type === "pen" && ann.points) {
    ctx.beginPath();
    for (let i = 0; i < ann.points.length; i++) {
      const [px, py] = ann.points[i];
      if (i === 0) ctx.moveTo(px * scale, py * scale);
      else ctx.lineTo(px * scale, py * scale);
    }
    ctx.stroke();
  } else if (ann.type === "text" && ann.text) {
    const fs = (ann.fontSize || 16) * scale;
    const maxW = (ann.textWidth || Infinity) * scale;
    ctx.font = `${fs}px -apple-system, sans-serif`;
    ctx.textBaseline = "top";
    drawMultilineText(ctx, ann.text, ann.x1 * scale, ann.y1 * scale, maxW, fs);
  } else if (ann.type === "number" && ann.number) {
    const r = ((ann.circleSize || 24) * scale) / 2;
    const fs = ((ann.circleSize || 24) * scale) * 0.6;
    const cx = ann.x1 * scale;
    const cy = ann.y1 * scale;
    ctx.beginPath();
    ctx.arc(cx, cy, r, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = "#ffffff";
    ctx.font = `bold ${fs}px -apple-system, sans-serif`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText(String(ann.number), cx, cy);
    ctx.textAlign = "start";
  } else if (ann.type === "blur") {
    const bx = Math.min(ann.x1, ann.x2) * scale;
    const by = Math.min(ann.y1, ann.y2) * scale;
    const bw = Math.abs(ann.x2 - ann.x1) * scale;
    const bh = Math.abs(ann.y2 - ann.y1) * scale;
    if (bw < 2 || bh < 2) return;
    const opacity = ((lw || 5) / 10) * 0.85 + 0.1;
    const cell = Math.max(8, Math.min(bw, bh) / 8);
    const cols = Math.ceil(bw / cell);
    const rows = Math.ceil(bh / cell);
    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols; c++) {
        const hash = (c * 73856093 ^ r * 19349663) >>> 0;
        const variance = ((hash % 100) - 50) / 200;
        ctx.globalAlpha = Math.max(0, Math.min(1, opacity + variance));
        ctx.fillStyle = color;
        ctx.fillRect(bx + c * cell, by + r * cell, cell, cell);
      }
    }
    ctx.globalAlpha = 1;
  }
}

export function annBounds(ann: Annotation): { x: number; y: number; w: number; h: number } {
  if (ann.type === "text") {
    return { x: ann.x1 - 2, y: ann.y1 - 2, w: 200, h: (ann.fontSize || 16) + 6 };
  }
  if (ann.type === "number") {
    const r = (ann.circleSize || 24) / 2 + 2;
    return { x: ann.x1 - r, y: ann.y1 - r, w: r * 2, h: r * 2 };
  }
  if (ann.type === "pen" && ann.points && ann.points.length > 0) {
    const xs = ann.points.map(p => p[0]);
    const ys = ann.points.map(p => p[1]);
    return {
      x: Math.min(...xs) - 4, y: Math.min(...ys) - 4,
      w: Math.max(...xs) - Math.min(...xs) + 8,
      h: Math.max(...ys) - Math.min(...ys) + 8,
    };
  }
  return {
    x: Math.min(ann.x1, ann.x2) - 4,
    y: Math.min(ann.y1, ann.y2) - 4,
    w: Math.abs(ann.x2 - ann.x1) + 8,
    h: Math.abs(ann.y2 - ann.y1) + 8,
  };
}

// 精确命中：空心标注（rect/oval/line/arrow）检查到线条的距离，填充标注用 bounding box。
// anns 由调用方传入（抽取自 Screenshot 组件，原闭包 annotations 改为参数）。
export function hitTestAnnotationPrecise(
  mx: number,
  my: number,
  anns: Annotation[],
): number | null {
  for (let i = anns.length - 1; i >= 0; i--) {
    const ann = anns[i];
    if (ann.type === "rect") {
      // 矩形：检查到四条边的距离
      const x = Math.min(ann.x1, ann.x2);
      const y = Math.min(ann.y1, ann.y2);
      const w = Math.abs(ann.x2 - ann.x1);
      const h = Math.abs(ann.y2 - ann.y1);
      const onEdge = (Math.abs(mx - x) <= HIT_DIST || Math.abs(mx - (x + w)) <= HIT_DIST) && my >= y - HIT_DIST && my <= y + h + HIT_DIST
        || (Math.abs(my - y) <= HIT_DIST || Math.abs(my - (y + h)) <= HIT_DIST) && mx >= x - HIT_DIST && mx <= x + w + HIT_DIST;
      if (onEdge) return i;
    } else if (ann.type === "oval") {
      // 椭圆：检查到椭圆轮廓的距离
      const cx = (ann.x1 + ann.x2) / 2;
      const cy = (ann.y1 + ann.y2) / 2;
      const rx = Math.abs(ann.x2 - ann.x1) / 2;
      const ry = Math.abs(ann.y2 - ann.y1) / 2;
      if (rx < 1 || ry < 1) continue;
      const dx = (mx - cx) / rx;
      const dy = (my - cy) / ry;
      const dist = Math.abs(Math.sqrt(dx * dx + dy * dy) - 1) * Math.min(rx, ry);
      if (dist <= HIT_DIST) return i;
    } else if (ann.type === "line" || ann.type === "arrow") {
      // 线段：点到线段的距离
      if (pointToSegmentDist(mx, my, ann.x1, ann.y1, ann.x2, ann.y2) <= HIT_DIST) return i;
    } else if (ann.type === "pen" && ann.points) {
      // 自由曲线：检查到任意一段的距离
      for (let j = 1; j < ann.points.length; j++) {
        const [px1, py1] = ann.points[j - 1];
        const [px2, py2] = ann.points[j];
        if (pointToSegmentDist(mx, my, px1, py1, px2, py2) <= HIT_DIST) return i;
      }
    } else {
      // 文字/序号：bounding box
      const b = annBounds(ann);
      if (mx >= b.x && mx <= b.x + b.w && my >= b.y && my <= b.y + b.h) return i;
    }
  }
  return null;
}

export function pointToSegmentDist(px: number, py: number, x1: number, y1: number, x2: number, y2: number): number {
  const dx = x2 - x1, dy = y2 - y1;
  const lenSq = dx * dx + dy * dy;
  if (lenSq === 0) return Math.sqrt((px - x1) ** 2 + (py - y1) ** 2);
  let t = ((px - x1) * dx + (py - y1) * dy) / lenSq;
  t = Math.max(0, Math.min(1, t));
  const projX = x1 + t * dx;
  const projY = y1 + t * dy;
  return Math.sqrt((px - projX) ** 2 + (py - projY) ** 2);
}
