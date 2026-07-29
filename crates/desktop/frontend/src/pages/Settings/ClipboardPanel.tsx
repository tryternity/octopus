import { useState, useEffect, useCallback, useRef, useMemo } from "react";
import { invoke } from "@/lib/tauri";
import { openUrl } from "@tauri-apps/plugin-opener";
import { cn } from "@/lib/utils";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import type { ClipboardItem } from "@/types/clipboard";
import { metaParts, typeAccent, imageMeta, fileMeta, detectUrl } from "@/types/clipboard";
import {
  Star, Mic, Type, Image as ImageIcon, FileText,
  LayoutGrid, Search, Trash2, Download, FolderOpen,
  ScanText, Loader2, Link as LinkIcon, SquarePen, ChevronDown, Copy, Check, AlertCircle,
} from "lucide-react";
import SaveImagePopover from "@/components/SaveImagePopover";
import { openCompactEditorTab } from "@/lib/compactEditor";
import { useT, t as ti18n } from "@/lib/i18n";
import { Button } from "@/components/ui/button";

const PAGE_SIZE = 50;

// 过滤分组：主分类 | 内容类型 | 状态。视觉上用分隔线区分层级，而非平铺 8 个等权按钮。
const FILTER_GROUPS: { labelKey?: string; items: { value: string; icon: any; labelKey: string; svg?: string }[] }[] = [
  { items: [{ value: "all", icon: LayoutGrid, labelKey: "settings.clipboardPanel.filterAll" }] },
  {
    labelKey: "settings.clipboardPanel.groupType",
    items: [
      { value: "asr", icon: null, labelKey: "settings.clipboardPanel.filterVoice", svg: "voice" },
      { value: "ocr", icon: ScanText, labelKey: "OCR" },
      { value: "text", icon: null, labelKey: "settings.clipboardPanel.filterText", svg: "text" },
      { value: "image", icon: null, labelKey: "settings.clipboardPanel.filterImage", svg: "images" },
      { value: "file", icon: null, labelKey: "settings.clipboardPanel.filterFile", svg: "files" },
    ],
  },
  { labelKey: "settings.clipboardPanel.groupStatus", items: [
    { value: "favorite", icon: Star, labelKey: "settings.clipboardPanel.filterFavorite", svg: "favorite" },
    { value: "unfavorite", icon: Star, labelKey: "settings.clipboardPanel.filterNonFavorite", svg: "un-favorite" },
  ] },
];

export default function ClipboardPanel({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
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
      showToast(t("settings.clipboardPanel.loadFailed") + e);
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
    setSelectedIds(checked ? new Set(items.filter((i) => !i.isFavorite).map((i) => i.id)) : new Set());
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
      showToast(t("settings.clipboardPanel.deletedN", { n: selectedIds.size }));
      setSelectedIds(new Set());
      setConfirmDelete(false);
      fetchData(true);
    } catch (e) {
      showToast(t("settings.clipboardPanel.deleteFailed") + e);
    }
  };

  const selectableItems = items.filter((i) => !i.isFavorite);
  const allChecked = selectableItems.length > 0 && selectableItems.every((i) => selectedIds.has(i.id));
  const hasSelection = selectedIds.size > 0;
  const activeFilterLabel = (() => {
    const item = FILTER_GROUPS.flatMap(g => g.items).find(it => it.value === filter);
    if (!item) return undefined;
    return item.labelKey === "OCR" ? "OCR" : t(item.labelKey);
  })();

  return (
    <div className="flex flex-col h-full">
      {/* ── 筛选区：分组过滤 + 搜索 ── */}
      <div className="space-y-2 pb-3 border-b border-border">
        <div className="flex items-center gap-1 flex-wrap">
          {FILTER_GROUPS.map((group, gi) => (
            <div key={gi} className="flex items-center gap-1">
              {gi > 0 && <div className="w-px h-4 bg-border mx-1" />}
              {group.items.map(({ value: v, icon: Icon, labelKey, svg }) => {
                const label = labelKey === "OCR" ? "OCR" : t(labelKey);
                return (
                <button
                  key={v}
                  title={label}
                  className={cn(
                    "flex items-center justify-center gap-1 px-2.5 py-1 rounded-md text-xs transition-all duration-150",
                    filter === v
                      ? "bg-primary text-primary-foreground font-medium shadow-sm"
                      : "text-muted-foreground hover:text-foreground hover:bg-accent",
                  )}
                  onClick={() => setFilter(v)}
                >
                  {svg ? (
                    <img src={`icons/${svg}.svg`} alt={label} className="w-3.5 h-3.5" style={{ filter: filter === v ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
                  ) : (
                    <Icon className="w-3.5 h-3.5" />
                  )}
                  <span>{label}</span>
                </button>
                );
              })}
            </div>
          ))}
          <div className="flex-1" />
          <div className="flex items-center gap-2 px-2.5 py-1 bg-muted rounded-md border border-border focus-within:border-voice/40 transition-colors">
            <Search className="w-3.5 h-3.5 text-muted-foreground" />
            <input
              type="text"
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              placeholder={t("settings.clipboardPanel.searchPlaceholder")}
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              className="w-44 bg-transparent text-xs outline-none placeholder:text-muted-foreground"
            />
            {search && (
              <button onClick={() => setSearch("")} className="text-muted-foreground hover:text-foreground text-xs">×</button>
            )}
          </div>
        </div>
      </div>

      {/* ── 列表 ── */}
      <div className="flex-1 overflow-y-auto thin-scrollbar -mx-1 px-1">
        {loading && items.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-20 gap-2 text-muted-foreground">
            <Loader2 className="w-5 h-5 animate-spin text-muted-foreground/50" />
            <span className="text-xs">{t("settings.clipboardPanel.loading")}</span>
          </div>
        ) : items.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-20 gap-3 text-muted-foreground">
            <div className="w-12 h-12 rounded-full bg-muted flex items-center justify-center">
              <ClipboardEmptyIcon />
            </div>
            <div className="text-center">
              <p className="text-sm text-muted-foreground font-medium">
                {t("settings.clipboardPanel.empty")}
              </p>
              <p className="text-xs text-muted-foreground/70 mt-1">
                {search ? t("settings.clipboardPanel.emptySearch", { search }) : t("settings.clipboardPanel.emptyHint")}
              </p>
            </div>
          </div>
        ) : (
          <div className="flex flex-col">
            {/* 列表 header：全选（sticky） */}
            <div className="sticky top-0 z-10 flex items-center gap-2 px-3 py-1.5 border-b border-border bg-background/95 backdrop-blur-sm group/header">
              <label className="flex items-center gap-1.5 cursor-pointer">
                <input
                  type="checkbox"
                  className="w-3.5 h-3.5 accent-primary"
                  checked={allChecked}
                  onChange={(e) => toggleSelectAll(e.target.checked)}
                />
                <span className="text-[10px] text-muted-foreground group-hover/header:text-foreground transition-colors">
                  {hasSelection ? t("settings.clipboardPanel.selectedN", { n: selectedIds.size }) : t("settings.clipboardPanel.selectAll")}
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
              <div className="flex items-center justify-center py-4 gap-2 text-muted-foreground">
                <Loader2 className="w-3.5 h-3.5 animate-spin" />
                <span className="text-xs">{t("settings.clipboardPanel.loading")}</span>
              </div>
            )}
            {!loading && !noMore && items.length > 0 && (
              <Button
                variant="outline"
                size="sm"
                className="mx-auto my-3"
                onClick={() => fetchData()}
              >
                <ChevronDown />
                {t("settings.clipboardPanel.loadMore")}
              </Button>
            )}
            {!loading && noMore && items.length > 0 && (
              <div className="text-center py-4 text-muted-foreground/50 text-[10px] tracking-wider">{t("settings.clipboardPanel.allLoaded")}</div>
            )}
          </div>
        )}
      </div>

      {/* ── 底部：状态 + 批量操作 ── */}
      <div className="flex items-center justify-between py-2 border-t border-border">
        <span className="text-[10px] text-muted-foreground tabular-nums">
          {t("settings.clipboardPanel.totalN", { n: total })}{filter !== "all" && activeFilterLabel ? ` · ${activeFilterLabel}` : ""}
        </span>
        {hasSelection ? (
          <Button
            variant={confirmDelete ? "destructive" : "destructive-ghost"}
            size="sm"
            onClick={handleBatchDelete}
          >
            <Trash2 />
            {confirmDelete ? t("settings.clipboardPanel.confirmDeleteN", { n: selectedIds.size }) : t("settings.clipboardPanel.deleteSelected")}
          </Button>
        ) : (
          <span className="text-[10px] text-muted-foreground/50 tabular-nums">{t("settings.clipboardPanel.showingN", { n: items.length })}</span>
        )}
      </div>
    </div>
  );
}

/// 单条记录行——类型感知布局：voice 为签名主角，其余克制处理。
/// 左缘 3px 类型色条让整列形成色彩编码档案；hover 显操作按钮。
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
  const t = useT();
  const [deletePending, setDeletePending] = useState(false);
  const [showSavePopover, setShowSavePopover] = useState(false);
  const [thumbSrc, setThumbSrc] = useState<string | null>(null);
  const [fileMissing, setFileMissing] = useState(false);
  const deleteTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [copied, setCopied] = useState(false);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const saveBtnRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    return () => {
      if (deleteTimer.current) clearTimeout(deleteTimer.current);
      if (copyTimer.current) clearTimeout(copyTimer.current);
    };
  }, []);

  useEffect(() => {
    if (item.itemType !== "image") return;
    setThumbSrc(null);
    setFileMissing(false);
    let cancelled = false;
    invoke<string>("get_image_thumb", { id: item.id })
      .then((dataUrl) => { if (!cancelled) setThumbSrc(dataUrl); })
      .catch(() => {});
    invoke<boolean>("check_image_file_exists", { id: item.id })
      .then((exists) => { if (!cancelled) setFileMissing(!exists); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [item.id, item.itemType]);

  const handleCopy = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await invoke("copy_clipboard_item", { id: item.id });
      setCopied(true);
      if (copyTimer.current) clearTimeout(copyTimer.current);
      copyTimer.current = setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      setFileMissing(true);
      showToast(t("settings.clipboardPanel.imageLost"));
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
        .then(() => { onChanged(); showToast(t("settings.clipboardPanel.deleted")); })
        .catch((e) => showToast(t("settings.clipboardPanel.deleteFailed") + e));
    }
  };

  const handleOpenFile = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await invoke("open_file_item", { id: item.id });
    } catch (e) {
      showToast(t("settings.clipboardPanel.openFailed") + e);
    }
  };

  const handleEditOrPreview = (e: React.MouseEvent) => {
    e.stopPropagation();
    openCompactEditorTab(item.id).catch((e) => showToast(t("settings.clipboardPanel.openFailed") + e));
  };

  const handleSaveImage = (e: React.MouseEvent) => {
    e.stopPropagation();
    setShowSavePopover((v) => !v);
  };

  const Icon = item.itemType === "voice" ? Mic
    : item.itemType === "ocr" ? ScanText
    : item.itemType === "image" ? ImageIcon
    : item.itemType === "file" ? FileText
    : Type;
  const isVoice = item.itemType === "voice";
  // 第一行行尾元数据：text/ocr→N字；voice→N字·Xs；image→W×H·size；file→类型/N个
  const row1Meta =
    item.itemType === "image" ? imageMeta(item)
    : item.itemType === "file" ? fileMeta(item)
    : metaParts(item);
  const accent = typeAccent[item.itemType];
  const link = item.itemType === "text" ? detectUrl(item.content) : null;
  const isUrl = !!link?.isLink;

  // 类型左缘色：voice 用 amber 渐变（声波暗示），其余用对应类型色低饱和
  const edgeClass = isVoice
    ? "bg-gradient-to-b from-amber-400 to-amber-600"
    : item.itemType === "ocr" ? "bg-teal-500/50"
    : item.itemType === "image" ? "bg-indigo-400/50"
    : item.itemType === "file" ? "bg-emerald-500/50"
    : "bg-muted-foreground/30";

  // 选中态用 voice 左缘覆盖
  const showSelectEdge = isSelected && !deletePending;

  // 预览文本：voice 行略大字号（签名主角），其余统一
  const previewSize = isVoice ? "text-[13px]" : "text-xs";

  const preview = useMemo(() => {
    if (item.content.length <= 200) return item.content;
    const chars = [...item.content];
    return chars.length > 200 ? chars.slice(0, 200).join("") + "……" : item.content;
  }, [item.content]);

  return (
    <div
      className={cn(
        "group relative flex items-center gap-2.5 pl-4 pr-3 py-2 border-b border-border/50 transition-colors cursor-pointer",
        deletePending ? "bg-destructive/10"
          : isSelected ? "bg-accent"
          : "hover:bg-muted",
      )}
      onClick={onToggleSelect}
    >
      {/* 左缘类型色条：选中时变 voice，删除确认时变 destructive */}
      <div className={cn(
        "absolute left-0 top-1.5 bottom-1.5 w-[3px] rounded-r-full transition-colors",
        deletePending ? "bg-destructive" : showSelectEdge ? "bg-voice" : edgeClass,
      )} />

      <input
        type="checkbox"
        className="w-3.5 h-3.5 flex-shrink-0 accent-primary"
        checked={isSelected}
        onChange={(e) => { e.stopPropagation(); onToggleSelect(); }}
        onClick={(e) => e.stopPropagation()}
      />
      {/* 类型图标(单击复制)：跨两行垂直居中、放大一档(w-3.5→w-4)，作条目「头像」 */}
      <button
        type="button"
        onClick={handleCopy}
        onDoubleClick={(e) => e.stopPropagation()}
        title={t("settings.clipboardPanel.clickToCopy")}
        className="relative flex-shrink-0 cursor-pointer rounded p-0.5 transition-transform duration-150 hover:scale-110 active:scale-90"
      >
        <Icon className={cn(
          "w-4 h-4 transition-all duration-150",
          accent,
          copied && "scale-125 text-success",
        )} />
        {copied && (
          <span className="pointer-events-none absolute left-full top-1/2 z-10 ml-1 -translate-y-1/2 whitespace-nowrap rounded bg-success px-1.5 py-0.5 text-[10px] font-medium text-success-foreground shadow">
            {t("settings.clipboardPanel.copied")}
          </span>
        )}
      </button>

      {/* 右侧两行内容栏：第一行 内容+元数据；第二行 时间戳+操作。
          原右侧 hover 操作 rail 占位留白，下移到第二行后内容栏 flex-1 铺满宽度。 */}
      <div className="flex-1 min-w-0">
        {/* 第一行：内容 / 缩略图 / 文件路径 + 行尾元数据 */}
        <div className="flex items-center gap-2">
          <div className="flex-1 min-w-0">
            {item.itemType === "image" ? (
              fileMissing ? (
                <div className="w-10 h-10 rounded bg-destructive/10 flex items-center justify-center flex-shrink-0 ring-1 ring-destructive/20" title={t("settings.clipboardPanel.imageLost")}>
                  <AlertCircle className="w-4 h-4 text-destructive/60" />
                </div>
              ) : thumbSrc && (
                <img src={thumbSrc} className="w-10 h-10 rounded object-cover flex-shrink-0 ring-1 ring-black/5" alt="" />
              )
            ) : item.itemType === "file" ? (
              <span className="block truncate text-xs text-muted-foreground">{formatFilePaths(item.refData)}</span>
            ) : (
              <p className={cn(
                "leading-snug text-foreground break-words line-clamp-1",
                previewSize,
                isVoice && "font-medium",
              )}>{preview}</p>
            )}
          </div>
          {row1Meta && (
            <span className={cn("flex-shrink-0 text-[10px] font-medium tabular-nums", accent)}>{row1Meta}</span>
          )}
        </div>

        {/* 第二行：时间戳 + 操作（复制居首，最常用） */}
        <div className="mt-1 flex items-center justify-between" onDoubleClick={(e) => e.stopPropagation()}>
          <span className="text-[10px] text-muted-foreground tabular-nums">{item.createdAt}</span>
          {/* 操作：复制/链接/编辑/删除/收藏 */}
          <div className="flex flex-shrink-0 items-center gap-0.5">
            <button
              className={cn(
                "p-1 rounded transition-opacity hover:scale-110",
                copied ? "opacity-100" : "opacity-0 group-hover:opacity-50 hover:!opacity-100",
              )}
              onClick={handleCopy}
              title={t("settings.clipboardPanel.copy")}
            >
                {copied ? (
                  <Check className="w-3.5 h-3.5 text-success" />
                ) : (
                  <Copy className="w-3.5 h-3.5 text-muted-foreground" />
                )}
              </button>
              {isUrl && (
                <button
                  className="p-1 rounded opacity-0 group-hover:opacity-50 hover:!opacity-100 transition-opacity"
                  onClick={(e) => { e.stopPropagation(); if (link) openUrl(link.url).catch(console.error); }}
                  title={t("settings.clipboardPanel.openLink")}
                >
                  <LinkIcon className="w-3.5 h-3.5 text-info" />
                </button>
              )}
              {item.itemType !== "image" && item.itemType !== "file" && (
                <button
                  className="p-1 rounded opacity-0 group-hover:opacity-50 hover:!opacity-100 transition-opacity"
                  onClick={handleEditOrPreview}
                  title={t("settings.clipboardPanel.edit")}
                >
                  <SquarePen className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
                </button>
              )}
              {item.itemType === "image" && (
                <button
                  className="p-1 rounded opacity-0 group-hover:opacity-50 hover:!opacity-100 transition-opacity"
                  onClick={handleEditOrPreview}
                  title={t("settings.clipboardPanel.preview")}
                >
                  <img src="icons/eye-edit.svg" alt={t("settings.clipboardPanel.preview")} className="w-3.5 h-3.5" style={{ filter: "var(--icon-filter)" }} />
                </button>
              )}
              {item.itemType === "image" && (
                <div className="relative">
                  <button
                    ref={saveBtnRef}
                    className={cn(
                      "p-1 rounded transition-opacity hover:scale-110",
                      showSavePopover ? "opacity-100" : "opacity-0 group-hover:opacity-50 hover:!opacity-100",
                    )}
                    onClick={handleSaveImage}
                    title={t("settings.clipboardPanel.saveToFile")}
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
              {item.itemType === "file" && (
                <button
                  className="p-1 rounded opacity-0 group-hover:opacity-50 hover:!opacity-100 transition-opacity"
                  onClick={handleOpenFile}
                  title={t("settings.clipboardPanel.openFile")}
                >
                  <FolderOpen className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
                </button>
              )}
              <button
                className={cn(
                  "p-1 rounded transition-all",
                  deletePending
                    ? "opacity-100 bg-destructive/15"
                    : "opacity-0 group-hover:opacity-50 hover:!opacity-100 transition-opacity",
                )}
                onClick={handleDeleteClick}
                title={deletePending ? t("settings.clipboardPanel.deleteConfirm") : t("settings.clipboardPanel.delete")}
              >
                <Trash2 className={cn(
                  "w-3.5 h-3.5 transition-colors",
                  deletePending ? "text-destructive" : "text-muted-foreground hover:text-destructive",
                )} />
              </button>
              <button
                className={cn(
                  "p-1 rounded transition-opacity hover:scale-110",
                  item.isFavorite ? "opacity-100" : "opacity-0 group-hover:opacity-50 hover:!opacity-100",
                )}
                onClick={handleFavorite}
              >
                <Star className={cn(
                  "w-3.5 h-3.5",
                  item.isFavorite ? "fill-amber-400 text-amber-400" : "text-muted-foreground",
                )} />
              </button>
            </div>
        </div>
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

function formatFilePaths(refData?: string): string {
  if (!refData) return ti18n("settings.clipboardPanel.fileFallback");
  try {
    const paths: string[] = JSON.parse(refData);
    const display = paths.slice(0, 3).map((raw) => {
      const stripped = raw.replace(/^file:\/\//, "");
      const path = raw.startsWith("file://") ? decodeURIComponent(stripped) : stripped;
      const parts = path.split(/[\\/]/).filter(Boolean);
      return "…/" + parts.slice(-2).join("/");
    });
    if (paths.length > 3) return display.join("  ") + `  +${paths.length - 3}`;
    return display.join("  ") + (paths.length > 1 ? ` (${paths.length})` : "");
  } catch {
    return ti18n("settings.clipboardPanel.fileFallback");
  }
}

function ClipboardEmptyIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" className="w-6 h-6 text-muted-foreground/50">
      <rect x="5" y="4" width="14" height="17" rx="2" stroke="currentColor" strokeWidth="1.5" />
      <path d="M9 4V2.5C9 2.2 9.2 2 9.5 2h5c.3 0 .5.2.5.5V4" stroke="currentColor" strokeWidth="1.5" />
      <path d="M8.5 11h7M8.5 14h4" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}
