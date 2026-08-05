// QR hook：封装二维码识别 state + handleQrScan。
// 从 ImagePreview/index.tsx 拆出（2026-07-30）。
// 仅依赖 imageId，与其他轴零耦合。

import { useState, useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

export function useQr(imageId: string | null) {
  const [qrScanning, setQrScanning] = useState(false);
  const [qrResult, setQrResult] = useState<string[] | null>(null);

  // imageId 变化时重置
  useEffect(() => {
    setQrResult(null);
    setQrScanning(false);
  }, [imageId]);

  const handleQrScan = useCallback(async () => {
    if (imageId == null) return;
    setQrScanning(true);
    setQrResult(null);
    try {
      const codes = await invoke<string[]>("scan_qrcode_image", { imageId });
      setQrResult(codes ?? []);
    } catch (e) {
      setQrResult([]);
      console.error("QR scan failed:", e);
    } finally {
      setQrScanning(false);
    }
  }, [imageId]);

  const closeQr = useCallback(() => {
    setQrResult(null);
    setQrScanning(false);
  }, []);

  return { qrScanning, qrResult, handleQrScan, closeQr };
}
