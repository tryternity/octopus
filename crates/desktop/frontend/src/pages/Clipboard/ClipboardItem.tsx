import { useState, useRef, useEffect, memo, useMemo } from "react";
import { cn } from "@/lib/utils";
import { Star, Mic, Type, Image as ImageIcon, FileText, Trash2, Download, FolderOpen, ScanText, SquarePen, Link as LinkIcon, Copy, Check } from "lucide-react";
import { invoke } from "@/lib/tauri";
import { openUrl } from "@tauri-apps/plugin-opener";
import { openCompactEditorTab } from "@/lib/compactEditor";
import type { ClipboardItem } from "@/types/clipboard";
import { metaParts, typeAccent, imageMeta, fileMeta, detectUrl } from "@/types/clipboard";
import SaveImagePopover from "./SaveImagePopover";

function ClipboardItemRow({
  item,
  index,
  isLast,
  isSelected,
  onSelect,
  onChanged,
}: {
  item: ClipboardItem;
  index: number;
  isLast: boolean;
  isSelected: boolean;
  onSelect: (index: number) => void;
  onChanged: () => void;
}) {
  const [deletePending, setDeletePending] = useState(false);
  const [showSavePopover, setShowSavePopover] = useState(false);
  const [thumbSrc, setThumbSrc] = useState<string | null>(null);
  const deleteTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [copied, setCopied] = useState(false);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Download 触发按钮 ref：传给 SaveImagePopover，让其 outside-click 检测忽略它，
  // 否则 mousedown 关闭与 click toggle 时序冲突，表现为再次点击 Download 关不掉 popover。
  const saveBtnRef = useRef<HTMLButtonElement>(null);

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
    onSelect(index);
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

  const link = item.item_type === "text" ? detectUrl(item.content) : null;
  const isUrl = !!link?.isLink;

  // 预览文本：码元数 ≤200 时字符数必 ≤200（emoji 占多码元），直接返回原串零开销；
  // 超过才展开字符数组精确按字符截断。原代码每次渲染两次 [...content] 全量展开，
  // 长文本（几万字）滚动时掉帧。useMemo 在 item.content 不变（如选中行切换）时复用。
  const preview = useMemo(() => {
    if (item.content.length <= 200) return item.content;
    const chars = [...item.content];
    return chars.length > 200 ? chars.slice(0, 200).join("") + "……" : item.content;
  }, [item.content]);

  const Icon = item.item_type === "voice" ? Mic
    : item.item_type === "ocr" ? ScanText
    : item.item_type === "image" ? ImageIcon
    : item.item_type === "file" ? FileText
    : Type;

  const isVoice = item.item_type === "voice";
  // 第一行行尾元数据：text/ocr→N字；voice→N字·Xs；image→w×h·size；file→类型/N个
  const row1Meta =
    item.item_type === "image" ? imageMeta(item)
    : item.item_type === "file" ? fileMeta(item)
    : metaParts(item);
  const accent = typeAccent[item.item_type];

  return (
    <div
      data-clip-index={index}
      className={cn(
        "group relative px-3 py-2 cursor-pointer transition-colors flex items-center gap-2.5",
        isSelected && !deletePending ? "bg-accent" : "hover:bg-accent",
        deletePending && "bg-red-50",
      )}
      onClick={handleClick}
      onDoubleClick={handleDoubleClick}
    >
      {isVoice && (
        <div className="absolute left-0 top-2 bottom-2 w-[2px] rounded-r bg-voice/50" />
      )}

      {/* 类型图标(单击复制)：左列跨两行垂直居中、放大一档(w-4→w-5)，作为条目「头像」。
          外层 flex = 图标列 + 右侧两行内容栏。 */}
      <button
        type="button"
        onClick={handleCopy}
        onDoubleClick={(e) => e.stopPropagation()}
        title="单击复制"
        className="relative flex-shrink-0 cursor-pointer rounded p-0.5 transition-transform duration-150 hover:scale-110 active:scale-90"
      >
        <Icon className={cn(
          "w-5 h-5 transition-all duration-150",
          accent,
          copied && "scale-125 text-emerald-500",
        )} />
        {copied && (
          <span className="pointer-events-none absolute left-full top-1/2 z-10 ml-1.5 -translate-y-1/2 whitespace-nowrap rounded-md bg-emerald-500 px-2 py-0.5 text-[10px] font-semibold text-white shadow-md">
            已复制
          </span>
        )}
      </button>

      {/* 右侧两行内容栏：第一行 内容+元数据；第二行 时间戳+操作。 */}
      <div className="flex-1 min-w-0">
      <div className="flex items-center gap-2">

        <div className="flex-1 min-w-0">
          {item.item_type === "image" ? (
            thumbSrc && (
              <img src={thumbSrc} className="w-10 h-10 rounded-md object-cover flex-shrink-0 ring-1 ring-black/5" alt="" />
            )
          ) : item.item_type === "file" ? (
            <span className="block truncate text-[12px] text-muted-foreground">{formatFilePaths(item.ref_data)}</span>
          ) : (
            <p className="break-all line-clamp-1 text-[12.5px] leading-snug text-foreground/90">{preview}</p>
          )}
        </div>

        {row1Meta && (
          <span className={cn("flex-shrink-0 text-[10px] font-medium tabular-nums", accent)}>{row1Meta}</span>
        )}
      </div>

      {/* 第二行：时间戳 + 操作（内容栏内，已对齐图标右侧，无需 pl 缩进）。 */}
      <div className="mt-1 flex items-center justify-between" onDoubleClick={(e) => e.stopPropagation()}>
        <span className="tabular-nums text-[10px] text-muted-foreground/60">{item.created_at}</span>
        <div className="flex flex-shrink-0 items-center gap-0.5">
          <button
            className={cn(
              "p-0.5 transition-opacity hover:scale-110",
              copied ? "opacity-100" : "opacity-0 group-hover:opacity-60 hover:!opacity-100",
            )}
            onClick={handleCopy}
            title="复制"
          >
            {copied ? (
              <Check className="w-3.5 h-3.5 text-emerald-500" />
            ) : (
              <Copy className="w-3.5 h-3.5 text-muted-foreground" />
            )}
          </button>
          {isUrl && (
            <button
              className="p-1 rounded-md opacity-0 group-hover:opacity-50 hover:!opacity-100 transition-opacity"
              onClick={(e) => {
                e.stopPropagation();
                if (link) openUrl(link.url).catch(console.error);
              }}
              title="打开链接"
            >
              <LinkIcon className="w-3.5 h-3.5 text-blue-500 hover:text-blue-600" />
            </button>
          )}
          {item.item_type !== "image" && item.item_type !== "file" && (
            <button
              className="p-1 rounded-md opacity-0 group-hover:opacity-50 hover:opacity-100 transition-opacity"
              onClick={handleEditText}
              title="编辑"
            >
              <SquarePen className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
            </button>
          )}
          {item.item_type === "image" && (
            <button
              className="p-1 rounded-md opacity-0 group-hover:opacity-50 hover:opacity-100 transition-opacity"
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
                ref={saveBtnRef}
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
                <SaveImagePopover id={item.id} triggerRef={saveBtnRef} onClose={() => setShowSavePopover(false)} />
              )}
            </div>
          )}
          {item.item_type === "file" && (
            <button
              className="p-1 rounded-md opacity-0 group-hover:opacity-50 hover:opacity-100 transition-opacity"
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
      </div>
      </div>

      {!isLast && <div className="absolute bottom-0 left-2.5 right-2.5 h-px bg-border/50" />}
    </div>
  );
}

// memo：selectedId 变化时父组件重绘，仅原选中行与新选中行（isSelected 变）需更新，
// 其余行 props 浅比较不变即跳过（重绘数 50 → 2）。需配合 index.tsx 稳定的
// onSelect（useCallback）/ onChanged（refresh useCallback）句柄，否则 inline 箭头
// 令每行 prop 引用每帧变化、memo 形同虚设。
export default memo(ClipboardItemRow);

/// ref_data 是 JSON 路径数组，取每个路径最后 2 段显示。
function formatFilePaths(refData?: string): string {
  if (!refData) return "文件";
  try {
    const paths: string[] = JSON.parse(refData);
    const display = paths.slice(0, 3).map((raw) => {
      // Linux X11/Wayland 存 file:// URI + 百分号编码；macOS/Windows 存已解码的普通路径。
      // 仅 file:// 开头才 decodeURIComponent，避免对含字面 %XX 的普通路径误伤。
      const stripped = raw.replace(/^file:\/\//, "");
      const path = raw.startsWith("file://") ? decodeURIComponent(stripped) : stripped;
      // 同时按 / 与 \ 切割：Linux/macOS 用正斜杠，Windows（clipboard-rs FileList）
      // 存反斜杠普通路径 C:\\…，仅 split("/") 会无法截断而整段溢出。
      const parts = path.split(/[\\/]/).filter(Boolean);
      const tail = parts.slice(-2).join("/");
      return "…/" + tail;
    });
    if (paths.length > 3) {
      return display.join("  ") + `  +${paths.length - 3}`;
    }
    return display.join("  ") + (paths.length > 1 ? ` (${paths.length})` : "");
  } catch {
    return "文件";
  }
}
