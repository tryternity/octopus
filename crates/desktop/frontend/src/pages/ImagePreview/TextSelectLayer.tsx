// HTML 文字层——每个 word 一个 span，原生拖选（对标 macOS Live Text）。
//
// 两种模式（mode prop）：
//   - select：透明文字（rgba(0,0,0,0.01)），用户看到原图文字 + 选择高亮
//   - mask：黑色文字 + 白色背景 rect，纯文字展示 + 仍可选中复制
//
// 设计要点：
//   - 容器 pointer-events: auto（WKWebView 对 pointer-events:none 的容器内选择扩展不稳定）
//   - 空白区点击冒泡到 wrapper 做抓手平移（不 stopPropagation）
//   - -webkit-user-select: text + user-select: text
//   - 容器 transform: scale(zoom) + 自然像素坐标 → zoom 变化零重算

import { memo } from "react";
import type { OcrBlock, OcrWord } from "./useOcr";

interface Props {
  blocks: OcrBlock[];
  natW: number;
  natH: number;
  zoom: number;
  mode: "select" | "mask";
  imgLeft: number;
  imgTop: number;
}

/** fallback：block 无 words 时用整行作为一个"word" */
function blockToWords(b: OcrBlock): OcrWord[] {
  return b.words ?? [{ text: b.text, x: b.x, y: b.y, w: b.w, h: b.h }];
}

function TextSelectLayerBase({ blocks, natW, natH, zoom, mode, imgLeft, imgTop }: Props) {
  if (blocks.length === 0) return null;
  return (
    <div
      className="text-select-layer"
      style={{
        position: "absolute",
        left: imgLeft,
        top: imgTop,
        width: natW,
        height: natH,
        transform: `scale(${zoom})`,
        transformOrigin: "0 0",
        pointerEvents: "auto", // 选择扩展稳定；空白区 mousedown 冒泡到 wrapper 做抓手平移
        zIndex: 5,
      }}
    >
      {blocks.flatMap((b, bi) => [
        // mask 模式：block 白色背景遮住原图
        mode === "mask" && (
          <div key={`bg-${bi}`} style={{
            position: "absolute",
            left: b.x, top: b.y, width: b.w, height: b.h,
            background: "rgba(255,255,255,0.92)",
            border: "1px solid rgba(0,0,0,0.1)",
            borderRadius: 2,
            pointerEvents: "none",
          }} />
        ),
        // words
        ...blockToWords(b).map((w, wi) => (
          <span
            key={`${bi}-${wi}`}
            style={{
              position: "absolute",
              left: w.x,
              top: w.y,
              width: w.w,
              height: w.h,
              overflow: "hidden",
              fontSize: w.h * 0.85,
              lineHeight: `${w.h}px`,
              color: mode === "mask" ? "rgba(0,0,0,0.85)" : "rgba(0,0,0,0.01)",
              WebkitUserSelect: "text",
              userSelect: "text",
              cursor: "text",
              whiteSpace: "pre",
            }}
          >
            {w.text}
          </span>
        )),
      ])}
    </div>
  );
}

export const TextSelectLayer = memo(TextSelectLayerBase);
