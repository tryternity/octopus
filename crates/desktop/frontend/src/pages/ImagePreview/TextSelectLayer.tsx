// HTML 透明文字层——每个 word 一个 span，原生拖选（对标 macOS Live Text）。
//
// 设计要点：
//   - color: rgba(0,0,0,0.01)（几乎不可见但非零 alpha——WKWebView 对 color:transparent
//     的文字不显示选择高亮，非零 alpha 让选择高亮可见）
//   - -webkit-user-select: text + user-select: text（WKWebView 需 WebKit 前缀）
//   - 容器 transform: scale(zoom) + 自然像素坐标 → zoom 变化零重算
//   - 容器 pointerEvents: none（事件穿透到 wrapper——空白区不挡抓手平移）
//   - 每个 span pointerEvents 受 tool 控制：tool="none" → auto（接管拖选）；
//     其他工具 → none（放行到标注层）。原生选择仍工作——mousedown 在 span 启动，
//     浏览器跨 pointer-events: none 区域扩展选择。

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
      className="text-select-layer"
      style={{
        position: "absolute",
        left: imgLeft,
        top: imgTop,
        width: natW,
        height: natH,
        transform: `scale(${zoom})`,
        transformOrigin: "0 0",
        pointerEvents: "none", // 容器穿透到 wrapper——空白区不挡抓手平移
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
              width: w.w,
              height: w.h,
              overflow: "hidden",
              fontSize: w.h * 0.85,
              lineHeight: `${w.h}px`,
              color: "rgba(0,0,0,0.01)", // 非零 alpha——WKWebView 对 transparent 不显示选择高亮
              WebkitUserSelect: "text",
              userSelect: "text",
              cursor: "text",
              whiteSpace: "pre",
              pointerEvents: tool === "none" ? "auto" : "none",
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
