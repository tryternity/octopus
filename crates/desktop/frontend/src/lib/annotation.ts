// 共享标注类型 + 纯绘制/命中函数。Screenshot 与 ImagePreview 共用。
// 坐标空间由调用方决定：这些函数对坐标数值本身不做假设，
// 调用方负责把 ctx 变换（translate/scale）设好后再传入标注坐标。

export type Tool = "none" | "rect" | "oval" | "diamond" | "line" | "arrow" | "pen" | "highlight" | "text" | "number" | "blur" | "eraser";

/** 标注工具预设色板（Screenshot ToolPropsPopover 与 ImagePreview Toolbar 共用，含白色）。 */
export const PRESET_COLORS = ["#ef4444", "#f97316", "#eab308", "#22c55e", "#3b82f6", "#8b5cf6", "#000000", "#ffffff"];

export interface Annotation {
  type: "rect" | "oval" | "diamond" | "line" | "arrow" | "pen" | "highlight" | "text" | "number" | "blur";
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
  blurMode?: "pixelate" | "gaussian" | "redact"; // 仅 type="blur" 时有意义，默认 "pixelate"（老数据兼容）
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
  } else if (ann.type === "highlight") {
    // 荧光笔：pen 变体——multiply 混合 + alpha 0.35 + 粗线宽（默认 15）
    // save/restore 隔离 globalCompositeOperation/globalAlpha，避免污染后续标注
    ctx.save();
    ctx.globalCompositeOperation = "multiply";
    ctx.globalAlpha = 0.35;
    ctx.lineWidth = ann.lineWidth || 15;
    ctx.strokeStyle = color;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    if (ann.points && ann.points.length > 0) {
      ctx.beginPath();
      for (let i = 0; i < ann.points.length; i++) {
        const [px, py] = ann.points[i];
        if (i === 0) ctx.moveTo(px, py);
        else ctx.lineTo(px, py);
      }
      ctx.stroke();
    }
    ctx.restore();
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
    const mode = ann.blurMode ?? "pixelate";
    if (mode === "redact") {
      // 黑条预览：半透明黑色矩形（接近最终效果）
      ctx.fillStyle = "rgba(0, 0, 0, 0.85)";
      ctx.fillRect(bx, by, bw, bh);
    } else if (mode === "gaussian") {
      // 高斯预览：半透明灰色矩形 + 虚线边框（表示「将模糊」，不实时算 blur 避免卡顿）
      ctx.fillStyle = "rgba(128, 128, 128, 0.5)";
      ctx.fillRect(bx, by, bw, bh);
      ctx.strokeStyle = "rgba(128, 128, 128, 0.8)";
      ctx.lineWidth = 1;
      ctx.setLineDash([4, 4]);
      ctx.strokeRect(bx, by, bw, bh);
      ctx.setLineDash([]);
    } else {
      // pixelate 预览：色块网格（保留原有视觉）
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
  } else if (ann.type === "highlight") {
    // 荧光笔（合成到原图分辨率）：与 drawAnnotation 同语义，所有尺寸 × scale
    ctx.save();
    ctx.globalCompositeOperation = "multiply";
    ctx.globalAlpha = 0.35;
    ctx.lineWidth = (ann.lineWidth || 15) * scale;
    ctx.strokeStyle = color;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    if (ann.points && ann.points.length > 0) {
      ctx.beginPath();
      for (let i = 0; i < ann.points.length; i++) {
        const [px, py] = ann.points[i];
        if (i === 0) ctx.moveTo(px * scale, py * scale);
        else ctx.lineTo(px * scale, py * scale);
      }
      ctx.stroke();
    }
    ctx.restore();
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
    const mode = ann.blurMode ?? "pixelate";
    if (mode === "redact") {
      ctx.fillStyle = "rgba(0, 0, 0, 0.85)";
      ctx.fillRect(bx, by, bw, bh);
    } else if (mode === "gaussian") {
      ctx.fillStyle = "rgba(128, 128, 128, 0.5)";
      ctx.fillRect(bx, by, bw, bh);
      ctx.strokeStyle = "rgba(128, 128, 128, 0.8)";
      ctx.lineWidth = 1;
      ctx.setLineDash([4, 4]);
      ctx.strokeRect(bx, by, bw, bh);
      ctx.setLineDash([]);
    } else {
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
}

/**
 * 像素马赛克（blur 标注导出专用）：对 ctx 当前 canvas 上 ann 矩形区域做降采样模糊 + 色块网格叠加。
 * 与 drawAnnotation(Scaled) 的 blur 分支（仅预览色块网格、不降采样）的区别：这里先把背景缩小再放大 = 真正像素化。
 * scale 为坐标缩放系数（Screenshot 传 scale，ImagePreview 传 1）。
 * 调用方须在遍历标注时单独先处理 blur，并在随后的 drawAnnotation(Scaled) 循环里跳过 blur，
 * 否则色块网格叠加两次（见 Screenshot composePngBytes / ImagePreview composeAndCropBytes）。
 */
export function drawMosaic(ctx: CanvasRenderingContext2D, ann: Annotation, scale: number = 1) {
  const bx = Math.round(Math.min(ann.x1, ann.x2) * scale);
  const by = Math.round(Math.min(ann.y1, ann.y2) * scale);
  const bw = Math.round(Math.abs(ann.x2 - ann.x1) * scale);
  const bh = Math.round(Math.abs(ann.y2 - ann.y1) * scale);
  if (bw < 2 || bh < 2) return;
  const block = Math.max(4, Math.floor(Math.min(bw, bh) / 8));
  const tmp = document.createElement("canvas");
  tmp.width = Math.max(1, Math.floor(bw / block));
  tmp.height = Math.max(1, Math.floor(bh / block));
  const tctx = tmp.getContext("2d")!;
  tctx.imageSmoothingEnabled = false;
  tctx.drawImage(ctx.canvas, bx, by, bw, bh, 0, 0, tmp.width, tmp.height);
  ctx.imageSmoothingEnabled = false;
  ctx.drawImage(tmp, 0, 0, tmp.width, tmp.height, bx, by, bw, bh);
  ctx.imageSmoothingEnabled = true;
  const opacity = ((ann.lineWidth || 5) / 10) * 0.85 + 0.1;
  const blurColor = ann.color || "#808080";
  const cols = Math.ceil(bw / block);
  const rows = Math.ceil(bh / block);
  for (let r = 0; r < rows; r++) {
    for (let col = 0; col < cols; col++) {
      const hash = (col * 73856093 ^ r * 19349663) >>> 0;
      const variance = ((hash % 100) - 50) / 200;
      ctx.globalAlpha = Math.max(0, Math.min(1, opacity + variance));
      ctx.fillStyle = blurColor;
      ctx.fillRect(bx + col * block, by + r * block, block, block);
    }
  }
  ctx.globalAlpha = 1;
}

/**
 * 高斯模糊（Stackblur 算法——纯 JS 像素操作，不依赖 ctx.filter）。
 *
 * ctx.filter='blur' 在 WKWebView 上对 canvas 自身 drawImage 不可靠（自画自未定义行为），
 * 改用 stackblur 算法直接操作 ImageData，跨平台稳定。
 * 算法参考：Mario Klingemann 的 Stackblur，O(n) 复杂度。
 */
export function drawGaussian(ctx: CanvasRenderingContext2D, ann: Annotation, scale: number = 1) {
  const bx = Math.round(Math.min(ann.x1, ann.x2) * scale);
  const by = Math.round(Math.min(ann.y1, ann.y2) * scale);
  const bw = Math.round(Math.abs(ann.x2 - ann.x1) * scale);
  const bh = Math.round(Math.abs(ann.y2 - ann.y1) * scale);
  if (bw < 2 || bh < 2) return;
  const radius = Math.max(4, (ann.lineWidth || 3) * 3);
  // 1. 取选区像素
  const imageData = ctx.getImageData(bx, by, bw, bh);
  // 2. Stackblur
  stackBlurRGBA(imageData.data, bw, bh, radius);
  // 3. 画回原 canvas（clip 限定选区边界）
  ctx.save();
  ctx.beginPath();
  ctx.rect(bx, by, bw, bh);
  ctx.clip();
  ctx.putImageData(imageData, bx, by);
  ctx.restore();
}

/// Stackblur RGBA（Mario Klingemann 算法，O(n)）。
/// 原理：用 3 个栈（R/G/B）做水平 + 垂直两趟模糊，每趟用滑动窗口累加。
function stackBlurRGBA(pixels: Uint8ClampedArray, w: number, h: number, radius: number) {
  if (radius < 1) return;
  const div = radius * 2 + 1;
  const w4 = w * 4;
  const widthMinus1 = w - 1;
  const heightMinus1 = h - 1;
  const radiusPlus1 = radius + 1;

  const temp = new Uint8ClampedArray(pixels.length);

  // 水平模糊 → temp
  for (let y = 0; y < h; y++) {
    let sumR = 0, sumG = 0, sumB = 0, sumA = 0;
    const yOffset = y * w4;
    // 初始窗口（第一个像素重复 radius+1 次 + 后续 radius 个像素各 1 次）
    for (let i = -radius; i <= radius; i++) {
      const idx = yOffset + (Math.min(widthMinus1, Math.max(0, i)) * 4);
      sumR += pixels[idx]; sumG += pixels[idx + 1]; sumB += pixels[idx + 2]; sumA += pixels[idx + 3];
    }
    for (let x = 0; x < w; x++) {
      const outIdx = yOffset + x * 4;
      temp[outIdx] = sumR / div;
      temp[outIdx + 1] = sumG / div;
      temp[outIdx + 2] = sumB / div;
      temp[outIdx + 3] = sumA / div;
      // 滑动窗口：减左边出窗 + 加右边入窗
      const leftIdx = yOffset + (Math.max(0, x - radius) * 4);
      const rightIdx = yOffset + (Math.min(widthMinus1, x + radiusPlus1) * 4);
      sumR += pixels[rightIdx] - pixels[leftIdx];
      sumG += pixels[rightIdx + 1] - pixels[leftIdx + 1];
      sumB += pixels[rightIdx + 2] - pixels[leftIdx + 2];
      sumA += pixels[rightIdx + 3] - pixels[leftIdx + 3];
    }
  }

  // 垂直模糊 temp → pixels
  for (let x = 0; x < w; x++) {
    let sumR = 0, sumG = 0, sumB = 0, sumA = 0;
    const x4 = x * 4;
    for (let i = -radius; i <= radius; i++) {
      const idx = Math.min(heightMinus1, Math.max(0, i)) * w4 + x4;
      sumR += temp[idx]; sumG += temp[idx + 1]; sumB += temp[idx + 2]; sumA += temp[idx + 3];
    }
    for (let y = 0; y < h; y++) {
      const outIdx = y * w4 + x4;
      pixels[outIdx] = sumR / div;
      pixels[outIdx + 1] = sumG / div;
      pixels[outIdx + 2] = sumB / div;
      pixels[outIdx + 3] = sumA / div;
      const topIdx = Math.max(0, y - radius) * w4 + x4;
      const botIdx = Math.min(heightMinus1, y + radiusPlus1) * w4 + x4;
      sumR += temp[botIdx] - temp[topIdx];
      sumG += temp[botIdx + 1] - temp[topIdx + 1];
      sumB += temp[botIdx + 2] - temp[topIdx + 2];
      sumA += temp[botIdx + 3] - temp[topIdx + 3];
    }
  }
}

/**
 * 纯黑遮挡（Redact）——正式文档完全遮挡敏感信息。
 */
export function drawRedact(ctx: CanvasRenderingContext2D, ann: Annotation, scale: number = 1) {
  const bx = Math.round(Math.min(ann.x1, ann.x2) * scale);
  const by = Math.round(Math.min(ann.y1, ann.y2) * scale);
  const bw = Math.round(Math.abs(ann.x2 - ann.x1) * scale);
  const bh = Math.round(Math.abs(ann.y2 - ann.y1) * scale);
  if (bw < 2 || bh < 2) return;
  ctx.fillStyle = "#000000";
  ctx.fillRect(bx, by, bw, bh);
}

/**
 * blur 标注分发器——根据 ann.blurMode 调对应函数。
 * 调用方（composeAndCropBytes / ImagePreview / RecordAnnotation）统一用此入口，
 * 替换原直接调 drawMosaic 的 3 处调用点。
 */
export function drawBlur(ctx: CanvasRenderingContext2D, ann: Annotation, scale: number = 1) {
  switch (ann.blurMode ?? "pixelate") {
    case "gaussian": drawGaussian(ctx, ann, scale); break;
    case "redact":   drawRedact(ctx, ann, scale);   break;
    default:         drawMosaic(ctx, ann, scale);   break;
  }
}

export interface WatermarkOpts {
  text: string;
  opacity: number;       // 0-1
  fontSize: number;
  color?: string;        // 默认 "#ffffff"
  density: number;       // 0-1，控制平铺密度（0=单个居中，1=排满）
  angle: number;         // 旋转角度 0-360
}

/**
 * 截图水印——平铺模式。按 density 控制间距，angle 控制旋转角度。
 * 不进 annotations 数组（全局叠加层，config 驱动）。
 *
 * 算法：以 canvas 中心为原点旋转坐标系，在旋转后的网格上平铺水印文字。
 * density 0 = 网格间距很大（只显示 1 个居中水印）；density 1 = 紧密排列排满。
 */
export function drawWatermark(ctx: CanvasRenderingContext2D, canvasW: number, canvasH: number, opts: WatermarkOpts) {
  if (!opts.text) return;
  const cx = canvasW / 2;
  const cy = canvasH / 2;
  ctx.save();
  // 移到中心 + 旋转
  ctx.translate(cx, cy);
  ctx.rotate((opts.angle * Math.PI) / 180);
  ctx.globalAlpha = Math.max(0, Math.min(1, opts.opacity));
  ctx.fillStyle = opts.color ?? "#ffffff";
  ctx.font = `${opts.fontSize}px -apple-system, system-ui, sans-serif`;
  // 测量单个水印尺寸
  const tw = ctx.measureText(opts.text).width;
  const th = opts.fontSize;
  // 网格间距：density 0 = 很大间距（只 1 个），density 1 = 紧贴
  // 间距范围：tw*8（最稀疏）→ tw*1.5（最密集），垂直间距同理用 th 倍数
  const gapX = tw + tw * (1 - opts.density) * 6 + th * 0.5;
  const gapY = th + th * (1 - opts.density) * 4 + th * 0.5;
  // 旋转后需要覆盖更大的范围才能填满 canvas（对角线长度）
  const diag = Math.sqrt(canvasW * canvasW + canvasH * canvasH);
  const halfDiag = diag / 2;
  // 在旋转坐标系下平铺
  for (let y = -halfDiag; y <= halfDiag; y += gapY) {
    for (let x = -halfDiag; x <= halfDiag; x += gapX) {
      ctx.fillText(opts.text, x, y + th);
    }
  }
  ctx.restore();
}

export function annBounds(ann: Annotation): { x: number; y: number; w: number; h: number } {
  if (ann.type === "text") {
    return { x: ann.x1 - 2, y: ann.y1 - 2, w: 200, h: (ann.fontSize || 16) + 6 };
  }
  if (ann.type === "number") {
    const r = (ann.circleSize || 24) / 2 + 2;
    return { x: ann.x1 - r, y: ann.y1 - r, w: r * 2, h: r * 2 };
  }
  if ((ann.type === "pen" || ann.type === "highlight") && ann.points && ann.points.length > 0) {
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

// 精确命中：空心标注（rect/oval/diamond/line/arrow）检查到线条的距离；实心 rect/oval/diamond
// 判定鼠标是否在图形内部；文字/序号用 bounding box。anns 由调用方传入
// （抽取自 Screenshot 组件，原闭包 annotations 改为参数）。
export function hitTestAnnotationPrecise(
  mx: number,
  my: number,
  anns: Annotation[],
): number | null {
  for (let i = anns.length - 1; i >= 0; i--) {
    const ann = anns[i];
    if (ann.type === "rect") {
      // 矩形：实心查内部命中；空心查到四条边的距离
      const x = Math.min(ann.x1, ann.x2);
      const y = Math.min(ann.y1, ann.y2);
      const w = Math.abs(ann.x2 - ann.x1);
      const h = Math.abs(ann.y2 - ann.y1);
      if (ann.filled) {
        if (mx >= x && mx <= x + w && my >= y && my <= y + h) return i;
      } else {
        const onEdge = (Math.abs(mx - x) <= HIT_DIST || Math.abs(mx - (x + w)) <= HIT_DIST) && my >= y - HIT_DIST && my <= y + h + HIT_DIST
          || (Math.abs(my - y) <= HIT_DIST || Math.abs(my - (y + h)) <= HIT_DIST) && mx >= x - HIT_DIST && mx <= x + w + HIT_DIST;
        if (onEdge) return i;
      }
    } else if (ann.type === "oval") {
      // 椭圆：实心查内部命中；空心查到椭圆轮廓的距离
      const cx = (ann.x1 + ann.x2) / 2;
      const cy = (ann.y1 + ann.y2) / 2;
      const rx = Math.abs(ann.x2 - ann.x1) / 2;
      const ry = Math.abs(ann.y2 - ann.y1) / 2;
      if (rx < 1 || ry < 1) continue;
      const dx = (mx - cx) / rx;
      const dy = (my - cy) / ry;
      const r = Math.sqrt(dx * dx + dy * dy);
      if (ann.filled) {
        if (r <= 1 + HIT_DIST / Math.min(rx, ry)) return i;
      } else {
        if (Math.abs(r - 1) * Math.min(rx, ry) <= HIT_DIST) return i;
      }
    } else if (ann.type === "diamond") {
      // 菱形：实心查内部命中；空心查到四条斜边的距离（与 rect/oval 行为一致）
      const x = Math.min(ann.x1, ann.x2);
      const y = Math.min(ann.y1, ann.y2);
      const w = Math.abs(ann.x2 - ann.x1);
      const h = Math.abs(ann.y2 - ann.y1);
      const cx = x + w / 2, cy = y + h / 2;
      const halfW = w / 2, halfH = h / 2;
      if (halfW < 1 || halfH < 1) continue;
      // L1 范数归一：菱形边界 nd=1，内部 nd<1（|dx|/halfW + |dy|/halfH）
      const nd = Math.abs(mx - cx) / halfW + Math.abs(my - cy) / halfH;
      if (ann.filled) {
        if (nd <= 1 + HIT_DIST / Math.min(halfW, halfH)) return i;
      } else {
        if (Math.abs(nd - 1) * Math.min(halfW, halfH) <= HIT_DIST) return i;
      }
    } else if (ann.type === "line" || ann.type === "arrow") {
      // 线段：点到线段的距离
      if (pointToSegmentDist(mx, my, ann.x1, ann.y1, ann.x2, ann.y2) <= HIT_DIST) return i;
    } else if ((ann.type === "pen" || ann.type === "highlight") && ann.points) {
      // 自由曲线（pen / highlight 均为 points polyline）：检查到任意一段的距离
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
