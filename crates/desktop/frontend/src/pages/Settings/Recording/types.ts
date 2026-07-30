// 后端类型镜像（crates/record/src/store.rs::RecordingMeta）。
// 从 RecordingPanel.tsx 拆出（2026-07-30）。

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
export interface MergeResult {
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
  // undefined / "polished" = 正常润色或未润色（无提示）；其余值触发降级提示。
  polishOutcome?: string;
}

// 字幕生成阶段（与 crates/record/src/subtitle.rs::SubtitleProgress 对齐，外层 kebab-case tag）。
export type SubtitleStage =
  | "extracting-audio"
  | "recognizing"
  | "polishing"
  | "finalizing"
  | "done"
  | "error";

// ── LLM 润色相关类型（Phase 4，与 crates/desktop/src/subtitle_polish.rs 对齐）──
export interface PolishOption {
  llmKey: string | null;
}

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
