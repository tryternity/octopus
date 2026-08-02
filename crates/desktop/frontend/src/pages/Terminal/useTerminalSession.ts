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
import { SearchAddon } from "@xterm/addon-search";
import "@xterm/xterm/css/xterm.css";

import { openPty, type PtySession } from "./pty-bridge";
import { readlineSequence, isShiftEnter, isFindShortcut, isNewTabShortcut, isFontShortcut } from "./keymap";
import { registerCwdHandler, registerPromptTracker, createShellIntegrationState } from "./osc-handlers";

/** 平台判定（macOS Option/Cmd 组合键映射用）。 */
const IS_MAC =
  typeof navigator !== "undefined" &&
  /Mac|iPhone|iPad/.test(navigator.userAgent);

/** 默认字体族——opts.fontFamily 未传时兜底。 */
const DEFAULT_FONT_FAMILY =
  '"SF Mono", Menlo, Monaco, "Cascadia Code", "Roboto Mono", monospace';
/** 默认字号——opts.fontSize 未传时兜底。 */
const DEFAULT_FONT_SIZE = 13;

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
  /** SearchAddon 实例（终端内搜索用，未初始化时 null）。 */
  searchAddon: SearchAddon | null;
  /** 当前工作目录（OSC 7 追踪，null = 尚未收到）。 */
  cwd: string | null;
  // ── 右键菜单需要的 xterm 操作 ──
  hasSelection: () => boolean;
  getSelection: () => string | undefined;
  paste: (text: string) => void;
  selectAll: () => void;
  clear: () => void;
  // ── 运行时改字体（设置页字号/字体变化时调）──
  /** 改字号：更新 term.options + fit（cols/rows 可能变）+ refresh 重绘。 */
  setFontSize: (size: number) => void;
  /** 改字体族：更新 term.options + refresh 重绘（无需 fit，列数不变）。 */
  setFontFamily: (family: string) => void;
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
  /** 终端字号（默认 13；运行时可用 setFontSize 改）。 */
  fontSize?: number;
  /** 终端字体族（默认 SF Mono/Menlo 栈；运行时可用 setFontFamily 改）。 */
  fontFamily?: string;
  /** Cmd+F 触发时回调（TerminalPane 打开搜索栏）。 */
  onSearchOpen?: () => void;
  /** Cmd/Ctrl+T 触发时回调（新建 tab）。 */
  onNewTab?: () => void;
  /** Cmd/Ctrl+= / - 触发时回调（字号 +/-）。delta=1 增大，-1 减小。父组件负责 clamp + persist。 */
  onFontResize?: (delta: 1 | -1) => void;
  onExit?: (code: number) => void;
}): TerminalSession {
  const { container, cwd, onExit, onSearchOpen, onNewTab, onFontResize } = opts;
  const active = opts.active ?? true;
  const termRef = useRef<Terminal | null>(null);
  const ptyRef = useRef<PtySession | null>(null);
  const searchAddonRef = useRef<SearchAddon | null>(null);
  // fitAddon 在 useEffect 内创建，setFontSize 运行时需要触发 fit（字号变 cols/rows 会变），
  // 提到外层 ref 让 return 对象能访问到。
  const fitAddonRef = useRef<FitAddon | null>(null);
  // 初始值用 openPty 的 cwd——OSC 7 只在 shell 执行命令后（precmd）才发，
  // 刚开终端时 trackedCwd 为 null 会导致文件树空白。用启动目录做兜底初始值。
  const [trackedCwd, setTrackedCwd] = useState<string | null>(cwd ?? null);
  // OSC 133 shell 集成状态——持有 inCommand（命令行输入态），供点击定位光标门控读。
  // 提到外层 ref 是因为 return 在 useEffect 外，访问不到 term.open 闭包内的局部变量。
  // 回调用 ref 持有最新版本——xterm handler 在 useEffect([]) 注册，
  // 闭包捕获的是首次 render 的回调，后续更新（如 addTab 随 tabs 变化重建）
  // 不会反映到 handler 里。用 ref 中转让 handler 始终调最新的。
  const onSearchOpenRef = useRef(onSearchOpen);
  const onNewTabRef = useRef(onNewTab);
  const onFontResizeRef = useRef(onFontResize);
  onSearchOpenRef.current = onSearchOpen;
  onNewTabRef.current = onNewTab;
  onFontResizeRef.current = onFontResize;
  const webglRef = useRef<WebglAddon | null>(null);
  const [ptyId, setPtyId] = useState<number | null>(null);

  useEffect(() => {
    const el = container.current;
    if (!el) return;

    // 1. 创建 xterm Terminal
    const term = new Terminal({
      fontFamily: opts.fontFamily ?? DEFAULT_FONT_FAMILY,
      fontSize: opts.fontSize ?? DEFAULT_FONT_SIZE,
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
    fitAddonRef.current = fitAddon;
    term.loadAddon(fitAddon);
    term.loadAddon(new WebLinksAddon());
    const searchAddon = new SearchAddon();
    term.loadAddon(searchAddon);
    searchAddonRef.current = searchAddon;
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
    // rAF 节流：每帧（~16ms）最多 write 一次。yes 这种持续高速输出
    // 会产生大量 onData 回调——用 rAF 合并，积压时只保留最新块丢弃中间。
    // 效果：xterm write buffer 不会无限积压，Ctrl+C 后很快消化完显示 prompt。
    // htop（alternate screen TUI）每帧重绘量小，不受影响。
    let pendingOutput: Uint8Array | null = null;
    let rafScheduled = false;
    const flushOutput = () => {
      rafScheduled = false;
      if (disposed || !pendingOutput) return;
      const data = pendingOutput;
      pendingOutput = null;
      term.write(data);
    };

    openPty(cols, rows, {
      onData: (bytes) => {
        if (rafScheduled) {
          // 本帧已调度 → 新数据覆盖暂存（丢中间保最新）
          pendingOutput = bytes;
        } else {
          pendingOutput = bytes;
          rafScheduled = true;
          requestAnimationFrame(flushOutput);
        }
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
        // macOS Option/Cmd 组合键 + IME 兼容 + Shift+Enter
        // Ctrl 组合键（Ctrl+A/C/...）由 xterm 默认处理走 onData，这里不拦截
        term.attachCustomKeyEventHandler((event) => {
          // IME 组合中（中文拼音等）：拦截原生 keydown（含提交候选的 Enter），
          // xterm 通过 compositionend 收最终字符串，否则会吞字/重复
          if (event.isComposing || event.keyCode === 229) return false;

          // Cmd+F（Mac）/ Ctrl+F（其他）→ 触发终端内搜索
          if (isFindShortcut(event)) {
            event.preventDefault();
            if (event.type === "keydown") onSearchOpenRef.current?.();
            return false;
          }

          // Cmd+T（Mac）/ Ctrl+T（其他）→ 新建终端 tab
          if (isNewTabShortcut(event)) {
            event.preventDefault();
            if (event.type === "keydown") onNewTabRef.current?.();
            return false;
          }

          // Cmd/Ctrl+= / - → 字号 +/-（父组件 clamp + persist + 反向同步回 setFontSize）
          const fontAction = isFontShortcut(event);
          if (fontAction) {
            event.preventDefault();
            if (event.type === "keydown") {
              onFontResizeRef.current?.(fontAction === "increase" ? 1 : -1);
            }
            return false;
          }

          // readline 序列（Option/Cmd 导航+删除）——alternate screen 交 TUI 应用
          const isAltScreen = term.buffer.active.type === "alternate";
          const seq = readlineSequence(event, {
            isMac: IS_MAC,
            isAlternateScreen: isAltScreen,
          });
          if (seq) {
            event.preventDefault();
            if (event.type === "keydown") void pty.write(seq);
            return false; // xterm 不再处理
          }

          // Shift+Enter → \x1b\r（部分 TUI 多行输入用）
          if (isShiftEnter(event)) {
            event.preventDefault();
            if (event.type === "keydown") void pty.write("\x1b\r");
            return false;
          }

          // 其余（Ctrl+A/C/...、Cmd+C/V 复制粘贴）交 xterm 默认
          return true;
        });
        // OSC 7 cwd 追踪 + OSC 133 prompt tracker（安全过滤）
        // 复用外层 ref——return 通过 shellStateRef.current.inCommand 读取最新态
        const shellState = createShellIntegrationState();
        registerPromptTracker(term, shellState);
        registerCwdHandler(term, (c) => setTrackedCwd(c), shellState);
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
      // rAF 可能已调度但未执行——用标志位让它 no-op（无法直接 cancelAnimationFrame
      // 因为没存 handle，但 disposed=true + flushOutput 检查 pendingOutput 即可）
      pendingOutput = null;
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
      fitAddonRef.current = null;
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

  // 6. 字体 prop 变化 → 即时套到 xterm（get_config 异步读回后 + Cmd+=/- 后 index.tsx 改 state）
  // 初始值在 useEffect([]) 创建 Terminal 时已用 opts.fontSize/fontFamily，这里只处理「后续变化」。
  // 用 lastAppliedRef 跳过首次——首次 effect 跑时构造器已套好同样的值，再调一遍 fit/refresh 浪费。
  // useTerminalSession 返回的 setFontSize/setFontFamily 已封装 fit+refresh，直接复用。
  const sessionSelfRef = useRef<{ setFontSize: (n: number) => void; setFontFamily: (s: string) => void } | null>(null);
  // 初始 ref = 首次渲染的 opts 值——首值构造器已套，effect 跳过避免冗余 fit/refresh。
  const lastFontSizeRef = useRef<number | undefined>(opts.fontSize);
  const lastFontFamilyRef = useRef<string | undefined>(opts.fontFamily);
  useEffect(() => {
    if (opts.fontSize != null && sessionSelfRef.current && opts.fontSize !== lastFontSizeRef.current) {
      sessionSelfRef.current.setFontSize(opts.fontSize);
      lastFontSizeRef.current = opts.fontSize;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [opts.fontSize]);
  useEffect(() => {
    if (opts.fontFamily != null && sessionSelfRef.current && opts.fontFamily !== lastFontFamilyRef.current) {
      sessionSelfRef.current.setFontFamily(opts.fontFamily);
      lastFontFamilyRef.current = opts.fontFamily;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [opts.fontFamily]);

  const session = {
    write: (data: string) => {
      ptyRef.current?.write(data);
    },
    focus: () => {
      termRef.current?.focus();
    },
    ptyId,
    searchAddon: searchAddonRef.current,
    cwd: trackedCwd,
    hasSelection: () => termRef.current?.hasSelection() ?? false,
    getSelection: () => termRef.current?.getSelection() ?? undefined,
    paste: (text: string) => termRef.current?.paste(text),
    selectAll: () => {
      const term = termRef.current;
      if (!term) return;
      // xterm select(column, row, length)——选中从 (0,0) 开始的所有字符
      const buf = term.buffer.active;
      term.select(0, 0, term.cols * buf.length);
    },
    clear: () => termRef.current?.clear(),
    setFontSize: (size: number) => {
      const term = termRef.current;
      if (!term) return;
      term.options.fontSize = size;
      // 字号变 → cols/rows 可能变 → 重新 fit 后通知 PTY resize（onResize 自动触发）
      fitAddonRef.current?.fit();
      term.refresh(0, term.rows - 1);
    },
    setFontFamily: (family: string) => {
      const term = termRef.current;
      if (!term) return;
      term.options.fontFamily = family;
      // 字体族等宽情况下列数不变，但字宽可能微调——fit 一下保险
      fitAddonRef.current?.fit();
      term.refresh(0, term.rows - 1);
    },
  };
  // 暴露给字体 prop 变化 effect（同次渲染同步赋值——effect 在 commit 后跑，能拿到最新）
  sessionSelfRef.current = session;
  return session;
}
