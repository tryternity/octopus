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
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
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
  Pencil,
  Clapperboard,
  Loader2,
  Combine,
  ChevronDown,
  Copy,
  CopyCheck,
  Download,
  Info,
} from "lucide-react";
import { useT } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import type { ToastVariant } from "@/lib/useToast";
import { Button } from "@/components/ui/button";
import { PermissionGate } from "@/components/record/PermissionGate";
import { useRecordSession } from "@/hooks/useRecordSession";

// ── 后端类型镜像（crates/record/src/store.rs::RecordingMeta）──────────────────

// 音轨（crates/record/src/audioTracks.rs::AudioTrack，serde rename_all=camelCase）
// source enum rename_all=lowercase：'microphone' | 'system' | 'merged' | 'unknown'
export interface AudioTrack {
  index: number;
  source: 'microphone' | 'system' | 'merged' | 'unknown';
  codec: string;
  sampleRate: number;
  channels: number;
  deviceName?: string;
}

export interface RecordingMeta {
  id: number;
  filePath: string;
  title: string;
  durationMs: number;
  width: number;
  height: number;
  fps: number;
  codec: string;
  hasSystemAudio: boolean;
  hasMicrophone: boolean;
  audioTracks: AudioTrack[];
  sourceType: string;
  fileSize: number;
  hasThumbnail: boolean;
  isFavorite: boolean;
  createdAt: string;
  isDeleted: boolean;
}

// 字幕 cue（与 crates/record/src/subtitle.rs::SubtitleCue 对齐，camelCase）。
export interface SubtitleCue {
  startMs: number;
  endMs: number;
  text: string;
}

// merge_audio_tracks 命令的返回值（crates/desktop/src/record_commands.rs::MergeResult）。
interface MergeResult {
  newId: number;
  filePath: string;
}

// 字幕生成结果（与 crates/record/src/subtitle.rs::SubtitleResult 对齐，camelCase）。
// trackUsed 对应 AudioTrackSource（serde rename_all=lowercase）。
export interface SubtitleResult {
  cues: SubtitleCue[];
  srtText: string;
  model: string;
  trackUsed: "microphone" | "system" | "merged" | "unknown";
}

// 字幕生成阶段（与 crates/record/src/subtitle.rs::SubtitleProgress 对齐，外层 kebab-case tag）。
// 用于 record://task 事件的 SubtitleProgress 变体（stage 字段 + 额外 percent/cueCount/message）。
export type SubtitleStage =
  | "extracting-audio"
  | "recognizing"
  | "finalizing"
  | "done"
  | "error";

// record://task 事件 payload 子集（仅字幕相关变体）。
export interface SubtitleProgressPayload {
  id: number;
  stage: SubtitleStage;
  percent?: number;
  cueCount?: number;
  message?: string;
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

/** 把 fileSize bytes 格式化为 KB/MB/GB（参考 octopus 既有简短格式）。 */
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

/** 把 ISO8601 createdAt 格式化为本地短日期（YYYY-MM-DD HH:MM）。 */
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

/**
 * 把 cue 的 ms 时间戳格式化为紧凑时间码：
 * <1h → "MM:SS"（如 01:23），≥1h → "H:MM:SS"（如 1:02:03）。
 * 字幕面板的时间区间（00:00 → 00:08）专用，区别于 formatDuration（录屏总时长）。
 */
function formatMs(ms: number): string {
  const totalSec = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}

// ── Panel 主组件 ─────────────────────────────────────────────────

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
  const [confirmDelete, setConfirmDelete] = useState(false);
  // GIF 导出：一次只导出一个（按 id 跟踪，null=空闲）。row 据此切换按钮 disabled/spinner。
  const [gifExportingId, setGifExportingId] = useState<number | null>(null);
  // 音轨合并：一次只合并一个（按 id 跟踪，null=空闲）。仿 gifExportingId 模式。
  const [mergingId, setMergingId] = useState<number | null>(null);
  // 字幕生成：一次只生成一个（按 id 跟踪，null=空闲）。仿 gifExportingId 模式。
  // 后端 generate_subtitle 是 async 但内部跑数秒 ffmpeg+ASR；loading 态也由
  // `record://task` 事件（subtitle-started/done/failed）维持，确保跨窗口同步。
  const [subtitleGeneratingId, setSubtitleGeneratingId] = useState<number | null>(null);
  // 已拉取的字幕结果缓存（按 recording id 索引）。subtitle-done 事件触发 read_subtitle 拉取后填入。
  const [subtitleResults, setSubtitleResults] = useState<Record<number, SubtitleResult>>({});
  // 字幕生成错误文案（按 id 暂存）。subtitle-failed 事件或 generate_subtitle reject 时填，
  // 行内红字展示（区别于 toast 一过性提示）。成功后清。
  const [subtitleError, setSubtitleError] = useState<Record<number, string>>({});
  // 当前展开字幕预览面板的 recording id（null=全收起）。一次只展开一个（列表节奏感）。
  const [expandedSubtitleId, setExpandedSubtitleId] = useState<number | null>(null);
  // ffmpeg 可用性（mount 时探测，决定 GIF 按钮灰禁 + tooltip 引导）。
  // null=探测中（默认 true 可点，避免闪烁），true=可用，false=未找到（灰禁 + tooltip）。
  const [ffmpegAvailable, setFfmpegAvailable] = useState<boolean | null>(null);
  useEffect(() => {
    invoke<boolean>("check_ffmpeg").then(setFfmpegAvailable).catch(() => setFfmpegAvailable(true));
  }, []);

  // ── 订阅 record://task 事件（字幕生成进度，仿 useRecordSession 的 record://event 范式）──
  // 后端 RecordTaskEvent（record_commands.rs:941）外层 kebab-case + 变体 camelCase：
  //   subtitle-started { id } / subtitle-done { id, cueCount } / subtitle-failed { id, error }
  //   subtitle-progress { id, stage: SubtitleProgress }  —— Task 4.2 详情面板再用，本任务暂忽略。
  // 监听用于跨窗口同步（A 窗口触发生成，B 窗口也能收到 done 自动刷新字幕缓存）。
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
      // Tauri 2 listen callback 收到的是 Event<T>，数据在 msg.payload（不是 msg 本身）。
      const e = msg.payload as {
        event: string;
        id: number;
        cueCount?: number;
        error?: string;
      };
      if (e.event === "subtitle-started") {
        setSubtitleGeneratingId(e.id);
      } else if (e.event === "subtitle-done") {
        setSubtitleGeneratingId(null);
        // 清行内错误（如有）。重新拉取该 recording 的字幕（含空 cues 的「正常无字幕」场景）。
        setSubtitleError((prev) => {
          if (!prev[e.id]) return prev;
          const next = { ...prev };
          delete next[e.id];
          return next;
        });
        // read_subtitle 返回 Option<SubtitleResult>：null=未生成。
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
        // 行内红字 + toast 双通道：行内留存方便用户回看，toast 跨 panel 可见。
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

  // 触发字幕生成。loading 态同时由本 handler（乐观）和 record://task 事件（权威）维持。
  // 注意：generate_subtitle 内部跑数秒 ffmpeg+ASR，invoke promise 何时 resolve 与
  // subtitle-done 事件到达的先后无保证——故 finally 不清 subtitleGeneratingId，
  // 改由事件回调（done/failed）清，避免按钮提前转回 idle 又被事件点亮。
  const onGenerateSubtitle = useCallback(
    async (id: number, track?: string) => {
      setSubtitleGeneratingId(id);
      try {
        const result = await invoke<SubtitleResult>("generate_subtitle", {
          id,
          track: track ?? null,
        });
        // 乐观更新缓存（done 事件到达前先显示）。空 cues（无声）也存——前端显示「无字幕」。
        setSubtitleResults((prev) => ({ ...prev, [id]: result }));
        setSubtitleError((prev) => {
          if (!prev[id]) return prev;
          const next = { ...prev };
          delete next[id];
          return next;
        });
      } catch (e) {
        // 行内红字 + toast 双通道（与 subtitle-failed 事件回调保持一致）。
        const msg = String(e);
        setSubtitleError((prev) => ({ ...prev, [id]: msg }));
        showToast(t("settings.recordings.subtitleFailed") + ": " + msg, "error");
        // 失败兜底清 loading（事件回调也会清，这里防事件丢失）。
        setSubtitleGeneratingId(null);
      }
    },
    [showToast, t],
  );

  // ── 字幕面板操作（Task 4.2）──
  // 导出 SRT：弹原生 save 对话框 → invoke export_subtitle 写文件。失败 toast。
  // 注意 destPath camelCase：tauri invoke 默认 snake→camel 转换参数名。
  // 在 Finder 显示最新 SRT 文件（v2：替代 export_subtitle——SRT 已直接生成在磁盘）。
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

  // 复制单条 cue：单击 cue 行触发。复制后由 SubtitlePanel 行内显示「已复制」反馈。
  // 反馈状态由 SubtitlePanel 内部 useTransitionalState 管理，本 handler 只负责写剪贴板。
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

  // 复制全部 cue 文本（不含时间戳，纯文本拼接，方便贴到笔记/聊天）。
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

  // 展开/收起字幕面板：点击 toggle，再次点击同一 id 收起。
  const onToggleExpandSubtitle = useCallback((id: number) => {
    setExpandedSubtitleId((prev) => (prev === id ? null : id));
  }, []);
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
              onRenamed={loadList}
              gifExportingId={gifExportingId}
              onExportGif={(gid) => setGifExportingId(gid)}
              ffmpegAvailable={ffmpegAvailable}
              mergingId={mergingId}
              onMergeAudio={(mid) => setMergingId(mid)}
              onMerged={loadList}
              subtitleGeneratingId={subtitleGeneratingId}
              subtitleResult={subtitleResults[rec.id]}
              subtitleError={subtitleError[rec.id]}
              onGenerateSubtitle={onGenerateSubtitle}
              expandedSubtitleId={expandedSubtitleId}
              onToggleExpandSubtitle={onToggleExpandSubtitle}
              onRevealSubtitle={onRevealSubtitle}
              onCopyCue={onCopyCue}
              onCopyAll={onCopyAll}
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
  onRenamed: () => void;
  gifExportingId: number | null;
  onExportGif: (id: number | null) => void;
  ffmpegAvailable: boolean | null;
  mergingId: number | null;
  onMergeAudio: (id: number | null) => void;
  onMerged: () => void;
  /** 字幕生成中 id（null=空闲）。控制 Captions 按钮 spinner / disabled。 */
  subtitleGeneratingId: number | null;
  /** 该 recording 已生成的字幕（无则 undefined）。Task 4.2 详情面板消费。 */
  subtitleResult?: SubtitleResult;
  /** 字幕生成失败文案（无则 undefined）。行内红字展示。 */
  subtitleError?: string;
  /** 触发字幕生成。track 留空走后端 Auto 选轨。 */
  onGenerateSubtitle: (id: number, track?: string) => void;
  /** 当前展开字幕预览的 recording id（null=全收起）。 */
  expandedSubtitleId: number | null;
  /** 展开/收起字幕预览面板。 */
  onToggleExpandSubtitle: (id: number) => void;
  /** 在 Finder 显示最新 SRT 文件。 */
  onRevealSubtitle: (id: number) => void;
  /** 复制单条 cue 文本到剪贴板。 */
  onCopyCue: (cue: SubtitleCue) => void;
  /** 复制全部 cue 纯文本到剪贴板。 */
  onCopyAll: (result: SubtitleResult) => void;
}

function RecordingRow({
  rec,
  isSelected,
  onToggleSelect,
  showToast,
  onDeleted,
  onFavoriteToggled,
  onRenamed,
  gifExportingId,
  onExportGif,
  ffmpegAvailable,
  mergingId,
  onMergeAudio,
  onMerged,
  subtitleGeneratingId,
  subtitleResult,
  subtitleError,
  onGenerateSubtitle,
  expandedSubtitleId,
  onToggleExpandSubtitle,
  onRevealSubtitle,
  onCopyCue,
  onCopyAll,
}: RecordingRowProps) {
  const t = useT();
  const [deletePending, setDeletePending] = useState(false);
  const [favoriteLoading, setFavoriteLoading] = useState(false);
  const deleteTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // 重命名 inline input（WKWebView 不支持 window.prompt，用 inline input 仿 HotwordPanel 范式）
  const [renaming, setRenaming] = useState(false);
  const [renameVal, setRenameVal] = useState("");
  const renameCancelledRef = useRef(false);

  useEffect(() => {
    return () => {
      if (deleteTimer.current) clearTimeout(deleteTimer.current);
    };
  }, []);

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

  // ── 重命名（inline input，仿 HotwordPanel 范式）──
  // WKWebView 不支持 window.prompt，用 inline input。
  // Enter / blur → 提交；Escape → 取消（renameCancelledRef 防 blur 重复触发）。
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

  // ── GIF 导出（F20）── invoke export_gif 命令，loading 状态由父 gifExportingId 控制
  const isExportingGif = gifExportingId === rec.id;
  // ffmpeg 缺失时灰禁（null=探测中，按可用处理避免闪烁；false=未找到，灰禁 + tooltip）
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

  // ── 音轨合并（仿 handleExportGif 模式）── invoke merge_audio_tracks 命令，
  // loading 状态由父 mergingId 控制；成功后调 onMerged 刷新列表（新记录加入）。
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

  // ── 转字幕（Task 4.1 激活）── invoke generate_subtitle 命令，
  // loading 状态由父 subtitleGeneratingId 控制（同 GIF 模式）。
  // track 留空走后端 Auto 选轨；subtitleResult 已存在时仍可重新生成（覆盖旧结果）。
  const isGeneratingSubtitle = subtitleGeneratingId === rec.id;
  const handleGenerateSubtitle = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (isGeneratingSubtitle) return;
    onGenerateSubtitle(rec.id);
  };

  // ── 字幕面板展开态（Task 4.2）──
  const isSubtitleExpanded = expandedSubtitleId === rec.id;
  const hasSubtitle = !!subtitleResult;

  return (
    <div
      className={cn(
        "group relative border-b border-border/60 transition-colors",
        isSelected ? "bg-accent" : "hover:bg-muted",
        deletePending && "bg-red-50/10 dark:bg-red-950/20",
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
        {/* Title（renaming 时显示 inline input，仿 HotwordPanel）*/}
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
              : // 与 GIF / Merge 按钮对齐：默认可见（opacity-60），避免被当成装饰
                "opacity-60 group-hover:opacity-70 hover:!opacity-100 cursor-pointer",
          )}
          onClick={handleGenerateSubtitle}
          disabled={isGeneratingSubtitle}
          title={
            isGeneratingSubtitle
              ? t("settings.recordings.subtitleGenerating")
              : subtitleResult
                ? t("settings.recordings.transcriptRegenerate")
                : t("settings.recordings.transcript")
          }
        >
          {isGeneratingSubtitle ? (
            <Loader2 className="w-3.5 h-3.5 text-muted-foreground animate-spin" />
          ) : (
            <Captions className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
          )}
        </button>
        {/* 字幕面板展开/收起 toggle：仅在有字幕结果（含空 cues）时显示。
            视觉上与 Captions 按钮成对——Captions 生成，Chevron 预览。
            展开时图标旋转 180° 作状态反馈。 */}
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
                : // 与 favorite 对齐：默认可见（opacity-60），不要像 Play/Reveal 那样隐藏
                  // —— 用户反馈找不到 GIF 导出按钮（之前 opacity-40 太暗被当成装饰）
                  "opacity-60 group-hover:opacity-70 hover:!opacity-100 cursor-pointer",
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

      {/* 行内字幕错误（生成失败留存文案，区别于 toast 一过性）。
          仅在有错误且面板未展开时显示在行底（展开时改由面板内显示更完整上下文）。 */}
      {subtitleError && !isSubtitleExpanded && (
        <div className="px-3 pb-1.5 -mt-1 flex items-start gap-1 text-[10px] text-destructive">
          <Info className="w-3 h-3 mt-px flex-shrink-0" />
          <span className="break-all">{subtitleError}</span>
        </div>
      )}

      {/* ── 字幕预览面板（Task 4.2，展开态）── */}
      {isSubtitleExpanded && subtitleResult && (
        <SubtitlePanel
          result={subtitleResult}
          error={subtitleError}
          onExport={() => onRevealSubtitle(rec.id)}
          onCopyCue={onCopyCue}
          onCopyAll={() => onCopyAll(subtitleResult)}
          t={t}
        />
      )}
      </div>
    </div>
  );
}

// ── 字幕预览面板（Task 4.2）──────────────────────────────────────────
//
// 设计意图（frontend-design）：
// 这不是一个浮层，而是行内的「展开抽屉」——和 RecordingRow 共享背景层（muted），
// 视觉上像行「长出」了一块腹地。三段式纵向布局，每段一个职责：
//
//   ① 顶部 meta 条：cue 计数 + 模型名 + track 来源标签。等宽小字（text-[10px]），
//      与行 meta row 同语汇，保持「精密仪表」气质（不做时间轴可视化——列表节奏不兼容）。
//
//   ② cue 列表：等宽时间戳 + 箭头分隔（00:00 → 00:08），文本跟随。
//      单击复制（hover 露出 copy 图标，符合行内「hover reveal 操作」范式）；
//      复制成功后该行短暂切到 CopyCheck 绿色图标（1.2s）作 micro-feedback。
//      列表自身可滚（max-h-40 + thin-scrollbar），长字幕不撑爆行高。
//
//   ③ 底部操作条：复制全部 / 导出 SRT。两个 ghost 按钮，左对齐，不抢 cue 列表焦点。
//
// fallback 提示（trackUsed !== 'microphone'）放最顶——系统音频/合并轨的 ASR 准确率
// 显著低于麦克风直采，用户需先知道「这段字幕可能不太准」再看内容。用 amber/warning
// 色（非 destructive 红），左竖条 + Info 图标，语气是提醒而非报错。

interface SubtitlePanelProps {
  result: SubtitleResult;
  /** 行内错误文案（生成失败留存）。面板展开时也展示，方便用户看完整上下文。 */
  error?: string;
  onExport: () => void;
  onCopyCue: (cue: SubtitleCue) => void;
  onCopyAll: () => void;
  t: (key: string, params?: Record<string, string | number>) => string;
}

function SubtitlePanel({
  result,
  error,
  onExport,
  onCopyCue,
  onCopyAll,
  t,
}: SubtitlePanelProps) {
  // 最近一次成功复制的 cue 文本（用于行内 CopyCheck 反馈）。1.2s 后清。
  // 用 text 而非 index 做 key——cue 文本相同时合并反馈无伤大雅。
  const [copiedText, setCopiedText] = useState<string | null>(null);
  const copiedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => {
      if (copiedTimer.current) clearTimeout(copiedTimer.current);
    };
  }, []);

  const handleCopyCue = (cue: SubtitleCue) => {
    onCopyCue(cue);
    // 不论 onCopyCue 内部成功失败都先标反馈——失败时 onCopyCue 已 toast，行内反馈短暂亮一下无害。
    setCopiedText(cue.text);
    if (copiedTimer.current) clearTimeout(copiedTimer.current);
    copiedTimer.current = setTimeout(() => setCopiedText(null), 1200);
  };

  const isFallback = result.trackUsed !== "microphone";
  const cueCount = result.cues.length;

  // track 来源标签文案 + 色调：microphone 绿（success），其他 amber（warning，呼应 fallback）。
  const trackLabel =
    result.trackUsed === "microphone"
      ? t("settings.recordings.subtitleTrackMic")
      : result.trackUsed === "system"
        ? t("settings.recordings.subtitleTrackSystem")
        : result.trackUsed === "merged"
          ? t("settings.recordings.subtitleTrackMerged")
          : t("settings.recordings.subtitleTrackUnknown");

  return (
    <div className="px-3 pb-2.5 pt-1 border-t border-border/60 bg-surface/40">
      {/* ① fallback 提示（仅 system/merged/unknown 轨）—— 必须最先看到 */}
      {isFallback && (
        <div className="flex items-start gap-1.5 mb-2 px-2 py-1.5 rounded border-l-2 border-warning bg-warning/10 text-[10px] leading-relaxed text-foreground/80">
          <Info className="w-3 h-3 mt-px flex-shrink-0 text-warning" />
          <span>{t("settings.recordings.subtitleFallbackSystem")}</span>
        </div>
      )}

      {/* ① 行内错误（生成失败但缓存里有旧结果——理论上不应同时存在，兜底展示） */}
      {error && !isFallback && (
        <div className="flex items-start gap-1.5 mb-2 px-2 py-1.5 rounded border-l-2 border-destructive bg-destructive/10 text-[10px] leading-relaxed text-destructive">
          <Info className="w-3 h-3 mt-px flex-shrink-0" />
          <span className="break-all">{error}</span>
        </div>
      )}

      {/* ② meta 条：cue 计数 · 模型 · track 来源 */}
      <div className="flex items-center gap-1.5 mb-1.5 text-[10px] text-muted-foreground flex-wrap">
        <span className="tabular-nums">
          {t("settings.recordings.subtitleCount", { count: cueCount })}
        </span>
        <span className="text-muted-foreground/40">·</span>
        <span className="font-mono-vault text-muted-foreground/80">{result.model}</span>
        <span className="text-muted-foreground/40">·</span>
        <span
          className={cn(
            "px-1.5 py-0.5 rounded font-medium",
            result.trackUsed === "microphone"
              ? "bg-success/10 text-success"
              : "bg-warning/10 text-warning",
          )}
        >
          {trackLabel}
        </span>
      </div>

      {/* ② cue 列表（可滚，单击复制）—— 空 cues 走空状态邀请行动 */}
      {cueCount === 0 ? (
        <div className="py-3 text-center text-[10px] text-muted-foreground">
          {t("settings.recordings.subtitleEmpty")}
        </div>
      ) : (
        <div className="max-h-40 overflow-y-auto thin-scrollbar -mx-1 px-1 space-y-px">
          {result.cues.map((cue, i) => {
            const isCopied = copiedText === cue.text;
            return (
              <button
                key={i}
                onClick={(e) => {
                  e.stopPropagation();
                  handleCopyCue(cue);
                }}
                className={cn(
                  "group/cue w-full flex items-start gap-2 px-1.5 py-1 rounded text-left transition-colors",
                  "hover:bg-accent",
                )}
                title={t("settings.recordings.subtitleCopyCueHint")}
              >
                {/* 等宽时间戳——脚本/字幕编辑器语汇。tabular-nums 保证位对齐。 */}
                <span className="flex-shrink-0 font-mono-vault text-[10px] tabular-nums text-muted-foreground/80 pt-px">
                  <span>{formatMs(cue.startMs)}</span>
                  <span className="mx-0.5 text-muted-foreground/40">→</span>
                  <span>{formatMs(cue.endMs)}</span>
                </span>
                {/* cue 文本 */}
                <span className="flex-1 min-w-0 text-xs leading-relaxed text-foreground/90 break-words">
                  {cue.text}
                </span>
                {/* 复制反馈图标——hover 露出 copy，复制成功切 CopyCheck 绿 */}
                <span className="flex-shrink-0 pt-px">
                  {isCopied ? (
                    <CopyCheck className="w-3 h-3 text-success" />
                  ) : (
                    <Copy className="w-3 h-3 text-muted-foreground/40 opacity-0 group-hover/cue:opacity-100 transition-opacity" />
                  )}
                </span>
              </button>
            );
          })}
        </div>
      )}

      {/* ③ 底部操作条：复制全部 + 导出 SRT（ghost 按钮，左对齐） */}
      {cueCount > 0 && (
        <div className="flex items-center gap-1 mt-2 pt-1.5 border-t border-border/40">
          <button
            onClick={(e) => {
              e.stopPropagation();
              onCopyAll();
            }}
            className="flex items-center gap-1 px-2 py-1 rounded text-[10px] font-medium text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
          >
            <Copy className="w-3 h-3" />
            {t("settings.recordings.subtitleCopyAll")}
          </button>
          <button
            onClick={(e) => {
              e.stopPropagation();
              onExport();
            }}
            className="flex items-center gap-1 px-2 py-1 rounded text-[10px] font-medium text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
          >
            <Download className="w-3 h-3" />
            {t("settings.recordings.subtitleExport")}
          </button>
        </div>
      )}
    </div>
  );
}
