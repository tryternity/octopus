import { useState, useRef, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { listen } from "@tauri-apps/api/event";

interface Selection {
  x: number;
  y: number;
  w: number;
  h: number;
}

type Mode = "idle" | "selecting" | "selected" | "move" | "resize";

const HANDLE_SIZE = 8;
const MIN_SIZE = 10;

export default function Screenshot() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const bgImgRef = useRef<HTMLImageElement | null>(null);
  const screenWRef = useRef(0);
  const screenHRef = useRef(0);
  const startPtRef = useRef({ x: 0, y: 0 });
  const moveStartRef = useRef({ x: 0, y: 0 });
  const selStartRef = useRef<Selection>({ x: 0, y: 0, w: 0, h: 0 });

  const [mode, setMode] = useState<Mode>("idle");
  const [sel, setSel] = useState<Selection | null>(null);
  const [resizeHandle, setResizeHandle] = useState<string | null>(null);
  const [ready, setReady] = useState(false);

  const dpr = window.devicePixelRatio || 1;

  // 监听 screenshot://ready 事件
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<{ image: string; width: number; height: number }>(
      "screenshot://ready",
      (e) => {
        const { image, width, height } = e.payload;
        screenWRef.current = width;
        screenHRef.current = height;
        const img = new Image();
        img.onload = () => {
          bgImgRef.current = img;
          setReady(true);
        };
        img.src = `data:image/png;base64,${image}`;
      }
    ).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, []);

  // 绘制
  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    const bg = bgImgRef.current;
    if (!canvas || !bg || !ready) return;

    const cssW = window.innerWidth;
    const cssH = window.innerHeight;
    canvas.width = cssW * dpr;
    canvas.height = cssH * dpr;
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

    // 底层：全屏原图（按 CSS 像素尺寸绘制）
    ctx.drawImage(bg, 0, 0, cssW, cssH);

    // 上层：暗遮罩
    ctx.fillStyle = "rgba(0, 0, 0, 0.5)";

    if (sel) {
      // 选区外四块遮罩
      const { x, y, w, h } = sel;
      ctx.fillRect(0, 0, cssW, y);           // 上
      ctx.fillRect(0, y + h, cssW, cssH - y - h);  // 下
      ctx.fillRect(0, y, x, h);               // 左
      ctx.fillRect(x + w, y, cssW - x - w, h); // 右

      // 选区边框
      ctx.strokeStyle = "#3b82f6";
      ctx.lineWidth = 2;
      ctx.strokeRect(x, y, w, h);

      // 8 手柄
      if (mode === "selected" || mode === "move" || mode === "resize") {
        ctx.fillStyle = "#3b82f6";
        const handles = getHandles(sel);
        for (const [hx, hy] of handles) {
          ctx.fillRect(hx - HANDLE_SIZE / 2, hy - HANDLE_SIZE / 2, HANDLE_SIZE, HANDLE_SIZE);
        }

        // 尺寸标注
        const physW = Math.round(w * dpr);
        const physH = Math.round(h * dpr);
        const label = `${physW} × ${physH}`;
        ctx.font = "12px -apple-system, sans-serif";
        const tw = ctx.measureText(label).width;
        ctx.fillStyle = "rgba(255, 255, 255, 0.9)";
        const lx = x + w - tw - 8;
        const ly = y + h + 4;
        ctx.fillRect(lx, ly, tw + 8, 18);
        ctx.fillStyle = "#1a1a1a";
        ctx.fillText(label, lx + 4, ly + 13);
      }
    } else {
      // 全屏暗遮罩
      ctx.fillRect(0, 0, cssW, cssH);
    }
  }, [sel, mode, ready, dpr]);

  useEffect(() => {
    draw();
  }, [draw]);

  // 判断鼠标在哪个手柄上
  function getHandles(s: Selection): [number, number][] {
    return [
      [s.x, s.y],           // nw
      [s.x + s.w / 2, s.y], // n
      [s.x + s.w, s.y],     // ne
      [s.x + s.w, s.y + s.h / 2], // e
      [s.x + s.w, s.y + s.h],     // se
      [s.x + s.w / 2, s.y + s.h], // s
      [s.x, s.y + s.h],           // sw
      [s.x, s.y + s.h / 2],       // w
    ];
  }

  function hitTest(mx: number, my: number): string | null {
    if (!sel) return null;
    const handles = getHandles(sel);
    const names = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];
    for (let i = 0; i < handles.length; i++) {
      const [hx, hy] = handles[i];
      if (Math.abs(mx - hx) < HANDLE_SIZE && Math.abs(my - hy) < HANDLE_SIZE) {
        return names[i];
      }
    }
    return null;
  }

  function inSelection(mx: number, my: number): boolean {
    if (!sel) return false;
    return mx >= sel.x && mx <= sel.x + sel.w && my >= sel.y && my <= sel.y + sel.h;
  }

  // 鼠标事件
  function onMouseDown(e: React.MouseEvent) {
    const mx = e.clientX;
    const my = e.clientY;
    startPtRef.current = { x: mx, y: my };

    if (mode === "selected" || mode === "idle") {
      const handle = hitTest(mx, my);
      if (handle) {
        setResizeHandle(handle);
        setMode("resize");
        if (sel) selStartRef.current = { ...sel };
        return;
      }
      if (sel && inSelection(mx, my)) {
        setMode("move");
        moveStartRef.current = { x: mx, y: my };
        selStartRef.current = { ...sel };
        return;
      }
    }

    // 重新框选
    setSel({ x: mx, y: my, w: 0, h: 0 });
    setMode("selecting");
  }

  function onMouseMove(e: React.MouseEvent) {
    const mx = e.clientX;
    const my = e.clientY;

    // 更新光标
    if (mode === "idle" || mode === "selected") {
      const handle = hitTest(mx, my);
      if (handle) {
        const cursors: Record<string, string> = {
          nw: "nwse-resize", se: "nwse-resize",
          ne: "nesw-resize", sw: "nesw-resize",
          n: "ns-resize", s: "ns-resize",
          e: "ew-resize", w: "ew-resize",
        };
        (e.currentTarget as HTMLCanvasElement).style.cursor = cursors[handle] || "crosshair";
      } else if (sel && inSelection(mx, my)) {
        (e.currentTarget as HTMLCanvasElement).style.cursor = "move";
      } else {
        (e.currentTarget as HTMLCanvasElement).style.cursor = "crosshair";
      }
    }

    if (mode === "selecting") {
      const sx = startPtRef.current.x;
      const sy = startPtRef.current.y;
      setSel(normalize(sx, sy, mx, my));
    } else if (mode === "move" && sel) {
      const dx = mx - moveStartRef.current.x;
      const dy = my - moveStartRef.current.y;
      const cssW = window.innerWidth;
      const cssH = window.innerHeight;
      const ns = selStartRef.current;
      let nx = ns.x + dx;
      let ny = ns.y + dy;
      nx = Math.max(0, Math.min(nx, cssW - ns.w));
      ny = Math.max(0, Math.min(ny, cssH - ns.h));
      setSel({ ...ns, x: nx, y: ny });
    } else if (mode === "resize" && sel && resizeHandle) {
      const ns = resizeSel(selStartRef.current, resizeHandle, mx, my);
      setSel(ns);
    }
  }

  function onMouseUp() {
    if (mode === "selecting" && sel) {
      if (sel.w < MIN_SIZE || sel.h < MIN_SIZE) {
        setSel(null);
        setMode("idle");
      } else {
        setMode("selected");
      }
    } else if (mode === "move" || mode === "resize") {
      setMode("selected");
      setResizeHandle(null);
    }
  }

  // 键盘事件
  function onKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Escape") {
      invoke("cancel_screenshot").catch(() => {});
    } else if (e.key === "Enter" && sel && sel.w >= MIN_SIZE && sel.h >= MIN_SIZE) {
      // CSS 像素 → 物理像素
      invoke("confirm_screenshot", {
        x: Math.round(sel.x * dpr),
        y: Math.round(sel.y * dpr),
        w: Math.round(sel.w * dpr),
        h: Math.round(sel.h * dpr),
      }).catch(() => {});
    }
  }

  // 右键取消
  function onContextMenu(e: React.MouseEvent) {
    e.preventDefault();
    invoke("cancel_screenshot").catch(() => {});
  }

  function normalize(x1: number, y1: number, x2: number, y2: number): Selection {
    return {
      x: Math.min(x1, x2),
      y: Math.min(y1, y2),
      w: Math.abs(x2 - x1),
      h: Math.abs(y2 - y1),
    };
  }

  function resizeSel(start: Selection, handle: string, mx: number, my: number): Selection {
    let { x, y, w, h } = start;
    if (handle.includes("w")) { const rx = x + w; x = Math.min(mx, rx - MIN_SIZE); w = rx - x; }
    if (handle.includes("e")) { w = Math.max(MIN_SIZE, mx - x); }
    if (handle.includes("n")) { const by = y + h; y = Math.min(my, by - MIN_SIZE); h = by - y; }
    if (handle.includes("s")) { h = Math.max(MIN_SIZE, my - y); }
    return { x, y, w, h };
  }

  if (!ready) {
    return (
      <div style={{ width: "100vw", height: "100vh", background: "#000", display: "flex", alignItems: "center", justifyContent: "center" }}>
        <span style={{ color: "#666", fontSize: 14 }}>正在截屏…</span>
      </div>
    );
  }

  return (
    <canvas
      ref={canvasRef}
      style={{
        position: "fixed",
        top: 0,
        left: 0,
        width: "100vw",
        height: "100vh",
        cursor: "crosshair",
        outline: "none",
      }}
      tabIndex={0}
      autoFocus
      onMouseDown={onMouseDown}
      onMouseMove={onMouseMove}
      onMouseUp={onMouseUp}
      onKeyDown={onKeyDown}
      onContextMenu={onContextMenu}
    />
  );
}
