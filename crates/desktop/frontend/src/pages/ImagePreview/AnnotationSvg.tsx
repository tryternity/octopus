import { memo } from "react";
import type { Annotation } from "@/lib/annotation";

/**
 * 标注 SVG 渲染：每种标注类型 → 对应 SVG 元素。
 *
 * 坐标在 viewBox 自然空间（0 0 natW natH），SVG 自动缩放到 CSS 尺寸（= zoom）。
 * stroke / font 在自然坐标空间定义，显示时自动 ×zoom 缩放——与原 canvas ctx.scale(zoom) 行为一致。
 */
function AnnotationSvgImpl({ ann }: { ann: Annotation }) {
  const color = ann.color || "#ef4444";
  const lw = ann.lineWidth || 3;
  const strokeProps = {
    stroke: color, strokeWidth: lw, fill: "none",
    strokeLinecap: "round" as const, strokeLinejoin: "round" as const,
  };

  switch (ann.type) {
    case "rect": {
      const x = Math.min(ann.x1, ann.x2);
      const y = Math.min(ann.y1, ann.y2);
      const w = Math.abs(ann.x2 - ann.x1);
      const h = Math.abs(ann.y2 - ann.y1);
      return <rect x={x} y={y} width={w} height={h} {...strokeProps} />;
    }
    case "oval": {
      const cx = (ann.x1 + ann.x2) / 2;
      const cy = (ann.y1 + ann.y2) / 2;
      const rx = Math.max(1, Math.abs(ann.x2 - ann.x1) / 2);
      const ry = Math.max(1, Math.abs(ann.y2 - ann.y1) / 2);
      return <ellipse cx={cx} cy={cy} rx={rx} ry={ry} {...strokeProps} />;
    }
    case "line":
      return <line x1={ann.x1} y1={ann.y1} x2={ann.x2} y2={ann.y2} {...strokeProps} />;
    case "arrow": {
      const dx = ann.x2 - ann.x1;
      const dy = ann.y2 - ann.y1;
      const len = Math.sqrt(dx * dx + dy * dy);
      if (len < 5) return null;
      const angle = Math.atan2(dy, dx);
      const headLen = Math.max(12, lw * 3);
      const p2x = ann.x2 - headLen * Math.cos(angle - Math.PI / 6);
      const p2y = ann.y2 - headLen * Math.sin(angle - Math.PI / 6);
      const p3x = ann.x2 - headLen * Math.cos(angle + Math.PI / 6);
      const p3y = ann.y2 - headLen * Math.sin(angle + Math.PI / 6);
      return (
        <g>
          <line x1={ann.x1} y1={ann.y1} x2={ann.x2} y2={ann.y2} {...strokeProps} />
          <polygon points={`${ann.x2},${ann.y2} ${p2x},${p2y} ${p3x},${p3y}`} fill={color} />
        </g>
      );
    }
    case "pen":
      return (
        <polyline
          points={ann.points?.map((p) => `${p[0]},${p[1]}`).join(" ") || ""}
          {...strokeProps}
        />
      );
    case "text": {
      if (!ann.text) return null;
      const fs = ann.fontSize || 16;
      const lines = ann.text.split("\n");
      return (
        <g>
          {lines.map((line, i) => (
            <text
              key={i}
              x={ann.x1}
              y={ann.y1 + i * fs * 1.3}
              fontSize={fs}
              fill={color}
              fontFamily="-apple-system, sans-serif"
              dominantBaseline="hanging"
            >
              {line}
            </text>
          ))}
        </g>
      );
    }
    case "number": {
      if (!ann.number) return null;
      const r = (ann.circleSize || 24) / 2;
      const fs = (ann.circleSize || 24) * 0.6;
      return (
        <g>
          <circle cx={ann.x1} cy={ann.y1} r={r} fill={color} />
          <text
            x={ann.x1}
            y={ann.y1}
            fontSize={fs}
            fill="#ffffff"
            fontWeight="bold"
            fontFamily="-apple-system, sans-serif"
            textAnchor="middle"
            dominantBaseline="central"
          >
            {ann.number}
          </text>
        </g>
      );
    }
    case "blur": {
      const x = Math.min(ann.x1, ann.x2);
      const y = Math.min(ann.y1, ann.y2);
      const w = Math.abs(ann.x2 - ann.x1);
      const h = Math.abs(ann.y2 - ann.y1);
      // 粗细映射为不透明度（1=几乎透明 … 10=几乎不透明）
      const opacity = ((ann.lineWidth || 5) / 10) * 0.85 + 0.1;
      const cell = Math.max(8, Math.min(w, h) / 8);  // 马赛克块大小
      const cols = Math.ceil(w / cell);
      const rows = Math.ceil(h / cell);
      const blocks: React.ReactNode[] = [];
      for (let r = 0; r < rows; r++) {
        for (let c = 0; c < cols; c++) {
          // 每块用伪随机色调（基于坐标 hash），模拟马赛克色块
          const hash = (c * 73856093 ^ r * 19349663) >>> 0;
          const variance = ((hash % 100) - 50) / 200;  // ±0.25 色调偏移
          blocks.push(
            <rect
              key={`${r}-${c}`}
              x={x + c * cell} y={y + r * cell}
              width={cell} height={cell}
              fill={color}
              opacity={opacity + variance}
            />
          );
        }
      }
      return <g>{blocks}</g>;
    }
    default:
      return null;
  }
}

export const AnnotationSvg = memo(AnnotationSvgImpl);
