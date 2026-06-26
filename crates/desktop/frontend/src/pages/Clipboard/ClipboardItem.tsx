import { cn } from "@/lib/utils";
import { Star, Mic, Type, Image as ImageIcon, FileText } from "lucide-react";
import { invoke } from "@/lib/tauri";
import type { ClipboardItem } from "@/types/clipboard";

export default function ClipboardItemRow({
  item,
  isLast,
  onChanged,
}: {
  item: ClipboardItem;
  isLast: boolean;
  onChanged: () => void;
}) {
  const handleFavorite = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await invoke("toggle_clipboard_favorite", { id: item.id });
      onChanged();
    } catch (e) {
      console.error(e);
    }
  };

  const handleClick = async () => {
    try {
      await invoke("copy_clipboard_item", { id: item.id });
      getCurrentWindow().hide();
    } catch (e) {
      console.error(e);
    }
  };

  const Icon = item.source === "asr" ? Mic
    : item.item_type === "image" ? ImageIcon
    : item.item_type === "file" ? FileText
    : Type;

  const isVoice = item.source === "asr";

  return (
    <div
      className="group relative flex items-start gap-2 px-2.5 py-2 cursor-pointer hover:bg-accent transition-colors"
      onClick={handleClick}
    >
      {/* ASR 条目左侧色条 — 一眼区分语音 vs 复制 */}
      {isVoice && (
        <div className="absolute left-0 top-1.5 bottom-1.5 w-[2px] rounded-r bg-voice/60" />
      )}

      <Icon className={cn(
        "w-4 h-4 mt-px flex-shrink-0 transition-colors",
        isVoice ? "text-voice" : "text-muted-foreground group-hover:text-foreground",
      )} />
      <div className="flex-1 min-w-0">
        {item.item_type === "image" && item.image_meta ? (
          <div className="text-xs text-muted-foreground">
            图片 {item.image_meta.width}×{item.image_meta.height}
          </div>
        ) : item.item_type === "file" && item.file_meta ? (
          <div className="text-xs text-muted-foreground truncate">
            {item.file_meta.file_count} 个文件
          </div>
        ) : (
          <p className="text-xs leading-relaxed text-foreground/90 break-all line-clamp-2">{item.content}</p>
        )}
        {isVoice && item.asr_meta && (
          <span className="inline-block mt-0.5 text-[10px] text-voice/70 font-medium">
            {item.asr_meta.engine}
          </span>
        )}
      </div>
      <button
        className={cn(
          "flex-shrink-0 p-0.5 transition-opacity",
          item.is_favorite ? "opacity-100" : "opacity-0 group-hover:opacity-70 hover:!opacity-100",
        )}
        onClick={handleFavorite}
      >
        <Star
          className={cn("w-4 h-4", item.is_favorite ? "fill-amber-400 text-amber-400" : "text-muted-foreground")}
        />
      </button>

      {/* Hairline 分隔线 — 替代斑马纹 */}
      {!isLast && <div className="absolute bottom-0 left-2.5 right-2.5 h-px bg-border/50" />}
    </div>
  );
}

function getCurrentWindow() {
  return (window as any).__TAURI__?.window?.getCurrentWindow?.() ?? { hide: () => {} };
}
