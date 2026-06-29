import { useState, useCallback, useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@/lib/tauri";
import { useClipboardHistory } from "@/hooks/useClipboardHistory";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import FilterTabs from "./FilterTabs";
import SearchBar from "./SearchBar";
import ClipboardItemRow from "./ClipboardItem";
import { Pin, X, Settings2, CircleCheck, CircleX } from "lucide-react";
import { cn } from "@/lib/utils";

interface ConfigResponse {
  config: Record<string, string | number | boolean>;
}

export default function Clipboard() {
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  const [pinned, setPinned] = useState(false);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [recording, setRecording] = useState(true);

  const { items, total, refresh } = useClipboardHistory(filter, search);

  // 监听开关：mount 读 get_config + 监听 config-changed 同步（与设置页 toggle 双向同步）。
  const loadRecording = useCallback(async () => {
    try {
      const resp = await invoke<ConfigResponse>("get_config");
      setRecording(resp.config.clipboard_enabled !== false);
    } catch (e) {
      console.error(e);
    }
  }, []);
  useEffect(() => { loadRecording(); }, [loadRecording]);
  useTauriEvent("config-changed", () => loadRecording());

  const toggleRecording = useCallback(async () => {
    const next = !recording;
    setRecording(next); // 乐观更新；config-changed 回调会校正
    try {
      await invoke("set_config", { key: "clipboard_enabled", value: next });
    } catch (e) {
      setRecording(!next); // 回滚
      console.error(e);
    }
  }, [recording]);

  const togglePin = useCallback(async () => {
    const next = !pinned;
    setPinned(next);
    try {
      const win = getCurrentWindow();
      await win.setAlwaysOnTop(next);
    } catch (e) {
      console.error(e);
    }
  }, [pinned]);

  return (
    <div className="flex flex-col h-screen bg-background text-foreground select-none overflow-hidden rounded-xl border border-border shadow-2xl shadow-black/8" data-tauri-drag-region>
      {/* Title bar — 极简，去掉"历史" */}
      {/* deep：点击标题文本/空白均触发拖动；按钮仍因 clickable 元素被 drag.js 跳过，不受影响 */}
      <div className="flex items-center justify-between px-2 py-1.5 cursor-grab active:cursor-grabbing" data-tauri-drag-region="deep">
        <button
          className="p-1 rounded cursor-default hover:bg-accent text-muted-foreground hover:text-foreground transition-colors"
          onClick={() => getCurrentWindow().hide()}
          title="关闭"
        >
          <X className="w-3.5 h-3.5" />
        </button>
        <span className="text-[11px] font-medium tracking-wide text-muted-foreground">剪贴板</span>
        <div className="flex items-center gap-0.5">
          {/* 监听开关：复制敏感内容前可在此快速暂停。与 Pin 同为状态 toggle，成组于右侧。 */}
          <button
            className={cn(
              "p-1 rounded cursor-default transition-colors",
              recording
                ? "text-green-500 hover:bg-green-500/15"
                : "text-red-500 bg-red-500/15 hover:bg-red-500/25",
            )}
            onClick={toggleRecording}
            title={recording ? "暂停监听" : "恢复监听"}
          >
            {recording
              ? <CircleCheck className="w-3.5 h-3.5" />
              : <CircleX className="w-3.5 h-3.5" />}
          </button>
          <button
            className={cn(
              "p-1 rounded cursor-default transition-colors",
              pinned ? "text-voice bg-voice/10" : "text-muted-foreground hover:bg-accent hover:text-foreground",
            )}
            onClick={togglePin}
            title="置顶"
          >
            <Pin className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      {/* Search + Filter */}
      <div className="px-2 pb-1.5 flex flex-col gap-1.5">
        <SearchBar value={search} onChange={setSearch} />
        <FilterTabs value={filter} onChange={setFilter} />
      </div>

      {/* List */}
      <div className="clipboard-list flex-1 overflow-y-auto pb-1">
        {items.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full gap-1 text-muted-foreground/50">
            <span className="text-xs">暂无记录</span>
          </div>
        ) : (
          items.map((item, index) => (
            <ClipboardItemRow
              key={item.id}
              item={item}
              isLast={index === items.length - 1}
              isSelected={selectedId === item.id}
              onSelect={() => setSelectedId(item.id)}
              onChanged={refresh}
            />
          ))
        )}
      </div>

      {/* Footer */}
      <div className="flex items-center justify-between px-3 py-1 border-t border-border text-[10px] text-muted-foreground/80">
        <span>{total} 条</span>
        <button
          className="flex items-center gap-0.5 hover:text-foreground transition-colors"
          onClick={() => invoke("open_settings", { initialPage: "clipboard" })}
          title="管理剪贴板"
        >
          <Settings2 className="w-2.5 h-2.5" />
          管理
        </button>
      </div>
    </div>
  );
}
