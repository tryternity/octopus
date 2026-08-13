// OCR hook：封装 OCR state + effect + handleOcr。
// 从 ImagePreview/index.tsx 拆出（2026-07-30）。
// 仅依赖 imageId，与其他轴（标注/QR/canvas）零耦合。

import { useState, useRef, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openCompactEditorTab } from "@/lib/compactEditor";

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
  const [ocrOverlay, setOcrOverlay] = useState<'off' | 'overlay' | 'mask'>('off');
  const [ocrCopied, setOcrCopied] = useState(false);
  const [ocrWarn, setOcrWarn] = useState(false);
  const ocrDoneRef = useRef(false);
  // 第十五轮 P3-组4 #7：两处 setTimeout（ocrCopied / ocrWarn）原裸调用无 ref，
  // unmount 后仍 setState + 连续触发 timer stacking。各加独立 ref + 统一 unmount cleanup effect。
  const ocrCopiedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const ocrWarnTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => {
    if (ocrCopiedTimerRef.current) clearTimeout(ocrCopiedTimerRef.current);
    if (ocrWarnTimerRef.current) clearTimeout(ocrWarnTimerRef.current);
  }, []);

  // 截图 OCR 推送事件监听（仅注册 listener，不拉缓存）。
  // 开图流程的缓存拉取 / 自动 OCR 由下方 [imageId] effect 负责——
  // 那里有意不 setOcrOverlay 以保持图片干净（仅 TextSelectLayer 透明文字层）。
  // 若此处也拉缓存并 setOcrOverlay('overlay')，首图（mount effect 跑）会显示蓝框，
  // 后续切图（mount effect 不再跑）不显示 → UX 不一致。
  useEffect(() => {
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

  // 自动 OCR（图片打开无感知 OCR，文字层立即可选）。
  // 先拉截图缓存，无缓存则触发 ocr_image；OcrLockGuard 互斥（"还未完成"）时 1s 后重试一次。
  // 不 setOcrOverlay——仅填充 ocrBlocks 让 TextSelectLayer 显示；手动 OCR 按钮仍可循环切 overlay/mask 态。
  useEffect(() => {
    if (!imageId || ocrDoneRef.current) return;
    let cancelled = false;
    // OcrLockGuard 互斥重试 timer——unmount / imageId 切换时需 clearTimeout，
    // 否则滞后的 retry 会在组件已卸载后仍 invoke + setState。
    let retryTimer: ReturnType<typeof setTimeout> | null = null;
    invoke<{ text: string; blocks: OcrBlock[] } | null>("get_last_screenshot_ocr", { imageId })
      .then((cached) => {
        if (cancelled) return;
        if (cached?.blocks?.length) {
          setOcrBlocks(cached.blocks);
          ocrDoneRef.current = true;
          return;
        }
        // 无缓存 → 自动 OCR
        invoke<{ text: string; blocks: OcrBlock[] }>("ocr_image", { id: imageId })
          .then((result) => {
            if (cancelled) return;
            if (result?.blocks?.length) setOcrBlocks(result.blocks);
            ocrDoneRef.current = true;
          })
          .catch((e) => {
            if (cancelled) return;
            const msg = String(e);
            if (msg.includes("还未完成")) {
              // OcrLockGuard 互斥 → 1s 后重试一次（重试前再确认未完成）
              retryTimer = setTimeout(() => {
                if (cancelled || ocrDoneRef.current) return;
                invoke<{ text: string; blocks: OcrBlock[] }>("ocr_image", { id: imageId })
                  .then((r) => {
                    if (cancelled) return;
                    if (r?.blocks?.length) setOcrBlocks(r.blocks);
                    ocrDoneRef.current = true;
                  })
                  .catch(() => { if (!cancelled) ocrDoneRef.current = true; }); // 放弃，不影响看图
              }, 1000);
            } else {
              ocrDoneRef.current = true; // 其他错误静默
            }
          });
      })
      .catch(() => { if (!cancelled) ocrDoneRef.current = true; });
    return () => {
      cancelled = true;
      if (retryTimer) clearTimeout(retryTimer);
    };
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
        if (ocrCopiedTimerRef.current) clearTimeout(ocrCopiedTimerRef.current);
        ocrCopiedTimerRef.current = setTimeout(() => setOcrCopied(false), 1500);
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
  }, [imageId, ocrOverlay]);

  return {
    ocrBlocks,
    ocrOverlay,
    ocrCopied,
    ocrWarn,
    handleOcr,
  };
}
