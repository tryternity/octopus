/**
 * InstantOverlay — talk / PTT 模式的只读指示浮窗。
 *
 * 由 Rust `instant-state` 事件驱动（payload: `{ state, text }`）：
 * - listening：波形动画 + "正在聆听…"（text 可为实时识别文字，非空则展示）
 * - processing：spinner + "识别中…"
 * - polishing：spinner + "润色中…"
 * - done：展示最终文字（短暂停留后由 Rust hide）
 *
 * 设计：极简、不抢焦点、底部居中。无编辑能力（区别于 result_window）。
 */
import { useState, useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { cn } from "@/lib/utils";

type InstantState = "listening" | "processing" | "polishing" | "done";

interface InstantStatePayload {
  state: InstantState;
  text: string;
}

/** 波形动画条（listening 状态用）。 */
function Waveform() {
  // 5 根高度交替的条形，CSS animation 错峰播放。
  const bars = [0, 1, 2, 3, 4];
  return (
    <div className="flex items-end gap-[3px] h-4">
      {bars.map((i) => (
        <span
          key={i}
          className="w-[3px] rounded-full bg-voice"
          style={{
            height: "100%",
            transformOrigin: "bottom",
            animation: `instant-wave 0.9s ease-in-out ${i * 0.12}s infinite`,
          }}
        />
      ))}
    </div>
  );
}

/** spinner 圆环（processing / polishing 状态用，纯 CSS 无依赖）。 */
function Spinner() {
  return (
    <span
      className="inline-block w-4 h-4 rounded-full border-2 border-voice/30 border-t-voice"
      style={{ animation: "instant-spin 0.7s linear infinite" }}
    />
  );
}

const STATE_LABEL: Record<InstantState, string> = {
  listening: "正在聆听…",
  processing: "识别中…",
  polishing: "润色中…",
  done: "",
};

export default function InstantOverlay() {
  const [payload, setPayload] = useState<InstantStatePayload | null>(null);

  useEffect(() => {
    const win = getCurrentWindow();
    const unlisten = win.listen<InstantStatePayload>("instant-state", (event) => {
      setPayload(event.payload);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // 初始无状态（窗口刚 show 但首帧事件未到）→ 不渲染可见内容，
  // 保持透明（避免空壳闪烁）。事件到达后立即可见。
  if (!payload) return null;

  const showText = payload.state === "done"
    ? payload.text
    : (payload.text && payload.state === "listening" ? payload.text : "");

  return (
    <div
      data-instant-overlay
      className="flex items-center gap-2.5 px-4 h-[80px] w-[400px] rounded-[14px] border border-border/40 shadow-2xl shadow-black/30 overflow-hidden bg-background/90 backdrop-blur-2xl"
    >
      {/* 左：状态指示 */}
      <div className="flex items-center justify-center w-5 shrink-0">
        {payload.state === "listening" && <Waveform />}
        {(payload.state === "processing" || payload.state === "polishing") && <Spinner />}
        {payload.state === "done" && (
          <span className="w-2 h-2 rounded-full bg-green-500 shrink-0" />
        )}
      </div>

      {/* 中：文字 / 标签 */}
      <div className="flex-1 min-w-0 flex items-center">
        {showText ? (
          <span
            className="text-[14px] font-medium leading-tight text-foreground truncate"
            title={showText}
          >
            {showText}
          </span>
        ) : (
          <span
            className={cn(
              "text-[14px] font-medium leading-none whitespace-nowrap",
              "text-foreground/80",
            )}
          >
            {STATE_LABEL[payload.state] || payload.state}
          </span>
        )}
      </div>
    </div>
  );
}
