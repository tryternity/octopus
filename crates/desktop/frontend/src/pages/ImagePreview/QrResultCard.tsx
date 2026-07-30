// 二维码识别结果白卡——纯展示组件。
// 从 ImagePreview/index.tsx 拆出（2026-07-30）。

import { openUrl } from "@tauri-apps/plugin-opener";
import { useT } from "@/lib/i18n";

interface QrResultCardProps {
  scanning: boolean;
  result: string[] | null;
  onClose: () => void;
}

export default function QrResultCard({ scanning, result, onClose }: QrResultCardProps) {
  const t = useT();

  return (
    <div style={{
      position: "absolute",
      top: 44 + 12,
      left: "50%",
      transform: "translateX(-50%)",
      width: "min(360px, 90%)",
      padding: "10px 12px",
      background: "#ffffff",
      color: "#1a1a1a",
      borderRadius: 10,
      boxShadow: "0 8px 24px rgba(0,0,0,0.25)",
      zIndex: 210,
      fontSize: 13,
      fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
    }}>
      <button
        onClick={onClose}
        title="✕"
        style={{
          position: "absolute", top: 4, right: 4,
          width: 22, height: 22, borderRadius: 5, border: "none", cursor: "pointer",
          background: "transparent", color: "#71717a", fontSize: 14, lineHeight: 1,
          display: "flex", alignItems: "center", justifyContent: "center",
        }}
        onMouseEnter={(e) => { e.currentTarget.style.background = "#f4f4f5"; }}
        onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
      >✕</button>

      {scanning ? (
        <div style={{ padding: "6px 0", color: "#52525b", textAlign: "center" }}>{t("imagePreview.qrScanning")}</div>
      ) : result && result.length > 0 ? (
        <div>
          <div style={{ display: "flex", flexDirection: "column", gap: 6, paddingRight: 20 }}>
            {result.map((c, i) => {
              const isUrl = /^https?:\/\//i.test(c);
              return (
                <div key={i} style={{ display: "flex", alignItems: "flex-start", gap: 4 }}>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    {isUrl ? (
                      <a
                        href={c}
                        onClick={(e) => { e.preventDefault(); openUrl(c).catch(() => {}); }}
                        style={{ color: "#2563eb", textDecoration: "underline", wordBreak: "break-all", cursor: "pointer", fontSize: 13, lineHeight: 1.4 }}
                        title={c}
                      >{c}</a>
                    ) : (
                      <div style={{ wordBreak: "break-all", whiteSpace: "pre-wrap", fontSize: 13, lineHeight: 1.4, color: "#1a1a1a" }}>{c}</div>
                    )}
                  </div>
                  <button
                    onClick={() => navigator.clipboard.writeText(c).catch(() => {})}
                    title="复制"
                    style={{
                      width: 24, height: 24, borderRadius: 4, border: "none",
                      cursor: "pointer", background: "transparent", color: "#71717a",
                      display: "flex", alignItems: "center", justifyContent: "center",
                      fontSize: 12, marginTop: -1,
                    }}
                    onMouseEnter={(e) => { e.currentTarget.style.background = "#f4f4f5"; e.currentTarget.style.color = "#3b82f6"; }}
                    onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; e.currentTarget.style.color = "#71717a"; }}
                  ><img src="icons/copy.svg" alt="复制" className="w-[14px] h-[14px]" style={{ filter: "var(--icon-filter)" }} /></button>
                </div>
              );
            })}
          </div>
          {result.length > 1 && (
            <div style={{ marginTop: 8, paddingTop: 6, borderTop: "1px solid #f0f0f0" }}>
              <button
                onClick={() => navigator.clipboard.writeText(result.join("\n")).catch(() => {})}
                style={{
                  width: "100%", padding: "5px 0", borderRadius: 5, border: "1px solid #e4e4e7",
                  cursor: "pointer", background: "#fafafa", color: "#52525b",
                  fontSize: 12, fontWeight: 500,
                }}
                onMouseEnter={(e) => { e.currentTarget.style.background = "#f4f4f5"; e.currentTarget.style.color = "#3b82f6"; }}
                onMouseLeave={(e) => { e.currentTarget.style.background = "#fafafa"; e.currentTarget.style.color = "#52525b"; }}
              >{t("imagePreview.qrCopyAll")}</button>
            </div>
          )}
        </div>
      ) : (
        <div style={{ padding: "6px 0", color: "#71717a", textAlign: "center" }}>{t("imagePreview.qrNoResult")}</div>
      )}
    </div>
  );
}
