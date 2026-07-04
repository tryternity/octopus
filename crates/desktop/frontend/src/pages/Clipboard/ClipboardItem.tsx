import { useState, useRef, useEffect } from "react";
import { cn } from "@/lib/utils";
import { Star, Mic, Type, Image as ImageIcon, FileText, Trash2, Download, FolderOpen, ScanText, SquarePen, Link as LinkIcon, Copy as CopyIcon } from "lucide-react";
import { invoke } from "@/lib/tauri";
import { openCompactEditorTab } from "@/lib/compactEditor";
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
  const [thumbSrc, setThumbSrc] = useState<string | null>(null);
  const deleteTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [copied, setCopied] = useState(false);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => {
      if (deleteTimer.current) clearTimeout(deleteTimer.current);
      if (copyTimer.current) clearTimeout(copyTimer.current);
    };
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

  const handleOpenFile = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await invoke("open_file_item", { id: item.id });
    } catch (e) {
      console.error(e);
    }
  };

  // 单击左侧类型图标 → 复制（copy_clipboard_item，不隐藏浮窗、不触发粘贴）。
  // 触效：icon 放大回弹 + 闪绿；右侧弹「已复制」气泡 1.5s。
  const handleCopy = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await invoke("copy_clipboard_item", { id: item.id });
      setCopied(true);
      if (copyTimer.current) clearTimeout(copyTimer.current);
      copyTimer.current = setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      console.error(e);
    }
  };

  const handleEditText = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (item.item_type === "image" || item.item_type === "file") return;
    openCompactEditorTab(item.id);
  };

  const isUrl = item.item_type === "text" && /^https?:\/\//i.test(item.content.trim());

  const Icon = item.source === "asr" ? Mic
    : item.source === "ocr" ? ScanText
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

      <button
        type="button"
        onClick={handleCopy}
        onDoubleClick={(e) => e.stopPropagation()}
        title="单击复制"
        className="relative flex-shrink-0 mt-px -m-0.5 cursor-pointer rounded p-0.5 transition-transform duration-150 hover:scale-110 active:scale-90"
      >
        <Icon className={cn(
          "w-4 h-4 transition-all duration-150",
          isVoice ? "text-voice" : "text-muted-foreground group-hover:text-foreground",
          copied && "scale-125 text-emerald-500",
        )} />
        {copied && (
          <span className="pointer-events-none absolute left-full top-1/2 z-10 ml-1 -translate-y-1/2 whitespace-nowrap rounded bg-emerald-500 px-1.5 py-0.5 text-[10px] font-medium text-white shadow">
            已复制
          </span>
        )}
      </button>
      {/* 复制图标（紧随类型图标，点击触发同样复制操作） */}
      <button
        type="button"
        onClick={handleCopy}
        onDoubleClick={(e) => e.stopPropagation()}
        title="复制"
        className="flex-shrink-0 mt-px -m-0.5 cursor-pointer rounded p-0.5 opacity-0 group-hover:opacity-60 hover:!opacity-100 transition-opacity"
      >
        <CopyIcon className="w-3 h-3 text-muted-foreground hover:text-foreground" />
      </button>
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
          <p className="text-xs leading-relaxed text-foreground/90 break-all line-clamp-2">{[...item.content].length > 200 ? [...item.content].slice(0, 200).join("") + "……" : item.content}</p>
        )}
        {isVoice && item.asr_meta && (
          <span className="inline-block mt-0.5 text-[10px] text-voice/70 font-medium">
            {item.asr_meta.engine}
          </span>
        )}
      </div>

      {/* 右侧操作：编辑/预览/保存/OCR/打开/删除 + 收藏置末（已收藏常显高亮，置首会使其后 hover 按钮被遮、视觉怪）。
          dblclick 阻止冒泡：连续快速点操作按钮（如删多条）不应被条目 onDoubleClick 误判为双击 → 粘贴 + 隐藏浮窗。 */}
      <div className="flex-shrink-0 flex items-center gap-0.5" onDoubleClick={(e) => e.stopPropagation()}>
        {isUrl && (
          <button
            className="p-0.5 opacity-60 hover:!opacity-100 transition-opacity"
            onClick={(e) => {
              e.stopPropagation();
              window.open(item.content.trim(), "_blank");
            }}
            title="打开链接"
          >
            <LinkIcon className="w-3.5 h-3.5 text-blue-500 hover:text-blue-600" />
          </button>
        )}
        {item.item_type !== "image" && item.item_type !== "file" && (
          <button
            className="p-0.5 opacity-0 group-hover:opacity-60 hover:!opacity-100 transition-opacity"
            onClick={handleEditText}
            title="编辑"
          >
            <SquarePen className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
          </button>
        )}
        {item.item_type === "image" && (
          <button
            className="p-0.5 opacity-0 group-hover:opacity-60 hover:!opacity-100 transition-opacity"
            onClick={(e) => {
              e.stopPropagation();
              openCompactEditorTab(item.id);
            }}
            title="预览"
          >
            <img src="icons/eye-edit.svg" alt="预览" className="w-3.5 h-3.5" />
          </button>
        )}
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
