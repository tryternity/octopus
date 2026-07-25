/**
 * RecordingPanel —— 录屏历史列表（spec §8.3 MVP）。
 *
 * 视觉沿用 octopus Settings 既有 panel 规范（参考 HistoryPanel 的列表+元数据+批量操作模式）：
 * - 顶部 sticky 全选 header（仅列表非空时显示）
 * - 单行：缩略图占位 + 标题/文件名 + meta（时长/分辨率/创建时间/源类型）+ hover 操作
 * - 底部：状态计数 + 批量删除（二次确认）
 *
 * 功能范围（Task 13）：
 * - ✅ 列表加载（list_recordings，limit 50）
 * - ✅ 单行操作：播放 / Finder 定位 / 收藏 toggle / 软删（二次确认）
 * - ✅ 批量删除（selectedIds 模式，二次确认）
 * - ✅ 空状态邀请行动
 * - ✅ 权限 banner（PermissionGate 包裹）
 * - ✅ 转字幕按钮灰占位（跳转 models 页）
 * - ✅ 顶部录制中状态 banner（useRecordSession state === "recording"/"paused"）
 * - ❌ 搜索框（P2 推迟，灰禁用 placeholder）
 * - ❌ 缩略图抽取（spec §9.2 F12 推迟，用 placeholder icon）
 * - ❌ 网格视图切换（spec §8.3 双视图，MVP 仅列表）
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Play,
  FolderOpen,
  Star,
  Trash2,
  Search,
  Film,
  Captions,
  Circle,
  Pause,
} from "lucide-react";
import { useT } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import type { ToastVariant } from "@/lib/useToast";
import { Button } from "@/components/ui/button";
import { PermissionGate } from "@/components/record/PermissionGate";
import { useRecordSession } from "@/hooks/useRecordSession";

// ── 后端类型镜像（crates/record/src/store.rs::RecordingMeta）──────────────────

export interface RecordingMeta {
  id: number;
  file_path: string;
  title: string;
  duration_ms: number;
  width: number;
  height: number;
  fps: number;
  codec: string;
  has_system_audio: boolean;
  has_microphone: boolean;
  source_type: string;
  file_size: number;
  has_thumbnail: boolean;
  is_favorite: boolean;
  created_at: string;
  deleted_at: string | null;
}

// ── 工具：格式化时长 ms → "MM:SS"（<1h）或 "H:MM:SS"（≥1h）─────────────────

function formatDuration(ms: number): string {
  const totalSec = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  if (h > 0) return `${h}:${pad(m)}:${pad(s)}`;
  return `${pad(m)}:${pad(s)}`;
}

/** 把 file_size bytes 格式化为 KB/MB/GB（参考 octopus 既有简短格式）。 */
function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes}B`;
  const units = ["KB", "MB", "GB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v >= 10 ? 0 : 1)}${units[i]}`;
}

/** 把 ISO8601 created_at 格式化为本地短日期（YYYY-MM-DD HH:MM）。 */
function formatCreatedAt(iso: string): string {
  if (!iso) return "";
  // 后端写的是 %Y-%m-%dT%H:%M:%SZ（UTC）；用 Date 解析后转本地。
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(
    d.getHours(),
  )}:${pad(d.getMinutes())}`;
}

// ── Panel 主组件 ─────────────────────────────────────────────────

interface RecordingPanelProps {
  showToast: (msg: string, variant?: ToastVariant) => void;
  /** 跳转到 Settings 内部其他 page（用于「转字幕」灰占位跳转到 models 页）。 */
  onNavigate?: (page: string) => void;
}

export default function RecordingPanel({
  showToast,
  onNavigate,
}: RecordingPanelProps) {
  const t = useT();
  const [records, setRecords] = useState<RecordingMeta[]>([]);
  const [loading, setLoading] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [confirmDelete, setConfirmDelete] = useState(false);
  // 顶部「正在录制中」banner + 控制按钮（start/pause/resume 由本 panel 触发，
  // stop 走 record_stop 命令需要 recording_id 等参数，本 panel MVP 不持有这些上下文，
  // 让用户用 Esc 快捷键或 tray menu 停止）。
  const {
    state: sessionState,
    duration,
    startDefault,
    pause: pauseSession,
    resume: resumeSession,
  } = useRecordSession();

  const [starting, setStarting] = useState(false);
  const handleStartDefault = useCallback(async () => {
    setStarting(true);
    try {
      await startDefault();
    } catch (e) {
      showToast(t("settings.recordings.startFailed") + e, "error");
    } finally {
      setStarting(false);
    }
  }, [startDefault, showToast, t]);

  const handlePauseResume = useCallback(async () => {
    try {
      if (sessionState === "recording") {
        await pauseSession();
      } else if (sessionState === "paused") {
        await resumeSession();
      }
    } catch (e) {
      showToast(t("settings.recordings.startFailed") + e, "error");
    }
  }, [sessionState, pauseSession, resumeSession, showToast, t]);

  const loadList = useCallback(async () => {
    setLoading(true);
    try {
      const recs = await invoke<RecordingMeta[]>("list_recordings", {
        filter: {
          limit: 50,
          offset: 0,
          includeDeleted: false,
          favoritesOnly: false,
        },
      });
      setRecords(recs);
      setSelectedIds(new Set());
      setConfirmDelete(false);
    } catch (e) {
      showToast(t("settings.recordings.loadFailed") + e, "error");
    }
    setLoading(false);
  }, [showToast, t]);

  useEffect(() => {
    loadList();
  }, [loadList]);

  const toggleSelect = (id: number) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
    setConfirmDelete(false);
  };

  const allChecked = records.length > 0 && selectedIds.size === records.length;
  const hasSelection = selectedIds.size > 0;

  const handleBatchDelete = async () => {
    if (selectedIds.size === 0) return;
    if (!confirmDelete) {
      setConfirmDelete(true);
      setTimeout(() => setConfirmDelete(false), 3000);
      return;
    }
    try {
      const ids = Array.from(selectedIds);
      await Promise.all(
        ids.map((id) =>
          invoke("delete_recording", { id, permanent: false }),
        ),
      );
      showToast(t("settings.recordings.deletedN", { n: ids.length }));
      setSelectedIds(new Set());
      setConfirmDelete(false);
      loadList();
    } catch (e) {
      showToast(t("settings.recordings.deleteFailed") + e, "error");
    }
  };

  const handleRowDeleted = () => {
    setSelectedIds(new Set());
    loadList();
  };

  const handleFavoriteToggled = () => {
    // 收藏 toggle 后刷新列表，确保收藏标识 / favorites_only 过滤正确
    loadList();
  };

  return (
    <PermissionGate onError={showToast}>
      <div className="flex flex-col h-full">
        {/* ── 标题区 + 搜索（置顶）── */}
        <div className="pb-3 border-b border-border space-y-2">
          <div className="flex items-center justify-between gap-2">
            <div className="min-w-0">
              <h2 className="text-sm font-semibold text-foreground">
                {t("settings.recordings.title")}
              </h2>
              <p className="text-[10px] text-muted-foreground mt-0.5">
                {t("settings.recordings.subtitle")}
              </p>
            </div>
            {sessionState === "recording" || sessionState === "paused" ? (
              <div className="flex items-center gap-1.5">
                <div
                  className={cn(
                    "flex items-center gap-1.5 px-2 py-1 rounded-md text-[10px] font-medium",
                    sessionState === "recording"
                      ? "bg-destructive/10 text-destructive"
                      : "bg-muted text-muted-foreground",
                  )}
                >
                  {sessionState === "recording" ? (
                    <Circle className="w-2 h-2 fill-current" />
                  ) : (
                    <Pause className="w-2.5 h-2.5" />
                  )}
                  <span>
                    {t(
                      sessionState === "recording"
                        ? "settings.recordings.recording"
                        : "settings.recordings.paused",
                    )}
                    {sessionState === "recording" && (
                      <span className="ml-1 tabular-nums">
                        {formatDuration(duration * 1000)}
                      </span>
                    )}
                  </span>
                </div>
                {/* 暂停/恢复按钮（Esc 或 tray menu 停止）*/}
                <Button
                  variant="outline"
                  size="sm"
                  onClick={handlePauseResume}
                  className="h-6 px-2 text-[10px]"
                >
                  {sessionState === "recording"
                    ? t("settings.recordings.pauseBtn")
                    : t("settings.recordings.resumeBtn")}
                </Button>
              </div>
            ) : (
              <Button
                variant="primary"
                size="sm"
                onClick={handleStartDefault}
                disabled={starting || sessionState === "starting"}
                className="h-6 px-2 text-[10px] gap-1"
              >
                <Circle className="w-2 h-2 fill-current" />
                {starting || sessionState === "starting"
                  ? t("settings.recordings.starting")
                  : t("settings.recordings.startBtn")}
              </Button>
            )}
          </div>
          {/* 搜索框：MVP 灰禁用 + placeholder 指向 P2 */}
          <div className="flex items-center gap-2 px-2.5 py-1.5 bg-muted rounded-md border border-border opacity-60">
            <Search className="w-3.5 h-3.5 text-muted-foreground" />
            <input
              type="text"
              disabled
              placeholder={t("settings.recordings.searchPlaceholder")}
              className="flex-1 bg-transparent text-xs outline-none placeholder:text-muted-foreground cursor-not-allowed"
            />
          </div>
        </div>

        {/* ── 列表 ── */}
        <div className="flex-1 overflow-y-auto thin-scrollbar -mx-1 px-1">
          {records.length > 0 && (
            <div className="sticky top-0 z-10 flex items-center gap-2 px-3 py-1.5 border-b border-border bg-muted group/header">
              <label className="flex items-center gap-1.5 cursor-pointer">
                <input
                  type="checkbox"
                  className="w-3.5 h-3.5 accent-primary"
                  checked={allChecked}
                  onChange={(e) =>
                    setSelectedIds(
                      e.target.checked
                        ? new Set(records.map((r) => r.id))
                        : new Set(),
                    )
                  }
                />
                <span className="text-[10px] text-muted-foreground group-hover/header:text-foreground transition-colors">
                  {hasSelection
                    ? t("settings.recordings.selectedN", {
                        n: selectedIds.size,
                      })
                    : t("settings.recordings.selectAll")}
                </span>
              </label>
            </div>
          )}

          {records.length === 0 && !loading && (
            <div className="flex flex-col items-center justify-center py-16 gap-1 text-muted-foreground">
              <Film className="w-8 h-8 mb-2 opacity-40" />
              <span className="text-sm text-center max-w-xs">
                {t("settings.recordings.empty")}
              </span>
            </div>
          )}

          {records.map((rec) => (
            <RecordingRow
              key={rec.id}
              rec={rec}
              isSelected={selectedIds.has(rec.id)}
              onToggleSelect={() => toggleSelect(rec.id)}
              showToast={showToast}
              onDeleted={handleRowDeleted}
              onFavoriteToggled={handleFavoriteToggled}
              onTranscribeClick={
                onNavigate ? () => onNavigate("models") : undefined
              }
            />
          ))}

          {loading && (
            <div className="text-center py-4 text-muted-foreground text-xs">
              {t("settings.recordings.loading")}
            </div>
          )}
        </div>

        {/* ── 底部：状态 + 批量操作 ── */}
        <div className="flex items-center justify-between py-2 border-t border-border">
          <span className="text-[10px] text-muted-foreground">
            {t("settings.recordings.totalN", { n: records.length })}
          </span>
          {hasSelection ? (
            <button
              className={cn(
                "flex items-center gap-1 px-2.5 py-1 rounded-md text-xs font-medium transition-all duration-150",
                confirmDelete
                  ? "bg-red-600 text-white"
                  : "border border-red-400 text-red-500 hover:bg-red-50 dark:hover:bg-red-950/30",
              )}
              onClick={handleBatchDelete}
            >
              <Trash2 className="w-3 h-3" />
              {confirmDelete
                ? t("settings.recordings.confirmDeleteN", {
                    n: selectedIds.size,
                  })
                : t("settings.recordings.deleteSelected")}
            </button>
          ) : null}
        </div>
      </div>
    </PermissionGate>
  );
}

// ── 单行组件 ──────────────────────────────────────────────────────

interface RecordingRowProps {
  rec: RecordingMeta;
  isSelected: boolean;
  onToggleSelect: () => void;
  showToast: (msg: string, variant?: ToastVariant) => void;
  onDeleted: () => void;
  onFavoriteToggled: () => void;
  onTranscribeClick?: () => void;
}

function RecordingRow({
  rec,
  isSelected,
  onToggleSelect,
  showToast,
  onDeleted,
  onFavoriteToggled,
  onTranscribeClick,
}: RecordingRowProps) {
  const t = useT();
  const [deletePending, setDeletePending] = useState(false);
  const [favoriteLoading, setFavoriteLoading] = useState(false);
  const deleteTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => {
      if (deleteTimer.current) clearTimeout(deleteTimer.current);
    };
  }, []);

  const title = rec.title || rec.file_path.split("/").pop() || `#${rec.id}`;
  const durationLabel = rec.duration_ms > 0 ? formatDuration(rec.duration_ms) : null;
  const resolutionLabel =
    rec.width > 0 && rec.height > 0 ? `${rec.width}×${rec.height}` : null;
  const sizeLabel = rec.file_size > 0 ? formatSize(rec.file_size) : null;

  const handlePlay = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await invoke("open_recording_file", { id: rec.id });
    } catch (e) {
      showToast(t("settings.recordings.loadFailed") + e, "error");
    }
  };

  const handleReveal = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await invoke("reveal_recording", { id: rec.id });
    } catch (e) {
      showToast(t("settings.recordings.loadFailed") + e, "error");
    }
  };

  const handleFavorite = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (favoriteLoading) return;
    setFavoriteLoading(true);
    try {
      await invoke("toggle_recording_favorite", { id: rec.id });
      onFavoriteToggled();
    } catch (e) {
      showToast(t("settings.recordings.deleteFailed") + e, "error");
    }
    setFavoriteLoading(false);
  };

  const handleDeleteClick = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!deletePending) {
      setDeletePending(true);
      deleteTimer.current = setTimeout(() => setDeletePending(false), 1500);
    } else {
      if (deleteTimer.current) clearTimeout(deleteTimer.current);
      try {
        await invoke("delete_recording", { id: rec.id, permanent: false });
        showToast(t("settings.recordings.deleted"));
        onDeleted();
      } catch (e) {
        showToast(t("settings.recordings.deleteFailed") + e, "error");
      }
    }
  };

  const handleTranscribeClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    onTranscribeClick?.();
  };

  return (
    <div
      className={cn(
        "group relative flex items-start gap-2.5 px-3 py-2.5 border-b border-border/60 transition-colors cursor-pointer",
        isSelected ? "bg-accent" : "hover:bg-muted",
        deletePending && "bg-red-50/10 dark:bg-red-950/20",
      )}
      onClick={onToggleSelect}
    >
      {rec.is_favorite && (
        <div className="absolute left-0 top-2 bottom-2 w-[2px] rounded-r bg-amber-600/40" />
      )}
      <input
        type="checkbox"
        className="w-3.5 h-3.5 mt-0.5 flex-shrink-0"
        checked={isSelected}
        onChange={(e) => {
          e.stopPropagation();
          onToggleSelect();
        }}
        onClick={(e) => e.stopPropagation()}
      />
      {/* 缩略图占位（spec §9.2 F12 真实缩略图抽取推迟） */}
      <div className="flex-shrink-0 w-16 h-9 rounded bg-muted border border-border flex items-center justify-center">
        <Film className="w-4 h-4 text-muted-foreground/50" />
      </div>
      <div className="flex-1 min-w-0">
        {/* Meta row */}
        <div className="flex items-center gap-1.5 mb-0.5 flex-wrap">
          <span className="text-[10px] text-muted-foreground">
            {formatCreatedAt(rec.created_at)}
          </span>
          {durationLabel && (
            <span className="text-[10px] text-muted-foreground px-1 rounded bg-muted tabular-nums">
              {durationLabel}
            </span>
          )}
          {resolutionLabel && (
            <span className="text-[10px] text-muted-foreground/70 tabular-nums">
              {resolutionLabel}
            </span>
          )}
          {rec.fps > 0 && (
            <span className="text-[10px] text-muted-foreground/70 tabular-nums">
              {rec.fps}fps
            </span>
          )}
          {sizeLabel && (
            <span className="text-[10px] text-muted-foreground/70 tabular-nums">
              {sizeLabel}
            </span>
          )}
          <span
            className={cn(
              "text-[10px] px-1.5 py-0.5 rounded font-medium",
              rec.has_microphone
                ? "bg-voice/10 text-voice"
                : "text-muted-foreground/60",
            )}
          >
            {rec.source_type}
          </span>
        </div>
        {/* Title */}
        <p className="text-xs leading-relaxed text-foreground truncate" title={title}>
          {title}
        </p>
      </div>

      {/* 右侧操作：播放 + Finder + 收藏 + 转字幕（灰） + 删除 */}
      <div className="flex-shrink-0 flex items-center gap-0.5">
        <button
          className="p-1 rounded opacity-0 group-hover:opacity-50 hover:!opacity-100 transition-opacity"
          onClick={handlePlay}
          title={t("settings.recordings.play")}
        >
          <Play className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
        </button>
        <button
          className="p-1 rounded opacity-0 group-hover:opacity-50 hover:!opacity-100 transition-opacity"
          onClick={handleReveal}
          title={t("settings.recordings.reveal")}
        >
          <FolderOpen className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
        </button>
        <button
          className="p-1 rounded opacity-60 group-hover:opacity-70 hover:!opacity-100 transition-opacity disabled:opacity-30"
          onClick={handleFavorite}
          disabled={favoriteLoading}
          title={
            rec.is_favorite
              ? t("settings.recordings.unfavorite")
              : t("settings.recordings.favorite")
          }
        >
          <Star
            className={cn(
              "w-3.5 h-3.5 transition-colors",
              rec.is_favorite
                ? "fill-amber-500 text-amber-500"
                : "text-muted-foreground hover:text-foreground",
            )}
          />
        </button>
        <button
          className={cn(
            "p-1 rounded transition-opacity",
            onTranscribeClick
              ? "opacity-40 group-hover:opacity-60 hover:!opacity-100 cursor-pointer"
              : "opacity-30 cursor-not-allowed",
          )}
          onClick={handleTranscribeClick}
          disabled={!onTranscribeClick}
          title={t("settings.recordings.transcriptTooltip")}
        >
          <Captions className="w-3.5 h-3.5 text-muted-foreground" />
        </button>
        <button
          className={cn(
            "p-1 rounded transition-all",
            deletePending
              ? "opacity-100 bg-red-100 dark:bg-red-950/40"
              : "opacity-0 group-hover:opacity-50 hover:!opacity-100 transition-opacity",
          )}
          onClick={handleDeleteClick}
          title={
            deletePending
              ? t("settings.recordings.deleteConfirm")
              : t("settings.recordings.delete")
          }
        >
          <Trash2
            className={cn(
              "w-3.5 h-3.5 transition-colors",
              deletePending
                ? "text-red-600"
                : "text-muted-foreground hover:text-red-500",
            )}
          />
        </button>
      </div>
    </div>
  );
}
