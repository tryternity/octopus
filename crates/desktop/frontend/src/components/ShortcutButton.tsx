import { useT } from "@/lib/i18n";

/**
 * 快捷键展示 + 录制按钮。
 *
 * 纯展示组件——不持有录制状态。父组件通过 `capturing` 控制是否进入录制态，
 * 点击时触发 `onClick`，由父组件决定何时开始/结束录制（父组件挂载 keydown 监听器）。
 *
 * 约定：`shortcut` 为 Tauri global-shortcut 字符串（如 "CmdOrCtrl+Shift+P"），
 * 用 "+" 分隔；修饰键会被渲染为对应符号（⌘ / ⌥ / ⇧）。
 *
 * 键帽样式：默认用主题 token 色（修复原 stone 硬编码在深色主题下显白块的 bug）；
 * raycast 主题下叠加 .raycast-key 获得 DESIGN.md Level 4 物理按键质感（渐变 + 多层阴影）。
 */
export default function ShortcutButton({
  shortcut,
  capturing,
  onClick,
}: {
  shortcut: string;
  capturing: boolean;
  onClick: () => void;
}) {
  const t = useT();
  if (capturing) {
    return (
      <button
        className="px-3 py-1.5 rounded-md text-xs font-medium text-voice bg-voice/5 border border-voice/40 cursor-pointer animate-pulse"
        onClick={onClick}
      >
        {t("settings.general.shortcutRecordingHint")}
      </button>
    );
  }
  const keys = shortcut.split("+");
  return (
    <button
      className="flex items-center gap-1 px-2.5 py-1.5 rounded-md border border-border bg-muted/40 hover:border-foreground/30 cursor-pointer transition-colors group"
      onClick={onClick}
    >
      {shortcut === "" ? (
        <span className="px-1.5 py-0.5 text-[11px] text-muted-foreground/50">—</span>
      ) : (
        keys.map((k, i) => (
          <span key={i} className="flex items-center gap-1">
            {i > 0 && <span className="text-muted-foreground/40 text-[10px]">+</span>}
            <kbd className="raycast-key min-w-[20px] px-1.5 py-0.5 text-[11px] font-medium text-foreground bg-surface rounded border border-border group-hover:border-foreground/30 transition-colors">
              {k === "CmdOrCtrl" ? "⌘" : k === "Alt" ? "⌥" : k === "Shift" ? "⇧" : k}
            </kbd>
          </span>
        ))
      )}
    </button>
  );
}
