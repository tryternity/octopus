import { useState } from "react";
import { cn } from "@/lib/utils";
import {
  MousePointer2, Square, Circle, Minus, Type, Undo2,
  Download, Copy, ScanText, Pin, PinOff, Palette, Check,
} from "lucide-react";
import type { Tool } from "@/lib/annotation";

const PRESET_COLORS = ["#ef4444", "#f59e0b", "#eab308", "#22c55e", "#3b82f6", "#8b5cf6", "#ec4899", "#000000"];

/**
 * 图片预览工具栏。布局：工具组(选择/矩形/椭圆/直线/文字/撤销) | 颜色·粗细浮窗 | 保存/复制/OCR/置顶。
 * 无关闭按钮——用窗口右上角 × 或 Esc。镜像 Screenshot 工具栏的 ToolButton 样式（32×32，激活蓝）。
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
  const [showProps, setShowProps] = useState(false);

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

      {/* 颜色·粗细浮窗 */}
      <div className="relative ml-1">
        <ToolButton title="颜色 / 粗细" active={showProps}
          onClick={() => setShowProps((v) => !v)}>
          <Palette className="h-4 w-4" />
        </ToolButton>
        {showProps && (
          <div className="absolute left-0 top-9 z-10 w-44 rounded-lg bg-neutral-800 p-2 shadow-xl border border-neutral-700">
            <div className="mb-2 flex flex-wrap gap-1.5">
              {PRESET_COLORS.map((c) => (
                <button key={c} type="button"
                  onClick={() => props.setToolColor(c)}
                  className={cn("h-5 w-5 rounded-full border",
                    props.toolColor === c ? "ring-2 ring-white" : "border-neutral-600")}
                  style={{ backgroundColor: c }} />
              ))}
            </div>
            <label className="block text-[11px] text-neutral-400">粗细 {props.toolWidth}</label>
            <input type="range" min={1} max={10} value={props.toolWidth}
              onChange={(e) => props.setToolWidth(Number(e.target.value))}
              className="w-full" />
            {props.tool === "text" && (
              <>
                <label className="mt-1 block text-[11px] text-neutral-400">字号 {props.toolFontSize}</label>
                <input type="range" min={10} max={48} value={props.toolFontSize}
                  onChange={(e) => props.setToolFontSize(Number(e.target.value))}
                  className="w-full" />
              </>
            )}
          </div>
        )}
      </div>

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
  );
}
