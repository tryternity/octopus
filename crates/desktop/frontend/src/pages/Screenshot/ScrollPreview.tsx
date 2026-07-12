import { invoke } from "@/lib/tauri";
import { useT } from "@/lib/i18n";

interface ScrollPreviewProps {
  sel: { x: number; y: number; w: number; h: number };
  scrollPreview: string;
  scrollHeight: number;
}

export function ScrollPreview({ sel, scrollPreview, scrollHeight }: ScrollPreviewProps) {
  const t = useT();
  return (
    <div style={{
      position: "fixed",
      left: sel.x + sel.w + 12 + 200 <= window.innerWidth
        ? sel.x + sel.w + 12
        : sel.x - 12 - 200,
      bottom: window.innerHeight - sel.y - sel.h,
      width: 200,
      maxHeight: "80vh",
      background: "rgba(15,15,17,0.92)",
      backdropFilter: "blur(16px)",
      WebkitBackdropFilter: "blur(16px)",
      borderRadius: 10,
      padding: 10,
      display: "flex",
      flexDirection: "column",
      gap: 8,
      zIndex: 102,
      overflow: "hidden",
      boxShadow: "0 8px 32px rgba(0,0,0,0.5), 0 0 0 1px rgba(255,255,255,0.06)",
    }}>
      {/* 状态条：脉冲录制点 + 等宽高度计数器 */}
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", padding: "0 2px" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
          <div style={{
            width: 7, height: 7, borderRadius: "50%", background: "#f59e0b",
            boxShadow: "0 0 6px #f59e0b",
            animation: "pulse 1.5s ease-in-out infinite",
          }} />
          <span style={{ fontSize: 10, color: "#f59e0b", fontWeight: 600, letterSpacing: 0.3 }}>REC</span>
        </div>
        <span style={{ fontSize: 11, color: "rgba(255,255,255,0.55)", fontFamily: "SF Mono, Menlo, monospace", fontVariantNumeric: "tabular-nums" }}>
          {scrollHeight}px
        </span>
      </div>
      {/* 预览图 */}
      <div style={{ flex: 1, overflow: "hidden", borderRadius: 6, display: "flex", flexDirection: "column", justifyContent: "flex-end", background: "rgba(0,0,0,0.3)" }}>
        <img src={`data:image/png;base64,${scrollPreview}`} alt="preview" style={{ width: "100%", display: "block" }} />
      </div>
      {/* 按钮区：保存 复制 取消 */}
      <div style={{ display: "flex", gap: 6, height: 32 }}>
        <button onClick={() => invoke("stop_scroll_recording_with_mode", { mode: "save" }).catch(() => {})} style={{
          flex: 1, borderRadius: 6, border: "none",
          background: "var(--color-voice)", color: "#fff",
          fontSize: 12, fontWeight: 600, cursor: "pointer",
          transition: "background 0.15s",
        }}
        onMouseEnter={(e) => { e.currentTarget.style.background = "var(--color-voice)"; }}
        onMouseLeave={(e) => { e.currentTarget.style.background = "var(--color-voice)"; }}>
          {t("screenshot.save")}
        </button>
        <button onClick={() => invoke("stop_scroll_recording_with_mode", { mode: "copy" }).catch(() => {})} style={{
          flex: 1, borderRadius: 6, border: "none",
          background: "#22c55e", color: "#fff",
          fontSize: 12, fontWeight: 600, cursor: "pointer",
          transition: "background 0.15s",
        }}
        onMouseEnter={(e) => e.currentTarget.style.background = "#16a34a"}
        onMouseLeave={(e) => e.currentTarget.style.background = "#22c55e"}>
          {t("screenshot.copy")}
        </button>
        <button onClick={() => invoke("stop_scroll_recording_with_mode", { mode: "cancel" }).catch(() => {})} style={{
          flex: 1, borderRadius: 6,
          border: "1px solid rgba(255,255,255,0.15)",
          background: "transparent", color: "rgba(255,255,255,0.5)",
          fontSize: 12, cursor: "pointer",
          transition: "all 0.15s",
        }}
        onMouseEnter={(e) => { e.currentTarget.style.borderColor = "rgba(255,255,255,0.3)"; e.currentTarget.style.color = "rgba(255,255,255,0.8)"; }}
        onMouseLeave={(e) => { e.currentTarget.style.borderColor = "rgba(255,255,255,0.15)"; e.currentTarget.style.color = "rgba(255,255,255,0.5)"; }}>
          {t("screenshot.cancel")}
        </button>
      </div>
    </div>
  );
}
