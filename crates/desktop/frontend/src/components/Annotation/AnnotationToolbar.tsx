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

import type { ReactNode, MutableRefObject } from "react";
import type { Tool } from "@/lib/annotation";
import { ToolPropsPopover } from "@/pages/Screenshot/ToolPropsPopover";
import { useT } from "@/lib/i18n";
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

  /** 工具栏根 div 是否显示（业务侧条件控制，默认 true） */
  visible?: boolean;
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
    children,
    visible = true,
  } = props;

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

        {/* 9 个标注工具 */}
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

        {/* 业务按钮 slot */}
        {children}
      </div>

      {/* 工具属性 popover */}
      {popoverY !== undefined && state.tool !== "none" && state.showPopover && (
        <ToolPropsPopover
          x={state.popoverX}
          y={popoverY}
          color={state.toolColor}
          width={state.toolWidth}
          fontSize={state.toolFontSize}
          circleSize={state.toolCircleSize}
          isText={state.tool === "text"}
          isNumber={state.tool === "number"}
          isShape={state.tool === "rect" || state.tool === "oval" || state.tool === "diamond"}
          filled={state.toolFilled}
          onColorChange={state.setToolColor}
          onWidthChange={state.setToolWidth}
          onFontSizeChange={state.setToolFontSize}
          onCircleSizeChange={state.setToolCircleSize}
          onFilledChange={state.setToolFilled}
        />
      )}
    </>
  );
}
