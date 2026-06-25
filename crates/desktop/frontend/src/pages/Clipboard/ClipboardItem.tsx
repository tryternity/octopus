import { cn } from "@/lib/utils";
import { Star, Mic, Type, Image as ImageIcon, FileText } from "lucide-react";
import { invoke } from "@/lib/tauri";
import type { ClipboardItem } from "@/types/clipboard";

export default function ClipboardItemRow({
  item,
  onChanged,
}: {
  item: ClipboardItem;
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
      await invoke("hide_window", {}).catch(() => {
        getCurrentWindow().hide();
      });
    } catch (e) {
      console.error(e);
    }
  };

  const Icon = item.source === "asr" ? Mic
    : item.item_type === "image" ? ImageIcon
    : item.item_type === "file" ? FileText
    : Type;

  return (
    <div
      className="flex items-start gap-2.5 px-3 py-2 hover:bg-accent rounded-md cursor-pointer group"
      onClick={handleClick}
    >
      <Icon className="w-4 h-4 mt-0.5 flex-shrink-0 text-muted-foreground" />
      <div className="flex-1 min-w-0">
        {item.item_type === "image" && item.image_meta ? (
          <div className="text-xs text-muted-foreground">
            [图片] {item.image_meta.width}×{item.image_meta.height}
          </div>
        ) : item.item_type === "file" && item.file_meta ? (
          <div className="text-xs text-muted-foreground truncate">
            [文件] {item.file_meta.file_count} 个文件
          </div>
        ) : (
          <p className="text-sm break-all line-clamp-2">{item.content}</p>
        )}
        {item.source === "asr" && item.asr_meta && (
          <span className="inline-block mt-0.5 text-[10px] px-1.5 py-0.5 rounded bg-primary/10 text-primary">
            {item.asr_meta.engine}
          </span>
        )}
      </div>
      <button
        className={cn(
          "flex-shrink-0 p-0.5 transition-opacity",
          item.is_favorite ? "opacity-100" : "opacity-0 group-hover:opacity-50 hover:!opacity-100",
        )}
        onClick={handleFavorite}
      >
        <Star
          className={cn("w-4 h-4", item.is_favorite ? "fill-yellow-400 text-yellow-400" : "text-muted-foreground")}
        />
      </button>
    </div>
  );
}

function getCurrentWindow() {
  return (window as any).__TAURI__?.window?.getCurrentWindow?.() ?? { hide: () => {} };
}
