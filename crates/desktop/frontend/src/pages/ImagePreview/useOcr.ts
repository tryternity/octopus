// OCR hook：封装 OCR state + effect + handleOcr。
// 从 ImagePreview/index.tsx 拆出（2026-07-30）。
// 仅依赖 imageId，与其他轴（标注/QR/canvas）零耦合。

import { useState, useRef, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface OcrWord {
  text: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface OcrBlock {
  text: string;
  x: number;
  y: number;
  w: number;
  h: number;
  score: number;
  words?: OcrWord[];
}

export function useOcr(imageId: string | null) {
  const [ocrBlocks, setOcrBlocks] = useState<OcrBlock[]>([]);
  const [ocrOverlay, setOcrOverlay] = useState<'off' | 'select' | 'mask'>('off');
  const [ocrWarn, setOcrWarn] = useState(false);
  const ocrDoneRef = useRef(false);
  const ocrWarnTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => {
    if (ocrWarnTimerRef.current) clearTimeout(ocrWarnTimerRef.current);
  }, []);

  // 截图 OCR 推送事件监听——后台截图 OCR 推 blocks 过来时填充 ocrBlocks，
  // 但不自动 setOcrOverlay（用户打开图片是干净的，需手动点 OCR 按钮进入 select 态）。
  useEffect(() => {
    const unlistenOcr = listen<{ text: string; blocks: OcrBlock[] }>("ocr-screenshot://result", (e) => {
      if (e.payload.blocks.length > 0) {
        ocrDoneRef.current = true;
        setOcrBlocks(e.payload.blocks);
      }
    });
    return () => { unlistenOcr.then((f) => f()); };
  }, []);

  // imageId 变化时重置
  useEffect(() => {
    setOcrBlocks([]);
    setOcrOverlay('off');
    ocrDoneRef.current = false;
  }, [imageId]);

  const handleOcr = useCallback(async () => {
    if (imageId == null) return;
    // 已有 OCR 结果 → select ↔ mask 两态循环（select 透明文字层可选中，mask 纯文字白底黑字）
    if (ocrDoneRef.current && ocrBlocks.length > 0) {
      setOcrOverlay(prev => prev === 'select' ? 'mask' : 'select');
      return;
    }
    // 首次点击 → 触发 OCR → 进 select 态
    try {
      const result = await invoke<{ text: string; blocks: OcrBlock[] }>("ocr_image", { id: imageId });
      if (result?.blocks?.length) {
        ocrDoneRef.current = true;
        setOcrBlocks(result.blocks);
        setOcrOverlay('select');
      }
    } catch (e) {
      const msg = String(e);
      if (msg.includes("还未完成")) {
        setOcrWarn(true);
        if (ocrWarnTimerRef.current) clearTimeout(ocrWarnTimerRef.current);
        ocrWarnTimerRef.current = setTimeout(() => setOcrWarn(false), 1800);
      } else {
        console.error(e);
      }
    }
  }, [imageId, ocrBlocks.length]);

  return {
    ocrBlocks,
    ocrOverlay,
    ocrWarn,
    handleOcr,
  };
}
