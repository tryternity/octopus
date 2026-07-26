/**
 * AreaPicker —— 录屏区域选区组件。
 *
 * 多屏全屏透明覆盖（每屏一个窗口，由后端 record_area_picker.rs 创建），
 * 用户拖框选区域，松开即确认（调 confirm_record_area_picker）。
 *
 * 视觉：半透明黑遮罩 rgba(0,0,0,0.5) + 蓝色选区边框 + 实时尺寸提示。
 * 与 screenshot 选区 UI 同模式，但精简掉标注工具/工具栏/OCR/滚动——
 * 区域录制选区是「一次性框定」，标注是录屏开始后的事（RecordAnnotation 组件）。
 *
 * 坐标：CSS 像素（mousedown/move 给的 e.clientX/Y），后端做物理像素转换。
 */
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { computeToolbarPosition, computeToolbarCenterX } from "@/components/Annotation";
import { useT } from "@/lib/i18n";

interface Selection {
  x: number;
  y: number;
  w: number;
  h: number;
}

const MIN_SIZE = 10;

export default function AreaPicker() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [sel, setSel] = useState<Selection | null>(null);
  const [dragging, setDragging] = useState(false);
  const startPtRef = useRef<{ x: number; y: number } | null>(null);
  const selRef = useRef<Selection | null>(null);
  const draggingRef = useRef(false);

  // 同步 ref（draw 闭包用 ref 避免 stale）
  useEffect(() => {
    selRef.current = sel;
  }, [sel]);
  useEffect(() => {
    draggingRef.current = dragging;
  }, [dragging]);

  // mount 时通知后端 ready（累加 READY_COUNT，达总数后统一 show）
  useEffect(() => {
    invoke("show_record_area_picker_window").catch(() => {});
  }, []);

  // 绘制（遮罩 + 选区边框 + 尺寸提示）
  const draw = () => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const cssW = window.innerWidth;
    const cssH = window.innerHeight;
    if (canvas.width !== cssW || canvas.height !== cssH) {
      canvas.width = cssW;
      canvas.height = cssH;
    }
    ctx.clearRect(0, 0, cssW, cssH);

    const s = selRef.current;
    if (!s) {
      // 无选区：整屏半透明遮罩
      ctx.fillStyle = "rgba(0, 0, 0, 0.5)";
      ctx.fillRect(0, 0, cssW, cssH);
      return;
    }

    // 4 个矩形遮罩（镂空出选区）
    ctx.fillStyle = "rgba(0, 0, 0, 0.5)";
    ctx.fillRect(0, 0, cssW, s.y);
    ctx.fillRect(0, s.y + s.h, cssW, cssH - s.y - s.h);
    ctx.fillRect(0, s.y, s.x, s.h);
    ctx.fillRect(s.x + s.w, s.y, cssW - s.x - s.w, s.h);

    // 选区边框（蓝色）
    ctx.strokeStyle = "#3b82f6";
    ctx.lineWidth = 2;
    ctx.strokeRect(s.x, s.y, s.w, s.h);

    // 尺寸提示（物理像素，右上角）
    const dpr = window.devicePixelRatio || 1;
    const label = `${Math.round(s.w * dpr)} × ${Math.round(s.h * dpr)}`;
    ctx.font = "12px -apple-system, sans-serif";
    const tw = ctx.measureText(label).width + 12;
    const th = 20;
    let lx = s.x + s.w - tw - 4;
    let ly = s.y + 4;
    if (lx < 0) lx = s.x + 4;
    if (ly < 0) ly = s.y + s.h + 4;
    ctx.fillStyle = "rgba(0, 0, 0, 0.8)";
    ctx.fillRect(lx, ly, tw, th);
    ctx.fillStyle = "#fff";
    ctx.fillText(label, lx + 6, ly + 14);
  };

  // redraw on sel change
  useEffect(() => {
    draw();
  }, [sel]);

  // 适配窗口尺寸变化
  useEffect(() => {
    const handler = () => draw();
    window.addEventListener("resize", handler);
    return () => window.removeEventListener("resize", handler);
  }, []);

  const normalize = (x1: number, y1: number, x2: number, y2: number): Selection => {
    const cx = Math.max(0, Math.min(Math.min(x1, x2), window.innerWidth));
    const cy = Math.max(0, Math.min(Math.min(y1, y2), window.innerHeight));
    const cw = Math.min(Math.abs(x2 - x1), window.innerWidth - cx);
    const ch = Math.min(Math.abs(y2 - y1), window.innerHeight - cy);
    return { x: cx, y: cy, w: cw, h: ch };
  };

  const onMouseDown = (e: React.MouseEvent) => {
    if (e.button !== 0) return; // 仅左键
    const mx = e.clientX;
    const my = e.clientY;
    startPtRef.current = { x: mx, y: my };
    setDragging(true);
    setSel({ x: mx, y: my, w: 0, h: 0 });
  };

  const onMouseMove = (e: React.MouseEvent) => {
    if (!draggingRef.current || !startPtRef.current) return;
    setSel(normalize(startPtRef.current.x, startPtRef.current.y, e.clientX, e.clientY));
  };

  const onMouseUp = async () => {
    if (!draggingRef.current) return;
    setDragging(false);
    const s = selRef.current;
    if (!s || s.w < MIN_SIZE || s.h < MIN_SIZE) {
      // 选区太小，丢弃
      setSel(null);
      return;
    }
    // 松手不自动确认——用户看工具栏（含尺寸提示 + 「开始录制」按钮），
    // 点「开始录制」才 confirm。这与截图流程一致（选区确定后工具栏出现，用户操作）。
  };

  // Esc / 右键取消
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        invoke("cancel_record_area_picker").catch(() => {});
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const onContextMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    invoke("cancel_record_area_picker").catch(() => {});
  };

  const t = useT();
  // 工具栏位置（与截图同算法，sel 存在即跟随）
  const toolbarW = 180; // 估算（开始录制 + 取消两按钮）
  const tbPos = sel ? computeToolbarPosition(sel, window.innerHeight) : null;
  const toolbarY = tbPos ? tbPos.y : 0;
  const toolbarX = sel ? computeToolbarCenterX(sel, window.innerWidth, toolbarW) : 0;

  return (
    <>
      <canvas
        ref={canvasRef}
        style={{
          position: "fixed",
          inset: 0,
          width: "100vw",
          height: "100vh",
          cursor: "crosshair",
          display: "block",
        }}
        onMouseDown={onMouseDown}
        onMouseMove={onMouseMove}
        onMouseUp={onMouseUp}
        onContextMenu={onContextMenu}
      />
      {/* 工具栏（跟随选区，与截图同位置算法）。
          sel 存在即显示——拖框时实时跟随，松手后停留（用户可点「开始录制」确认）。
          拖框中（dragging）也显示，让用户直观看到工具栏位置。*/}
      {sel && sel.w >= MIN_SIZE && (
        <div
          style={{
            position: "fixed",
            top: `${toolbarY}px`,
            left: `${toolbarX}px`,
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
          {/* 尺寸提示（物理像素）*/}
          <span style={{ fontSize: 11, color: "var(--color-muted-foreground)", fontFamily: "SF Mono, Menlo, monospace", fontVariantNumeric: "tabular-nums", padding: "0 4px" }}>
            {Math.round(sel.w * (window.devicePixelRatio || 1))} × {Math.round(sel.h * (window.devicePixelRatio || 1))}
          </span>
          <div style={{ width: 1, height: 20, background: "var(--color-border)", margin: "0 4px" }} />
          {/* 开始录制（绿色，与截图确认按钮同风格）*/}
          <button
            onClick={() => {
              const winLabel = getCurrentWindow().label;
              invoke("confirm_record_area_picker", { winLabel, x: sel.x, y: sel.y, w: sel.w, h: sel.h }).catch(() => {});
            }}
            style={{
              padding: "4px 10px", fontSize: 12, fontWeight: 600,
              borderRadius: 6, border: "none",
              background: "var(--color-voice)", color: "#fff",
              cursor: "pointer",
            }}
          >
            {t("recordConfig.areaPickRecord")}
          </button>
          {/* 取消 */}
          <button
            onClick={() => invoke("cancel_record_area_picker").catch(() => {})}
            style={{
              padding: "4px 10px", fontSize: 12,
              borderRadius: 6, border: "1px solid var(--color-border)",
              background: "transparent", color: "var(--color-muted-foreground)",
              cursor: "pointer",
            }}
          >
            {t("recordConfig.cancel")}
          </button>
        </div>
      )}
    </>
  );
}
