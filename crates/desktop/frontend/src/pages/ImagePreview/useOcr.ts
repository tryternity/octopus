// OCR hook：封装 OCR state + effect + handleOcr。
// 从 ImagePreview/index.tsx 拆出（2026-07-30）。
// 仅依赖 imageId，与其他轴（标注/QR/canvas）零耦合。

import { useState, useRef, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openCompactEditorTab } from "@/lib/compactEditor";

export interface OcrBlock {
  text: string;
  x: number;
  y: number;
  w: number;
  h: number;
  score: number;
}

export function useOcr(imageId: string | null) {
  const [ocrBlocks, setOcrBlocks] = useState<OcrBlock[]>([]);
  const [ocrOverlay, setOcrOverlay] = useState<'off' | 'overlay' | 'mask'>('off');
  const [ocrCopied, setOcrCopied] = useState(false);
  const [ocrWarn, setOcrWarn] = useState(false);
  const [ocrCopiedText, setOcrCopiedText] = useState<string | null>(null);
  const ocrDoneRef = useRef(false);

  // 截图 OCR → 推送 OCR blocks。mount 时同时拉后端缓存。
  useEffect(() => {
    if (imageId != null) {
      invoke<{ text: string; blocks: OcrBlock[] } | null>("get_last_screenshot_ocr", { imageId })
        .then((res) => {
          if (!res || res.blocks.length === 0) return;
          ocrDoneRef.current = true;
          setOcrBlocks(res.blocks);
          setOcrOverlay('overlay');
        })
        .catch(() => {});
    }
    const unlistenOcr = listen<{ text: string; blocks: OcrBlock[] }>("ocr-screenshot://result", (e) => {
      if (e.payload.blocks.length > 0) {
        ocrDoneRef.current = true;
        setOcrBlocks(e.payload.blocks);
        setOcrOverlay('overlay');
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
    if (ocrDoneRef.current) {
      setOcrOverlay(ocrOverlay === 'off' ? 'overlay' : ocrOverlay === 'overlay' ? 'mask' : 'off');
      return;
    }
    try {
      const result = await invoke<{ text: string; blocks: OcrBlock[] }>("ocr_image", { id: imageId });
      if (result.text) {
        ocrDoneRef.current = true;
        setOcrBlocks(result.blocks);
        setOcrOverlay('overlay');
        const ocrId = await invoke<string>("insert_ocr_clipboard_item", { text: result.text });
        await openCompactEditorTab(ocrId);
        setOcrCopied(true);
        setTimeout(() => setOcrCopied(false), 1500);
      }
    } catch (e) {
      const msg = String(e);
      if (msg.includes("还未完成")) {
        setOcrWarn(true);
        setTimeout(() => setOcrWarn(false), 1800);
      } else {
        console.error(e);
      }
    }
  }, [imageId, ocrOverlay]);

  const handleOcrBlockCopy = useCallback((text: string, label: string) => {
    navigator.clipboard?.writeText(text).then(() => {
      setOcrCopiedText(label);
      setTimeout(() => setOcrCopiedText(null), 2000);
    }).catch(() => {});
  }, []);

  return {
    ocrBlocks,
    ocrOverlay,
    ocrCopied,
    ocrWarn,
    ocrCopiedText,
    handleOcr,
    handleOcrBlockCopy,
    setOcrCopiedText,
  };
}
