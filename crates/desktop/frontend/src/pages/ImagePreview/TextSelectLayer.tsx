// HTML 透明文字层——每个 word 一个 span，原生拖选（对标 macOS Live Text）。
//
// 设计要点：
//   - color: transparent（用户看到原图文字，选中 overlay 透明文字）
//   - user-select: text（浏览器原生选择引擎）
//   - 容器 transform: scale(zoom) + 自然像素坐标 → zoom 变化零重算
//   - pointerEvents 受 tool 控制：tool="none" 时接管（拖选），其他工具放行（标注）

import { memo } from "react";
import type { OcrBlock, OcrWord } from "./useOcr";

interface Props {
  blocks: OcrBlock[];
  natW: number;
  natH: number;
  zoom: number;
  tool: string;
  imgLeft: number;
  imgTop: number;
}

/** fallback：block 无 words 时用整行作为一个"word" */
function blockToWords(b: OcrBlock): OcrWord[] {
  return b.words ?? [{ text: b.text, x: b.x, y: b.y, w: b.w, h: b.h }];
}

function TextSelectLayerBase({ blocks, natW, natH, zoom, tool, imgLeft, imgTop }: Props) {
  if (blocks.length === 0) return null;
  return (
    <div
      style={{
        position: "absolute",
        left: imgLeft,
        top: imgTop,
        width: natW,
        height: natH,
        transform: `scale(${zoom})`,
        transformOrigin: "0 0",
        pointerEvents: tool === "none" ? "auto" : "none",
        zIndex: 5,
      }}
    >
      {blocks.flatMap((b, bi) =>
        blockToWords(b).map((w, wi) => (
          <span
            key={`${bi}-${wi}`}
            style={{
              position: "absolute",
              left: w.x,
              top: w.y,
              fontSize: w.h * 0.85,
              lineHeight: `${w.h}px`,
              color: "transparent",
              userSelect: "text",
              cursor: "text",
              whiteSpace: "pre",
            }}
          >
            {w.text}
          </span>
        ))
      )}
    </div>
  );
}

export const TextSelectLayer = memo(TextSelectLayerBase);
