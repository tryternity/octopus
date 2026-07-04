import { useRef, useState, useEffect } from "react";
import {
  ZoomIn, ZoomOut, Expand, MoveHorizontal,
} from "lucide-react";
import type { Tool } from "@/lib/annotation";

// SVG 图标 img（与截图工具一致，激活时变白）
const SvgIcon = ({ src, alt, active }: { src: string; alt: string; active?: boolean }) => (
  <img src={src} alt={alt} className="w-[18px] h-[18px]" style={{ filter: active ? "brightness(0) invert(1)" : "none" }} />
);

// 预设色与截图 ToolPropsPopover 一致（含白色）
const PRESET_COLORS = ["#ef4444", "#f97316", "#eab308", "#22c55e", "#3b82f6", "#8b5cf6", "#000000", "#ffffff"];
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
        background: active ? "#3b82f6" : "transparent",
        color: active ? "#fff" : "#44403c",
        transition: "background 0.15s",
      }}
      onMouseEnter={(e) => { if (!active) e.currentTarget.style.background = "rgba(0,0,0,0.06)"; }}
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
  onUndo: () => void; canUndo: boolean;
  onRedo: () => void; canRedo: boolean;
  ocrCopied: boolean;
  ocrWarn: boolean;
  ocrMode: 'off' | 'overlay' | 'mask';
  zoom: number; onZoomIn: () => void; onZoomOut: () => void; onZoomReset: () => void;
  onZoomFitWidth: () => void; onZoomFitWindow: () => void;
  filled: boolean; setFilled: (f: boolean) => void;
  popoverDismissKey: number;  // 变化时收起浮窗
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  // 浮窗左偏移（相对工具卡），跟随被点击的标注按钮
  // 浮窗显隐：独立 state，不绑死 tool。用户操作画布时自动收起，需要改属性时重新点按钮弹出。
  const [showPopover, setShowPopover] = useState(false);
  // 用户在画布上操作时收起浮窗（popoverDismissKey 由 index.tsx mousedown 时递增）
  useEffect(() => { setShowPopover(false); }, [props.popoverDismissKey]);

  const [popoverLeft, setPopoverLeft] = useState(0);

  const isText = props.tool === "text";
  const isBlur = props.tool === "blur";
  const showProps = showPopover && props.tool !== "none";
  const sizeValue = isText ? props.toolFontSize : props.toolWidth;
  const setSize = isText ? props.setToolFontSize : props.setToolWidth;
  const min = isText ? 10 : 1;
  const max = isText ? 48 : 10;
  const label = isText ? "字号" : isBlur ? "遮挡" : "粗细";

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

  const tools: { key: Tool; icon: React.ReactNode; title: string }[] = [
    { key: "none", icon: <SvgIcon src="icons/arrow-pointer.svg" alt="选择" active={props.tool === "none"} />, title: "选择/移动" },
    { key: "rect", icon: <SvgIcon src="icons/square.svg" alt="矩形" active={props.tool === "rect"} />, title: "矩形" },
    { key: "oval", icon: <SvgIcon src="icons/circle.svg" alt="椭圆" active={props.tool === "oval"} />, title: "椭圆" },
    { key: "diamond", icon: <SvgIcon src="icons/diamond.svg" alt="菱形" active={props.tool === "diamond"} />, title: "菱形" },
    { key: "line", icon: <SvgIcon src="icons/straight-line.svg" alt="直线" active={props.tool === "line"} />, title: "直线" },
    { key: "arrow", icon: <SvgIcon src="icons/arrow-line.svg" alt="箭头" active={props.tool === "arrow"} />, title: "箭头" },
    { key: "pen", icon: <SvgIcon src="icons/sketching.svg" alt="画笔" active={props.tool === "pen"} />, title: "画笔（自由曲线）" },
    { key: "text", icon: <SvgIcon src="icons/text.svg" alt="文字" active={props.tool === "text"} />, title: "文字" },
    { key: "number", icon: <SvgIcon src="icons/sequence-note.svg" alt="序号" active={props.tool === "number"} />, title: "序号" },
    { key: "blur", icon: <SvgIcon src="icons/mosaic.svg" alt="马赛克" active={props.tool === "blur"} />, title: "马赛克" },
  ];

  return (
    // 外层 absolute 容器（相对 ImagePreview 父容器定位）
    <div ref={containerRef} style={{ position: "absolute", left: "50%", top: 8, transform: "translateX(-50%)", zIndex: 100 }}>
      {/* 工具卡：白底 r8 + 截图同款 shadow */}
      <div style={{
        display: "flex", alignItems: "center", gap: 4,
        padding: "6px 8px", background: "#fff", borderRadius: 8,
        boxShadow: "0 4px 16px rgba(0,0,0,0.3)",
      }}>
        {/* 输出操作：保存 / 复制 / OCR（截图 SVG 图标） */}
        <ToolButton title="保存为文件" active={false} onClick={() => props.onSave()}>
          <img src="icons/save.svg" alt="保存" className="w-[18px] h-[18px]" />
        </ToolButton>
        <ToolButton title="复制到剪贴板" active={false} onClick={() => props.onCopy()}>
          <img src="icons/copy.svg" alt="复制" className="w-[18px] h-[18px]" />
        </ToolButton>
        <div style={{ position: "relative" }}>
          <ToolButton title={props.ocrWarn ? "前一个 OCR 还未完成，请稍后" : "OCR 识别"} active={props.ocrCopied || props.ocrWarn || props.ocrMode !== 'off'} onClick={() => props.onOcr()}>
            {props.ocrCopied ? <img src="icons/check.svg" alt="完成" className="w-[18px] h-[18px]" style={{ filter: "brightness(0) invert(1)" }} /> : props.ocrMode === 'overlay' ? <img src="icons/ocr-all.svg" alt="OCR 叠加" className="w-[18px] h-[18px]" style={{ filter: "brightness(0) invert(1)" }} /> : props.ocrMode === 'mask' ? <img src="icons/ocr-text.svg" alt="OCR 遮罩" className="w-[18px] h-[18px]" style={{ filter: "brightness(0) invert(1)" }} /> : <img src="icons/ocr-ai.svg" alt="OCR" className="w-[18px] h-[18px]" />}
          </ToolButton>
          {props.ocrWarn && (
            <span style={{
              position: "absolute", top: "calc(100% + 6px)", left: "50%", transform: "translateX(-50%)",
              whiteSpace: "nowrap", background: "#f59e0b", color: "#fff",
              fontSize: 10, fontWeight: 500, padding: "3px 8px", borderRadius: 5,
              boxShadow: "0 2px 8px rgba(0,0,0,0.2)", pointerEvents: "none", zIndex: 110,
            }}>
              前一个 OCR 还未完成，请稍后
            </span>
          )}
        </div>

        <Divider />

        {/* 标注工具：选择/矩形/椭圆/直线/箭头/画笔/文字/撤销 */}
        {tools.map((t) => (
          <ToolButton key={t.key} title={t.title} active={props.tool === t.key}
            onClick={(e) => onToolClick(t.key, e)}>
            {t.icon}
          </ToolButton>
        ))}
        <ToolButton title="撤销 (Cmd/Ctrl+Z)" active={false} onClick={() => props.onUndo()}>
          <img src="icons/restore.svg" alt="撤销" className="w-[18px] h-[18px]" style={{ opacity: props.canUndo ? 1 : 0.3 }} />
        </ToolButton>
        <ToolButton title="重做 (Cmd/Ctrl+Shift+Z)" active={false} onClick={() => props.onRedo()}>
          <img src="icons/redo.svg" alt="重做" className="w-[18px] h-[18px]" style={{ opacity: props.canRedo ? 1 : 0.3 }} />
        </ToolButton>

        {/* 缩放：缩小 + 当前百分比(点击重置 100%) + 放大 */}
        <Divider />
        <ToolButton title="缩小" active={false} onClick={() => props.onZoomOut()}>
          <ZoomOut className="h-[18px] w-[18px]" />
        </ToolButton>
        <button
          type="button"
          title="重置为 100%"
          onClick={() => props.onZoomReset()}
          style={{
            height: 32, minWidth: 52, padding: "0 6px", border: "none", cursor: "pointer",
            display: "flex", alignItems: "center", justifyContent: "center",
            borderRadius: 6, background: "transparent", color: "#44403c",
            fontSize: 12, fontWeight: 600,
            fontFamily: "SF Mono, Menlo, monospace", fontVariantNumeric: "tabular-nums",
            transition: "background 0.15s",
          }}
          onMouseEnter={(e) => { e.currentTarget.style.background = "rgba(0,0,0,0.06)"; }}
          onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
        >
          {Math.round(props.zoom * 100)}%
        </button>
        <ToolButton title="放大" active={false} onClick={() => props.onZoomIn()}>
          <ZoomIn className="h-[18px] w-[18px]" />
        </ToolButton>
        <ToolButton title="自适应宽度" active={false} onClick={() => props.onZoomFitWidth()}>
          <MoveHorizontal className="h-[18px] w-[18px]" />
        </ToolButton>
        <ToolButton title="自适应窗口" active={false} onClick={() => props.onZoomFitWindow()}>
          <Expand className="h-[18px] w-[18px]" />
        </ToolButton>

        {/* 置顶单独推到最右 */}
        <Divider />
        <ToolButton title={props.alwaysOnTop ? "取消置顶" : "窗口置顶"}
          active={props.alwaysOnTop} onClick={() => props.onToggleTop()}>
          <img src="icons/pin.svg" alt="置顶" className="w-[18px] h-[18px]" style={{ filter: props.alwaysOnTop ? "brightness(0) invert(1)" : "none" }} />
        </ToolButton>
      </div>

      {/* 属性浮窗：复刻截图 ToolPropsPopover —— 跟随激活按钮（left=popoverLeft），白卡 r10、两行 */}
      {showProps && (
        <div
          onMouseDown={(e) => e.stopPropagation()}
          style={{
            position: "absolute", left: popoverLeft, top: "calc(100% + 6px)",
            padding: "10px 12px", background: "#fff", borderRadius: 10,
            boxShadow: "0 8px 24px -4px rgba(0,0,0,0.2), 0 2px 8px -2px rgba(0,0,0,0.1)",
            display: "flex", flexDirection: "column", gap: 10, width: POPOVER_W,
          }}
        >
          {/* 行 1：粗细/字号滑轨 + 当前色（最右） */}
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <span style={{ fontSize: 10, color: "#a8a29e", width: 20, fontWeight: 500, flexShrink: 0 }}>{label}</span>
            <input
              type="range" min={min} max={max} value={sizeValue}
              onChange={(e) => setSize(Number(e.target.value))}
              style={{ flex: 1, height: 4, borderRadius: 2, cursor: "pointer", accentColor: props.toolColor }}
            />
            <span style={{ fontSize: 10, color: "#57534e", width: 18, textAlign: "center", fontWeight: 600,
              fontFamily: "SF Mono, Menlo, monospace", fontVariantNumeric: "tabular-nums" }}>
              {sizeValue}
            </span>
            {/* 当前色：粗白边 + 阴影，与下方预设色区分 */}
            <div style={{
              width: 20, height: 20, borderRadius: "50%", background: props.toolColor, flexShrink: 0,
              border: "3px solid #fff",
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
          {/* 行 3：实心开关（仅 rect/oval） */}
          {(props.tool === "rect" || props.tool === "oval" || props.tool === "diamond") && (
            <>
              <div style={{ height: 1, background: "rgba(0,0,0,0.06)", margin: "0 -4px" }} />
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
                <span style={{ fontSize: 10, color: "#a8a29e", fontWeight: 500 }}>实心填充</span>
                <button
                  type="button"
                  onClick={() => props.setFilled(!props.filled)}
                  style={{
                    width: 32, height: 18, borderRadius: 9, border: "none", cursor: "pointer",
                    background: props.filled ? "#3b82f6" : "rgba(0,0,0,0.15)",
                    position: "relative", transition: "background 0.2s",
                  }}
                >
                  <span style={{
                    position: "absolute", top: 2, left: props.filled ? 16 : 2,
                    width: 14, height: 14, borderRadius: "50%", background: "#fff",
                    transition: "left 0.2s", boxShadow: "0 1px 2px rgba(0,0,0,0.2)",
                  }} />
                </button>
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
