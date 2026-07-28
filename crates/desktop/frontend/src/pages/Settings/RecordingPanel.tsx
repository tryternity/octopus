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
  Sparkles,
  X,
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
  // LLM 润色结果（Phase 4 加，对应后端 polish_outcome: Option<String>）。
  // undefined / "polished" = 正常润色或未润色（无提示）；其余值触发降级提示：
  //   "fallbackRatio" → warning「标记解析失败，已粗略拆分」
  //   "noLlmConfig"   → error「未配置可用 LLM，使用原始识别」
  //   "failed:msg"    → error「LLM 润色失败：msg，使用原始识别」
  polishOutcome?: string;
}

// 字幕生成阶段（与 crates/record/src/subtitle.rs::SubtitleProgress 对齐，外层 kebab-case tag）。
// 用于 record://task 事件的 SubtitleProgress 变体（stage 字段 + 额外 percent/cueCount/message）。
// Phase 4 加 "polishing"（LLM 润色，可选阶段）。
export type SubtitleStage =
  | "extracting-audio"
  | "recognizing"
  | "polishing"
  | "finalizing"
  | "done"
  | "error";

// ── LLM 润色相关类型（Phase 4，与 crates/desktop/src/subtitle_polish.rs 对齐）──
// PolishOption：generate_subtitle 命令的 polish 参数（serde rename_all=camelCase）。
//   null = 不润色；{ llmKey: "openai:gpt-4o" } = 用指定 LLM 润色。
//   llmKey 可为 null（后端用默认 LLM），MVP 前端始终传非空 key。
export interface PolishOption {
  llmKey: string | null;
}

// LlmOption：list_subtitle_llms 命令返回项（serde rename_all=camelCase）。
//   key="openai:gpt-4o"（provider:model），label="gpt-4o (Openai)"。
export interface LlmOption {
  key: string;
  label: string;
}

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

// ── polish_outcome 降级提示（Phase 4，Task 4.2）──
// 根据 SubtitleResult.polishOutcome 显示对应颜色 toast：
//   "fallbackRatio" → warning「LLM 标记解析失败，已粗略拆分」
//   "noLlmConfig"   → error「未配置可用 LLM，使用原始识别」
//   "failed:msg"    → error「LLM 润色失败：msg，使用原始识别」
//   "polished" / undefined → 无提示（正常润色或未润色）。
// 提示只在润色「降级」时出现——让用户知道字幕可能质量打折，但流程仍完成了。
function showPolishOutcomeToast(
  outcome: string | undefined,
  showToast: (msg: string, variant?: ToastVariant) => void,
  t: (key: string, params?: Record<string, string | number>) => string,
) {
  if (!outcome || outcome === "polished") return;
  if (outcome === "fallbackRatio") {
    showToast(t("settings.recordings.subtitlePolishOutcomeFallbackRatio"), "warning");
  } else if (outcome === "noLlmConfig") {
    showToast(t("settings.recordings.subtitlePolishOutcomeNoLlmConfig"), "error");
  } else if (outcome.startsWith("failed:")) {
    const msg = outcome.slice("failed:".length);
    showToast(
      t("settings.recordings.subtitlePolishOutcomeFailed", { msg }),
      "error",
    );
  }
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
  // 删除确认弹框：null=关闭；number=单条删除该 id；'batch'=批量删除选中项。
  const [deleteDialog, setDeleteDialog] = useState<number | "batch" | null>(null);
  // 字幕生成当前阶段（按 recording id 索引，null/undefined=空闲或未知）。
  // 用于行内进度文案：polishing 阶段显示「✨ LLM 润色中...」。其他阶段沿用 spinner。
  const [subtitleStage, setSubtitleStage] = useState<Record<number, SubtitleStage | undefined>>({});
  // ffmpeg 可用性（mount 时探测，决定 GIF 按钮灰禁 + tooltip 引导）。
  // null=探测中（默认 true 可点，避免闪烁），true=可用，false=未找到（灰禁 + tooltip）。
  const [ffmpegAvailable, setFfmpegAvailable] = useState<boolean | null>(null);
  useEffect(() => {
    invoke<boolean>("check_ffmpeg").then(setFfmpegAvailable).catch(() => setFfmpegAvailable(true));
  }, []);

  // ── 字幕 LLM 润色默认配置（Phase 4，Task 4.3）──
  // 持久化到 DB app_config（key=subtitle_llm_polish_default / subtitle_polish_llm_key）。
  // 这些 key 不在 AppConfig struct，get_config 不返回——前端默认值启动（MVP 不回显），
  // 用户在 Settings 改后持久化。弹框默认值从这两个 state 取（与 Settings 同步）。
  const [polishDefault, setPolishDefault] = useState(false);
  const [polishLlmKey, setPolishLlmKey] = useState<string>("");
  // 可用 LLM 列表（弹框 + Settings 下拉填充）。mount 时拉取，空数组兜底（下拉显示占位）。
  const [llmOptions, setLlmOptions] = useState<LlmOption[]>([]);
  useEffect(() => {
    invoke<LlmOption[]>("list_subtitle_llms")
      .then(setLlmOptions)
      .catch(() => setLlmOptions([]));
  }, []);

  // Settings 面板持久化润色默认值（toggle + 下拉）。乐观更新 UI，写 DB 失败仅静默。
  // 这两个 handler 也会被弹框「记住我的选择」复用——弹框确认时同步写默认值（可选，MVP 暂不写）。
  const handlePolishDefaultChange = useCallback((next: boolean) => {
    setPolishDefault(next);
    invoke("set_config", { key: "subtitle_llm_polish_default", value: next }).catch(() => {
      /* 写配置失败仅静默，UI 已切换 */
    });
  }, []);
  const handlePolishLlmKeyChange = useCallback((key: string) => {
    setPolishLlmKey(key);
    invoke("set_config", { key: "subtitle_polish_llm_key", value: key }).catch(() => {
      /* 写配置失败仅静默 */
    });
  }, []);

  // ── 转字幕弹对话框状态（Phase 4，Task 4.1）──
  // 当前弹框关联的 recording id（null=关闭）。一次只对一个 recording 弹框。
  const [polishDialogId, setPolishDialogId] = useState<number | null>(null);
  // 弹框内 checkbox / 下拉的本地选择（确认才生效）。打开弹框时按 polishDefault 初始化。
  const [dialogPolishEnabled, setDialogPolishEnabled] = useState(false);
  const [dialogLlmKey, setDialogLlmKey] = useState<string>("");

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
        setSubtitleStage((prev) => ({ ...prev, [e.id]: undefined }));
      } else if (e.event === "subtitle-progress") {
        // 进度阶段（extracting-audio / recognizing / polishing / finalizing）。
        // 仅用于行内文案：polishing 阶段显示「✨ LLM 润色中...」。
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
        // 清行内错误（如有）。重新拉取该 recording 的字幕（含空 cues 的「正常无字幕」场景）。
        setSubtitleError((prev) => {
          if (!prev[e.id]) return prev;
          const next = { ...prev };
          delete next[e.id];
          return next;
        });
        // read_subtitle 返回 Option<SubtitleResult>：null=未生成。
        // 注意：read_subtitle 的 polish_outcome 恒为 None（v2 不持久化到 srt 文件），
        // 故降级提示只能从 generate_subtitle 的返回值拿（见 onGenerateSubtitle）。
        // done 事件到达时仅刷新缓存 + 显示「字幕已生成」success toast。
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
  //
  // Phase 4：polish 参数（null=不润色；{llmKey}=润色）。润色结果（polishOutcome）从
  // generate_subtitle 的返回值拿——done 事件触发的 read_subtitle 不带 outcome（v2 不持久化），
  // 故降级提示只能在这里发（invoke 成功 resolve 时 result.polishOutcome 是权威值）。
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
        // 乐观更新缓存（done 事件到达前先显示）。空 cues（无声）也存——前端显示「无字幕」。
        setSubtitleResults((prev) => ({ ...prev, [id]: result }));
        setSubtitleError((prev) => {
          if (!prev[id]) return prev;
          const next = { ...prev };
          delete next[id];
          return next;
        });
        // polish_outcome 降级提示（仅润色启用时可能有非 polished 值）。
        // polished / undefined → 无提示（正常）。
        showPolishOutcomeToast(result.polishOutcome, showToast, t);
      } catch (e) {
        // 行内红字 + toast 双通道（与 subtitle-failed 事件回调保持一致）。
        const msg = String(e);
        setSubtitleError((prev) => ({ ...prev, [id]: msg }));
        showToast(t("settings.recordings.subtitleFailed") + ": " + msg, "error");
        // 失败兜底清 loading（事件回调也会清，这里防事件丢失）。
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

  const handleBatchDelete = () => {
    if (selectedIds.size === 0) return;
    setDeleteDialog("batch");
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

          {/* ── 字幕默认润色设置（Phase 4，Task 4.3）──
              与标题区同语汇：muted 卡片 + 10px 标签 + Sparkles 标识润色。
              开关持久化到 subtitle_llm_polish_default，下拉到 subtitle_polish_llm_key。
              下拉仅在开关开启时可点（checkbox off 时灰禁，避免误操作）。 */}
          <SubtitlePolishDefaults
            polishDefault={polishDefault}
            polishLlmKey={polishLlmKey}
            llmOptions={llmOptions}
            onPolishDefaultChange={handlePolishDefaultChange}
            onPolishLlmKeyChange={handlePolishLlmKeyChange}
            t={t}
          />
        </div>

        {/* ── 转字幕润色弹对话框（Phase 4，Task 4.1）──
            点 Captions 按钮触发，用户选润色与否 + LLM，确认才 invoke generate_subtitle。
            overlay + 居中卡片，遵循 SubtitlePanel 的 surface/muted 配色 + 左竖条强调。 */}
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

        {/* ── 字幕预览浮层（v2 改为 overlay，不再行内展开挤压布局）──
            expandedSubtitleId 非 null 且对应字幕已加载时，居中浮层展示 cue 列表。
            点遮罩 / Esc / ChevronDown 再次点击关闭。 */}
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

        {/* ── 删除确认弹框（单条 + 批量共用）──
            deleteDialog=null 关闭；number=单条删该 id；'batch'=批量删选中项。
            checkbox「同时删除磁盘文件」默认勾，勾=permanent:true（删文件+DB），不勾=permanent:false（仅DB）。 */}
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
                setConfirmDelete(false);
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
              onDeleted={handleRowDeleted}
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
              subtitleStage={subtitleStage[rec.id]}
              subtitleResult={subtitleResults[rec.id]}
              subtitleError={subtitleError[rec.id]}
              onRequestPolishDialog={(id) => {
                // 打开弹框：用 Settings 默认值初始化 checkbox + 下拉。
                setDialogPolishEnabled(polishDefault);
                setDialogLlmKey(polishLlmKey || llmOptions[0]?.key || "");
                setPolishDialogId(id);
              }}
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

// ── 单行组件 ──────────────────────────────────────────────────────

interface RecordingRowProps {
  rec: RecordingMeta;
  isSelected: boolean;
  onToggleSelect: () => void;
  showToast: (msg: string, variant?: ToastVariant) => void;
  onDeleted: () => void;
  /** 触发删除确认弹框（单条）。 */
  onRequestDelete: (id: number) => void;
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
  /** 该 recording 当前的字幕生成阶段（polishing 阶段显示「✨ LLM 润色中...」）。 */
  subtitleStage?: SubtitleStage;
  /** 该 recording 已生成的字幕（无则 undefined）。Task 4.2 详情面板消费。 */
  subtitleResult?: SubtitleResult;
  /** 字幕生成失败文案（无则 undefined）。行内红字展示。 */
  subtitleError?: string;
  /** 请求弹转字幕润色对话框（Phase 4：点 Captions 不再直接生成，先弹框确认）。 */
  onRequestPolishDialog: (id: number) => void;
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
  subtitleStage,
  subtitleResult,
  subtitleError,
  onRequestPolishDialog,
  expandedSubtitleId,
  onToggleExpandSubtitle,
  onRevealSubtitle,
  onCopyCue,
  onCopyAll,
}: RecordingRowProps) {
  const t = useT();
  const [favoriteLoading, setFavoriteLoading] = useState(false);
  // 重命名 inline input（WKWebView 不支持 window.prompt，用 inline input 仿 HotwordPanel 范式）
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

  const handleDeleteClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    onRequestDelete(rec.id);
  };

  // ── 转字幕（Task 4.1 激活；Phase 4 改为弹框确认）──
  // 点 Captions 按钮不再直接 invoke，而是弹润色对话框（checkbox + LLM 下拉 + 确认）。
  // 确认后由父 onGenerateSubtitle 调 invoke generate_subtitle（带 polish 参数）。
  // loading 状态由父 subtitleGeneratingId 控制（同 GIF 模式）。
  const isGeneratingSubtitle = subtitleGeneratingId === rec.id;
  const handleGenerateSubtitle = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (isGeneratingSubtitle) return;
    onRequestPolishDialog(rec.id);
  };

  // ── 字幕面板展开态（Task 4.2）──
  const isSubtitleExpanded = expandedSubtitleId === rec.id;
  const hasSubtitle = !!subtitleResult;

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
              ? subtitleStage === "polishing"
                ? t("settings.recordings.subtitlePolishing")
                : t("settings.recordings.subtitleGenerating")
              : subtitleResult
                ? t("settings.recordings.transcriptRegenerate")
                : t("settings.recordings.transcript")
          }
        >
          {isGeneratingSubtitle ? (
            // polishing 阶段用 Sparkles 替代 Loader2——signature 元素，让「润色中」可感知。
            // 其余阶段（extracting/recognizing/finalizing）沿用 spinner。
            subtitleStage === "polishing" ? (
              <Sparkles className="w-3.5 h-3.5 text-warning animate-pulse" />
            ) : (
              <Loader2 className="w-3.5 h-3.5 text-muted-foreground animate-spin" />
            )
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
          className="p-1 rounded transition-all opacity-0 group-hover:opacity-50 hover:!opacity-100 transition-opacity"
          onClick={handleDeleteClick}
          title={t("settings.recordings.delete")}
        >
          <Trash2 className="w-3.5 h-3.5 transition-colors text-muted-foreground hover:text-red-500" />
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

      {/* ── 字幕生成中：polishing 阶段行内进度提示（Phase 4，Task 4.2）──
          沿用 recording 状态 banner 的 chip 语汇（warning 色，amber，呼应润色「增值」语气），
          但更紧凑（10px + Sparkles 脉冲）。仅在 polishing 阶段且面板未展开时显示。 */}
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

// ── 字幕预览面板（Task 4.2）──────────────────────────────────────────
//
// 设计意图（frontend-design）：
// 这是浮层 overlay（v2 改造，原为行内展开抽屉——挤压布局已改浮层）。
// fixed 全屏遮罩 + 居中卡片，不挤压 RecordingRow 布局。Esc / 点遮罩 / X 按钮关闭。
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
  /** 关闭浮层（点遮罩/Esc/关闭按钮触发）。 */
  onClose: () => void;
  t: (key: string, params?: Record<string, string | number>) => string;
}

function SubtitlePanel({
  result,
  error,
  onExport,
  onCopyCue,
  onCopyAll,
  onClose,
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

  // Esc 关闭浮层（与 SubtitlePolishDialog 同模式）。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

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
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      onClick={onClose}
    >
      <div
        className="relative w-full max-w-2xl max-h-[80vh] mx-4 rounded-lg border border-border bg-surface shadow-xl flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题栏：cue 计数 · 模型 · track 来源 · 关闭按钮 */}
        <div className="flex items-center gap-1.5 px-4 py-2.5 border-b border-border/60 text-[10px] text-muted-foreground flex-wrap">
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
          <button
            onClick={onClose}
            className="ml-auto p-1 rounded hover:bg-accent text-muted-foreground hover:text-foreground transition-colors"
            title={t("settings.recordings.subtitleCollapse")}
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>

        {/* 提示区（fallback / error） */}
        {(isFallback || (error && !isFallback)) && (
          <div className="px-4 pt-2">
            {isFallback && (
              <div className="flex items-start gap-1.5 mb-1 px-2 py-1.5 rounded border-l-2 border-warning bg-warning/10 text-[10px] leading-relaxed text-foreground/80">
                <Info className="w-3 h-3 mt-px flex-shrink-0 text-warning" />
                <span>{t("settings.recordings.subtitleFallbackSystem")}</span>
              </div>
            )}
            {error && !isFallback && (
              <div className="flex items-start gap-1.5 mb-1 px-2 py-1.5 rounded border-l-2 border-destructive bg-destructive/10 text-[10px] leading-relaxed text-destructive">
                <Info className="w-3 h-3 mt-px flex-shrink-0" />
                <span className="break-all">{error}</span>
              </div>
            )}
          </div>
        )}

        {/* cue 列表（flex-1 占满中间空间，可滚） */}

      {/* cue 列表（flex-1 占满浮层中间，可滚，单击复制）—— 空 cues 走空状态 */}
      <div className="flex-1 overflow-y-auto thin-scrollbar px-4 py-2">
        {cueCount === 0 ? (
          <div className="py-6 text-center text-[10px] text-muted-foreground">
            {t("settings.recordings.subtitleEmpty")}
          </div>
        ) : (
          <div className="space-y-px">
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
      </div>

      {/* 底部操作条：复制全部 + 在 Finder 显示（ghost 按钮） */}
      {cueCount > 0 && (
        <div className="flex items-center gap-1 px-4 py-2 border-t border-border/40">
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
    </div>
  );
}

// ── 字幕默认润色设置（Phase 4，Task 4.3）──────────────────────────────
//
// 设计意图（frontend-design）：
// 这是 Settings 区的一个「子卡片」，不是独立 panel。沿用标题区的 muted 卡片语汇
// （bg-muted + border-border + rounded-md），左侧 Sparkles 图标点题（润色 = 增值魔法）。
// 布局：单行 checkbox + 内联下拉。checkbox off 时下拉灰禁（视觉强关联——开关关了，
// 选 LLM 无意义）。文案用 10px 标签 + 11px 控件，与列表行 meta 同密度。
//
// 配色克制：开关用 accent-primary（黑），LLM 选项文字用 muted-foreground。
// 不用 warning 色——这是「设置默认值」而非「警示」。warning 色留给 polishing 进度。

interface SubtitlePolishDefaultsProps {
  polishDefault: boolean;
  polishLlmKey: string;
  llmOptions: LlmOption[];
  onPolishDefaultChange: (next: boolean) => void;
  onPolishLlmKeyChange: (key: string) => void;
  t: (key: string, params?: Record<string, string | number>) => string;
}

function SubtitlePolishDefaults({
  polishDefault,
  polishLlmKey,
  llmOptions,
  onPolishDefaultChange,
  onPolishLlmKeyChange,
  t,
}: SubtitlePolishDefaultsProps) {
  return (
    <div className="flex items-center gap-2 px-2.5 py-1.5 bg-muted rounded-md border border-border">
      <Sparkles className="w-3.5 h-3.5 text-muted-foreground flex-shrink-0" />
      <label className="flex items-center gap-1.5 cursor-pointer flex-shrink-0">
        <input
          type="checkbox"
          className="w-3.5 h-3.5 accent-primary"
          checked={polishDefault}
          onChange={(e) => onPolishDefaultChange(e.target.checked)}
        />
        <span className="text-[10px] text-foreground whitespace-nowrap">
          {t("settings.recordings.subtitlePolishDefault")}
        </span>
      </label>
      {/* LLM 下拉：checkbox off 时灰禁（opacity-50 + cursor-not-allowed）。 */}
      <div className="flex items-center gap-1 ml-auto">
        <span
          className={cn(
            "text-[10px] text-muted-foreground whitespace-nowrap",
            !polishDefault && "opacity-50",
          )}
        >
          {t("settings.recordings.subtitlePolishLlm")}
        </span>
        <select
          value={polishLlmKey}
          onChange={(e) => onPolishLlmKeyChange(e.target.value)}
          disabled={!polishDefault}
          className={cn(
            "bg-background border border-border rounded text-[10px] px-1.5 py-0.5 text-foreground outline-none max-w-[140px] truncate",
            "focus:border-primary disabled:cursor-not-allowed disabled:opacity-50",
          )}
        >
          {llmOptions.length === 0 ? (
            <option value="">
              {t("settings.recordings.subtitlePolishNoLlm")}
            </option>
          ) : (
            llmOptions.map((opt) => (
              <option key={opt.key} value={opt.key}>
                {opt.label}
              </option>
            ))
          )}
        </select>
      </div>
    </div>
  );
}

// ── 转字幕润色弹对话框（Phase 4，Task 4.1）──────────────────────────────
//
// 设计意图（frontend-design）：
// 这是确认型弹框，不是表单——目的是「让用户知情地选择是否花一次 LLM 调用润色」。
// 视觉决策：
//   ① overlay 用半透明黑（不是毛玻璃模糊——Tauri WKWebView 毛玻璃开销大且分散注意力），
//      居中卡片用 surface 配色（与 SubtitlePanel 同），左竖条 primary 黑强调「这是决策点」。
//   ② 标题行：Sparkles 图标 + 文案。Sparkles 是这个功能的 signature——它出现在
//      设置卡、弹框标题、polishing 进度三处，构成一条视觉线索。克制使用（不滥用）。
//   ③ 录屏标题用 truncate，避免长标题撑爆窄弹框；下方 10px meta 复用行 meta 语汇。
//   ④ checkbox + 下拉垂直堆叠（不是横排）——避免窄弹框挤压；checkbox off 时下拉灰禁。
//   ⑤ 底部按钮：取消 ghost（左）+ 确认 primary 黑（右）。确认是主动作，视觉更重。
//      润色关闭时确认按钮文案是「生成」（不润色），开启时是「润色并生成」——动作名随状态变。
//
// 配色：warning 色不出现（弹框是中性确认，不是警示）。LLM 选项 disabled 用 opacity。

interface SubtitlePolishDialogProps {
  /** 弹框关联的 recording（找不到时退化为只显示标题文案）。 */
  rec?: RecordingMeta;
  llmOptions: LlmOption[];
  polishEnabled: boolean;
  llmKey: string;
  onPolishEnabledChange: (next: boolean) => void;
  onLlmKeyChange: (key: string) => void;
  onCancel: () => void;
  /** 确认：返回 polish 参数（null=不润色，{llmKey}=润色），由父 invoke generate_subtitle。 */
  onConfirm: (polish: PolishOption | null) => void;
  t: (key: string, params?: Record<string, string | number>) => string;
}

function SubtitlePolishDialog({
  rec,
  llmOptions,
  polishEnabled,
  llmKey,
  onPolishEnabledChange,
  onLlmKeyChange,
  onCancel,
  onConfirm,
  t,
}: SubtitlePolishDialogProps) {
  const title = rec
    ? rec.title || rec.filePath.split("/").pop() || `#${rec.id}`
    : t("settings.recordings.subtitlePolishDialogTitle");
  const durationLabel = rec && rec.durationMs > 0 ? formatDuration(rec.durationMs) : null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      onClick={onCancel}
    >
      <div
        className="w-[340px] max-w-[90vw] bg-surface border border-border rounded-lg shadow-lg overflow-hidden border-l-2 border-l-primary"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题行：Sparkles + 文案 + 关闭 */}
        <div className="flex items-center gap-1.5 px-3 py-2 border-b border-border/60">
          <Sparkles className="w-3.5 h-3.5 text-warning flex-shrink-0" />
          <span className="text-xs font-semibold text-foreground flex-1 min-w-0 truncate">
            {t("settings.recordings.subtitlePolishDialogTitle")}
          </span>
          <button
            onClick={onCancel}
            className="p-0.5 rounded text-muted-foreground hover:text-foreground hover:bg-accent transition-colors flex-shrink-0"
            title={t("settings.recordings.subtitlePolishCancel")}
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>

        {/* 录屏标题 + meta（让用户确认是对哪个录屏操作） */}
        <div className="px-3 py-2 border-b border-border/60">
          <p className="text-xs text-foreground truncate" title={title}>
            {title}
          </p>
          <div className="flex items-center gap-1.5 mt-0.5 text-[10px] text-muted-foreground">
            {durationLabel && (
              <span className="tabular-nums px-1 rounded bg-muted">{durationLabel}</span>
            )}
            {rec && rec.width > 0 && rec.height > 0 && (
              <span className="tabular-nums">
                {rec.width}×{rec.height}
              </span>
            )}
            <span
              className={cn(
                "px-1.5 py-0.5 rounded font-medium",
                rec?.hasMicrophone
                  ? "bg-voice/10 text-voice"
                  : "text-muted-foreground/60",
              )}
            >
              {rec?.sourceType || ""}
            </span>
          </div>
        </div>

        {/* 润色选项：checkbox + 下拉（垂直堆叠） */}
        <div className="px-3 py-2.5 space-y-2">
          <label className="flex items-start gap-2 cursor-pointer">
            <input
              type="checkbox"
              className="w-3.5 h-3.5 mt-0.5 accent-primary flex-shrink-0"
              checked={polishEnabled}
              onChange={(e) => onPolishEnabledChange(e.target.checked)}
            />
            <div className="flex flex-col min-w-0">
              <span className="text-xs text-foreground">
                {t("settings.recordings.subtitlePolishCheckbox")}
              </span>
              <span className="text-[10px] text-muted-foreground leading-relaxed mt-0.5">
                {t("settings.recordings.subtitlePolishHint")}
              </span>
            </div>
          </label>

          {/* LLM 下拉：checkbox off 时灰禁 + 折叠（避免占空间）。
              pl-[22px] 对齐 checkbox 宽度（w-3.5=14px + gap-2=8px），视觉缩进与 checkbox 文字齐。 */}
          {polishEnabled && (
            <div className="flex items-center gap-1.5 pl-[22px]">
              <span className="text-[10px] text-muted-foreground whitespace-nowrap flex-shrink-0">
                {t("settings.recordings.subtitlePolishLlm")}
              </span>
              <select
                value={llmKey}
                onChange={(e) => onLlmKeyChange(e.target.value)}
                disabled={!polishEnabled}
                className="flex-1 min-w-0 bg-background border border-border rounded text-[10px] px-1.5 py-1 text-foreground outline-none focus:border-primary truncate"
              >
                {llmOptions.length === 0 ? (
                  <option value="">
                    {t("settings.recordings.subtitlePolishNoLlm")}
                  </option>
                ) : (
                  llmOptions.map((opt) => (
                    <option key={opt.key} value={opt.key}>
                      {opt.label}
                    </option>
                  ))
                )}
              </select>
            </div>
          )}
        </div>

        {/* 底部按钮：取消（ghost）+ 确认（primary）。动作名随润色开关变。 */}
        <div className="flex items-center justify-end gap-1.5 px-3 py-2 border-t border-border/60 bg-muted/40">
          <button
            onClick={onCancel}
            className="px-2.5 py-1 rounded text-[10px] font-medium text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
          >
            {t("settings.recordings.subtitlePolishCancel")}
          </button>
          <button
            onClick={() =>
              onConfirm(polishEnabled ? { llmKey: llmKey || null } : null)
            }
            className="flex items-center gap-1 px-2.5 py-1 rounded text-[10px] font-medium bg-primary text-primary-foreground hover:bg-primary/90 transition-colors"
          >
            {polishEnabled && <Sparkles className="w-3 h-3" />}
            {polishEnabled
              ? t("settings.recordings.subtitlePolishConfirm")
              : t("settings.recordings.subtitlePolishConfirmPlain")}
          </button>
        </div>
      </div>
    </div>
  );
}

// ── 删除确认弹框（单条 + 批量共用）──────────────────────────────────────
//
// 设计意图（frontend-design）：
// 与 SubtitlePolishDialog 同模式（fixed overlay + 居中卡片），但语义是「破坏性操作确认」
// ——用 destructive 色调（红）强调不可逆。核心交互：checkbox「同时删除磁盘文件」默认勾，
// 因为用户删录屏通常想连文件一起清掉（你反馈「逻辑删除堆积数据没地方清理」）。
//
// 不勾时仅删 DB 行（permanent:false），磁盘文件保留（下次启动 cleanup 孤儿清理会删）。
// 勾时删 DB 行 + 磁盘 mp4 + 关联 .N.srt 字幕文件（permanent:true）。
interface DeleteConfirmDialogProps {
  /** 删除目标描述（单条用文件名，批量用「N 个录屏」）。 */
  targetLabel: string;
  /** 删除数量（影响文案：单数/复数）。 */
  count: number;
  onCancel: () => void;
  onConfirm: (permanent: boolean) => void;
  t: (key: string, params?: Record<string, string | number>) => string;
}

function DeleteConfirmDialog({
  targetLabel,
  count,
  onCancel,
  onConfirm,
  t,
}: DeleteConfirmDialogProps) {
  const [deleteFile, setDeleteFile] = useState(true);
  const isBatch = count > 1;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      onClick={onCancel}
    >
      <div
        className="relative w-full max-w-sm mx-4 rounded-lg border border-border bg-surface shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题区：destructive 色调 + Trash2 图标 */}
        <div className="flex items-center gap-2 px-3 py-2.5 border-b border-border/60">
          <div className="flex items-center gap-1.5 text-destructive">
            <Trash2 className="w-3.5 h-3.5" />
            <span className="text-xs font-medium">
              {t("settings.recordings.delete")}
            </span>
          </div>
        </div>

        {/* 内容区：目标 + checkbox */}
        <div className="px-3 py-3 space-y-2.5">
          <p className="text-[11px] text-foreground/80 break-words">
            {isBatch
              ? t("settings.recordings.confirmDeleteN", { n: count })
              : t("settings.recordings.deleteConfirm")}
            {!isBatch && (
              <span className="block mt-0.5 font-mono-vault text-[10px] text-muted-foreground break-all">
                {targetLabel}
              </span>
            )}
          </p>
          <label className="flex items-start gap-2 cursor-pointer group">
            <input
              type="checkbox"
              className="mt-0.5 w-3.5 h-3.5 accent-destructive flex-shrink-0"
              checked={deleteFile}
              onChange={(e) => setDeleteFile(e.target.checked)}
            />
            <span className="text-[11px] leading-relaxed text-foreground/80 group-hover:text-foreground transition-colors">
              {t("settings.recordings.deleteAlsoFile")}
            </span>
          </label>
        </div>

        {/* 底部按钮：取消 + 删除（destructive） */}
        <div className="flex items-center justify-end gap-1.5 px-3 py-2 border-t border-border/60 bg-muted/40">
          <button
            onClick={onCancel}
            className="px-2.5 py-1 rounded text-[10px] font-medium text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
          >
            {t("settings.recordings.subtitlePolishCancel")}
          </button>
          <button
            onClick={() => onConfirm(deleteFile)}
            className="px-2.5 py-1 rounded text-[10px] font-medium bg-destructive text-white hover:bg-destructive/90 transition-colors"
          >
            {t("settings.recordings.delete")}
          </button>
        </div>
      </div>
    </div>
  );
}
