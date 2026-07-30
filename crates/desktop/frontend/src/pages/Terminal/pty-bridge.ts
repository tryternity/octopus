/**
 * 前端 → Rust PTY 命令桥。
 *
 * 对齐 Task 5 的 Rust 命令签名（terminal_commands.rs）：
 * - `pty_open`：Channel<ArrayBuffer> onData + Channel<number> onExit → 返回 ptyId
 * - `pty_write`：**raw body (Uint8Array) + `x-pty-id` header**，绕过 JSON（按键延迟敏感）
 * - `pty_resize` / `pty_close`：标准 JSON invoke
 *
 * 参考 Terax pty-bridge.ts，去掉 workspace/blocks/shell override（Phase 1 不需要）。
 */
import { invoke, Channel } from "@tauri-apps/api/core";

const textEncoder = new TextEncoder();

export type PtyHandlers = {
  /** PTY 输出字节（xterm.write 直接消费）。 */
  onData: (bytes: Uint8Array) => void;
  /** 子进程退出码。 */
  onExit?: (code: number) => void;
};

/** 一个 PTY 会话的句柄——openPty 返回，持有 id + write/resize/close。 */
export type PtySession = {
  id: number;
  /** 写入用户输入（raw body + header，无 JSON 序列化）。 */
  write: (data: string) => Promise<void>;
  /** 调整 PTY 尺寸（窗口/tab resize 时）。 */
  resize: (cols: number, rows: number) => Promise<void>;
  /** 关闭 PTY（关 tab 时）。幂等。 */
  close: () => Promise<void>;
};

/**
 * 打开一个 PTY session。
 *
 * @param cols/rows 初始尺寸（xterm fitAddon 算出）
 * @param handlers onData/onExit 回调
 * @param cwd 可选 shell 启动目录（ActionBar 联动时传项目目录）
 */
export async function openPty(
  cols: number,
  rows: number,
  handlers: PtyHandlers,
  cwd?: string,
): Promise<PtySession> {
  // 原始字节 Channel——无 base64/JSON 往返，消息以 ArrayBuffer 到达。
  const onData = new Channel<ArrayBuffer>();
  const onExit = new Channel<number>();

  // 防退出后回调再触发（Channel 可能延迟派发最后一条）。
  let released = false;
  const noop = () => {};
  const releaseHandlers = () => {
    if (released) return;
    released = true;
    onData.onmessage = noop;
    onExit.onmessage = noop;
  };

  onData.onmessage = (buf) => handlers.onData(new Uint8Array(buf));
  onExit.onmessage = (code) => {
    handlers.onExit?.(code);
    releaseHandlers();
  };

  const id = await invoke<number>("pty_open", {
    cols,
    rows,
    cwd: cwd ?? null,
    onData,
    onExit,
  });

  let closed = false;
  // pty_write 走 raw body：header 带 id，body 是编码后的字节。
  const headers = { "x-pty-id": String(id) };

  return {
    id,
    write: (data) => invoke("pty_write", textEncoder.encode(data), { headers }),
    resize: (c, r) => invoke("pty_resize", { id, cols: c, rows: r }),
    close: async () => {
      if (closed) return;
      closed = true;
      try {
        await invoke("pty_close", { id });
      } finally {
        releaseHandlers();
      }
    },
  };
}
