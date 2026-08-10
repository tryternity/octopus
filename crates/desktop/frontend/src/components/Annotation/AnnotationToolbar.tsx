// 标注工具栏组件 —— Screenshot 与 RecordAnnotation 共用。
//
// 抽取自：
//   - Screenshot/index.tsx L886-998（工具栏 JSX + 9 工具按钮 + divider + undo/redo + popover）
//   - RecordAnnotation/index.tsx L501-592（同上）
//
// 业务侧职责（通过 props / children 注入）：
//   - 位置（top/left）：业务侧用 computeToolbarPosition + computeToolbarCenterX 算好
//   - popover Y：业务侧按 placement 决定 popover 在工具栏上/下方，传 popoverY
//   - 业务命令按钮（OCR/save/stop/...）：作为 children 注入
//   - 工具切换透传：onToolChange 回调（录屏 passthrough、截图可选取消选区 move）
//
// 工具按钮 onClick 内部逻辑：
//   - 同一工具点第二次：popover 显示则收起+切回 none；隐藏则重新弹出
//   - 不同工具：切换 + 弹出 popover + 记录按钮中心 x（popover 跟随）
//   - 末尾调 onToolChange(t) 让业务侧做透传

import { useState, useEffect, useRef } from "react";
import type { ReactNode, MutableRefObject } from "react";
import type { Tool } from "@/lib/annotation";
import { PRESET_COLORS } from "@/lib/annotation";
import { ToolPropsPopover } from "@/pages/Screenshot/ToolPropsPopover";
import { useT } from "@/lib/i18n";
import { invoke } from "@/lib/tauri";
import type { AnnotationState } from "./useAnnotationState";


export interface AnnotationToolbarProps {
  /** 注入 useAnnotationState 返回的 state/actions */
  state: AnnotationState;

  /**
   * 工具切换回调（业务侧透传）。
   * 录屏：t === "none" → set_annotation_passthrough(true)
   * 截图：可选（如取消 sel move 模式）
   */
  onToolChange?: (t: Tool) => void;

  /**
   * 业务侧测量工具栏宽度用的 ref。
   * AnnotationToolbar 把 ref 接到工具栏根 div，业务侧 useLayoutEffect 读取 offsetWidth。
   */
  toolbarRef?: MutableRefObject<HTMLDivElement | null>;

  /** 工具栏 top（业务侧 computeToolbarPosition 算好） */
  top: number;
  /** 工具栏 left（业务侧 computeToolbarCenterX 算好，已 clamp） */
  left: number;

  /**
   * popover Y 位置（业务侧按 placement 算好）。
   * 传 undefined 时不渲染 popover（避免业务侧 sel=null 等边界场景）。
   */
  popoverY?: number;

  /** 业务按钮（OCR / scroll / save / pin / confirm / cancel / stop 等） */
  children?: ReactNode;

  /**
   * popover X 位置（业务侧算好，含 fallback）。
   * 不传时用 state.popoverX（首次为 0，会偏到屏幕左边缘——业务侧应传 fallback）。
   * 截图：popoverX || (sel.x + sel.w / 2)
   * 录屏：popoverX || (canvasRect.ox + canvasRect.w / 2)
   */
  popoverX?: number;

  /** 工具栏根 div 是否显示（业务侧条件控制，默认 true） */
  visible?: boolean;

  /** 是否显示 highlight 工具按钮（默认 true） */
  showHighlight?: boolean;

  /**
   * 是否显示水印按钮（默认 false）。
   * 水印按钮不走 tool 选中逻辑，点击弹独立输入框（Task 8）。
   * 截图工具栏传 true（需 config 水印），录屏不传 / 传 false。
   */
  showWatermark?: boolean;
}

// ── 内联 ToolButton（原 pages/Screenshot/ToolButton.tsx，19 行 dumb component）──
// 不复用原文件，因为：
//  1. 内联后 AnnotationToolbar 自包含，业务侧只需 import 一个组件
//  2. 避免 pages/Screenshot/ToolButton.tsx 仍被 RecordAnnotation 间接依赖（耦合反向）
//  3. 原 ToolButton 文件保留供 phase 2 ImagePreview 迁移时评估
function ToolButton({
  active,
  onClick,
  label,
  icon,
}: {
  active?: boolean;
  onClick: (e: React.MouseEvent<HTMLButtonElement>) => void;
  label: string;
  icon: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      title={label}
      style={{
        width: 32,
        height: 32,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        borderRadius: 6,
        border: "none",
        background: active ? "var(--color-voice)" : "transparent",
        cursor: "pointer",
        transition: "background 0.15s",
      }}
    >
      {icon}
    </button>
  );
}

function Divider() {
  return <div style={{ width: 1, height: 20, background: "var(--color-border)", margin: "0 4px" }} />;
}

// 工具图标渲染：active 时变白，否则用 icon-filter
function ToolIcon({ src, alt, active }: { src: string; alt: string; active: boolean }) {
  return (
    <img
      src={src}
      alt={alt}
      className="w-[18px] h-[18px]"
      style={{ filter: active ? "brightness(0) invert(1)" : "var(--icon-filter)" }}
    />
  );
}

export function AnnotationToolbar(props: AnnotationToolbarProps) {
  const t = useT();
  const {
    state,
    onToolChange,
    toolbarRef,
    top,
    left,
    popoverY,
    popoverX,
    children,
    visible = true,
    showHighlight = true,
    showWatermark = false,
  } = props;

  // ── 水印按钮 popover 输入框状态（Task 8）──
  // 水印不走 tool 选中逻辑——点击水印按钮弹输入框，输入文字 → set_config 持久化。
  // 后端 set_config 末尾统一 emit "config-changed"（settings_commands.rs:317），
  // 父组件（Screenshot）监听 config-changed 重读 get_config 刷新 watermarkOpts → Canvas 重画。
  // 输入留空确认 = 清除水印（set_config 空 string）。
  const [showWatermarkPopover, setShowWatermarkPopover] = useState(false);
  const [watermarkInput, setWatermarkInput] = useState("");
  const [watermarkColor, setWatermarkColor] = useState("#ffffff");
  const [watermarkDensity, setWatermarkDensity] = useState(0.5);
  const [watermarkAngle, setWatermarkAngle] = useState(0);
  const watermarkPopoverRef = useRef<HTMLDivElement | null>(null);
  const watermarkBtnRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!showWatermarkPopover) return;
    const handler = (e: MouseEvent) => {
      const target = e.target as Node;
      if (watermarkPopoverRef.current?.contains(target)) return;
      if (watermarkBtnRef.current?.contains(target)) return;
      setShowWatermarkPopover(false);
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [showWatermarkPopover]);

  // 水印按钮点击：读当前 config 的水印文字 + 颜色 + density + angle 预填。
  const openWatermarkPopover = () => {
    invoke<{ config: Record<string, unknown> }>("get_config")
      .then((res) => {
        const c = res.config;
        setWatermarkInput((c.screenshot_watermark_text as string) || "");
        setWatermarkColor((c.screenshot_watermark_color as string) || "#ffffff");
        setWatermarkDensity(typeof c.screenshot_watermark_density === "number" ? c.screenshot_watermark_density : 0.5);
        setWatermarkAngle(typeof c.screenshot_watermark_angle === "number" ? c.screenshot_watermark_angle : 0);
      })
      .catch(() => { setWatermarkInput(""); setWatermarkColor("#ffffff"); setWatermarkDensity(0.5); setWatermarkAngle(0); });
    setShowWatermarkPopover(true);
  };

  // 确认：set_config 文字 + 颜色 + density + angle（空字符串=清除水印）。失败不阻塞 UI，静默。
  const confirmWatermark = () => {
    invoke("set_config", { key: "screenshot_watermark_text", value: watermarkInput }).catch(() => {});
    invoke("set_config", { key: "screenshot_watermark_color", value: watermarkColor }).catch(() => {});
    invoke("set_config", { key: "screenshot_watermark_density", value: watermarkDensity }).catch(() => {});
    invoke("set_config", { key: "screenshot_watermark_angle", value: watermarkAngle }).catch(() => {});
    setShowWatermarkPopover(false);
  };

  if (!visible) return null;

  // ── 工具按钮点击：toggle popover + 切换工具 ──
  const onToolSelect = (e: React.MouseEvent, target: Tool, extra?: () => void) => {
    const btn = e.currentTarget as HTMLElement;
    const rect = btn.getBoundingClientRect();
    state.setPopoverX(rect.left + rect.width / 2);

    if (state.tool === target) {
      // 同一工具：popover 显示则收起+切回 none；隐藏则重新弹出
      if (state.showPopover) {
        state.setShowPopover(false);
        state.setTool("none");
        onToolChange?.("none");
      } else {
        state.setShowPopover(true);
      }
    } else {
      // 不同工具：切换 + 弹出 popover
      state.setTool(target);
      state.setShowPopover(true);
      onToolChange?.(target);
      extra?.();
    }
  };

  // 直接切到 select（none）：不弹 popover，调 onToolChange
  const onSelectOnly = () => {
    state.setTool("none");
    state.setShowPopover(false);
    onToolChange?.("none");
  };

  const tools: { key: Tool; src: string; label: string }[] = [
    { key: "rect", src: "icons/square.svg", label: t("screenshot.tool.rect") },
    { key: "oval", src: "icons/oval-vertical.svg", label: t("screenshot.tool.ellipse") },
    { key: "diamond", src: "icons/diamond.svg", label: t("screenshot.tool.diamond") },
    { key: "line", src: "icons/straight-line.svg", label: t("screenshot.tool.line") },
    { key: "arrow", src: "icons/arrow-line.svg", label: t("screenshot.tool.arrow") },
    { key: "pen", src: "icons/sketching.svg", label: t("screenshot.tool.pen") },
    ...(showHighlight ? [{ key: "highlight" as Tool, src: "icons/highlighter.svg", label: t("screenshot.tool.highlight") }] : []),
    { key: "text", src: "icons/text.svg", label: t("screenshot.tool.text") },
    { key: "number", src: "icons/sequence-note.svg", label: t("screenshot.tool.number") },
    { key: "blur", src: "icons/mosaic.svg", label: t("screenshot.tool.mosaic") },
  ];

  return (
    <>
      <div
        ref={toolbarRef}
        style={{
          position: "fixed",
          top,
          left,
          transform: "translateX(-50%)",
          display: "flex",
          gap: 4,
          padding: "6px 8px",
          background: "var(--color-surface)",
          color: "var(--color-foreground)",
          borderRadius: 8,
          boxShadow: "0 4px 16px rgba(0,0,0,0.3)",
          zIndex: 100,
          alignItems: "center",
        }}
      >
        {/* 选择工具 */}
        <ToolButton
          active={state.tool === "none"}
          onClick={(e) => {
            e.stopPropagation();
            onSelectOnly();
          }}
          label={t("screenshot.tool.select")}
          icon={<ToolIcon src="icons/arrow-pointer.svg" alt={t("screenshot.tool.select")} active={state.tool === "none"} />}
        />

        {/* 标注工具（含 highlight，条件显示） */}
        {tools.map((it) => (
          <ToolButton
            key={it.key}
            active={state.tool === it.key}
            onClick={(e) => {
              // number 工具切换时重置计数（与原 Screenshot L929 / RecordAnnotation L543 一致）
              const extra = it.key === "number" ? () => state.setNumberCounter(1) : undefined;
              onToolSelect(e, it.key, extra);
            }}
            label={it.label}
            icon={<ToolIcon src={it.src} alt={it.label} active={state.tool === it.key} />}
          />
        ))}

        {/* 水印按钮（Task 8）：弹独立输入框，不走 tool 选中逻辑。
            输入文字 → set_config 持久化，后端 emit config-changed → 父组件 Screenshot
            监听后重读 get_config 刷新 watermarkOpts → Canvas 导出时画水印。 */}
        {showWatermark && (
          <div style={{ position: "relative" }}>
            <div ref={watermarkBtnRef}>
              <ToolButton
                active={false}
                onClick={(e) => {
                  e.stopPropagation();
                  if (showWatermarkPopover) {
                    setShowWatermarkPopover(false);
                  } else {
                    openWatermarkPopover();
                  }
                }}
                label={t("screenshot.tool.watermark")}
                icon={<img
                  src="icons/text.svg"
                  alt={t("screenshot.tool.watermark")}
                  className="w-[18px] h-[18px]"
                  style={{ filter: "var(--icon-filter)" }}
                />}
              />
            </div>
            {showWatermarkPopover && (
              <div
                ref={watermarkPopoverRef}
                style={{
                  position: "absolute",
                  top: "100%",
                  left: "50%",
                  transform: "translateX(-50%)",
                  marginTop: 4,
                  padding: 8,
                  background: "var(--color-surface)",
                  color: "var(--color-foreground)",
                  borderRadius: 8,
                  boxShadow: "0 8px 24px -4px rgba(0,0,0,0.2), 0 2px 8px -2px rgba(0,0,0,0.1)",
                  zIndex: 102,
                  display: "flex",
                  flexDirection: "column",
                  gap: 6,
                  minWidth: 240,
                }}
              >
                <input
                  type="text"
                  value={watermarkInput}
                  onChange={(e) => setWatermarkInput(e.target.value)}
                  onKeyDown={(e) => {
                    e.stopPropagation();
                    if (e.key === "Enter") {
                      e.preventDefault();
                      confirmWatermark();
                    } else if (e.key === "Escape") {
                      setShowWatermarkPopover(false);
                    }
                  }}
                  placeholder={t("screenshot.watermark.placeholder")}
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck={false}
                  autoFocus
                  style={{
                    padding: "5px 8px",
                    border: "1px solid var(--color-border)",
                    borderRadius: 5,
                    background: "var(--color-bg, transparent)",
                    color: "var(--color-foreground)",
                    fontSize: 13,
                    outline: "none",
                    width: "100%",
                    boxSizing: "border-box",
                  }}
                />
                {/* 水印颜色选择——预设色 + 调色板，同 ToolPropsPopover 风格 */}
                <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
                  {PRESET_COLORS.map((c) => (
                    <button
                      key={c}
                      onClick={(e) => { e.stopPropagation(); setWatermarkColor(c); }}
                      style={{
                        width: 16, height: 16, borderRadius: 4,
                        background: c,
                        border: c === "#ffffff" ? "1px solid #e0e0e0" : "none",
                        cursor: "pointer", padding: 0,
                        opacity: watermarkColor.toLowerCase() === c.toLowerCase() ? 1 : 0.45,
                        transform: watermarkColor.toLowerCase() === c.toLowerCase() ? "scale(1.1)" : "scale(1)",
                      }}
                    />
                  ))}
                  <label style={{ cursor: "pointer", display: "flex", alignItems: "center", flexShrink: 0, marginLeft: 2 }}>
                    <div style={{
                      width: 16, height: 16, borderRadius: 4,
                      background: "conic-gradient(from 0deg, #ff0000, #ffff00, #00ff00, #00ffff, #0000ff, #ff00ff, #ff0000)",
                      border: "1px solid rgba(0,0,0,0.1)",
                    }} />
                    <input
                      type="color"
                      value={watermarkColor}
                      onChange={(e) => setWatermarkColor(e.target.value)}
                      style={{ width: 0, height: 0, opacity: 0, position: "absolute" }}
                    />
                  </label>
                </div>
                {/* 密度滑块 */}
                <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                  <span style={{ fontSize: 10, color: "var(--color-muted-foreground)", width: 32, flexShrink: 0 }}>{t("settings.general.watermarkDensity")}</span>
                  <input
                    type="range" min={0} max={1} step={0.1} value={watermarkDensity}
                    onChange={(e) => setWatermarkDensity(Number(e.target.value))}
                    style={{ flex: 1, height: 4, cursor: "pointer" }}
                  />
                  <span style={{ fontSize: 10, color: "var(--color-muted-foreground)", width: 24, textAlign: "center" }}>{watermarkDensity.toFixed(1)}</span>
                </div>
                {/* 角度滑块 */}
                <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                  <span style={{ fontSize: 10, color: "var(--color-muted-foreground)", width: 32, flexShrink: 0 }}>{t("settings.general.watermarkAngle")}</span>
                  <input
                    type="range" min={0} max={360} step={15} value={watermarkAngle}
                    onChange={(e) => setWatermarkAngle(Number(e.target.value))}
                    style={{ flex: 1, height: 4, cursor: "pointer" }}
                  />
                  <span style={{ fontSize: 10, color: "var(--color-muted-foreground)", width: 24, textAlign: "center" }}>{Math.round(watermarkAngle)}°</span>
                </div>
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    confirmWatermark();
                  }}
                  style={{
                    padding: "5px 8px",
                    border: "none",
                    borderRadius: 5,
                    background: "var(--color-voice)",
                    color: "#fff",
                    cursor: "pointer",
                    fontSize: 12,
                    fontWeight: 500,
                  }}
                >
                  {t("screenshot.watermark.confirm")}
                </button>
              </div>
            )}
          </div>
        )}

        {/* 橡皮擦：不弹 popover，直接切工具 */}
        <ToolButton
          active={state.tool === "eraser"}
          onClick={(e) => {
            e.stopPropagation();
            if (state.tool === "eraser") {
              state.setTool("none");
              onToolChange?.("none");
            } else {
              state.setTool("eraser");
              state.setShowPopover(false);
              onToolChange?.("eraser");
            }
          }}
          label={t("screenshot.tool.eraser")}
          icon={<ToolIcon src="icons/eraser.svg" alt={t("screenshot.tool.eraser")} active={state.tool === "eraser"} />}
        />

        <Divider />

        {/* undo / redo */}
        <ToolButton
          onClick={(e) => {
            e.stopPropagation();
            state.undoAnnotation();
          }}
          label={t("screenshot.tool.undo")}
          icon={
            <img
              src="icons/restore.svg"
              alt={t("screenshot.tool.undo")}
              className="w-[18px] h-[18px]"
              style={{ filter: "var(--icon-filter)", opacity: state.annotations.length > 0 ? 1 : 0.3 }}
            />
          }
        />
        <ToolButton
          onClick={(e) => {
            e.stopPropagation();
            state.redoAnnotation();
          }}
          label={t("screenshot.tool.redo")}
          icon={
            <img
              src="icons/redo.svg"
              alt={t("screenshot.tool.redo")}
              className="w-[18px] h-[18px]"
              style={{ filter: "var(--icon-filter)", opacity: state.redoAvailable ? 1 : 0.3 }}
            />
          }
        />

        <Divider />

        {/* clearAll */}
        <ToolButton
          onClick={(e) => {
            e.stopPropagation();
            state.clearAllAnnotations();
          }}
          label={t("screenshot.tool.clear")}
          icon={
            <img
              src="icons/clear.svg"
              alt={t("screenshot.tool.clear")}
              className="w-[18px] h-[18px]"
              style={{ filter: "var(--icon-filter)", opacity: state.annotations.length === 0 ? 0.3 : 1 }}
            />
          }
        />

        {/* 业务按钮 slot */}
        {children}
      </div>

      {/* 工具属性 popover */}
      {popoverY !== undefined && state.tool !== "none" && state.showPopover && (
        <ToolPropsPopover
          x={popoverX ?? state.popoverX}
          y={popoverY}
          color={state.toolColor}
          width={state.toolWidth}
          fontSize={state.toolFontSize}
          circleSize={state.toolCircleSize}
          isText={state.tool === "text"}
          isNumber={state.tool === "number"}
          isShape={state.tool === "rect" || state.tool === "oval" || state.tool === "diamond"}
          isBlur={state.tool === "blur"}
          blurMode={state.blurMode}
          filled={state.toolFilled}
          onColorChange={state.setToolColor}
          onWidthChange={state.setToolWidth}
          onFontSizeChange={state.setToolFontSize}
          onCircleSizeChange={state.setToolCircleSize}
          onFilledChange={state.setToolFilled}
          onBlurModeChange={state.setBlurMode}
        />
      )}
    </>
  );
}
