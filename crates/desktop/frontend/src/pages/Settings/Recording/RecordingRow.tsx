// 录屏历史行组件——缩略图占位 + meta + 标题 + hover 操作。
// 从 RecordingPanel.tsx 拆出（2026-07-30）。

import { useState, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Play, FolderOpen, Star, Trash2, Film, Captions,
  Pencil, Clapperboard, Loader2, Combine, ChevronDown,
  Info, Sparkles,
} from "lucide-react";
import { useT } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import type { ToastVariant } from "@/lib/useToast";
import type { RecordingMeta, SubtitleStage, MergeResult } from "./types";
import { formatDuration, formatSize, formatCreatedAt } from "./format";

export interface RecordingRowProps {
  rec: RecordingMeta;
  isSelected: boolean;
  onToggleSelect: () => void;
  showToast: (msg: string, variant?: ToastVariant) => void;
  onRequestDelete: (id: number) => void;
  onFavoriteToggled: () => void;
  onRenamed: () => void;
  gifExportingId: number | null;
  onExportGif: (id: number | null) => void;
  ffmpegAvailable: boolean | null;
  mergingId: number | null;
  onMergeAudio: (id: number | null) => void;
  onMerged: () => void;
  subtitleGeneratingId: number | null;
  hasSubtitle: boolean;
  subtitleStage?: SubtitleStage;
  subtitleError?: string;
  onRequestPolishDialog: (id: number) => void;
  expandedSubtitleId: number | null;
  onToggleExpandSubtitle: (id: number) => void;
}

export default function RecordingRow({
  rec,
  isSelected,
  onToggleSelect,
  showToast,
  onRequestDelete,
  onFavoriteToggled,
  onRenamed,
  gifExportingId,
  onExportGif,
  ffmpegAvailable,
  mergingId,
  onMergeAudio,
  onMerged,
  subtitleGeneratingId,
  hasSubtitle,
  subtitleStage,
  subtitleError,
  onRequestPolishDialog,
  expandedSubtitleId,
  onToggleExpandSubtitle,
}: RecordingRowProps) {
  const t = useT();
  const [favoriteLoading, setFavoriteLoading] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [renameVal, setRenameVal] = useState("");
  const renameCancelledRef = useRef(false);

  const title = rec.title || rec.filePath.split("/").pop() || `#${rec.id}`;
  const durationLabel = rec.durationMs > 0 ? formatDuration(rec.durationMs) : null;
  const resolutionLabel =
    rec.width > 0 && rec.height > 0 ? `${rec.width}×${rec.height}` : null;
  const sizeLabel = rec.fileSize > 0 ? formatSize(rec.fileSize) : null;

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

  const commitRename = useCallback(async () => {
    if (renameCancelledRef.current) {
      renameCancelledRef.current = false;
      return;
    }
    const newTitle = renameVal.trim();
    setRenaming(false);
    if (!newTitle || newTitle === title) return;
    try {
      await invoke("rename_recording", { id: rec.id, title: newTitle });
      onRenamed();
    } catch (e) {
      showToast(t("settings.recordings.loadFailed") + e, "error");
    }
  }, [renameVal, title, rec.id, onRenamed, showToast, t]);

  const startRename = (e: React.MouseEvent) => {
    e.stopPropagation();
    renameCancelledRef.current = false;
    setRenameVal(title);
    setRenaming(true);
  };

  const isExportingGif = gifExportingId === rec.id;
  const ffmpegDisabled = ffmpegAvailable === false;
  const handleExportGif = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (isExportingGif || ffmpegDisabled) return;
    onExportGif(rec.id);
    try {
      const path = await invoke<string>("export_gif", { id: rec.id });
      showToast(t("settings.recordings.exportGifDone", { path }), "success");
    } catch (err) {
      showToast(t("settings.recordings.exportGifFailed") + String(err), "error");
    } finally {
      onExportGif(null);
    }
  };

  const isMerging = mergingId === rec.id;
  const handleMergeAudio = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (isMerging) return;
    onMergeAudio(rec.id);
    try {
      const result = await invoke<MergeResult>("merge_audio_tracks", { id: rec.id });
      showToast(
        t("settings.recordings.mergeAudioDone", { path: result.filePath }),
        "success",
      );
      onMerged();
    } catch (err) {
      showToast(t("settings.recordings.mergeAudioFailed") + String(err), "error");
    } finally {
      onMergeAudio(null);
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

  const handleDeleteClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    onRequestDelete(rec.id);
  };

  const isGeneratingSubtitle = subtitleGeneratingId === rec.id;
  const handleGenerateSubtitle = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (isGeneratingSubtitle) return;
    onRequestPolishDialog(rec.id);
  };

  const isSubtitleExpanded = expandedSubtitleId === rec.id;

  return (
    <div
      className={cn(
        "group relative border-b border-border/60 transition-colors",
        isSelected ? "bg-accent" : "hover:bg-muted",
        isSubtitleExpanded && "!bg-muted",
      )}
    >
    <div
      className={cn(
        "flex items-start gap-2.5 px-3 py-2.5 cursor-pointer",
      )}
      onClick={onToggleSelect}
    >
      {rec.isFavorite && (
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
            {formatCreatedAt(rec.createdAt)}
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
              rec.hasMicrophone
                ? "bg-voice/10 text-voice"
                : "text-muted-foreground/60",
            )}
          >
            {rec.sourceType}
          </span>
          {rec.audioTracks && rec.audioTracks.length > 0 && (
            <div className="flex gap-1 items-center text-[10px]">
              {rec.audioTracks.map((track, i) => (
                <span
                  key={i}
                  className="px-1.5 py-0.5 rounded bg-muted text-muted-foreground"
                  title={`${track.codec} ${track.sampleRate}Hz ${track.channels}ch`}
                >
                  {track.source === 'microphone' &&
                    `🎤${track.deviceName ? ` ${track.deviceName}` : ''}`}
                  {track.source === 'system' && '🔊'}
                  {track.source === 'merged' && '🎵 merged'}
                  {track.source === 'unknown' && '? unknown'}
                </span>
              ))}
            </div>
          )}
        </div>
        {/* Title（renaming 时显示 inline input）*/}
        {renaming ? (
          <input
            autoFocus
            value={renameVal}
            onChange={(e) => setRenameVal(e.target.value)}
            onBlur={commitRename}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
              if (e.key === "Escape") {
                renameCancelledRef.current = true;
                setRenaming(false);
              }
            }}
            onClick={(e) => e.stopPropagation()}
            className="text-xs leading-relaxed text-foreground bg-background border border-border rounded px-1 py-0.5 w-full outline-none focus:border-primary"
          />
        ) : (
          <p className="text-xs leading-relaxed text-foreground truncate" title={title}>
            {title}
          </p>
        )}
      </div>

      {/* 右侧操作：播放 + Finder + 收藏 + 转字幕 + 删除 */}
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
          className="p-1 rounded opacity-0 group-hover:opacity-50 hover:!opacity-100 transition-opacity"
          onClick={startRename}
          title={t("settings.recordings.rename")}
        >
          <Pencil className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
        </button>
        <button
          className="p-1 rounded opacity-60 group-hover:opacity-70 hover:!opacity-100 transition-opacity disabled:opacity-30"
          onClick={handleFavorite}
          disabled={favoriteLoading}
          title={
            rec.isFavorite
              ? t("settings.recordings.unfavorite")
              : t("settings.recordings.favorite")
          }
        >
          <Star
            className={cn(
              "w-3.5 h-3.5 transition-colors",
              rec.isFavorite
                ? "fill-amber-500 text-amber-500"
                : "text-muted-foreground hover:text-foreground",
            )}
          />
        </button>
        <button
          className={cn(
            "p-1 rounded transition-opacity",
            isGeneratingSubtitle
              ? "opacity-100 cursor-wait"
              : "opacity-60 group-hover:opacity-70 hover:!opacity-100 cursor-pointer",
          )}
          onClick={handleGenerateSubtitle}
          disabled={isGeneratingSubtitle}
          title={
            isGeneratingSubtitle
              ? subtitleStage === "polishing"
                ? t("settings.recordings.subtitlePolishing")
                : t("settings.recordings.subtitleGenerating")
              : hasSubtitle
                ? t("settings.recordings.transcriptRegenerate")
                : t("settings.recordings.transcript")
          }
        >
          {isGeneratingSubtitle ? (
            subtitleStage === "polishing" ? (
              <Sparkles className="w-3.5 h-3.5 text-warning animate-pulse" />
            ) : (
              <Loader2 className="w-3.5 h-3.5 text-muted-foreground animate-spin" />
            )
          ) : (
            <Captions className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
          )}
        </button>
        {hasSubtitle && !isGeneratingSubtitle && (
          <button
            className={cn(
              "p-1 rounded transition-opacity",
              isSubtitleExpanded
                ? "opacity-100"
                : "opacity-60 group-hover:opacity-70 hover:!opacity-100 cursor-pointer",
            )}
            onClick={(e) => {
              e.stopPropagation();
              onToggleExpandSubtitle(rec.id);
            }}
            title={
              isSubtitleExpanded
                ? t("settings.recordings.subtitleCollapse")
                : t("settings.recordings.subtitleExpand")
            }
          >
            <ChevronDown
              className={cn(
                "w-3.5 h-3.5 text-muted-foreground hover:text-foreground transition-transform duration-150",
                isSubtitleExpanded && "rotate-180",
              )}
            />
          </button>
        )}
        <button
          className={cn(
            "p-1 rounded transition-opacity",
            ffmpegDisabled
              ? "opacity-30 cursor-not-allowed"
              : isExportingGif
                ? "opacity-100"
                : "opacity-60 group-hover:opacity-70 hover:!opacity-100 cursor-pointer",
          )}
          onClick={handleExportGif}
          disabled={isExportingGif || ffmpegDisabled}
          title={
            ffmpegDisabled
              ? t("settings.recordings.ffmpegMissing")
              : t("settings.recordings.exportGif")
          }
        >
          {isExportingGif ? (
            <Loader2 className="w-3.5 h-3.5 text-muted-foreground animate-spin" />
          ) : (
            <Clapperboard className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
          )}
        </button>
        {rec.audioTracks && rec.audioTracks.length >= 2 && (
          <button
            className={cn(
              "p-1 rounded transition-opacity",
              isMerging
                ? "opacity-100"
                : "opacity-60 group-hover:opacity-70 hover:!opacity-100 cursor-pointer",
            )}
            onClick={handleMergeAudio}
            disabled={isMerging}
            title={
              isMerging
                ? t("settings.recordings.merging")
                : t("settings.recordings.mergeAudioTooltip")
            }
          >
            {isMerging ? (
              <Loader2 className="w-3.5 h-3.5 text-muted-foreground animate-spin" />
            ) : (
              <Combine className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
            )}
          </button>
        )}
        <button
          className="p-1 rounded transition-all opacity-0 group-hover:opacity-50 hover:!opacity-100 transition-opacity"
          onClick={handleDeleteClick}
          title={t("settings.recordings.delete")}
        >
          <Trash2 className="w-3.5 h-3.5 transition-colors text-muted-foreground hover:text-red-500" />
        </button>
      </div>

      {/* 行内字幕错误 */}
      {subtitleError && !isSubtitleExpanded && (
        <div className="px-3 pb-1.5 -mt-1 flex items-start gap-1 text-[10px] text-destructive">
          <Info className="w-3 h-3 mt-px flex-shrink-0" />
          <span className="break-all">{subtitleError}</span>
        </div>
      )}

      {/* polishing 阶段行内进度提示 */}
      {isGeneratingSubtitle &&
        subtitleStage === "polishing" &&
        !isSubtitleExpanded && (
          <div className="px-3 pb-1.5 -mt-1 flex items-center gap-1.5">
            <div className="flex items-center gap-1 px-1.5 py-0.5 rounded bg-warning/10 text-warning text-[10px] font-medium">
              <Sparkles className="w-2.5 h-2.5 animate-pulse" />
              <span>{t("settings.recordings.subtitlePolishing")}</span>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
