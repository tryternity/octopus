/**
 * 单个终端会话的 React hook——管理 xterm Terminal 实例 + PTY 生命周期。
 *
 * 简化版（相对 Terax useTerminalSession）：无 rendererPool 池化、无 dormantRing、
 * 无分屏 pane 树。每 tab 一个 xterm 实例，直接 new Terminal + fitAddon。
 *
 * 职责：
 * 1. 创建 xterm Terminal（挂到 container div）+ FitAddon + WebLinksAddon + WebGL
 * 2. openPty → onData 喂 xterm.write；term.onData → pty.write（用户输入）
 * 3. term.onResize → pty.resize；fitAddon 在容器尺寸变化时重新 fit
 * 4. WebGL renderer：默认启用，GPU 不可用/context loss 自动降级 Canvas；
 *    隐藏 tab（active=false）释放 WebGL context 防 WKWebView ~16 上限
 * 5. cleanup：pty.close + term.dispose + WebGL dispose
 *
 * @param container 容器 div 的 ref（xterm 挂载点）
 * @param cwd 可选初始工作目录
 * @param active tab 是否活跃（活跃 attach WebGL，隐藏 dispose 释放 context）
 * @param onExit PTY 退出回调
 */

import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";

import { openPty, type PtySession } from "./pty-bridge";

const TERMINAL_FONT_FAMILY =
  '"SF Mono", Menlo, Monaco, "Cascadia Code", "Roboto Mono", monospace';
const TERMINAL_FONT_SIZE = 13;

/**
 * WKWebView sleep/wake 或 GPU reset 后的 context 丢失恢复延迟（ms）。
 * 来自 Terax 实测 WEBGL_RECOVERY_DELAY_MS——WebKit GPU reset 窗口。
 */
const WEBGL_RECOVERY_DELAY_MS = 250;

export type TerminalSession = {
  /** 写入字符串到 PTY（外部触发，如 ActionBar 联动写命令）。 */
  write: (data: string) => void;
  /** 聚焦终端输入。 */
  focus: () => void;
  /** PTY session id（未连接时 null）。 */
  ptyId: number | null;
};

/**
 * 尝试 attach WebGL renderer 到 xterm Terminal。
 *
 * 成功返回 WebglAddon 实例；失败（GPU 不可用 / WKWebView 限制）返回 null，
 * xterm 自动回退 Canvas renderer——降级是正常路径，不抛错。
 *
 * context loss（sleep/wake / GPU reset）时：dispose 当前 addon →
 * 250ms 后（WebKit reset 窗口）重新 attach，重连后 refresh 重绘。
 *
 * @param term xterm Terminal 实例
 * @param webglRef 持有当前 WebglAddon 的 ref（context loss 重连时读写，防重复 attach）
 * @param factory 可选的 WebglAddon 工厂（测试注入 mock；生产默认 new WebglAddon()）
 * @returns attach 成功的 WebglAddon，或 null（降级）
 */
export function attachWebgl(
  term: Terminal,
  webglRef: React.RefObject<WebglAddon | null>,
  factory: () => WebglAddon = () => new WebglAddon(),
): WebglAddon | null {
  try {
    const webgl = factory();
    webgl.onContextLoss(() => {
      // 释放丢失的 context
      try {
        webgl.dispose();
      } catch {}
      if (webglRef.current === webgl) {
        webglRef.current = null;
      }
      // WebKit sleep/wake 或 GPU reset 后有短暂 context 丢失窗口，
      // 延迟后重新 attach（值来自 Terax 实测）。
      setTimeout(() => {
        if (webglRef.current) return; // 已被其他路径重连
        const reattached = attachWebgl(term, webglRef, factory);
        if (reattached) {
          webglRef.current = reattached;
          try {
            term.refresh(0, term.rows - 1);
          } catch {}
        }
      }, WEBGL_RECOVERY_DELAY_MS);
    });
    term.loadAddon(webgl);
    return webgl;
  } catch (e) {
    console.warn("[terminal-webgl] unavailable, fallback to canvas:", e);
    return null;
  }
}

export function useTerminalSession(opts: {
  container: React.RefObject<HTMLDivElement | null>;
  cwd?: string;
  active?: boolean;
  onExit?: (code: number) => void;
}): TerminalSession {
  const { container, cwd, onExit } = opts;
  const active = opts.active ?? true;
  const termRef = useRef<Terminal | null>(null);
  const ptyRef = useRef<PtySession | null>(null);
  const webglRef = useRef<WebglAddon | null>(null);
  const [ptyId, setPtyId] = useState<number | null>(null);

  useEffect(() => {
    const el = container.current;
    if (!el) return;

    // 1. 创建 xterm Terminal
    const term = new Terminal({
      fontFamily: TERMINAL_FONT_FAMILY,
      fontSize: TERMINAL_FONT_SIZE,
      cursorBlink: true,
      // 终端画布固定深色（终端惯例），不随主题切换——终端是 signature 元素
      theme: {
        background: "#0c0c0f",
        foreground: "#e4e4e7",
        cursor: "#e4e4e7",
        selectionBackground: "rgba(255,255,255,0.18)",
      },
      allowProposedApi: true,
    });
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.loadAddon(new WebLinksAddon());
    term.open(el);
    // 首次 fit 拿到 cols/rows 再 openPty（PTY 需要正确初始尺寸）
    fitAddon.fit();
    termRef.current = term;

    // WebGL renderer：默认 attach（GPU 不可用自动降级 Canvas）
    if (active) {
      webglRef.current = attachWebgl(term, webglRef);
    }

    const { cols, rows } = term;

    // 2. openPty + 接线 onData/onResize
    let disposed = false;
    openPty(cols, rows, {
      onData: (bytes) => {
        // PTY 输出 → xterm 渲染
        term.write(bytes);
      },
      onExit: (code) => {
        if (disposed) return;
        onExit?.(code);
      },
    }, cwd)
      .then((pty) => {
        if (disposed) {
          // 组件在 openPty 期间卸载了——立即关掉刚拿到的 session
          void pty.close();
          return;
        }
        ptyRef.current = pty;
        setPtyId(pty.id);
        // 用户输入 → PTY
        term.onData((str) => {
          void pty.write(str);
        });
        // 窗口 resize → PTY resize
        term.onResize(({ cols, rows }) => {
          void pty.resize(cols, rows);
        });
        term.focus();
      })
      .catch((e) => {
        // spawn 失败：把错误以红字写入 grid，让用户看到原因
        term.write(`\x1b[31m[pty open failed: ${String(e)}]\x1b[0m\r\n`);
        console.error("[pty] open failed:", e);
      });

    // 3. 容器尺寸变化时重新 fit（窗口 resize / tab 切换显隐）
    const resizeObserver = new ResizeObserver(() => {
      // 隐藏的容器（display:none）fit 会报错 / 拿到 0 尺寸——跳过
      if (el.offsetParent !== null) {
        try {
          fitAddon.fit();
        } catch {
          // xterm 在过渡动画期间可能抛——忽略
        }
      }
    });
    resizeObserver.observe(el);

    // 4. cleanup
    return () => {
      disposed = true;
      resizeObserver.disconnect();
      if (webglRef.current) {
        try {
          webglRef.current.dispose();
        } catch {}
        webglRef.current = null;
      }
      if (ptyRef.current) {
        void ptyRef.current.close();
        ptyRef.current = null;
      }
      term.dispose();
      termRef.current = null;
      setPtyId(null);
    };
    // cwd 变化不重建 session（cwd 只在首次 openPty 用）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 5. active 切换：隐藏 tab 释放 WebGL（防 ~16 context 上限），切回重连
  useEffect(() => {
    const term = termRef.current;
    if (!term) return;
    if (active) {
      // 切回 tab：attach WebGL（若尚未 attach）
      if (!webglRef.current) {
        webglRef.current = attachWebgl(term, webglRef);
      }
    } else {
      // 切走 tab：dispose WebGL（Canvas 兜底渲染保留 scrollback）
      if (webglRef.current) {
        try {
          webglRef.current.dispose();
        } catch {}
        webglRef.current = null;
      }
    }
  }, [active]);

  return {
    write: (data: string) => {
      ptyRef.current?.write(data);
    },
    focus: () => {
      termRef.current?.focus();
    },
    ptyId,
  };
}
