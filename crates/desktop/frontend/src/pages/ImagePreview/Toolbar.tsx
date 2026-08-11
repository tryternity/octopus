import { useRef, useState, useEffect } from "react";
import {
  ZoomIn, ZoomOut, Expand, MoveHorizontal,
} from "lucide-react";
import { type Tool, PRESET_COLORS } from "@/lib/annotation";
import { useT } from "@/lib/i18n";

// SVG 图标 img（与截图工具一致，激活时变白）
const SvgIcon = ({ src, alt, active }: { src: string; alt: string; active?: boolean }) => (
  <img src={src} alt={alt} className="w-[18px] h-[18px]" style={{ filter: active ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
);

const POPOVER_W = 240;

/**
 * 工具按钮：32×32 r6，激活蓝底白字（对齐截图 ToolButton）。
 * 用内联 style 而非 Tailwind —— 与截图 ToolButton 的样式出处保持一致，
 * 便于以后两处同步微调。
 */
function ToolButton({ active, onClick, title, children }: {
  active?: boolean; onClick: (e: React.MouseEvent<HTMLButtonElement>) => void; title: string; children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      title={title}
      onClick={onClick}
      style={{
        width: 32, height: 32,
        display: "flex", alignItems: "center", justifyContent: "center",
        borderRadius: 6, border: "none", cursor: "pointer",
        background: active ? "var(--color-voice)" : "transparent",
        color: active ? "#fff" : "var(--color-foreground)",
        transition: "background 0.15s",
      }}
      onMouseEnter={(e) => { if (!active) e.currentTarget.style.background = "var(--color-muted)"; }}
      onMouseLeave={(e) => { if (!active) e.currentTarget.style.background = "transparent"; }}
    >
      {children}
    </button>
  );
}

/** 分隔线：与截图主工具栏竖线一致。 */
function Divider() {
  return <div style={{ width: 1, height: 20, background: "rgba(0,0,0,0.08)", margin: "0 4px" }} />;
}

/**
 * 图片预览工具栏：浮动白卡（对齐截图主工具栏），属性浮窗复刻截图 ToolPropsPopover。
 *
 * 出现方式 / 使用方式与截图完全一致：
 *  - 工具栏 = 漂浮白色圆角卡（fixed 居中贴顶，shadow）；
 *  - 选了任一标注工具后，属性浮窗从**该按钮**下方浮出（跟随按钮位置，clamp 防右溢出）；
 *  - 再点已激活的标注工具 = 切回选择（"none"），浮窗收起。
 *
 * 图片预览比截图多出的能力（保存/复制/OCR、缩放、置顶）作为同一张白卡的分组扩展。
 */
export default function Toolbar(props: {
  tool: Tool; setTool: (t: Tool) => void;
  toolColor: string; setToolColor: (c: string) => void;
  toolWidth: number; setToolWidth: (n: number) => void;
  toolFontSize: number; setToolFontSize: (n: number) => void;
  alwaysOnTop: boolean; onToggleTop: () => void;
  onSave: () => void; onCopy: () => void; onOcr: () => void;
  onQrScan: () => void;
  qrActive: boolean;
  onUndo: () => void; canUndo: boolean;
  onRedo: () => void; canRedo: boolean;
  onClearAll: () => void; canClearAll: boolean;
  ocrCopied: boolean;
  ocrWarn: boolean;
  ocrMode: 'off' | 'overlay' | 'mask';
  zoom: number; onZoomIn: () => void; onZoomOut: () => void; onZoomReset: () => void;
  onZoomFitWidth: () => void; onZoomFitWindow: () => void;
  filled: boolean; setFilled: (f: boolean) => void;
  // 合并工具子模式（与截图 AnnotationToolbar 一致）
  shapeMode: "rect" | "oval" | "diamond"; setShapeMode: (m: "rect" | "oval" | "diamond") => void;
  lineMode: "line" | "arrow" | "pen" | "highlight" | "number"; setLineMode: (m: "line" | "arrow" | "pen" | "highlight" | "number") => void;
  blurMode: "pixelate" | "gaussian" | "redact"; setBlurMode: (m: "pixelate" | "gaussian" | "redact") => void;
  toolCircleSize: number; setToolCircleSize: (n: number) => void;
  // 水印——Toolbar 内部渲染输入 popover（不用 window.prompt，WKWebView 不支持）
  watermarkColor: string; watermarkDensity: number; watermarkAngle: number;
  onWatermarkConfirm: (text: string, color: string, density: number, angle: number) => void;
  popoverDismissKey: number;  // 变化时收起浮窗
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  // 浮窗左偏移（相对工具卡），跟随被点击的标注按钮
  // 浮窗显隐：独立 state，不绑死 tool。用户操作画布时自动收起，需要改属性时重新点按钮弹出。
  const [showPopover, setShowPopover] = useState(false);
  // 用户在画布上操作时收起浮窗（popoverDismissKey 由 index.tsx mousedown 时递增）
  useEffect(() => { setShowPopover(false); }, [props.popoverDismissKey]);

  const [popoverLeft, setPopoverLeft] = useState(0);
  // 水印输入 popover（WKWebView 不支持 window.prompt，Toolbar 内部渲染）
  const [showWatermarkPopover, setShowWatermarkPopover] = useState(false);
  const [watermarkInput, setWatermarkInput] = useState("");
  const [watermarkColor, setWatermarkColor] = useState("#ffffff");
  const [watermarkDensity, setWatermarkDensity] = useState(0.5);
  const [watermarkAngle, setWatermarkAngle] = useState(0);
  const watermarkBtnRef = useRef<HTMLDivElement>(null);
  const watermarkPopoverRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!showWatermarkPopover) return;
    const handler = (e: MouseEvent) => {
      if (watermarkPopoverRef.current?.contains(e.target as Node)) return;
      if (watermarkBtnRef.current?.contains(e.target as Node)) return;
      setShowWatermarkPopover(false);
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [showWatermarkPopover]);
  const t = useT();

  const isText = props.tool === "text";
  const isBlur = props.tool === "blur";
  const isNumber = props.tool === "number";
  const isShape = props.tool === "rect" || props.tool === "oval" || props.tool === "diamond";
  const isLine = props.tool === "line" || props.tool === "arrow" || props.tool === "pen" || props.tool === "highlight" || props.tool === "number";
  const showProps = showPopover && props.tool !== "none";
  const sizeValue = isText ? props.toolFontSize : isNumber ? props.toolCircleSize : props.toolWidth;
  const setSize = isText ? props.setToolFontSize : isNumber ? props.setToolCircleSize : props.setToolWidth;
  const min = isText ? 10 : isNumber ? 16 : 1;
  const max = isText ? 48 : isNumber ? 60 : 10;
  const label = isText ? t("imagePreview.props.fontSize") : isNumber ? t("imagePreview.props.circle") : t("imagePreview.props.thickness");

  // 形状/线条子模式图标映射（图标随当前子模式变化）
  const shapeIcons: Record<string, string> = { rect: "icons/square.svg", oval: "icons/circle.svg", diamond: "icons/diamond.svg" };
  const lineIcons: Record<string, string> = { line: "icons/straight-line.svg", arrow: "icons/arrow-line.svg", pen: "icons/sketching.svg", highlight: "icons/highlighter.svg", number: "icons/sequence-note.svg" };

  // 标注工具点击：已激活→收起浮窗+切回 none；未激活→切换工具+弹出浮窗
  const onToolClick = (key: Tool, e: React.MouseEvent<HTMLButtonElement>) => {
    if (props.tool === key) {
      if (showPopover) {
        // 浮窗已显示 → 收起 + 切回 none
        setShowPopover(false);
        props.setTool("none");
      } else {
        // 浮窗已收起（画完后）→ 重新弹出
        setShowPopover(true);
      }
      return;
    }
    props.setTool(key);
    setShowPopover(true);
    // 浮窗跟随按钮
    const btn = e.currentTarget;
    const card = containerRef.current;
    if (card) {
      const center = btn.offsetLeft + btn.offsetWidth / 2;
      const cardW = card.offsetWidth;
      setPopoverLeft(Math.max(0, Math.min(center - POPOVER_W / 2, cardW - POPOVER_W)));
    }
  };

  // 非合并工具：text / blur
  const tools: { key: Tool; icon: React.ReactNode; title: string }[] = [
    { key: "text", icon: <SvgIcon src="icons/text.svg" alt={t("imagePreview.tool.text")} active={props.tool === "text"} />, title: t("imagePreview.tool.text") },
    { key: "blur", icon: <SvgIcon src="icons/mosaic.svg" alt={t("imagePreview.tool.mosaic")} active={props.tool === "blur"} />, title: t("imagePreview.tool.mosaic") },
  ];

  return (
    // 外层 absolute 容器（相对 ImagePreview 父容器定位）
    <div ref={containerRef} style={{ position: "absolute", left: "50%", top: 6, transform: "translateX(-50%)", zIndex: 100 }}>
      {/* 工具卡：白底 r8 + 截图同款 shadow */}
      <div style={{
        display: "flex", alignItems: "center", gap: 4,
        padding: "6px 8px", background: "var(--color-surface)", color: "var(--color-foreground)", borderRadius: 8,
        boxShadow: "0 4px 16px rgba(0,0,0,0.3)",
      }}>
        {/* 输出操作：保存 / 复制 / OCR（截图 SVG 图标） */}
        <ToolButton title={t("imagePreview.saveToFile")} active={false} onClick={() => props.onSave()}>
          <img src="icons/save.svg" alt={t("imagePreview.saveToFile")} className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)" }} />
        </ToolButton>
        <ToolButton title={t("imagePreview.copyToClipboard")} active={false} onClick={() => props.onCopy()}>
          <img src="icons/copy.svg" alt={t("imagePreview.copyToClipboard")} className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)" }} />
        </ToolButton>
        <div style={{ position: "relative" }}>
          <ToolButton title={props.ocrWarn ? t("imagePreview.ocrBusy") : t("imagePreview.ocr")} active={props.ocrCopied || props.ocrWarn || props.ocrMode !== 'off'} onClick={() => props.onOcr()}>
            {props.ocrCopied ? <img src="icons/check.svg" alt="完成" className="w-[18px] h-[18px]" style={{ filter: "brightness(0) invert(1)" }} /> : props.ocrMode === 'overlay' ? <img src="icons/ocr-all.svg" alt="OCR 叠加" className="w-[18px] h-[18px]" style={{ filter: "brightness(0) invert(1)" }} /> : props.ocrMode === 'mask' ? <img src="icons/ocr-text.svg" alt="OCR 遮罩" className="w-[18px] h-[18px]" style={{ filter: "brightness(0) invert(1)" }} /> : <img src="icons/ocr-ai.svg" alt="OCR" className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)" }} />}
          </ToolButton>
          {props.ocrWarn && (
            <span style={{
              position: "absolute", top: "calc(100% + 6px)", left: "50%", transform: "translateX(-50%)",
              whiteSpace: "nowrap", background: "#f59e0b", color: "#fff",
              fontSize: 10, fontWeight: 500, padding: "3px 8px", borderRadius: 5,
              boxShadow: "0 2px 8px rgba(0,0,0,0.2)", pointerEvents: "none", zIndex: 110,
            }}>
              {t("imagePreview.ocrBusy")}
            </span>
          )}
        </div>

        {/* 二维码识别：scan_qrcode_image，识别结果在白卡展示 */}
        <ToolButton title={t("imagePreview.tool.qrcode")} active={props.qrActive} onClick={() => props.onQrScan()}>
          <img src="icons/qr-code.svg" alt={t("imagePreview.tool.qrcode")} className="w-[18px] h-[18px]" style={{ filter: props.qrActive ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
        </ToolButton>

        <Divider />

        {/* 选择工具 */}
        <ToolButton title={t("imagePreview.tool.select")} active={props.tool === "none"}
          onClick={(e) => onToolClick("none", e)}>
          <SvgIcon src="icons/arrow-pointer.svg" alt={t("imagePreview.tool.select")} active={props.tool === "none"} />
        </ToolButton>

        {/* 形状按钮（rect/oval/diamond 合并）——图标随当前子模式变化 */}
        <ToolButton title={t("imagePreview.tool.rect")} active={isShape}
          onClick={(e) => onToolClick(props.shapeMode, e)}>
          <SvgIcon src={shapeIcons[props.shapeMode]} alt={t("imagePreview.tool.rect")} active={isShape} />
        </ToolButton>

        {/* 线条按钮（line/arrow/pen/highlight/number 合并）——图标随当前子模式变化 */}
        <ToolButton title={t("imagePreview.tool.line")} active={isLine}
          onClick={(e) => onToolClick(props.lineMode, e)}>
          <SvgIcon src={lineIcons[props.lineMode]} alt={t("imagePreview.tool.line")} active={isLine} />
        </ToolButton>

        {/* 标注工具：text / blur */}
        {tools.map((tt) => (
          <ToolButton key={tt.key} title={tt.title} active={props.tool === tt.key}
            onClick={(e) => onToolClick(tt.key, e)}>
            {tt.icon}
          </ToolButton>
        ))}

        {/* 水印按钮——弹输入框 popover（WKWebView 不支持 window.prompt） */}
        <div style={{ position: "relative" }}>
          <div ref={watermarkBtnRef}>
            <ToolButton title={t("screenshot.tool.watermark")} active={false}
              onClick={() => { setShowWatermarkPopover(v => !v); }}>
              <img src="icons/water-mark.svg" alt={t("screenshot.tool.watermark")} className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)" }} />
            </ToolButton>
          </div>
          {showWatermarkPopover && (
            <div ref={watermarkPopoverRef} style={{
              position: "absolute", top: "100%", left: "50%", transform: "translateX(-50%)", marginTop: 4,
              padding: 8, background: "var(--color-surface)", color: "var(--color-foreground)", borderRadius: 8,
              boxShadow: "0 8px 24px -4px rgba(0,0,0,0.2), 0 2px 8px -2px rgba(0,0,0,0.1)",
              zIndex: 102, display: "flex", flexDirection: "column", gap: 6, minWidth: 200,
            }}>
              <input type="text" value={watermarkInput} autoFocus
                onChange={(e) => setWatermarkInput(e.target.value)}
                onKeyDown={(e) => {
                  e.stopPropagation();
                  if (e.key === "Enter") { e.preventDefault(); props.onWatermarkConfirm(watermarkInput, watermarkColor, watermarkDensity, watermarkAngle); setShowWatermarkPopover(false); }
                  else if (e.key === "Escape") { setShowWatermarkPopover(false); }
                }}
                placeholder={t("screenshot.watermark.placeholder")}
                autoCapitalize="off" autoCorrect="off" spellCheck={false}
                style={{ padding: "5px 8px", border: "1px solid var(--color-border)", borderRadius: 5, background: "transparent", color: "inherit", fontSize: 13, outline: "none", width: "100%", boxSizing: "border-box" }}
              />
              {/* 预设色 + 调色板（可选） */}
              <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
                {PRESET_COLORS.map((c) => (
                  <button key={c} onClick={(e) => { e.stopPropagation(); setWatermarkColor(c); }}
                    style={{ width: 16, height: 16, borderRadius: 4, background: c, border: c === "#ffffff" ? "1px solid #e0e0e0" : "none", cursor: "pointer", padding: 0, opacity: watermarkColor.toLowerCase() === c.toLowerCase() ? 1 : 0.45, transform: watermarkColor.toLowerCase() === c.toLowerCase() ? "scale(1.1)" : "scale(1)" }} />
                ))}
                <label style={{ cursor: "pointer", display: "flex", alignItems: "center", flexShrink: 0, marginLeft: 2 }}>
                  <div style={{ width: 16, height: 16, borderRadius: 4, background: "conic-gradient(from 0deg, #ff0000, #ffff00, #00ff00, #00ffff, #0000ff, #ff00ff, #ff0000)", border: "1px solid rgba(0,0,0,0.1)" }} />
                  <input type="color" value={watermarkColor} onChange={(e) => setWatermarkColor(e.target.value)} style={{ width: 0, height: 0, opacity: 0, position: "absolute" }} />
                </label>
              </div>
              {/* 密度滑块 */}
              <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                <span style={{ fontSize: 10, color: "var(--color-muted-foreground)", width: 32, flexShrink: 0 }}>{t("settings.general.watermarkDensity")}</span>
                <input type="range" min={0} max={1} step={0.1} value={watermarkDensity}
                  onChange={(e) => setWatermarkDensity(Number(e.target.value))}
                  style={{ flex: 1, height: 4, cursor: "pointer" }} />
                <span style={{ fontSize: 10, color: "var(--color-muted-foreground)", width: 24, textAlign: "center" }}>{watermarkDensity.toFixed(1)}</span>
              </div>
              {/* 角度滑块 */}
              <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                <span style={{ fontSize: 10, color: "var(--color-muted-foreground)", width: 32, flexShrink: 0 }}>{t("settings.general.watermarkAngle")}</span>
                <input type="range" min={0} max={360} step={15} value={watermarkAngle}
                  onChange={(e) => setWatermarkAngle(Number(e.target.value))}
                  style={{ flex: 1, height: 4, cursor: "pointer" }} />
                <span style={{ fontSize: 10, color: "var(--color-muted-foreground)", width: 24, textAlign: "center" }}>{Math.round(watermarkAngle)}°</span>
              </div>
              <button onClick={(e) => { e.stopPropagation(); props.onWatermarkConfirm(watermarkInput, watermarkColor, watermarkDensity, watermarkAngle); setShowWatermarkPopover(false); }}
                style={{ padding: "5px 8px", border: "none", borderRadius: 5, background: "var(--color-voice)", color: "#fff", cursor: "pointer", fontSize: 12, fontWeight: 500 }}>
                {t("screenshot.watermark.confirm")}
              </button>
            </div>
          )}
        </div>
        {/* 橡皮擦：不弹 popover，直接切工具 */}
        <ToolButton title={t("imagePreview.tool.eraser")} active={props.tool === "eraser"}
          onClick={() => {
            if (props.tool === "eraser") {
              props.setTool("none");
            } else {
              props.setTool("eraser");
              setShowPopover(false);
            }
          }}>
          <SvgIcon src="icons/eraser.svg" alt={t("imagePreview.tool.eraser")} active={props.tool === "eraser"} />
        </ToolButton>
        <ToolButton title={t("imagePreview.undo")} active={false} onClick={() => props.onUndo()}>
          <img src="icons/restore.svg" alt={t("imagePreview.undo")} className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)", opacity: props.canUndo ? 1 : 0.3 }} />
        </ToolButton>
        <ToolButton title={t("imagePreview.redo")} active={false} onClick={() => props.onRedo()}>
          <img src="icons/redo.svg" alt={t("imagePreview.redo")} className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)", opacity: props.canRedo ? 1 : 0.3 }} />
        </ToolButton>
        {/* 清空全部 */}
        <ToolButton title={t("imagePreview.clear")} active={false} onClick={() => props.onClearAll()}>
          <img src="icons/clear.svg" alt={t("imagePreview.clear")} className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)", opacity: props.canClearAll ? 1 : 0.3 }} />
        </ToolButton>

        {/* 缩放：缩小 + 当前百分比(点击重置 100%) + 放大 */}
        <Divider />
        <ToolButton title={t("imagePreview.zoomOut")} active={false} onClick={() => props.onZoomOut()}>
          <ZoomOut className="h-[18px] w-[18px]" />
        </ToolButton>
        <button
          type="button"
          title={t("imagePreview.resetZoom")}
          onClick={() => props.onZoomReset()}
          style={{
            height: 32, minWidth: 52, padding: "0 6px", border: "none", cursor: "pointer",
            display: "flex", alignItems: "center", justifyContent: "center",
            borderRadius: 6, background: "transparent", color: "var(--color-foreground)",
            fontSize: 12, fontWeight: 600,
            fontFamily: "SF Mono, Menlo, monospace", fontVariantNumeric: "tabular-nums",
            transition: "background 0.15s",
          }}
          onMouseEnter={(e) => { e.currentTarget.style.background = "var(--color-muted)"; }}
          onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
        >
          {Math.round(props.zoom * 100)}%
        </button>
        <ToolButton title={t("imagePreview.zoomIn")} active={false} onClick={() => props.onZoomIn()}>
          <ZoomIn className="h-[18px] w-[18px]" />
        </ToolButton>
        <ToolButton title={t("imagePreview.fitWidth")} active={false} onClick={() => props.onZoomFitWidth()}>
          <MoveHorizontal className="h-[18px] w-[18px]" />
        </ToolButton>
        <ToolButton title={t("imagePreview.fitWindow")} active={false} onClick={() => props.onZoomFitWindow()}>
          <Expand className="h-[18px] w-[18px]" />
        </ToolButton>

        {/* 置顶单独推到最右 */}
        <Divider />
        <ToolButton title={props.alwaysOnTop ? t("imagePreview.unpinWindow") : t("imagePreview.pinWindow")}
          active={props.alwaysOnTop} onClick={() => props.onToggleTop()}>
          <img src="icons/pin.svg" alt="置顶" className="w-[18px] h-[18px]" style={{ filter: props.alwaysOnTop ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
        </ToolButton>
      </div>

      {/* 属性浮窗：复刻截图 ToolPropsPopover —— 跟随激活按钮（left=popoverLeft），白卡 r10、两行 */}
      {showProps && (
        <div
          onMouseDown={(e) => e.stopPropagation()}
          style={{
            position: "absolute", left: popoverLeft, top: "calc(100% + 6px)",
            padding: "10px 12px", background: "var(--color-surface)", color: "var(--color-foreground)", borderRadius: 10,
            boxShadow: "0 8px 24px -4px rgba(0,0,0,0.2), 0 2px 8px -2px rgba(0,0,0,0.1)",
            display: "flex", flexDirection: "column", gap: 10, width: POPOVER_W,
          }}
        >
          {/* blur 模式选择（仅 blur 工具显示，置顶） */}
          {isBlur && (
            <>
              <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                {(["pixelate", "gaussian", "redact"] as const).map((m) => (
                  <button key={m} onClick={() => props.setBlurMode(m)} style={{
                    flex: 1, padding: "4px 6px", border: "none", borderRadius: 5, cursor: "pointer",
                    fontSize: 11, fontWeight: 500, transition: "background 0.15s",
                    background: props.blurMode === m ? "var(--color-voice)" : "var(--color-accent, rgba(0,0,0,0.06))",
                    color: props.blurMode === m ? "#fff" : "var(--color-foreground)",
                  }}>{t(`screenshot.tool.blur_${m}`)}</button>
                ))}
              </div>
              <div style={{ height: 1, background: "rgba(0,0,0,0.06)", margin: "0 -4px" }} />
            </>
          )}

          {/* 形状子模式（仅形状工具） */}
          {isShape && (
            <>
              <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                {(["rect", "oval", "diamond"] as const).map((m) => (
                  <button key={m} onClick={() => { props.setShapeMode(m); props.setTool(m); }} style={{
                    flex: 1, padding: "4px 6px", border: "none", borderRadius: 5, cursor: "pointer",
                    fontSize: 11, fontWeight: 500, transition: "background 0.15s",
                    background: props.shapeMode === m ? "var(--color-voice)" : "var(--color-accent, rgba(0,0,0,0.06))",
                    color: props.shapeMode === m ? "#fff" : "var(--color-foreground)",
                  }}>{t(`imagePreview.tool.${m === "oval" ? "ellipse" : m}`)}</button>
                ))}
              </div>
              {/* 实心填充 toggle */}
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <span style={{ fontSize: 10, color: "var(--color-muted-foreground)", fontWeight: 500 }}>{t("imagePreview.props.solidFill")}</span>
                <button onClick={() => props.setFilled(!props.filled)} style={{
                  width: 32, height: 18, borderRadius: 9, border: "none", cursor: "pointer",
                  background: props.filled ? "var(--color-voice)" : "var(--color-accent, rgba(0,0,0,0.15))",
                  position: "relative", transition: "background 0.15s",
                }}>
                  <div style={{ position: "absolute", top: 2, left: props.filled ? 16 : 2, width: 14, height: 14, borderRadius: "50%", background: "#fff", transition: "left 0.15s" }} />
                </button>
              </div>
              <div style={{ height: 1, background: "rgba(0,0,0,0.06)", margin: "0 -4px" }} />
            </>
          )}

          {/* 线条子模式（仅线条工具） */}
          {isLine && (
            <>
              <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
                {(["line", "arrow", "pen", "highlight", "number"] as const).map((m) => (
                  <button key={m} onClick={() => { props.setLineMode(m); props.setTool(m); }} style={{
                    flex: 1, padding: "4px 4px", border: "none", borderRadius: 5, cursor: "pointer",
                    fontSize: 10, fontWeight: 500, whiteSpace: "nowrap", transition: "background 0.15s",
                    background: props.lineMode === m ? "var(--color-voice)" : "var(--color-accent, rgba(0,0,0,0.06))",
                    color: props.lineMode === m ? "#fff" : "var(--color-foreground)",
                  }}>{t(`imagePreview.tool.${m}`)}</button>
                ))}
              </div>
              <div style={{ height: 1, background: "rgba(0,0,0,0.06)", margin: "0 -4px" }} />
            </>
          )}

          {/* 行 1：粗细/字号滑轨 + 当前色（最右） */}
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <span style={{ fontSize: 10, color: "var(--color-muted-foreground)", width: 20, fontWeight: 500, flexShrink: 0 }}>{label}</span>
            <input
              type="range" min={min} max={max} value={sizeValue}
              onChange={(e) => setSize(Number(e.target.value))}
              style={{ flex: 1, height: 4, borderRadius: 2, cursor: "pointer", accentColor: props.toolColor }}
            />
            <span style={{ fontSize: 10, color: "var(--color-foreground)", width: 18, textAlign: "center", fontWeight: 600,
              fontFamily: "SF Mono, Menlo, monospace", fontVariantNumeric: "tabular-nums" }}>
              {sizeValue}
            </span>
            {/* 当前色：粗白边 + 阴影，与下方预设色区分 */}
            <div style={{
              width: 20, height: 20, borderRadius: "50%", background: props.toolColor, flexShrink: 0,
              border: "3px solid var(--color-surface)",
              boxShadow: "0 0 0 1.5px rgba(0,0,0,0.2), 0 1px 3px rgba(0,0,0,0.15)",
            }} />
          </div>

          {/* 分隔线 */}
          <div style={{ height: 1, background: "rgba(0,0,0,0.06)", margin: "0 -4px" }} />

          {/* 行 2：预设色 —— 全 opacity，active 用蓝 ring 增强 */}
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            {PRESET_COLORS.map((c) => {
              const isActive = props.toolColor.toLowerCase() === c.toLowerCase();
              return (
                <button
                  key={c} type="button"
                  onClick={() => props.setToolColor(c)}
                  title={c}
                  style={{
                    width: 18, height: 18, borderRadius: 5, background: c, padding: 0, cursor: "pointer",
                    border: c === "#ffffff" ? "1px solid rgba(0,0,0,0.12)" : "none",
                    boxShadow: isActive ? "0 0 0 2px #fff, 0 0 0 3.5px #3b82f6" : "none",
                  }}
                />
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
