import { useState, useRef, useEffect } from "react";
import { cn } from "@/lib/utils";
import { Star, Mic, Type, Image as ImageIcon, FileText, Trash2 } from "lucide-react";
import { invoke } from "@/lib/tauri";
import type { ClipboardItem } from "@/types/clipboard";

export default function ClipboardItemRow({
  item,
  isLast,
  isSelected,
  onSelect,
  onChanged,
}: {
  item: ClipboardItem;
  isLast: boolean;
  isSelected: boolean;
  onSelect: () => void;
  onChanged: () => void;
}) {
  const [deletePending, setDeletePending] = useState(false);
  const deleteTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => { if (deleteTimer.current) clearTimeout(deleteTimer.current); };
  }, []);

  const handleFavorite = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await invoke("toggle_clipboard_favorite", { id: item.id });
      onChanged();
    } catch (e) {
      console.error(e);
    }
  };

  const handleDeleteClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!deletePending) {
      // 第一次点击：进入待确认状态，1.5s 后恢复
      setDeletePending(true);
      deleteTimer.current = setTimeout(() => setDeletePending(false), 1500);
    } else {
      // 第二次点击（1.5s 内）：确认删除
      if (deleteTimer.current) clearTimeout(deleteTimer.current);
      invoke("delete_clipboard_item", { id: item.id }).then(onChanged).catch(console.error);
    }
  };

  const handleClick = () => {
    if (deletePending) return; // 待确认状态下不触发选中
    onSelect();
  };

  const handleDoubleClick = async () => {
    try {
      // hide + restore_focus + paste 全在后端处理
      await invoke("paste_clipboard_item", { id: item.id });
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
      className={cn(
        "group relative flex items-start gap-2 px-2.5 py-2 cursor-pointer transition-colors",
        isSelected && !deletePending ? "bg-accent" : "hover:bg-accent",
        deletePending && "bg-red-50",
      )}
      onClick={handleClick}
      onDoubleClick={handleDoubleClick}
    >
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
        ) : item.item_type === "file" ? (
          <div className="text-xs text-muted-foreground truncate">
            {formatFilePaths(item.content, item.file_meta?.file_count)}
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

      {/* 右侧操作：收藏 + 删除 */}
      <div className="flex-shrink-0 flex items-center gap-0.5">
        <button
          className={cn(
            "p-0.5 transition-opacity hover:scale-110",
            item.is_favorite ? "opacity-100" : "opacity-0 group-hover:opacity-60 hover:!opacity-100",
          )}
          onClick={handleFavorite}
        >
          <Star
            className={cn("w-3.5 h-3.5", item.is_favorite ? "fill-amber-400 text-amber-400" : "text-muted-foreground")}
          />
        </button>
        <button
          className={cn(
            "p-0.5 transition-all",
            deletePending
              ? "opacity-100 bg-red-100 rounded"
              : "opacity-0 group-hover:opacity-60 hover:!opacity-100 transition-opacity",
          )}
          onClick={handleDeleteClick}
          title={deletePending ? "再次点击确认删除" : "删除"}
        >
          <Trash2 className={cn(
            "w-3.5 h-3.5 transition-colors",
            deletePending ? "text-red-600" : "text-muted-foreground hover:text-red-500",
          )} />
        </button>
      </div>

      {!isLast && <div className="absolute bottom-0 left-2.5 right-2.5 h-px bg-border/50" />}
    </div>
  );
}

/// content 是 JSON 路径数组，取每个路径最后 2 段显示。
function formatFilePaths(content: string, count?: number): string {
  try {
    const paths: string[] = JSON.parse(content);
    const display = paths.slice(0, 3).map((raw) => {
      const path = raw.replace(/^file:\/\//, "");
      const parts = path.split("/").filter(Boolean);
      const tail = parts.slice(-2).join("/");
      return "…/" + tail;
    });
    if (paths.length > 3) {
      return display.join("  ") + `  +${paths.length - 3}`;
    }
    return display.join("  ") + (count ? ` (${count})` : "");
  } catch {
    return count ? `${count} 个文件` : "文件";
  }
}
