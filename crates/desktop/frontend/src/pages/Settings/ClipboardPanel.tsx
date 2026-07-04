import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@/lib/tauri";
import { cn } from "@/lib/utils";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import type { ClipboardItem } from "@/types/clipboard";
import {
  Star, Mic, Type, Image as ImageIcon, FileText,
  LayoutGrid, Search, Trash2, Download, FolderOpen,
  ScanText, Loader2, Check,
} from "lucide-react";
import SaveImagePopover from "../Clipboard/SaveImagePopover";

const PAGE_SIZE = 50;
const TABS = [
  { value: "all", icon: LayoutGrid, label: "全部", svg: undefined },
  { value: "asr", icon: null, label: "语音", svg: "voice" },
  { value: "ocr", icon: ScanText, label: "OCR", svg: undefined },
  { value: "text", icon: null, label: "文本", svg: "text" },
  { value: "image", icon: null, label: "图片", svg: "images" },
  { value: "file", icon: null, label: "文件", svg: "files" },
  { value: "favorite", icon: Star, label: "收藏", svg: "favorite" },
  { value: "unfavorite", icon: Star, label: "非收藏", svg: "un-favorite" },
] as const;

/** 字节数 → 人类可读大小：<1M 显示 K（整数）、≥1M 显示 M（1 位小数）。 */
function formatSize(bytes: number): string {
  if (bytes <= 0) return "";
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))}K`;
  return `${(bytes / 1024 / 1024).toFixed(1)}M`;
}

export default function ClipboardPanel({ showToast }: { showToast: (msg: string) => void }) {
  const [items, setItems] = useState<ClipboardItem[]>([]);
  const [total, setTotal] = useState(0);
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [loading, setLoading] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [page, setPage] = useState(1);
  const [noMore, setNoMore] = useState(false);

  const debouncedSearch = useDebouncedValue(search, 300);

  const fetchData = useCallback(async (resetPage?: boolean) => {
    setLoading(true);
    const targetPage = resetPage ? 1 : page;
    try {
      const result = await invoke<ClipboardItem[]>("query_clipboard_history", {
        filter, search: debouncedSearch || null, page: targetPage, size: PAGE_SIZE,
      });
      if (resetPage) {
        setItems(result);
        setPage(1);
      } else {
        setItems((prev) => [...prev, ...result]);
      }
      setNoMore(result.length < PAGE_SIZE);
      if (!resetPage && result.length > 0) {
        setPage(targetPage + 1);
      }
      const count = await invoke<number>("clipboard_stats", { filter, search: debouncedSearch || null });
      setTotal(count);
    } catch (e) {
      showToast("加载失败：" + e);
    }
    setLoading(false);
  }, [filter, debouncedSearch, showToast, page]);

  useEffect(() => { fetchData(true); }, [filter, debouncedSearch]);
  useTauriEvent("clipboard://changed", () => fetchData(true));

  const toggleSelect = (id: number) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
    setConfirmDelete(false);
  };

  const toggleSelectAll = (checked: boolean) => {
    // 全选排除收藏：全选通常为批量删除，避免误把收藏条目带入选中
    setSelectedIds(checked ? new Set(items.filter((i) => !i.is_favorite).map((i) => i.id)) : new Set());
    setConfirmDelete(false);
  };

  const handleBatchDelete = async () => {
    if (selectedIds.size === 0) return;
    if (!confirmDelete) {
      setConfirmDelete(true);
      setTimeout(() => setConfirmDelete(false), 3000);
      return;
    }
    try {
      await invoke("delete_clipboard_items", { ids: Array.from(selectedIds) });
      showToast(`已删除 ${selectedIds.size} 条`);
      setSelectedIds(new Set());
      setConfirmDelete(false);
      fetchData(true);
    } catch (e) {
      showToast("删除失败：" + e);
    }
  };

  // 全选状态：所有「非收藏」条目都被选中才算（收藏不参与全选，但仍可手动单个选）
  const selectableItems = items.filter((i) => !i.is_favorite);
  const allChecked = selectableItems.length > 0 && selectableItems.every((i) => selectedIds.has(i.id));
  const hasSelection = selectedIds.size > 0;

  return (
    <div className="flex flex-col h-full">
      {/* ── 筛选区：过滤标签 + 搜索（主操作，置顶）── */}
      <div className="space-y-2.5 pb-3 border-b border-border">
        <div className="flex items-center gap-1">
          {TABS.map(({ value: v, icon: Icon, label, svg }) => (
            <button
              key={v}
              title={label}
              className={cn(
                "flex items-center justify-center gap-1 px-2.5 py-1.5 rounded-md transition-all duration-150",
                filter === v
                  ? "bg-stone-800 text-white"
                  : "text-stone-500 hover:text-stone-800 hover:bg-stone-100",
              )}
              onClick={() => setFilter(v)}
            >
              {svg ? (
                <img src={`icons/${svg}.svg`} alt={label} className="w-3.5 h-3.5" style={{ filter: filter === v ? "brightness(0) invert(1)" : "none" }} />
              ) : (
                <Icon className="w-3.5 h-3.5" />
              )}
              {v === "all" && <span className="text-xs font-medium">{label}</span>}
            </button>
          ))}
          <div className="flex-1" />
          <div className="flex items-center gap-2 px-2.5 py-1.5 bg-stone-50 rounded-md border border-stone-200">
            <Search className="w-3.5 h-3.5 text-stone-400" />
            <input
              type="text"
              placeholder="搜索..."
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              className="w-40 bg-transparent text-xs outline-none placeholder:text-stone-400"
            />
          </div>
        </div>
      </div>

      {/* ── 列表 ── */}
      <div className="flex-1 overflow-y-auto thin-scrollbar -mx-1 px-1">
        {loading ? (
          <div className="text-center py-8 text-stone-400 text-sm">加载中...</div>
        ) : items.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-16 gap-1 text-stone-400">
            <span className="text-sm">暂无记录</span>
          </div>
        ) : (
          <div className="flex flex-col">
            {/* 列表 header：全选（sticky 固定不随滚动） */}
            <div className="sticky top-0 z-10 flex items-center gap-2 px-3 py-1.5 border-b border-stone-100 bg-stone-50 group/header">
              <label className="flex items-center gap-1.5 cursor-pointer">
                <input
                  type="checkbox"
                  className="w-3.5 h-3.5"
                  checked={allChecked}
                  onChange={(e) => toggleSelectAll(e.target.checked)}
                />
                <span className="text-[10px] text-stone-400 group-hover/header:text-stone-600 transition-colors">
                  {hasSelection ? `已选 ${selectedIds.size} 项` : "全选"}
                </span>
              </label>
            </div>
            {items.map((item) => (
              <ClipboardRow
                key={item.id}
                item={item}
                isSelected={selectedIds.has(item.id)}
                onToggleSelect={() => toggleSelect(item.id)}
                onChanged={() => fetchData(true)}
                showToast={showToast}
              />
            ))}
            {loading && items.length > 0 && (
              <div className="text-center py-3 text-stone-400 text-xs">加载中...</div>
            )}
            {!loading && !noMore && items.length > 0 && (
              <button
                className="w-full py-3 text-xs text-stone-500 hover:text-stone-800 transition-colors"
                onClick={() => fetchData()}
              >
                加载更多
              </button>
            )}
            {!loading && noMore && items.length > 0 && (
              <div className="text-center py-3 text-stone-300 text-[10px]">— 没有更多了 —</div>
            )}
          </div>
        )}
      </div>

      {/* ── 底部：状态 + 批量操作（选中时浮现） ── */}
      <div className="flex items-center justify-between py-2 border-t border-border">
        <span className="text-[10px] text-stone-400">
          共 {total} 条{filter !== "all" ? `（${TABS.find(t => t.value === filter)?.label}）` : ""}
        </span>
        {hasSelection ? (
          <button
            className={cn(
              "flex items-center gap-1 px-2.5 py-1 rounded-md text-xs font-medium transition-all duration-150",
              confirmDelete
                ? "bg-red-600 text-white"
                : "border border-red-400 text-red-500 hover:bg-red-50",
            )}
            onClick={handleBatchDelete}
          >
            <Trash2 className="w-3 h-3" />
            {confirmDelete ? `确认删除 ${selectedIds.size} 项` : `删除选中`}
          </button>
        ) : (
          <span className="text-[10px] text-stone-300">显示 {items.length} 条</span>
        )}
      </div>
    </div>
  );
}

/// 单条记录行——含 checkbox + 内容 + hover 操作（复制/收藏/保存图片/打开文件/删除）
function ClipboardRow({
  item,
  isSelected,
  onToggleSelect,
  onChanged,
  showToast,
}: {
  item: ClipboardItem;
  isSelected: boolean;
  onToggleSelect: () => void;
  onChanged: () => void;
  showToast: (msg: string) => void;
}) {
  const [deletePending, setDeletePending] = useState(false);
  const [showSavePopover, setShowSavePopover] = useState(false);
  const [ocrLoading, setOcrLoading] = useState(false);
  const [ocrDone, setOcrDone] = useState(false);
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
    // 列表项切换（翻页/滚动复用实例）时先清旧缩略图，避免新图 base64 经 IPC 传回前
    // 短暂显示上一条（幽灵闪烁）；cancelled 防快速翻页/滚动时旧请求晚到覆盖新图。
    setThumbSrc(null);
    let cancelled = false;
    invoke<string>("get_image_thumb", { id: item.id })
      .then((dataUrl) => { if (!cancelled) setThumbSrc(dataUrl); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [item.id, item.item_type]);

  // 单击左侧类型图标 → 复制（触效：放大回弹 + 闪绿 + 「已复制」气泡 1.5s；失败走 page toast）。
  const handleCopy = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await invoke("copy_clipboard_item", { id: item.id });
      setCopied(true);
      if (copyTimer.current) clearTimeout(copyTimer.current);
      copyTimer.current = setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      showToast("复制失败：" + e);
    }
  };

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
      invoke("delete_clipboard_item", { id: item.id })
        .then(() => { onChanged(); showToast("已删除"); })
        .catch((e) => showToast("删除失败：" + e));
    }
  };

  const handleOpenFile = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await invoke("open_file_item", { id: item.id });
    } catch (e) {
      showToast("打开失败：" + e);
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
      } else if (msg.includes("还未完成")) {
        // 全局互斥：他处正在 OCR → 直接显示该提示（不加"OCR 失败"前缀）
        showToast(msg);
      } else {
        showToast("OCR 失败：" + e);
      }
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
        "group relative flex items-start gap-2.5 px-3 py-2.5 border-b border-stone-100/60 transition-colors cursor-pointer",
        isSelected ? "bg-stone-100" : "hover:bg-stone-50",
        deletePending && "bg-red-50",
      )}
      onClick={onToggleSelect}
    >
      {isVoice && (
        <div className="absolute left-0 top-2 bottom-2 w-[2px] rounded-r bg-amber-600/40" />
      )}
      <input
        type="checkbox"
        className="w-3.5 h-3.5 mt-0.5 flex-shrink-0"
        checked={isSelected}
        onChange={(e) => { e.stopPropagation(); onToggleSelect(); }}
        onClick={(e) => e.stopPropagation()}
      />
      <button
        type="button"
        onClick={handleCopy}
        onDoubleClick={(e) => e.stopPropagation()}
        title="单击复制"
        className="relative flex-shrink-0 mt-1 -m-0.5 cursor-pointer rounded p-0.5 transition-transform duration-150 hover:scale-110 active:scale-90"
      >
        <Icon className={cn(
          "w-3.5 h-3.5 transition-all duration-150",
          isVoice ? "text-amber-600" : "text-stone-400 group-hover:text-stone-600",
          copied && "scale-125 text-emerald-500",
        )} />
        {copied && (
          <span className="pointer-events-none absolute left-full top-1/2 z-10 ml-1 -translate-y-1/2 whitespace-nowrap rounded bg-emerald-500 px-1.5 py-0.5 text-[10px] font-medium text-white shadow">
            已复制
          </span>
        )}
      </button>
      <div className="flex-1 min-w-0">
        {item.item_type === "image" && item.image_meta ? (
          <div className="flex items-center gap-1.5">
            {thumbSrc && (
              <img src={thumbSrc} className="w-10 h-10 rounded object-cover flex-shrink-0" alt="" />
            )}
            <span className="text-xs text-stone-500">
              {item.image_meta.width}×{item.image_meta.height}
            </span>
          </div>
        ) : item.item_type === "file" ? (
          <div className="text-xs text-stone-500 truncate">
            {formatFilePaths(item.content, item.file_meta?.file_count)}
          </div>
        ) : (
          <p className="text-xs leading-relaxed text-stone-800 break-words line-clamp-2">{[...item.content].length > 200 ? [...item.content].slice(0, 200).join("") + "……" : item.content}</p>
        )}
        {isVoice && item.asr_meta && (
          <span className="inline-block mt-0.5 text-[10px] text-amber-700/60 font-medium">
            {item.asr_meta.engine}
          </span>
        )}
        <span className="ml-2 text-[10px] text-stone-300">{item.created_at}</span>
        {item.item_type === "image" && item.image_meta && item.image_meta.size > 0 && (
          <span className="ml-1 text-[11px] text-sky-700 font-medium tabular-nums">{formatSize(item.image_meta.size)}</span>
        )}
      </div>

      {/* 右侧操作栏：保存图片/OCR/打开文件/删除 + 收藏置末（已收藏常显高亮，置首会使其后 hover 按钮被遮、视觉怪） */}
      <div className="flex-shrink-0 flex items-center gap-0.5">
        {item.item_type === "image" && (
          <div className="relative">
            <button
              className={cn(
                "p-1 rounded transition-opacity hover:scale-110",
                showSavePopover ? "opacity-100" : "opacity-0 group-hover:opacity-50 hover:!opacity-100",
              )}
              onClick={handleSaveImage}
              title="保存为文件"
            >
              <Download className={cn(
                "w-3.5 h-3.5 text-stone-500",
                showSavePopover && "text-stone-800",
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
              "p-1 rounded transition-opacity",
              ocrLoading || ocrDone
                ? "opacity-100"
                : "opacity-0 group-hover:opacity-50 hover:!opacity-100",
            )}
            onClick={handleOcr}
            disabled={ocrLoading}
            title="OCR 识别"
          >
            {ocrLoading ? (
              <Loader2 className="w-3.5 h-3.5 text-stone-500 animate-spin" />
            ) : ocrDone ? (
              <Check className="w-3.5 h-3.5 text-emerald-600" />
            ) : (
              <ScanText className="w-3.5 h-3.5 text-stone-500 hover:text-stone-800" />
            )}
          </button>
        )}
        {item.item_type === "file" && (
          <button
            className="p-1 rounded opacity-0 group-hover:opacity-50 hover:!opacity-100 transition-opacity"
            onClick={handleOpenFile}
            title="打开文件"
          >
            <FolderOpen className="w-3.5 h-3.5 text-stone-500 hover:text-stone-800" />
          </button>
        )}
        <button
          className={cn(
            "p-1 rounded transition-all",
            deletePending
              ? "opacity-100 bg-red-100"
              : "opacity-0 group-hover:opacity-50 hover:!opacity-100 transition-opacity",
          )}
          onClick={handleDeleteClick}
          title={deletePending ? "再次点击确认删除" : "删除"}
        >
          <Trash2 className={cn(
            "w-3.5 h-3.5 transition-colors",
            deletePending ? "text-red-600" : "text-stone-500 hover:text-red-500",
          )} />
        </button>
        <button
          className={cn(
            "p-1 rounded transition-opacity hover:scale-110",
            item.is_favorite ? "opacity-100" : "opacity-0 group-hover:opacity-50 hover:!opacity-100",
          )}
          onClick={handleFavorite}
        >
          <Star className={cn(
            "w-3.5 h-3.5",
            item.is_favorite ? "fill-amber-400 text-amber-400" : "text-stone-500",
          )} />
        </button>
      </div>
    </div>
  );
}

function useDebouncedValue<T>(value: T, delay: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delay);
    return () => clearTimeout(timer);
  }, [value, delay]);
  return debounced;
}

function formatFilePaths(content: string, count?: number): string {
  try {
    const paths: string[] = JSON.parse(content);
    const display = paths.slice(0, 3).map((raw) => {
      // Linux X11/Wayland 存 file:// URI + 百分号编码；macOS/Windows 存已解码的普通路径。
      // 仅 file:// 开头才 decodeURIComponent，避免对含字面 %XX 的普通路径误伤。
      const stripped = raw.replace(/^file:\/\//, "");
      const path = raw.startsWith("file://") ? decodeURIComponent(stripped) : stripped;
      const parts = path.split("/").filter(Boolean);
      return "…/" + parts.slice(-2).join("/");
    });
    if (paths.length > 3) return display.join("  ") + `  +${paths.length - 3}`;
    return display.join("  ") + (count ? ` (${count})` : "");
  } catch {
    return count ? `${count} 个文件` : "文件";
  }
}
