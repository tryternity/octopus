/**
 * RecordingPanel —— 录屏历史列表（spec §8.3 MVP）。
 *
 * 2026-07-30 拆分：子组件 + 类型 + 工具函数移到 Recording/ 目录。
 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Circle, Pause, Film, Trash2 } from "lucide-react";
import { useT } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import type { ToastVariant } from "@/lib/useToast";
import { Button } from "@/components/ui/button";
import { PermissionGate } from "@/components/record/PermissionGate";
import { useRecordSession } from "@/hooks/useRecordSession";

// 拆分出的子模块
import type {
  RecordingMeta, SubtitleResult, SubtitleCue, SubtitleStage,
  PolishOption, LlmOption, SubtitleProgressPayload,
} from "./Recording/types";
import { formatDuration, showPolishOutcomeToast } from "./Recording/format";
import RecordingRow from "./Recording/RecordingRow";
import SubtitlePanel from "./Recording/SubtitlePanel";
import SubtitlePolishDialog from "./Recording/SubtitlePolishDialog";
import DeleteConfirmDialog from "./Recording/DeleteConfirmDialog";

interface RecordingPanelProps {
  showToast: (msg: string, variant?: ToastVariant) => void;
}

export default function RecordingPanel({
  showToast,
}: RecordingPanelProps) {
  const t = useT();
  const [records, setRecords] = useState<RecordingMeta[]>([]);
  const [loading, setLoading] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [gifExportingId, setGifExportingId] = useState<number | null>(null);
  const [mergingId, setMergingId] = useState<number | null>(null);
  const [subtitleGeneratingId, setSubtitleGeneratingId] = useState<number | null>(null);
  const [subtitleResults, setSubtitleResults] = useState<Record<number, SubtitleResult>>({});
  const [subtitleError, setSubtitleError] = useState<Record<number, string>>({});
  const [expandedSubtitleId, setExpandedSubtitleId] = useState<number | null>(null);
  const [deleteDialog, setDeleteDialog] = useState<number | "batch" | null>(null);
  const [subtitleStage, setSubtitleStage] = useState<Record<number, SubtitleStage | undefined>>({});
  const [ffmpegAvailable, setFfmpegAvailable] = useState<boolean | null>(null);
  useEffect(() => {
    invoke<boolean>("check_ffmpeg").then(setFfmpegAvailable).catch(() => setFfmpegAvailable(true));
  }, []);

  // ── 字幕 LLM 润色默认配置（Phase 4，Task 4.3）──
  const polishDefault = false;
  const polishLlmKey = "";
  const [llmOptions, setLlmOptions] = useState<LlmOption[]>([]);
  useEffect(() => {
    invoke<LlmOption[]>("list_subtitle_llms")
      .then(setLlmOptions)
      .catch(() => setLlmOptions([]));
  }, []);

  // ── 转字幕弹对话框状态 ──
  const [polishDialogId, setPolishDialogId] = useState<number | null>(null);
  const [dialogPolishEnabled, setDialogPolishEnabled] = useState(false);
  const [dialogLlmKey, setDialogLlmKey] = useState<string>("");

  // ── 订阅 record://task 事件（字幕生成进度）──
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen<{
      event: string;
      id: number;
      cueCount?: number;
      error?: string;
      stage?: SubtitleProgressPayload["stage"];
      percent?: number;
      message?: string;
    }>("record://task", (msg) => {
      const e = msg.payload as {
        event: string;
        id: number;
        cueCount?: number;
        error?: string;
      };
      if (e.event === "subtitle-started") {
        setSubtitleGeneratingId(e.id);
        setSubtitleStage((prev) => ({ ...prev, [e.id]: undefined }));
      } else if (e.event === "subtitle-progress") {
        const p = msg.payload as {
          event: string;
          id: number;
          stage?: { stage: SubtitleStage; percent?: number };
        };
        if (p.stage?.stage) {
          setSubtitleStage((prev) => ({ ...prev, [e.id]: p.stage!.stage }));
        }
      } else if (e.event === "subtitle-done") {
        setSubtitleGeneratingId(null);
        setSubtitleStage((prev) => {
          if (!prev[e.id]) return prev;
          const next = { ...prev };
          delete next[e.id];
          return next;
        });
        setSubtitleError((prev) => {
          if (!prev[e.id]) return prev;
          const next = { ...prev };
          delete next[e.id];
          return next;
        });
        invoke<SubtitleResult | null>("read_subtitle", { id: e.id }).then((r) => {
          if (r) {
            setSubtitleResults((prev) => ({ ...prev, [e.id]: r }));
            showToast(
              t("settings.recordings.subtitleDone", { count: r.cues.length }),
              "success",
            );
          }
        });
      } else if (e.event === "subtitle-failed") {
        setSubtitleGeneratingId(null);
        setSubtitleStage((prev) => {
          if (!prev[e.id]) return prev;
          const next = { ...prev };
          delete next[e.id];
          return next;
        });
        const msg = e.error || t("settings.recordings.subtitleFailed");
        setSubtitleError((prev) => ({ ...prev, [e.id]: msg }));
        showToast(t("settings.recordings.subtitleFailed") + ": " + msg, "error");
      }
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [showToast, t]);

  const onGenerateSubtitle = useCallback(
    async (id: number, track?: string, polish?: PolishOption | null) => {
      setSubtitleGeneratingId(id);
      setSubtitleStage((prev) => ({ ...prev, [id]: undefined }));
      try {
        const result = await invoke<SubtitleResult>("generate_subtitle", {
          id,
          track: track ?? null,
          polish: polish ?? null,
        });
        setSubtitleResults((prev) => ({ ...prev, [id]: result }));
        setSubtitleError((prev) => {
          if (!prev[id]) return prev;
          const next = { ...prev };
          delete next[id];
          return next;
        });
        showPolishOutcomeToast(result.polishOutcome, showToast, t);
      } catch (e) {
        const msg = String(e);
        setSubtitleError((prev) => ({ ...prev, [id]: msg }));
        showToast(t("settings.recordings.subtitleFailed") + ": " + msg, "error");
        setSubtitleGeneratingId(null);
        setSubtitleStage((prev) => {
          if (!prev[id]) return prev;
          const next = { ...prev };
          delete next[id];
          return next;
        });
      }
    },
    [showToast, t],
  );

  const onRevealSubtitle = useCallback(
    async (id: number) => {
      try {
        await invoke<string>("reveal_subtitle", { id });
        showToast(t("settings.recordings.subtitleRevealed"), "success");
      } catch (e) {
        showToast(t("settings.recordings.subtitleRevealFailed") + ": " + String(e), "error");
      }
    },
    [showToast, t],
  );

  const onCopyCue = useCallback(
    async (cue: SubtitleCue) => {
      try {
        await navigator.clipboard.writeText(cue.text);
        showToast(t("settings.recordings.subtitleCopied"), "success");
      } catch (e) {
        showToast(t("settings.recordings.subtitleCopyFailed") + ": " + String(e), "error");
      }
    },
    [showToast, t],
  );

  const onCopyAll = useCallback(
    async (result: SubtitleResult) => {
      const text = result.cues.map((c) => c.text).join("\n");
      try {
        await navigator.clipboard.writeText(text);
        showToast(t("settings.recordings.subtitleCopied"), "success");
      } catch (e) {
        showToast(t("settings.recordings.subtitleCopyFailed") + ": " + String(e), "error");
      }
    },
    [showToast, t],
  );

  const onToggleExpandSubtitle = useCallback((id: number) => {
    setExpandedSubtitleId((prev) => (prev === id ? null : id));
  }, []);

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
          favoritesOnly: false,
        },
      });
      setRecords(recs);
      setSelectedIds(new Set());
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
  };

  const allChecked = records.length > 0 && selectedIds.size === records.length;
  const hasSelection = selectedIds.size > 0;

  const handleBatchDelete = () => {
    if (selectedIds.size === 0) return;
    setDeleteDialog("batch");
  };

  const handleFavoriteToggled = () => {
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
        </div>

        {/* ── 转字幕润色弹对话框 ── */}
        {polishDialogId !== null && (
          <SubtitlePolishDialog
            rec={records.find((r) => r.id === polishDialogId)}
            llmOptions={llmOptions}
            polishEnabled={dialogPolishEnabled}
            llmKey={dialogLlmKey}
            onPolishEnabledChange={setDialogPolishEnabled}
            onLlmKeyChange={setDialogLlmKey}
            onCancel={() => setPolishDialogId(null)}
            onConfirm={(polish) => {
              const id = polishDialogId;
              setPolishDialogId(null);
              onGenerateSubtitle(id, undefined, polish);
            }}
            t={t}
          />
        )}

        {/* ── 字幕预览浮层 ── */}
        {expandedSubtitleId !== null &&
          subtitleResults[expandedSubtitleId] && (
            <SubtitlePanel
              result={subtitleResults[expandedSubtitleId]}
              error={subtitleError[expandedSubtitleId]}
              onExport={() => onRevealSubtitle(expandedSubtitleId)}
              onCopyCue={onCopyCue}
              onCopyAll={() =>
                onCopyAll(subtitleResults[expandedSubtitleId])
              }
              onClose={() => setExpandedSubtitleId(null)}
              t={t}
            />
          )}

        {/* ── 删除确认弹框 ── */}
        {deleteDialog !== null && (
          <DeleteConfirmDialog
            targetLabel={
              typeof deleteDialog === "number"
                ? (() => {
                    const r = records.find((x) => x.id === deleteDialog);
                    return r
                      ? r.title || r.filePath.split("/").pop() || `#${r.id}`
                      : `#${deleteDialog}`;
                  })()
                : ""
            }
            count={
              deleteDialog === "batch" ? selectedIds.size : 1
            }
            onCancel={() => setDeleteDialog(null)}
            onConfirm={async (permanent) => {
              const ids =
                deleteDialog === "batch"
                  ? Array.from(selectedIds)
                  : [deleteDialog as number];
              setDeleteDialog(null);
              try {
                await Promise.all(
                  ids.map((id) =>
                    invoke("delete_recording", { id, permanent }),
                  ),
                );
                showToast(
                  t("settings.recordings.deletedN", { n: ids.length }),
                );
                if (deleteDialog === "batch") setSelectedIds(new Set());
                loadList();
              } catch (e) {
                showToast(
                  t("settings.recordings.deleteFailed") + e,
                  "error",
                );
              }
            }}
            t={t}
          />
        )}

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
              onRequestDelete={(id) => setDeleteDialog(id)}
              onFavoriteToggled={handleFavoriteToggled}
              onRenamed={loadList}
              gifExportingId={gifExportingId}
              onExportGif={(gid) => setGifExportingId(gid)}
              ffmpegAvailable={ffmpegAvailable}
              mergingId={mergingId}
              onMergeAudio={(mid) => setMergingId(mid)}
              onMerged={loadList}
              subtitleGeneratingId={subtitleGeneratingId}
              hasSubtitle={!!subtitleResults[rec.id]}
              subtitleStage={subtitleStage[rec.id]}
              subtitleError={subtitleError[rec.id]}
              onRequestPolishDialog={(id) => {
                setDialogPolishEnabled(polishDefault);
                setDialogLlmKey(polishLlmKey || llmOptions[0]?.key || "");
                setPolishDialogId(id);
              }}
              expandedSubtitleId={expandedSubtitleId}
              onToggleExpandSubtitle={onToggleExpandSubtitle}
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
              className="flex items-center gap-1 px-2.5 py-1 rounded-md text-xs font-medium transition-all duration-150 border border-red-400 text-red-500 hover:bg-red-50 dark:hover:bg-red-950/30"
              onClick={handleBatchDelete}
            >
              <Trash2 className="w-3 h-3" />
              {t("settings.recordings.deleteSelected")}
            </button>
          ) : null}
        </div>
      </div>
    </PermissionGate>
  );
}
