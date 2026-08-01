/**
 * InstantView — talk / PTT 模式的只读指示卡（合并入 Result 页面，由 record-mode 切换显示）。
 *
 * 由父组件 Result/index.tsx 监听 `instant-state` 事件后传入 `state` / `text` props：
 * - listening：波形动画 + "正在聆听…"（text 可为实时识别文字，非空则展示）
 * - processing：spinner + "识别中…"
 * - polishing：spinner + "润色中…"
 * - done：展示最终文字（短暂停留后由 Rust hide）
 *
 * 设计：极简、不抢焦点、底部居中。无编辑能力（区别于 toggle 视图的 CM6 编辑器）。
 * 本组件为纯展示组件——事件订阅上移到 AsrWindow 根（Result/index.tsx）。
 */
import { cn } from "@/lib/utils";

type InstantState = "listening" | "processing" | "polishing" | "done";

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

/// listening 态显示尾部最新内容——用户说话时能看到最新说的词。
/// 尾部最新内容完整展示（不省略），前面累积的旧内容若超出卡片宽度则开头省略号（…）。
/// 实现用 CSS `direction: rtl`——`truncate`（text-overflow:ellipsis）的省略号出现在左侧，
/// 中文字符顺序不受影响（仅块方向变 RTL，强 LTR 的中文字符内部顺序不变）。
/// done 态保持默认 LTR + truncate（开头完整，尾部省略——终态完整度优先）。

export function InstantView({ state, text }: { state: string; text: string }) {
  // 初始无状态（窗口刚 show 但首帧事件未到）→ 不渲染可见内容，
  // 保持透明（避免空壳闪烁）。事件到达后立即可见。
  if (!state) return null;

  const typedState = state as InstantState;
  const isListeningTail = typedState === "listening" && !!text;
  const showText = (typedState === "done" || isListeningTail) ? text : "";

  return (
    // 720×480 透明区底部居中——容器是 absolute 定位，内部指示卡为可视元素。
    <div
      style={{
        position: "absolute",
        bottom: 0,
        left: "50%",
        transform: "translateX(-50%)",
        width: "400px",
      }}
    >
      <div
        data-instant-view
        className="flex items-center gap-2.5 px-4 h-[80px] w-[400px] rounded-[14px] border border-border/40 shadow-2xl shadow-black/30 overflow-hidden bg-background/90 backdrop-blur-2xl"
      >
        {/* 左：状态指示 */}
        <div className="flex items-center justify-center w-5 shrink-0">
          {typedState === "listening" && <Waveform />}
          {(typedState === "processing" || typedState === "polishing") && <Spinner />}
          {typedState === "done" && (
            <span className="w-2 h-2 rounded-full bg-green-500 shrink-0" />
          )}
        </div>

        {/* 中：文字 / 标签 */}
        <div className="flex-1 min-w-0 flex items-center">
          {showText ? (
            <span
              // listening 态 dir=rtl：truncate 的省略号出现在左侧（开头），尾部最新内容完整展示。
              // done 态保持默认 LTR：省略号在右侧（尾部），开头完整（终态完整度优先）。
              // 文本用 FSI/PDI（\u2068..\u2069）隔离——根据首字符方向自动选定段落方向，
              // 避免 RTL 容器把英文/数字尾巴顺序颠倒（中文首字符→隔离为 LTR，内部顺序不变）。
              dir={isListeningTail ? "rtl" : "ltr"}
              className="text-[14px] font-medium leading-tight text-foreground truncate"
              title={showText}
            >
              {isListeningTail ? `\u2068${showText}\u2069` : showText}
            </span>
          ) : (
            <span
              className={cn(
                "text-[14px] font-medium leading-none whitespace-nowrap",
                "text-foreground/80",
              )}
            >
              {STATE_LABEL[typedState] || state}
            </span>
          )}
        </div>
      </div>
    </div>
  );
}

export default InstantView;
