import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useT } from "@/lib/i18n";

// 截图翻译只读译文浮窗页面。
//
// 设计要点：
//   - listen translate-window://progress|done|reset → 流式渲染译文。
//   - listeners 注册完毕再 invoke set_translate_window_ready，防 ready flush 时
//     pending emit 早于 mount 丢失（后端 PENDING_TEXT 锁同节奏）。
//   - 不监听 blur 自动关闭——浮窗 always_on_top 置顶，用户可一边看译文一边操作其他
//     窗口（如对照原文/编辑器），失焦就消失会打断工作流。仅 Esc / ✕ 按钮关闭。
//   - Esc 隐藏窗口（不销毁——窗口预创建复用）。
//
// 复制：与项目内其它窗口（PasswordGenerator / CipherEditor / QrResultCard 等 10+
// 处）一致使用 navigator.clipboard.writeText，无需 @tauri-apps/plugin-clipboard-manager
//（该项目未将该 plugin 列入 dependencies）。
export default function Translate() {
  const [text, setText] = useState("");
  const [done, setDone] = useState(false);
  const [copied, setCopied] = useState(false);
  const t = useT();

  useEffect(() => {
    // 1. 先注册 listener（防 ready flush 时 pending emit 丢失）
    const unlistenProgress = listen<string>("translate-window://progress", (e) => {
      setText(e.payload);
      setCopied(false);
    });
    const unlistenDone = listen<string>("translate-window://done", (e) => {
      setText(e.payload);
      setDone(true);
      setCopied(false);
    });
    const unlistenReset = listen("translate-window://reset", () => {
      setText("");
      setDone(false);
      setCopied(false);
    });
    // 2. 通知后端 ready（触发 pending 文本一次性 emit）
    invoke("set_translate_window_ready").catch((e) => console.error("ready failed:", e));
    return () => {
      unlistenProgress.then((u) => u());
      unlistenDone.then((u) => u());
      unlistenReset.then((u) => u());
    };
  }, []);

  // Esc 关闭
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") getCurrentWindow().hide();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const handleCopy = async () => {
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      console.error("copy failed:", e);
    }
  };

  return (
    <div
      data-tauri-drag-region
      className="w-screen h-screen flex flex-col bg-background/90 text-foreground backdrop-blur-2xl rounded-lg overflow-hidden select-none"
    >
      <header data-tauri-drag-region className="flex items-center justify-between px-3 py-1.5 border-b border-border cursor-move">
        <span className="text-xs opacity-60">
          {done ? t("screenshot.translate.done") : t("screenshot.translate.translating")}
        </span>
        <button
          onClick={(e) => { e.stopPropagation(); getCurrentWindow().hide(); }}
          className="opacity-50 hover:opacity-100 text-xs"
          title={t("common.close")}
        >✕</button>
      </header>
      <main className="flex-1 overflow-auto p-3 text-sm leading-relaxed whitespace-pre-wrap break-words select-text">
        {text || <span className="opacity-50">⏳ {t("screenshot.translate.translating")}</span>}
      </main>
      <footer className="flex items-center justify-end gap-2 px-3 py-2 border-t border-border">
        <button
          onClick={handleCopy}
          disabled={!text}
          className="px-3 py-1 text-xs rounded bg-primary text-primary-foreground disabled:opacity-40 hover:bg-primary/90"
        >
          {copied ? t("common.copied") : t("common.copy")}
        </button>
      </footer>
    </div>
  );
}
