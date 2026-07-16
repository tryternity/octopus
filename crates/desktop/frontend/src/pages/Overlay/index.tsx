/**
 * Overlay — Run And Paste silent 模式的进度/toast 浮窗。
 *
 * 三种模式（由 overlay://show 事件 payload 决定）：
 * - loading：spinner + "正在执行 {action}... · 按 Esc 取消"
 * - toast warn：黄色图标 + message
 * - toast error：红色图标 + message
 */
import { useState, useEffect } from "react";
import { Loader2, AlertCircle, XCircle } from "lucide-react";
import { cn } from "@/lib/utils";
import { getCurrentWindow } from "@tauri-apps/api/window";

interface OverlayPayload {
  mode: "loading" | "toast";
  message: string;
  toastType: "warn" | "error";
  duration: number;
}

export default function Overlay() {
  const [payload, setPayload] = useState<OverlayPayload | null>(null);

  useEffect(() => {
    const unlisten = getCurrentWindow().listen<OverlayPayload>("overlay://show", (event) => {
      setPayload(event.payload);
    });
    const unlistenHide = getCurrentWindow().listen("overlay://hide", () => {
      setPayload(null);
    });
    return () => {
      unlisten.then((fn) => fn());
      unlistenHide.then((fn) => fn());
    };
  }, []);

  if (!payload) return null;

  const isToast = payload.mode === "toast";
  const isError = isToast && payload.toastType === "error";

  return (
    <div
      data-overlay
      className="flex items-center gap-2.5 px-4 h-[48px] rounded-[10px] border border-border/40 shadow-2xl shadow-black/20 overflow-hidden bg-background/90 backdrop-blur-2xl"
    >
      {payload.mode === "loading" && (
        <Loader2 className="w-4 h-4 animate-spin text-voice shrink-0" />
      )}
      {isToast && payload.toastType === "warn" && (
        <AlertCircle className="w-4 h-4 text-amber-500 shrink-0" />
      )}
      {isError && (
        <XCircle className="w-4 h-4 text-red-500 shrink-0" />
      )}
      <span
        className={cn(
          "text-[13px] font-medium leading-none whitespace-nowrap",
          isError ? "text-red-500" : "text-foreground",
        )}
      >
        {payload.message}
      </span>
      {payload.mode === "loading" && (
        <span className="text-[11px] text-muted-foreground/50 leading-none ml-auto">
          Esc 取消
        </span>
      )}
    </div>
  );
}
