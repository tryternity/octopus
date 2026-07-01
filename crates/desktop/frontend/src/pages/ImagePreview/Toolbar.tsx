import { cn } from "@/lib/utils";
import {
  MousePointer2, Square, Circle, Minus, Type, Undo2,
  Download, Copy, ScanText, Pin, PinOff, Check,
} from "lucide-react";
import type { Tool } from "@/lib/annotation";

// 预设色与截图 ToolPropsPopover 一致（含白色）
const PRESET_COLORS = ["#ef4444", "#f97316", "#eab308", "#22c55e", "#3b82f6", "#8b5cf6", "#000000", "#ffffff"];

/**
 * 图片预览工具栏 + 属性浮窗。
 *
 * 交互对齐截图（Screenshot）：选了任一标注工具（tool !== "none"）后，工具栏下方
 * **自动浮出**属性面板（颜色 + 粗细/字号），随工具切换内容（文字→字号、其余→粗细）；
 * 切回选择工具则收起。不再用单独的「调色板」按钮触发。
 *
 * 工具组：选择/矩形/椭圆/直线/文字/撤销 | 保存/复制/OCR/置顶。无关闭按钮（右上角 × 或 Esc）。
 */
export default function Toolbar(props: {
  tool: Tool; setTool: (t: Tool) => void;
  toolColor: string; setToolColor: (c: string) => void;
  toolWidth: number; setToolWidth: (n: number) => void;
  toolFontSize: number; setToolFontSize: (n: number) => void;
  alwaysOnTop: boolean; onToggleTop: () => void;
  onSave: () => void; onCopy: () => void; onOcr: () => void;
  onUndo: () => void; canUndo: boolean;
  ocrCopied: boolean;
}) {
  const isText = props.tool === "text";
  const showProps = props.tool !== "none";
  const sizeValue = isText ? props.toolFontSize : props.toolWidth;
  const setSize = isText ? props.setToolFontSize : props.setToolWidth;
  const min = isText ? 10 : 1;
  const max = isText ? 48 : 10;
  const label = isText ? "字号" : "粗细";

  const ToolButton = ({ active, onClick, title, children }: {
    active: boolean; onClick: () => void; title: string; children: React.ReactNode;
  }) => (
    <button
      type="button"
      title={title}
      onClick={onClick}
      className={cn(
        "flex h-8 w-8 items-center justify-center rounded transition-colors",
        active ? "bg-blue-500 text-white" : "text-neutral-300 hover:bg-neutral-700",
      )}
    >
      {children}
    </button>
  );

  const tools: { key: Tool; icon: React.ReactNode; title: string }[] = [
    { key: "none", icon: <MousePointer2 className="h-4 w-4" />, title: "选择/移动" },
    { key: "rect", icon: <Square className="h-4 w-4" />, title: "矩形" },
    { key: "oval", icon: <Circle className="h-4 w-4" />, title: "椭圆" },
    { key: "line", icon: <Minus className="h-4 w-4" />, title: "直线" },
    { key: "text", icon: <Type className="h-4 w-4" />, title: "文字" },
  ];

  return (
    <div className="relative">
      <div className="flex items-center gap-1 px-2 py-1.5 bg-neutral-800 border-b border-neutral-700">
        {tools.map((t) => (
          <ToolButton key={t.key} title={t.title} active={props.tool === t.key}
            onClick={() => props.setTool(t.key)}>
            {t.icon}
          </ToolButton>
        ))}
        <ToolButton title="撤销 (Cmd/Ctrl+Z)" active={false} onClick={props.onUndo}>
          <Undo2 className={cn("h-4 w-4", !props.canUndo && "opacity-30")} />
        </ToolButton>

        <div className="mx-1 h-5 w-px bg-neutral-700" />

        <ToolButton title="保存为文件" active={false} onClick={props.onSave}>
          <Download className="h-4 w-4" />
        </ToolButton>
        <ToolButton title="复制到剪贴板" active={false} onClick={props.onCopy}>
          <Copy className="h-4 w-4" />
        </ToolButton>
        <ToolButton title="OCR 识别（结果复制到剪贴板）" active={false} onClick={props.onOcr}>
          {props.ocrCopied ? <Check className="h-4 w-4 text-emerald-400" /> : <ScanText className="h-4 w-4" />}
        </ToolButton>
        <ToolButton title={props.alwaysOnTop ? "取消置顶" : "窗口置顶"}
          active={props.alwaysOnTop} onClick={props.onToggleTop}>
          {props.alwaysOnTop ? <PinOff className="h-4 w-4" /> : <Pin className="h-4 w-4" />}
        </ToolButton>
      </div>

      {/* 属性浮窗：选了标注工具后自动浮在工具栏下方（对齐截图 ToolPropsPopover） */}
      {showProps && (
        <div className="absolute left-2 top-full z-20 mt-1 flex flex-col gap-2 rounded-lg border border-neutral-200 bg-white p-2.5 shadow-xl"
          style={{ width: 240 }} onMouseDown={(e) => e.stopPropagation()}>
          {/* 行 1：粗细/字号滑轨 + 当前色 */}
          <div className="flex items-center gap-2.5">
            <span className="w-5 shrink-0 text-[10px] font-medium text-neutral-400">{label}</span>
            <input type="range" min={min} max={max} value={sizeValue}
              onChange={(e) => setSize(Number(e.target.value))}
              className="h-1 flex-1 cursor-pointer"
              style={{ accentColor: props.toolColor }} />
            <span className="w-5 text-center text-[10px] font-semibold tabular-nums text-neutral-600">{sizeValue}</span>
            <div className="h-5 w-5 shrink-0 rounded-full border-[3px] border-white"
              style={{ background: props.toolColor, boxShadow: "0 0 0 1.5px rgba(0,0,0,0.2)" }} />
          </div>

          <div className="-mx-1 h-px bg-neutral-200" />

          {/* 行 2：预设色 */}
          <div className="flex items-center gap-2.5">
            {PRESET_COLORS.map((c) => (
              <button key={c} type="button"
                onClick={() => props.setToolColor(c)}
                className={cn("h-[18px] w-[18px] rounded", c === "#ffffff" && "border border-neutral-200")}
                style={{ background: c, opacity: props.toolColor.toLowerCase() === c.toLowerCase() ? 1 : 0.45 }}
              />
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
