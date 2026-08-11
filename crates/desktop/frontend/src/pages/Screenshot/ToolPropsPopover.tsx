import { PRESET_COLORS } from "@/lib/annotation";
import { useT } from "@/lib/i18n";

export function ToolPropsPopover({
  x, y, color, width, fontSize, circleSize, isText, isNumber, isShape, isLine, isBlur, blurMode, shapeMode, lineMode, highlightVisible, filled, onColorChange, onWidthChange, onFontSizeChange, onCircleSizeChange, onFilledChange, onBlurModeChange, onShapeModeChange, onLineModeChange,
}: {
  x: number; y: number;
  color: string; width: number; fontSize: number; circleSize: number; isText: boolean; isNumber: boolean; isShape: boolean; isLine: boolean; isBlur: boolean; blurMode: "pixelate" | "gaussian" | "redact"; shapeMode: "rect" | "oval" | "diamond"; lineMode: "line" | "arrow" | "pen" | "highlight" | "number"; highlightVisible: boolean; filled: boolean;
  onColorChange: (c: string) => void;
  onWidthChange: (w: number) => void;
  onFontSizeChange: (s: number) => void;
  onCircleSizeChange: (s: number) => void;
  onFilledChange: (f: boolean) => void;
  onBlurModeChange: (m: "pixelate" | "gaussian" | "redact") => void;
  onShapeModeChange: (m: "rect" | "oval" | "diamond") => void;
  onLineModeChange: (m: "line" | "arrow" | "pen" | "highlight" | "number") => void;
}) {
  const t = useT();
  const sizeValue = isText ? fontSize : isNumber ? circleSize : width;
  const setSize = isText ? onFontSizeChange : isNumber ? onCircleSizeChange : onWidthChange;
  const min = isText ? 10 : isNumber ? 16 : 1;
  const max = isText ? 48 : isNumber ? 60 : 10;
  const label = isText ? t("screenshot.props.fontSize") : isNumber ? t("screenshot.props.circle") : t("screenshot.props.thickness");

  return (
    <div
      style={{
        position: "fixed",
        left: x,
        top: y,
        transform: "translateX(-50%)",
        padding: "10px 12px",
        background: "var(--color-surface)",
        color: "var(--color-foreground)",
        borderRadius: 10,
        boxShadow: "0 8px 24px -4px rgba(0,0,0,0.2), 0 2px 8px -2px rgba(0,0,0,0.1)",
        zIndex: 101,
        display: "flex",
        flexDirection: "column",
        gap: 10,
        width: 240,
      }}
    >
      {/* blur 模式选择（仅 blur 工具显示，置顶最显眼）：Pixelate / Gaussian / Redact */}
      {isBlur && (
        <>
          <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
            {(["pixelate", "gaussian", "redact"] as const).map((m) => (
              <button
                key={m}
                onClick={() => onBlurModeChange(m)}
                style={{
                  flex: 1,
                  padding: "4px 6px",
                  border: "none",
                  borderRadius: 5,
                  background: blurMode === m ? "var(--color-voice)" : "var(--color-accent, rgba(0,0,0,0.06))",
                  color: blurMode === m ? "#fff" : "var(--color-foreground)",
                  cursor: "pointer",
                  fontSize: 11,
                  fontWeight: 500,
                  transition: "background 0.15s",
                }}
              >
                {t(`screenshot.tool.blur_${m}`)}
              </button>
            ))}
          </div>
          <div style={{ height: 1, background: "var(--color-border)", margin: "0 -4px" }} />
        </>
      )}

      {/* 形状子模式选择（仅形状工具显示）：矩形 / 椭圆 / 菱形 */}
      {isShape && (
        <>
          <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
            {(["rect", "oval", "diamond"] as const).map((m) => (
              <button
                key={m}
                onClick={() => onShapeModeChange(m)}
                style={{
                  flex: 1,
                  padding: "4px 6px",
                  border: "none",
                  borderRadius: 5,
                  background: shapeMode === m ? "var(--color-voice)" : "var(--color-accent, rgba(0,0,0,0.06))",
                  color: shapeMode === m ? "#fff" : "var(--color-foreground)",
                  cursor: "pointer",
                  fontSize: 11,
                  fontWeight: 500,
                  transition: "background 0.15s",
                }}
              >
                {t(`screenshot.tool.${m === "oval" ? "ellipse" : m}`)}
              </button>
            ))}
          </div>
          {/* 实心填充 toggle */}
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <span style={{ fontSize: 10, color: "var(--color-muted-foreground)", fontWeight: 500 }}>{t("screenshot.props.solidFill")}</span>
            <button
              onClick={() => onFilledChange(!filled)}
              style={{
                width: 32, height: 18, borderRadius: 9, border: "none", cursor: "pointer",
                background: filled ? "var(--color-voice)" : "var(--color-accent, rgba(0,0,0,0.15))",
                position: "relative", transition: "background 0.15s",
              }}
            >
              <div style={{
                position: "absolute", top: 2, left: filled ? 16 : 2,
                width: 14, height: 14, borderRadius: "50%", background: "#fff",
                transition: "left 0.15s",
              }} />
            </button>
          </div>
          <div style={{ height: 1, background: "var(--color-border)", margin: "0 -4px" }} />
        </>
      )}

      {/* 线条子模式选择（仅线条工具显示）：直线 / 箭头 / 画笔 / 荧光 / 序号 */}
      {isLine && (
        <>
          <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
            {(highlightVisible
              ? (["line", "arrow", "pen", "highlight", "number"] as const)
              : (["line", "arrow", "pen", "number"] as const)
            ).map((m) => (
              <button
                key={m}
                onClick={() => onLineModeChange(m)}
                style={{
                  flex: 1,
                  padding: "4px 4px",
                  border: "none",
                  borderRadius: 5,
                  background: lineMode === m ? "var(--color-voice)" : "var(--color-accent, rgba(0,0,0,0.06))",
                  color: lineMode === m ? "#fff" : "var(--color-foreground)",
                  cursor: "pointer",
                  fontSize: 10,
                  fontWeight: 500,
                  transition: "background 0.15s",
                  whiteSpace: "nowrap",
                }}
              >
                {t(`screenshot.tool.${m}`)}
              </button>
            ))}
          </div>
          <div style={{ height: 1, background: "var(--color-border)", margin: "0 -4px" }} />
        </>
      )}

      {/* 粗细滑轨 + 当前色（最右） */}
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        <span style={{ fontSize: 10, color: "var(--color-muted-foreground)", width: 20, fontWeight: 500, flexShrink: 0 }}>{label}</span>
        <input
          type="range"
          min={min}
          max={max}
          value={sizeValue}
          onChange={(e) => setSize(Number(e.target.value))}
          style={{ flex: 1, height: 4, borderRadius: 2, cursor: "pointer", accentColor: color }}
        />
        <span style={{ fontSize: 10, color: "var(--color-muted-foreground)", fontVariantNumeric: "tabular-nums", width: 18, textAlign: "center", fontWeight: 600 }}>{sizeValue}</span>
        {/* 当前色 — 带粗白边 + 阴影，和下方预设色区分 */}
        <div style={{
          width: 20, height: 20, borderRadius: "50%",
          background: color,
          border: "3px solid #fff",
          boxShadow: "0 0 0 1.5px rgba(0,0,0,0.2), 0 1px 3px rgba(0,0,0,0.15)",
          flexShrink: 0,
        }} />
      </div>

      {/* 分隔线 */}
      <div style={{ height: 1, background: "var(--color-border)", margin: "0 -4px" }} />

      {/* 第二行：预设色 + 调色板 */}
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        {PRESET_COLORS.map((c) => (
          <button
            key={c}
            onClick={() => onColorChange(c)}
            style={{
              width: 18, height: 18, borderRadius: 5,
              background: c,
              border: c === "#ffffff" ? "1px solid #e0e0e0" : "none",
              cursor: "pointer",
              padding: 0,
              opacity: color.toLowerCase() === c.toLowerCase() ? 1 : 0.45,
              transform: color.toLowerCase() === c.toLowerCase() ? "scale(1.1)" : "scale(1)",
              transition: "opacity 0.15s, transform 0.15s",
            }}
          />
        ))}
        {/* 调色板 */}
        <label style={{ cursor: "pointer", display: "flex", alignItems: "center", flexShrink: 0, marginLeft: 2 }}>
          <div style={{
            width: 18, height: 18, borderRadius: 5,
            background: "conic-gradient(from 0deg, #ff0000, #ffff00, #00ff00, #00ffff, #0000ff, #ff00ff, #ff0000)",
            border: "1px solid rgba(0,0,0,0.1)",
          }} />
          <input
            type="color"
            value={color}
            onChange={(e) => onColorChange(e.target.value)}
            style={{ width: 0, height: 0, opacity: 0, position: "absolute" }}
          />
        </label>
      </div>
    </div>
  );
}
