import { useState, useRef, useEffect } from "react";
import { cn } from "@/lib/utils";
import { Star, Mic, Type, Image as ImageIcon, FileText, Trash2, Download, FolderOpen, Copy, ScanText, Loader2, Check } from "lucide-react";
import { invoke } from "@/lib/tauri";
import type { ClipboardItem } from "@/types/clipboard";
import SaveImagePopover from "./SaveImagePopover";

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
  const [showSavePopover, setShowSavePopover] = useState(false);
  const [ocrLoading, setOcrLoading] = useState(false);
  const [ocrDone, setOcrDone] = useState(false);
  const [thumbSrc, setThumbSrc] = useState<string | null>(null);
  const deleteTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => { if (deleteTimer.current) clearTimeout(deleteTimer.current); };
  }, []);

  useEffect(() => {
    if (item.item_type !== "image") return;
    // 虚拟列表滚动会复用组件实例：item.id 切换时先清旧缩略图，避免新图 base64
    // 经 IPC 传回前短暂显示上一条（幽灵闪烁）；cancelled 防快速滚动时旧请求晚到覆盖新图。
    setThumbSrc(null);
    let cancelled = false;
    invoke<string>("get_image_thumb", { id: item.id })
      .then((dataUrl) => { if (!cancelled) setThumbSrc(dataUrl); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [item.id, item.item_type]);

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
      setDeletePending(true);
      deleteTimer.current = setTimeout(() => setDeletePending(false), 1500);
    } else {
      if (deleteTimer.current) clearTimeout(deleteTimer.current);
      invoke("delete_clipboard_item", { id: item.id }).then(onChanged).catch(console.error);
    }
  };

  // 单击：选中条目（不复制）
  const handleClick = () => {
    if (deletePending) return;
    onSelect();
  };

  // 双击：写剪贴板 → 隐藏浮窗 → 恢复焦点 → 模拟 Cmd+V 粘贴（paste_clipboard_item，
  // 后端串起 hide clipboard_window + focus_tracker.restore_focus + simulate_paste）。
  // 仅浮窗双击走此路；显式「复制」按钮仍调 copy_clipboard_item（不隐藏窗口、不触发粘贴）。
  const handleDoubleClick = async () => {
    try {
      await invoke("paste_clipboard_item", { id: item.id });
    } catch (e) {
      console.error(e);
    }
  };

  const handleSaveImage = (e: React.MouseEvent) => {
    e.stopPropagation();
    setShowSavePopover((v) => !v);
  };

  const handleOcr = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (ocrLoading) return;
    setOcrLoading(true);
    try {
      await invoke("ocr_image", { id: item.id });
      setOcrLoading(false);
      setOcrDone(true);
      setTimeout(() => setOcrDone(false), 1000);
    } catch (e) {
      setOcrLoading(false);
      const msg = String(e);
      if (msg.includes("未识别到文本")) {
        setOcrDone(true);
        setTimeout(() => setOcrDone(false), 1000);
      } else {
        console.error(e);
      }
    }
  };

  const handleOpenFile = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await invoke("open_file_item", { id: item.id });
    } catch (e) {
      console.error(e);
    }
  };

  const handleCopy = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await invoke("copy_clipboard_item", { id: item.id });
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
          <div className="flex items-center gap-1.5">
            {thumbSrc && (
              <img src={thumbSrc} className="w-8 h-8 rounded object-cover flex-shrink-0" alt="" />
            )}
            <span className="text-xs text-muted-foreground">
              {item.image_meta.width}×{item.image_meta.height}
            </span>
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

      {/* 右侧操作：复制 + 收藏 + 保存/打开 + 删除 */}
      <div className="flex-shrink-0 flex items-center gap-0.5">
        <button
          className="p-0.5 opacity-0 group-hover:opacity-60 hover:!opacity-100 transition-opacity"
          onClick={handleCopy}
          title="复制"
        >
          <Copy className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
        </button>
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
        {item.item_type === "image" && (
          <div className="relative">
            <button
              className={cn(
                "p-0.5 transition-opacity hover:scale-110",
                showSavePopover
                  ? "opacity-100"
                  : "opacity-0 group-hover:opacity-60 hover:!opacity-100",
              )}
              onClick={handleSaveImage}
              title="保存为文件"
            >
              <Download className={cn(
                "w-3.5 h-3.5 text-muted-foreground",
                showSavePopover && "text-foreground",
              )} />
            </button>
            {showSavePopover && (
              <SaveImagePopover id={item.id} onClose={() => setShowSavePopover(false)} />
            )}
          </div>
        )}
        {item.item_type === "image" && (
          <button
            className={cn(
              "p-0.5 transition-opacity",
              ocrLoading || ocrDone
                ? "opacity-100"
                : "opacity-0 group-hover:opacity-60 hover:!opacity-100",
            )}
            onClick={handleOcr}
            disabled={ocrLoading}
            title="OCR 识别"
          >
            {ocrLoading ? (
              <Loader2 className="w-3.5 h-3.5 text-muted-foreground animate-spin" />
            ) : ocrDone ? (
              <Check className="w-3.5 h-3.5 text-emerald-600" />
            ) : (
              <ScanText className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
            )}
          </button>
        )}
        {item.item_type === "file" && (
          <button
            className="p-0.5 opacity-0 group-hover:opacity-60 hover:!opacity-100 transition-opacity"
            onClick={handleOpenFile}
            title="打开文件"
          >
            <FolderOpen className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
          </button>
        )}
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
      // Linux X11/Wayland 存 file:// URI + 百分号编码；macOS/Windows 存已解码的普通路径。
      // 仅 file:// 开头才 decodeURIComponent，避免对含字面 %XX 的普通路径误伤。
      const stripped = raw.replace(/^file:\/\//, "");
      const path = raw.startsWith("file://") ? decodeURIComponent(stripped) : stripped;
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
