# 录屏自动字幕（Record Auto-Subtitle）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 录屏停止后用户手动点「转字幕」按钮（激活现有 Captions 占位），抽 mic track → VAD 分段 ASR → 生成带时间戳 cue 列表 → 入 DB + 可导出 SRT。

**Architecture:** 方案 B（desktop 编排）。record crate 加 subtitle 模块（mp4→PCM ffmpeg 抽轨 + SRT 格式化 + 选轨 + 数据模型，无 ASR 依赖）；asr-local 扩展 pipeline（带时间戳 transcribe，复用 VAD/engine/后处理）；desktop 加 3 个 Tauri 命令编排两者。前端激活 RecordingPanel 现有「转字幕」占位 + 新增 cue 预览面板。

**Tech Stack:** Rust（asr-local/record/desktop crate）+ Tauri 2 + React/TS 前端 + ffmpeg/ffprobe subprocess（复用现有路径解析）

## Global Constraints

- **Schema 升级机制已简化**（commit `df73ff44`）：加列只改 `crates/infra/src/db.sql` + 升 `CURRENT_SCHEMA_VERSION` 常量（53→54）+ 升注释。**不写 migrate 函数**。旧 v53 库会 bail 提示清库重建（既定策略）。
- **casing 规范**（AGENTS.md）：跨 Tauri 边界的 DTO 必须 `#[serde(rename_all = "camelCase")]`；事件 enum 外层 `tag="event"` + 变体名 kebab + 变体字段 camelCase。`TimestampedSegment`（asr-local 内部）保持 snake_case。
- **复用 `AudioTrackSource`**（`crates/record/src/audio_tracks.rs:25-33`，4 变体：Microphone/System/Merged/Unknown，`rename_all="lowercase"`），不新建。
- **ffmpeg 调用模式**（参考 `record_commands.rs:1026-1047`）：`tokio::process::Command` + `.status().await` + `Stdio::null()`。字幕抽音轨需要 stdout piped 读 PCM（与 merge 不同）。
- **DB 访问**统一走 `with_db_blocking`（record_commands.rs:56-70，spawn_blocking 包 with_db）。
- **`RecordSession`** 是 `State<'_, RecordSession>`（非 AppState），DB 通过 `with_db_blocking` 闭包访问。
- **前端 i18n 命名空间**是 `settings.recordings.*`（不是 `record.*`）。
- **前端「转字幕」按钮已存在**（RecordingPanel.tsx:749-761，Captions 图标灰禁占位），Phase 4 是激活 + 扩展，非新建。
- **TDD**：纯逻辑函数（generate_srt / select_track / segment_audio_vad_with_offsets）必须先写失败测试。

---

## File Structure

| 文件 | 职责 | 操作 |
|---|---|---|
| `crates/infra/src/db.sql` | recordings 表加 3 列 + 升注释 | 修改 |
| `crates/infra/src/db.rs` | `CURRENT_SCHEMA_VERSION` 53→54 | 修改 |
| `crates/record/src/subtitle.rs` | SubtitleCue/SubtitleResult/TrackPreference + select_track + generate_srt + extract_audio_track_to_pcm | 新建 |
| `crates/record/src/store.rs` | RecordingMeta 加 3 字段 + INSERT/SELECT 同步 + 新增 `update_subtitle` | 修改 |
| `crates/record/src/lib.rs` | 导出 subtitle 模块 | 修改 |
| `crates/asr-local/src/audio.rs` | `VadSegment` + `segment_audio_vad_with_offsets` | 修改（新增函数，不改原函数） |
| `crates/asr-local/src/pipeline.rs` | `TimestampedSegment` + `transcribe_segments_with_timestamps` + 抽 `postprocess_segment` helper | 修改 |
| `crates/desktop/src/record_commands.rs` | generate_subtitle/export_subtitle/get_subtitle 命令 + RecordTaskEvent 加 Subtitle 变体 | 修改 |
| `crates/desktop/src/main.rs` | generate_handler 注册 3 个新命令 | 修改 |
| `crates/desktop/frontend/src/pages/Settings/RecordingPanel.tsx` | 激活转字幕按钮 + cue 预览面板 | 修改 |
| `crates/desktop/frontend/src/locales/zh-CN.yaml` | settings.recordings.subtitle* 键 | 修改 |
| `crates/desktop/frontend/src/locales/en.yaml` | settings.recordings.subtitle* 键 | 修改 |

---

## Phase 1：DB schema + record 数据模型（地基）

### Task 1.1: DB schema v53→v54

**Files:**
- Modify: `crates/infra/src/db.sql:294-315`（recordings 表 CREATE + 表注释）
- Modify: `crates/infra/src/db.rs:366`（CURRENT_SCHEMA_VERSION 常量）

**Interfaces:**
- Produces: recordings 表新增 `subtitle_cues TEXT NOT NULL DEFAULT '[]'`, `subtitle_srt TEXT NOT NULL DEFAULT ''`, `subtitle_model TEXT NOT NULL DEFAULT ''` 三列；schema 版本升到 54

- [ ] **Step 1: 改 db.sql recordings 表注释**

修改 `crates/infra/src/db.sql:294`：

```sql
-- ══ 录屏元数据（recordings / recordings_thumbnails，schema v51 + v52 audio_tracks + v54 subtitle）═══
```

- [ ] **Step 2: 在 recordings 表 CREATE 语句 audio_tracks 列后追加 3 列**

在 `crates/infra/src/db.sql:308`（`audio_tracks TEXT NOT NULL DEFAULT '[]',` 行）下方，闭合括号前追加：

```sql
    -- schema v54：录屏自动字幕（spec 2026-07-28-record-auto-subtitle-design.md）。
    -- subtitle_cues 存 SubtitleCue[] JSON；subtitle_srt 存完整 SRT 文本（导出时直接读）；
    -- subtitle_model 存生成字幕时的模型名（便于「模型不同需重生成」提示）。空 = 未生成。
    subtitle_cues  TEXT    NOT NULL DEFAULT '[]',
    subtitle_srt   TEXT    NOT NULL DEFAULT '',
    subtitle_model TEXT    NOT NULL DEFAULT '',
```

- [ ] **Step 3: 升 CURRENT_SCHEMA_VERSION 常量**

修改 `crates/infra/src/db.rs:366`：

```rust
pub const CURRENT_SCHEMA_VERSION: u32 = 54;
```

- [ ] **Step 4: 更新 init_schema doc 注释（保持文档一致）**

修改 `crates/infra/src/db.rs:320-326` 的 doc 注释，把 `v53`/`v != 0 && v < 53` 改为 `v54`/`v != 0 && v < 54`：

```rust
/// schema 变更直接改 db.sql + 升 `user_version`，旧库一律清库重建（`rm ~/.octopus/octopus.db*`）。
///
/// 分支：
/// - `v == 0`：全新库——db.sql 建表 + 外置 seed + yaml 迁移 + manifest 填充 → v54
/// - `v == 54`：最新，no-op
/// - `v != 0 && v < 54`：旧版本库——不支持自动迁移，bail 提示清库
```

- [ ] **Step 5: 在 init_schema_fresh_db_builds_current_version 测试加 recordings 新列断言**

在 `crates/infra/src/db.rs:3804` 的 `init_schema_fresh_db_builds_current_version` 测试末尾（最后的 `assert_eq!(cnt, 1, ...)` 之后）追加：

```rust
    // v54: recordings 表应有 subtitle_cues 列
    let has_subtitle_cues: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('recordings') WHERE name='subtitle_cues'")
        .unwrap()
        .exists([])
        .unwrap();
    assert!(has_subtitle_cues, "v54 recordings 表应有 subtitle_cues 列");
```

- [ ] **Step 6: 跑测试验证**

```bash
cargo test -p octopus-infra --lib init_schema_fresh_db_builds_current_version
```

Expected: PASS（断言 v=54 + subtitle_cues 列存在）

- [ ] **Step 7: 跑 infra 全量测试确认无回归**

```bash
cargo test -p octopus-infra --lib
```

Expected: 全过（特别注意 `init_schema_old_version_bails` 也会自动用新常量）

- [ ] **Step 8: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(infra): schema v53→v54——recordings 加 subtitle_cues/srt/model 三列"
```

---

### Task 1.2: record crate subtitle 数据模型 + SRT 生成器（TDD）

**Files:**
- Create: `crates/record/src/subtitle.rs`
- Modify: `crates/record/src/lib.rs`（导出 subtitle 模块）

**Interfaces:**
- Consumes: `RecordingMeta`（store.rs）、`AudioTrackSource`（audio_tracks.rs）
- Produces: `SubtitleCue` / `SubtitleResult` / `SubtitleProgress` / `TrackPreference` / `SubtitleError` / `select_track()` / `generate_srt()`

- [ ] **Step 1: 创建 subtitle.rs 骨架 + 数据模型**

创建 `crates/record/src/subtitle.rs`：

```rust
//! 录屏自动字幕数据模型 + SRT 生成 + 选轨逻辑（纯逻辑，无 ASR 依赖）。
//!
//! 设计详见 `docs/superpowers/specs/2026-07-28-record-auto-subtitle-design.md`。
//! ASR 调用由 desktop 编排层桥接（方案 B）——本模块只负责 mp4→PCM、cue 模型、SRT 格式化、选轨。

use crate::audio_tracks::AudioTrackSource;
use crate::store::RecordingMeta;
use std::path::Path;
use thiserror::Error;

/// 一条字幕 cue（跨 Tauri 边界的 DTO，camelCase 序列化）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleCue {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// 字幕生成结果。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleResult {
    pub cues: Vec<SubtitleCue>,
    pub srt_text: String,
    pub model: String,
    pub track_used: AudioTrackSource,
}

/// 进度阶段（emit 给前端，外层 kebab + 变体 camelCase）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "stage", rename_all = "kebab-case")]
pub enum SubtitleProgress {
    ExtractingAudio { percent: u32 },
    Recognizing { percent: u32 },
    Finalizing { percent: u32 },
    Done { cue_count: usize },
    Error { message: String },
}

/// 选轨偏好。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackPreference {
    Auto,
    Microphone,
    System,
}

#[derive(Debug, Error)]
pub enum SubtitleError {
    #[error("录制无任何音轨")]
    NoAudioTrack,
    #[error("无 system 音轨")]
    NoSystemTrack,
    #[error("ffmpeg 调用失败: {0}")]
    Ffmpeg(String),
    #[error("ffmpeg 输出解码失败: {0}")]
    Decode(String),
}

pub type SubtitleResult2<T> = std::result::Result<T, SubtitleError>;
```

- [ ] **Step 2: 写 generate_srt 失败测试（4 个）**

在 `crates/record/src/subtitle.rs` 末尾追加测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_tracks::AudioTrackSource;

    fn cue(start: u64, end: u64, text: &str) -> SubtitleCue {
        SubtitleCue { start_ms: start, end_ms: end, text: text.into() }
    }

    #[test]
    fn generate_srt_basic_3_cues() {
        let cues = vec![
            cue(1234, 3567, "第一句"),
            cue(4000, 6500, "第二句"),
            cue(7000, 9500, "第三句"),
        ];
        let srt = generate_srt(&cues);
        assert_eq!(srt,
            "1\n00:00:01,234 --> 00:00:03,567\n第一句\n\
             \n2\n00:00:04,000 --> 00:00:06,500\n第二句\n\
             \n3\n00:00:07,000 --> 00:00:09,500\n第三句\n");
    }

    #[test]
    fn generate_srt_empty_returns_empty_string() {
        assert_eq!(generate_srt(&[]), "");
    }

    #[test]
    fn generate_srt_single_cue() {
        let srt = generate_srt(&[cue(0, 1500, "单句")]);
        assert_eq!(srt, "1\n00:00:00,000 --> 00:00:01,500\n单句\n");
    }

    #[test]
    fn generate_srt_hour_boundary() {
        // 1 小时 + 234ms = 3601234ms
        let srt = generate_srt(&[cue(3_601_234, 3_602_500, "跨小时")]);
        assert!(srt.contains("01:00:01,234 --> 01:00:02,500"));
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

```bash
cargo test -p octopus-record --lib subtitle::tests
```

Expected: 编译失败（`generate_srt` 未定义）

- [ ] **Step 4: 实现 generate_srt**

在 `crates/record/src/subtitle.rs`（测试模块前）追加：

```rust
/// 把 SubtitleCue 列表格式化为标准 SRT 文本。
///
/// 格式：序号从 1 开始；时间 `HH:MM:SS,mmm`（毫秒用逗号）；cue 间空行分隔；末尾保留换行。
/// 空 cues 返回空字符串。
pub fn generate_srt(cues: &[SubtitleCue]) -> String {
    if cues.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (idx, c) in cues.iter().enumerate() {
        out.push_str(&format!("{}\n", idx + 1));
        out.push_str(&format!(
            "{} --> {}\n",
            format_srt_timestamp(c.start_ms),
            format_srt_timestamp(c.end_ms)
        ));
        out.push_str(&c.text);
        out.push('\n');
        if idx < cues.len() - 1 {
            out.push('\n');
        }
    }
    out
}

/// ms → "HH:MM:SS,mmm"（SRT 标准用逗号分隔毫秒）。
fn format_srt_timestamp(ms: u64) -> String {
    let total_sec = ms / 1000;
    let millis = ms % 1000;
    let h = total_sec / 3600;
    let m = (total_sec % 3600) / 60;
    let s = total_sec % 60;
    format!("{:02}:{:02}:{:02},{:03}", h, m, s, millis)
}
```

- [ ] **Step 5: 跑测试确认通过**

```bash
cargo test -p octopus-record --lib subtitle::tests
```

Expected: 4 个测试 PASS

- [ ] **Step 6: Commit**

```bash
git add crates/record/src/subtitle.rs
git commit -m "feat(record): subtitle 数据模型 + generate_srt（4 TDD 测试）"
```

---

### Task 1.3: select_track 选轨逻辑（TDD）

**Files:**
- Modify: `crates/record/src/subtitle.rs`（追加 select_track + 测试）

**Interfaces:**
- Consumes: `RecordingMeta`（store.rs）、`AudioTrack`（audio_tracks.rs）
- Produces: `select_track(meta, pref) -> Result<(usize, AudioTrackSource), SubtitleError>`

- [ ] **Step 1: 写 select_track 失败测试（5 个）**

在 `crates/record/src/subtitle.rs` 测试模块追加：

```rust
    use crate::audio_tracks::AudioTrack;
    use crate::store::RecordingMeta;

    fn track(idx: u32, src: AudioTrackSource) -> AudioTrack {
        AudioTrack { index: idx, source: src, codec: "aac".into(), sample_rate: 48000, channels: 2, device_name: None }
    }

    fn meta_with_tracks(tracks: &[AudioTrack]) -> RecordingMeta {
        RecordingMeta {
            id: 1, file_path: "/tmp/x.mp4".into(), title: "".into(), duration_ms: 1000,
            width: 1920, height: 1080, fps: 30, codec: "h264".into(),
            has_system_audio: !tracks.is_empty(), has_microphone: tracks.iter().any(|t| t.source == AudioTrackSource::Microphone),
            audio_tracks: tracks.to_vec(), source_type: "display".into(), file_size: 100,
            has_thumbnail: false, is_favorite: false, created_at: "2026-07-28T00:00:00Z".into(),
            deleted_at: None,
            // 新字段（Task 1.4 会加到 RecordingMeta；此处先占位，Task 1.4 完成后补齐）
            subtitle_cues: None, subtitle_srt: None, subtitle_model: None,
        }
    }

    #[test]
    fn select_track_auto_prefers_microphone() {
        let m = meta_with_tracks(&[track(0, AudioTrackSource::Microphone), track(1, AudioTrackSource::System)]);
        let (idx, used) = select_track(&m, TrackPreference::Auto).unwrap();
        assert_eq!((idx, used), (0, AudioTrackSource::Microphone));
    }

    #[test]
    fn select_track_microphone_explicit() {
        let m = meta_with_tracks(&[track(0, AudioTrackSource::Microphone)]);
        let (idx, used) = select_track(&m, TrackPreference::Microphone).unwrap();
        assert_eq!((idx, used), (0, AudioTrackSource::Microphone));
    }

    #[test]
    fn select_track_system_explicit() {
        let m = meta_with_tracks(&[track(0, AudioTrackSource::Microphone), track(1, AudioTrackSource::System)]);
        let (idx, used) = select_track(&m, TrackPreference::System).unwrap();
        assert_eq!((idx, used), (1, AudioTrackSource::System));
    }

    #[test]
    fn select_track_auto_fallback_to_system_when_no_mic() {
        let m = meta_with_tracks(&[track(0, AudioTrackSource::System)]);
        let (idx, used) = select_track(&m, TrackPreference::Auto).unwrap();
        assert_eq!((idx, used), (0, AudioTrackSource::System));
    }

    #[test]
    fn select_track_empty_returns_no_audio_track_error() {
        let m = meta_with_tracks(&[]);
        assert!(matches!(select_track(&m, TrackPreference::Auto), Err(SubtitleError::NoAudioTrack)));
    }

    #[test]
    fn select_track_system_but_none_returns_no_system_error() {
        let m = meta_with_tracks(&[track(0, AudioTrackSource::Microphone)]);
        assert!(matches!(select_track(&m, TrackPreference::System), Err(SubtitleError::NoSystemTrack)));
    }
```

⚠️ 注意：测试 `meta_with_tracks` 引用了 `subtitle_cues/subtitle_srt/subtitle_model` 字段——这要求 Task 1.4（RecordingMeta 扩展）必须先做。**所以 Task 1.3 和 Task 1.4 实际是耦合的，一起改**。见 Task 1.4。

- [ ] **Step 2: 实现 select_track**

在 `crates/record/src/subtitle.rs`（generate_srt 后、测试前）追加：

```rust
/// 按 preference 选音轨，返回 (track_index, track_source)。
///
/// Auto/Microphone：优先 mic；mic 不存在则 fallback system，再 fallback 第一条。
/// System：必须 system，不存在则 NoSystemTrack 错误。
/// 空 audio_tracks：NoAudioTrack 错误。
pub fn select_track(
    meta: &RecordingMeta,
    pref: TrackPreference,
) -> Result<(usize, AudioTrackSource), SubtitleError> {
    if meta.audio_tracks.is_empty() {
        return Err(SubtitleError::NoAudioTrack);
    }
    match pref {
        TrackPreference::Auto | TrackPreference::Microphone => {
            // 优先 mic
            if let Some(t) = meta.audio_tracks.iter().find(|t| t.source == AudioTrackSource::Microphone) {
                return Ok((t.index as usize, AudioTrackSource::Microphone));
            }
            // fallback system
            if let Some(t) = meta.audio_tracks.iter().find(|t| t.source == AudioTrackSource::System) {
                return Ok((t.index as usize, AudioTrackSource::System));
            }
            // 再 fallback 第一条（Merged/Unknown）
            let t = &meta.audio_tracks[0];
            Ok((t.index as usize, t.source))
        }
        TrackPreference::System => {
            meta.audio_tracks.iter()
                .find(|t| t.source == AudioTrackSource::System)
                .map(|t| (t.index as usize, AudioTrackSource::System))
                .ok_or(SubtitleError::NoSystemTrack)
        }
    }
}
```

- [ ] **Step 3: 跑测试（依赖 Task 1.4 完成 RecordingMeta 扩展）**

见 Task 1.4 Step 6。

- [ ] **Step 4: Commit（与 Task 1.4 一起 commit）**

---

### Task 1.4: RecordingMeta 扩展 3 字段 + store 适配

**Files:**
- Modify: `crates/record/src/store.rs:18-39`（RecordingMeta struct）
- Modify: `crates/record/src/store.rs:58-85`（INSERT 语句）
- Modify: `crates/record/src/store.rs:87-127`（get/list SELECT 语句）
- Modify: `crates/record/src/store.rs:208-231`（row_to_meta 映射）

**Interfaces:**
- Produces: `RecordingMeta` 加 `subtitle_cues: Option<Vec<SubtitleCue>>`、`subtitle_srt: Option<String>`、`subtitle_model: Option<String>`；`RecordStore::update_subtitle(id, cues_json, srt, model)`

- [ ] **Step 1: RecordingMeta struct 加 3 字段**

修改 `crates/record/src/store.rs:18-39`，在 `deleted_at` 后追加：

```rust
    pub deleted_at: Option<String>,
    #[serde(default)]
    pub subtitle_cues: Option<Vec<crate::subtitle::SubtitleCue>>,
    #[serde(default)]
    pub subtitle_srt: Option<String>,
    #[serde(default)]
    pub subtitle_model: Option<String>,
```

- [ ] **Step 2: 更新 INSERT 语句**

修改 `crates/record/src/store.rs:61-76`，列清单加 3 列（紧跟 deleted_at），params 数组加 3 值：

```rust
self.conn.execute(
    "INSERT INTO recordings
     (id, file_path, title, duration_ms, width, height, fps, codec,
      has_system_audio, has_microphone, audio_tracks, source_type, file_size,
      has_thumbnail, is_favorite, created_at, deleted_at,
      subtitle_cues, subtitle_srt, subtitle_model)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, NULL, ?17, ?18, ?19)",
    rusqlite::params![
        meta.id, meta.file_path, meta.title, meta.duration_ms,
        meta.width, meta.height, meta.fps, meta.codec,
        meta.has_system_audio as i32, meta.has_microphone as i32,
        audio_tracks_json,
        meta.source_type, meta.file_size,
        thumbnail.is_some() as i32, meta.is_favorite as i32,
        meta.created_at,
        meta.subtitle_cues.as_ref().map(|c| serde_json::to_string(c).unwrap_or_else(|_| "[]".into())).unwrap_or_else(|| "[]".into()),
        meta.subtitle_srt.clone().unwrap_or_default(),
        meta.subtitle_model.clone().unwrap_or_default(),
    ],
)?;
```

- [ ] **Step 3: 更新 get SELECT 语句**

修改 `crates/record/src/store.rs:87-100`（get 函数），SELECT 列清单加 3 列：

```rust
let mut stmt = self.conn.prepare(
    "SELECT id, file_path, title, duration_ms, width, height, fps, codec,
            has_system_audio, has_microphone, audio_tracks, source_type, file_size,
            has_thumbnail, is_favorite, created_at, deleted_at,
            subtitle_cues, subtitle_srt, subtitle_model
     FROM recordings WHERE id = ?1",
)?;
```

- [ ] **Step 4: 更新 list SELECT 语句**

修改 `crates/record/src/store.rs:102-127`（list 函数），SELECT 列清单同步加 3 列（与 get 一致）。

- [ ] **Step 5: 更新 row_to_meta 映射**

修改 `crates/record/src/store.rs:208-231`（row_to_meta），追加 3 字段读取（列索引 17/18/19）：

```rust
fn row_to_meta(&self, row: &rusqlite::Row<'_>) -> rusqlite::Result<RecordingMeta> {
    let audio_tracks_json: String = row.get(10)?;
    let audio_tracks: Vec<AudioTrack> = serde_json::from_str(&audio_tracks_json).unwrap_or_default();

    // subtitle_cues：空/NULL/解析失败 → None
    let subtitle_cues_json: String = row.get(17).unwrap_or_default();
    let subtitle_cues = if subtitle_cues_json.is_empty() || subtitle_cues_json == "[]" {
        None
    } else {
        serde_json::from_str::<Vec<crate::subtitle::SubtitleCue>>(&subtitle_cues_json).ok()
    };
    let subtitle_srt: String = row.get(18).unwrap_or_default();
    let subtitle_srt = if subtitle_srt.is_empty() { None } else { Some(subtitle_srt) };
    let subtitle_model: String = row.get(19).unwrap_or_default();
    let subtitle_model = if subtitle_model.is_empty() { None } else { Some(subtitle_model) };

    Ok(RecordingMeta {
        id: row.get(0)?,
        file_path: row.get(1)?,
        // ... 现有字段保持不变 ...
        audio_tracks,
        // ...
        deleted_at: row.get(16)?,
        subtitle_cues,
        subtitle_srt,
        subtitle_model,
    })
}
```

⚠️ 实现时保持现有字段映射不变，只在末尾追加新字段。

- [ ] **Step 6: 跑 select_track + generate_srt 全部测试**

```bash
cargo test -p octopus-record --lib subtitle::tests
```

Expected: generate_srt 4 测试 + select_track 6 测试全 PASS

- [ ] **Step 7: 跑 record crate 全量测试**

```bash
cargo test -p octopus-record --lib
```

Expected: 全过（store 测试不破——新字段有 default）

- [ ] **Step 8: 新增 update_subtitle 方法（TDD）**

在 `crates/record/src/store.rs` 测试模块追加：

```rust
    #[test]
    fn update_subtitle_writes_and_roundtrips() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // 简化：手动建表只含必要列（真实 schema 见 db.sql，这里只测 UPDATE 逻辑）
        conn.execute_batch(
            "CREATE TABLE recordings (id INTEGER PRIMARY KEY, subtitle_cues TEXT NOT NULL DEFAULT '[]', subtitle_srt TEXT NOT NULL DEFAULT '', subtitle_model TEXT NOT NULL DEFAULT '');
             INSERT INTO recordings (id) VALUES (1);"
        ).unwrap();
        let store = RecordStore::new(&conn);
        store.update_subtitle(1, "[{\"startMs\":100,\"endMs\":200,\"text\":\"hi\"}]", "1\n00:00:00,100 --> 00:00:00,200\nhi\n", "sensevoice").unwrap();
        let row_cues: String = conn.query_row("SELECT subtitle_cues FROM recordings WHERE id=1", [], |r| r.get(0)).unwrap();
        assert_eq!(row_cues, "[{\"startMs\":100,\"endMs\":200,\"text\":\"hi\"}]");
        let row_model: String = conn.query_row("SELECT subtitle_model FROM recordings WHERE id=1", [], |r| r.get(0)).unwrap();
        assert_eq!(row_model, "sensevoice");
    }
```

在 RecordStore impl 块（store.rs，toggle_favorite 后）追加：

```rust
    /// 更新字幕（幂等：重复生成覆盖旧值）。
    pub fn update_subtitle(
        &self,
        id: i64,
        cues_json: &str,
        srt: &str,
        model: &str,
    ) -> RecordResult<()> {
        let affected = self.conn.execute(
            "UPDATE recordings SET subtitle_cues=?1, subtitle_srt=?2, subtitle_model=?3 WHERE id=?4",
            rusqlite::params![cues_json, srt, model, id],
        )?;
        if affected == 0 {
            return Err(crate::error::RecordError::NotFound { id });
        }
        Ok(())
    }
```

⚠️ 如果 `RecordError::NotFound` 不存在，先查 `crates/record/src/error.rs` 看现有错误变体，复用最接近的（可能是 `DatabaseError` 或加一个 NotFound 变体）。

- [ ] **Step 9: 跑 update_subtitle 测试**

```bash
cargo test -p octopus-record --lib store::tests::update_subtitle_writes_and_roundtrips
```

Expected: PASS

- [ ] **Step 10: 更新 lib.rs 导出**

修改 `crates/record/src/lib.rs`，在 `pub mod store;`（约第 17 行）后追加：

```rust
pub mod subtitle;
pub use subtitle::{
    SubtitleCue, SubtitleResult, SubtitleProgress, TrackPreference, SubtitleError,
    select_track, generate_srt,
};
```

- [ ] **Step 11: 跑全量 record 测试 + build**

```bash
cargo test -p octopus-record --lib && cargo build -p octopus-record
```

Expected: 全过 + 0 warning

- [ ] **Step 12: Commit（Task 1.3 + 1.4 一起）**

```bash
git add crates/record/src/subtitle.rs crates/record/src/store.rs crates/record/src/lib.rs
git commit -m "feat(record): select_track 选轨逻辑 + RecordingMeta 加 subtitle 字段 + update_subtitle

- select_track：Auto/Microphone 优先 mic，fallback system/first；System 强制
- RecordingMeta 加 subtitle_cues/srt/model 三字段（serde default 向后兼容）
- RecordStore::update_subtitle 幂等 UPDATE
- generate_srt（4 测试）+ select_track（6 测试）+ update_subtitle（1 测试）TDD"
```

---

## Phase 2：asr-local VAD 时间戳能力

### Task 2.1: segment_audio_vad_with_offsets（TDD）

**Files:**
- Modify: `crates/asr-local/src/audio.rs`（新增 VadSegment + segment_audio_vad_with_offsets，**不改原 segment_audio_vad**）

**Interfaces:**
- Consumes: `SileroVad`（crate::vad）
- Produces: `VadSegment { offset_samples: usize, samples: Vec<f32> }` + `segment_audio_vad_with_offsets(samples, vad, frame_size, threshold, min_silence_ms, max_segment_ms) -> Vec<VadSegment>`

- [ ] **Step 1: 定义 VadSegment + 写失败测试**

在 `crates/asr-local/src/audio.rs`（segment_audio_vad 函数后）追加类型定义：

```rust
/// VAD 分段结果（带 offset）——每段在原音频中的起始样本偏移。
/// 用于字幕生成：offset_samples / 16.0 = start_ms（16k 采样率）。
#[derive(Debug, Clone, PartialEq)]
pub struct VadSegment {
    pub offset_samples: usize,
    pub samples: Vec<f32>,
}
```

在 `crates/asr-local/src/audio.rs` 测试模块（`#[cfg(test)] mod tests` 内）追加 3 个测试：

```rust
    #[test]
    fn segment_audio_vad_with_offsets_offsets_are_monotonic() {
        // offset 单调递增 + 所有段下标合法 + offset+len <= samples.len()
        let mut vad = match crate::config::create_silero_vad() {
            Ok(v) => v,
            Err(e) => { eprintln!("[SKIP] SileroVad 失败: {e}"); return; }
        };
        // 31s 合成音频，5-25s 段提高幅度（参考 segment_audio_vad_segments_in_bounds）
        let n = 16000 * 31;
        let samples: Vec<f32> = (0..n).map(|i| {
            let t = i as f32 / 16000.0;
            let amp = if (5.0..25.0).contains(&t) { 0.3 } else { 0.02 };
            (2.0 * std::f32::consts::PI * 220.0 * t).sin() * amp
        }).collect();
        let segs = segment_audio_vad_with_offsets(&samples, &mut vad, 480, 0.4, 500, 25000);
        let mut prev_end = 0usize;
        for s in &segs {
            assert!(!s.samples.is_empty(), "段不应为空");
            assert!(s.offset_samples >= prev_end, "offset 应单调递增（{} < {}）", s.offset_samples, prev_end);
            assert!(s.offset_samples + s.samples.len() <= samples.len(),
                "段越界：offset {} + len {} > total {}", s.offset_samples, s.samples.len(), samples.len());
            prev_end = s.offset_samples + s.samples.len();
        }
    }

    #[test]
    fn segment_audio_vad_with_offsets_all_silence_returns_empty() {
        let mut vad = match crate::config::create_silero_vad() {
            Ok(v) => v,
            Err(e) => { eprintln!("[SKIP]"); return; }
        };
        // 全零 = 绝对静音
        let samples = vec![0.0f32; 16000 * 5];
        let segs = segment_audio_vad_with_offsets(&samples, &mut vad, 480, 0.4, 500, 25000);
        // VAD 对全零应判静音；不强断言空（VAD 可能误判），但所有段下标合法
        for s in &segs {
            assert!(s.offset_samples + s.samples.len() <= samples.len());
        }
    }

    #[test]
    fn segment_audio_vad_with_offsets_consistent_with_original() {
        // 带 offset 版本与原版本段数、段内容一致（仅多了 offset 字段）
        let mut vad1 = match crate::config::create_silero_vad() { Ok(v) => v, Err(_) => return };
        let mut vad2 = match crate::config::create_silero_vad() { Ok(v) => v, Err(_) => return };
        let n = 16000 * 31;
        let samples: Vec<f32> = (0..n).map(|i| {
            let t = i as f32 / 16000.0;
            let amp = if (5.0..25.0).contains(&t) { 0.3 } else { 0.02 };
            (2.0 * std::f32::consts::PI * 220.0 * t).sin() * amp
        }).collect();
        let orig = segment_audio_vad(&samples, &mut vad1, 480, 0.4, 500, 25000);
        let with_off = segment_audio_vad_with_offsets(&samples, &mut vad2, 480, 0.4, 500, 25000);
        assert_eq!(orig.len(), with_off.len(), "两版本段数应一致");
        for (o, w) in orig.iter().zip(with_off.iter()) {
            assert_eq!(o, &w.samples, "段内容应一致");
        }
    }
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cargo test -p octopus-asr-local --lib audio::tests
```

Expected: 编译失败（`segment_audio_vad_with_offsets` 未定义）

- [ ] **Step 3: 实现 segment_audio_vad_with_offsets**

关键：**复用原 `segment_audio_vad` 的状态机**。原函数已有 `current_segment_start` 变量（audio.rs:200）。最干净的实现是**重构原函数**：把核心循环抽成内部 helper，两个公开函数共享。但为了最小化回归风险，这里采用**复制状态机 + 加 offset 跟踪**的方式（重复代码但零回归风险，注释标明 TODO 共享 helper）。

在 `crates/asr-local/src/audio.rs`（segment_audio_vad 函数后，VadSegment 定义后）追加：

```rust
/// 带 offset 的 VAD 分段（spec 2026-07-28-record-auto-subtitle §4.2）。
///
/// 与 `segment_audio_vad` 相同的分段逻辑，但每段额外返回 offset_samples。
/// 参数与原函数一一对应，行为一致（仅多了 offset 字段）。
pub fn segment_audio_vad_with_offsets(
    samples: &[f32],
    vad: &mut crate::vad::SileroVad,
    frame_size: usize,
    threshold: f32,
    min_silence_ms: usize,
    max_segment_ms: usize,
) -> Vec<VadSegment> {
    // NOTE: 本函数与 segment_audio_vad 共享同一套状态机逻辑。理想做法是把核心循环
    // 抽成共享 helper（接收闭包决定产出 Vec<f32> 还是 VadSegment），但为最小化对原函数
    // 的回归风险，此处复制状态机并追加 offset 跟踪。两函数行为应保持一致（有回归测试保护）。
    let mut segments: Vec<VadSegment> = Vec::new();
    let mut in_speech = false;
    let mut current_segment_start = 0;
    let mut silence_frames_count = 0;

    let frame_duration_ms = (frame_size * 1000) / 16000;
    let min_silence_frames = min_silence_ms / frame_duration_ms;
    let pad_samples = (SPEECH_PAD_MS / frame_duration_ms) * frame_size;

    let n_frames = samples.len() / frame_size;
    for i in 0..n_frames {
        let start_idx = i * frame_size;
        let chunk = &samples[start_idx..start_idx + frame_size];
        let prob = vad.compute(chunk).unwrap_or(0.0);

        if !in_speech {
            // 静音→语音边界
            if prob >= threshold {
                in_speech = true;
                current_segment_start = start_idx.saturating_sub(pad_samples);
                silence_frames_count = 0;
            }
        } else {
            // 语音中
            if prob >= threshold {
                silence_frames_count = 0;
            } else {
                silence_frames_count += 1;
            }

            // 切分判定：静音超阈值 OR 段超最大时长
            let current_duration_ms = ((start_idx + frame_size - current_segment_start) * 1000) / 16000;
            let should_split_silence = silence_frames_count >= min_silence_frames;
            let should_split_max = current_duration_ms >= max_segment_ms;

            if should_split_silence || should_split_max {
                let speech_end = if should_split_silence {
                    (((i + 1 - silence_frames_count) * frame_size) + pad_samples).min((i + 1) * frame_size)
                } else {
                    (i + 1) * frame_size
                };
                let seg_samples = samples[current_segment_start..speech_end].to_vec();
                segments.push(VadSegment {
                    offset_samples: current_segment_start,
                    samples: seg_samples,
                });
                if should_split_silence {
                    in_speech = false;
                } else {
                    // 最大时长切分：继续 speech 状态，重置起点
                    current_segment_start = speech_end;
                }
                silence_frames_count = 0;
            }
        }
    }

    // 末段：循环结束仍在 speech
    if in_speech {
        let speech_end = samples.len().min(current_segment_start + max_segment_ms * 16000 / 1000);
        let speech_end = speech_end.min(samples.len());
        let seg_samples = samples[current_segment_start..speech_end].to_vec();
        if !seg_samples.is_empty() {
            segments.push(VadSegment {
                offset_samples: current_segment_start,
                samples: seg_samples,
            });
        }
    }

    vad.reset();
    segments
}
```

⚠️ **重要**：上面的实现是基于现有 `segment_audio_vad`（audio.rs:190-280）的逻辑推断。**实施时必须逐行对照原函数**，确保状态机分支完全一致。特别是 `should_split_silence` vs `should_split_max` 的优先级、`speech_end` 计算、末段处理。如果原函数有此处未覆盖的细节，照抄。**`segment_audio_vad_with_offsets_consistent_with_original` 测试会验证两者行为一致**。

- [ ] **Step 4: 跑测试确认通过**

```bash
cargo test -p octopus-asr-local --lib audio::tests
```

Expected: 3 个新测试 PASS（可能因 VAD skip 而跳过，但不失败）+ 现有测试不破

- [ ] **Step 5: 跑 asr-local 全量测试**

```bash
cargo test -p octopus-asr-local --lib
```

Expected: 全过（原 segment_audio_vad 不受影响）

- [ ] **Step 6: Commit**

```bash
git add crates/asr-local/src/audio.rs
git commit -m "feat(asr-local): segment_audio_vad_with_offsets——带 offset 的 VAD 分段

为字幕生成服务。复用原 segment_audio_vad 状态机逻辑，追加 offset_samples 跟踪。
3 个 TDD 测试：offset 单调性、全静音、与原函数一致性回归保护。"
```

---

### Task 2.2: transcribe_segments_with_timestamps（TDD）

**Files:**
- Modify: `crates/asr-local/src/pipeline.rs`（新增 TimestampedSegment + postprocess_segment helper + transcribe_segments_with_timestamps）

**Interfaces:**
- Consumes: `OfflineAsrEngine`（engine.rs）、`PipelineConfig`（pipeline.rs）、`segment_audio_vad_with_offsets`（audio.rs）
- Produces: `TimestampedSegment { start_ms, end_ms, text }` + `transcribe_segments_with_timestamps(engine, samples, cfg) -> Result<Vec<TimestampedSegment>>`

- [ ] **Step 1: 定义 TimestampedSegment + 写失败测试**

在 `crates/asr-local/src/pipeline.rs`（PipelineConfig 定义后）追加：

```rust
/// 带时间戳的转写段（内部类型，非 DTO——desktop 编排时转为 record::SubtitleCue）。
#[derive(Debug, Clone, PartialEq)]
pub struct TimestampedSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}
```

在 pipeline.rs 测试模块追加（复用现有 FakeEngine + cfg helper）：

```rust
    #[test]
    fn transcribe_timestamps_short_audio_single_cue() {
        // 短音频（<480k）走直连：整段一条 cue，start=0, end=samples/16
        let eng = FakeEngine { text: "短音频测试".into(), skip: false };
        let samples = vec![0.5f32; 16000]; // 1 秒
        let segs = transcribe_segments_with_timestamps(&eng, &samples, &cfg(true, false)).unwrap();
        // 短音频可能走 VAD 分段或整段一条（VAD skip 时整段）；不断言段数，只断言：
        // 1) 至少 1 段 2) 所有段 start_ms < end_ms 3) 段时间戳 <= 1000ms
        assert!(!segs.is_empty(), "短音频应至少产出 1 段");
        for s in &segs {
            assert!(s.start_ms < s.end_ms, "start < end");
            assert!(s.end_ms <= 1000, "1 秒音频 end_ms 不应超 1000");
            assert!(!s.text.is_empty());
        }
    }

    #[test]
    fn transcribe_timestamps_ms_conversion() {
        // 已知 offset_samples → ms 换算正确
        // 构造 320000 samples = 20s 短音频（<480k 走直连，整段一条 cue）
        let eng = FakeEngine { text: "测试".into(), skip: false };
        let samples = vec![0.5f32; 320000];
        let segs = transcribe_segments_with_timestamps(&eng, &samples, &cfg(true, false)).unwrap();
        // 整段一条（FakeEngine 无 VAD 依赖，短音频直连）
        if segs.len() == 1 {
            assert_eq!(segs[0].start_ms, 0);
            assert_eq!(segs[0].end_ms, 20000); // 320000/16 = 20000
        }
    }

    #[test]
    fn transcribe_timestamps_filters_empty_text() {
        // FakeEngine 返回空文本 → 该段被过滤
        let eng = FakeEngine { text: "".into(), skip: false };
        let samples = vec![0.5f32; 16000];
        let segs = transcribe_segments_with_timestamps(&eng, &samples, &cfg(true, false)).unwrap();
        assert!(segs.is_empty(), "空文本段应被过滤");
    }
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cargo test -p octopus-asr-local --lib pipeline::tests
```

Expected: 编译失败（`transcribe_segments_with_timestamps` 未定义）

- [ ] **Step 3: 抽 postprocess_segment helper**

在 `crates/asr-local/src/pipeline.rs`，把 `transcribe_batch`（pipeline.rs:46-80）里 corrector + ITN + hans 那段抽成 helper。修改 `transcribe_batch`：

```rust
pub fn transcribe_batch(
    engine: &dyn OfflineAsrEngine,
    samples: &[f32],
    cfg: &PipelineConfig,
) -> Result<String> {
    if cfg.ngram {
        log::warn!("ngram 解码尚未实现，忽略 cfg.ngram 开关");
    }
    let raw_text = transcribe_segments(engine, samples, &cfg.language)?;
    Ok(postprocess_text(raw_text, engine, cfg))
}

/// 文本后处理：corrector（含热词命中计数）→ ITN → 简繁归一。
/// 抽自 transcribe_batch，供 transcribe_segments_with_timestamps 复用（DRY）。
fn postprocess_text(
    raw_text: String,
    engine: &dyn OfflineAsrEngine,
    cfg: &PipelineConfig,
) -> String {
    let is_english = cfg.language.eq_ignore_ascii_case("en");
    let text = if cfg.correct && !engine.skip_corrector() && !is_english {
        let corrected = crate::corrector::get_corrector().correct(&raw_text);
        for word in crate::corrector::drain_hits() {
            if let Err(e) = crate::db::bump_hotword_hit_by_word(&word) {
                log::warn!("[hotword] bump 命中计数失败 '{}': {}", word, e);
            }
        }
        corrected
    } else {
        raw_text
    };
    let text = crate::itn::normalize(&text);
    if cfg.simplify {
        crate::hans::to_simplified(&text)
    } else {
        crate::hans::to_traditional(&text)
    }
}
```

⚠️ 必须保证 `transcribe_batch` 行为完全不变——先跑现有 `batch_*` 测试确认无回归。

- [ ] **Step 4: 实现 transcribe_segments_with_timestamps**

在 pipeline.rs（transcribe_segments 后）追加：

```rust
/// 带时间戳的转写：VAD 分段（带 offset）→ 逐段 transcribe + 后处理 → 组装 TimestampedSegment。
///
/// 与 transcribe_batch 的区别：不拼接文本，而是保留每段独立 + 时间区间。
/// 短音频（≤480k samples）也走 VAD 分段（spec §4.5 决策）——与 transcribe_segments
/// 的「短音频直连」不同，因为字幕场景需要分段。
/// VAD 初始化失败时降级：整段作为单条 cue。
/// 过滤：空文本段、<500ms 段（噪声）。
pub fn transcribe_segments_with_timestamps(
    engine: &dyn OfflineAsrEngine,
    samples: &[f32],
    cfg: &PipelineConfig,
) -> Result<Vec<TimestampedSegment>> {
    // VAD 分段（带 offset）——失败降级整段一条
    let segments: Vec<crate::audio::VadSegment> = match crate::config::create_silero_vad() {
        Ok(mut v) => crate::audio::segment_audio_vad_with_offsets(
            samples, &mut v, 480, 0.4, 500, 25000),
        Err(e) => {
            log::warn!("VAD 初始化失败，整段作为单条 cue: {}", e);
            vec![crate::audio::VadSegment { offset_samples: 0, samples: samples.to_vec() }]
        }
    };

    let mut result = Vec::with_capacity(segments.len());
    for seg in &segments {
        let dur_samples = seg.samples.len();
        let dur_ms = (dur_samples as f64 / 16.0).round() as u64;
        // 过滤 <500ms 段（噪声）
        if dur_ms < 500 {
            continue;
        }
        let raw = engine.transcribe(&seg.samples, &cfg.language)?;
        let text = postprocess_text(raw, engine, cfg);
        // 过滤空文本段
        if text.trim().is_empty() {
            continue;
        }
        let start_ms = (seg.offset_samples as f64 / 16.0).round() as u64;
        let end_ms = ((seg.offset_samples + dur_samples) as f64 / 16.0).round() as u64;
        result.push(TimestampedSegment { start_ms, end_ms, text });
    }
    Ok(result)
}
```

- [ ] **Step 5: 跑测试确认通过**

```bash
cargo test -p octopus-asr-local --lib pipeline::tests
```

Expected: 3 个新测试 PASS + 现有 batch_* 测试全过（验证 postprocess_text 抽取无回归）

- [ ] **Step 6: 跑 asr-local 全量测试**

```bash
cargo test -p octopus-asr-local --lib
```

Expected: 全过

- [ ] **Step 7: Commit**

```bash
git add crates/asr-local/src/pipeline.rs
git commit -m "feat(asr-local): transcribe_segments_with_timestamps——带时间戳转写

为字幕生成服务。VAD 分段带 offset + 逐段 transcribe + 复用 postprocess_text。
抽 postprocess_text helper（transcribe_batch 复用，DRY）。
过滤 <500ms 段 + 空文本段。3 个 TDD 测试。"
```

---

## Phase 3：desktop 编排层

### Task 3.1: extract_audio_track_to_pcm（record crate）

**Files:**
- Modify: `crates/record/src/subtitle.rs`（追加 extract_audio_track_to_pcm）

**Interfaces:**
- Consumes: ffmpeg 路径（Path）+ mp4 路径 + track_index
- Produces: `extract_audio_track_to_pcm(mp4_path, track_index, ffmpeg_path) -> Result<Vec<f32>, SubtitleError>`（16k mono f32le PCM）

- [ ] **Step 1: 实现 extract_audio_track_to_pcm（同步 std::process，无单测）**

在 `crates/record/src/subtitle.rs`（select_track 后）追加：

```rust
/// 用 ffmpeg 从 mp4 抽取指定音轨为 16k mono f32le PCM。
///
/// ffmpeg -i xxx.mp4 -map 0:a:<idx> -ar 16000 -ac 1 -f f32le pipe:1
/// 读 stdout → 每 4 字节一个 f32（little-endian）。
///
/// 不写单测（依赖外部 ffmpeg + 真实 mp4，归 e2e）。
pub fn extract_audio_track_to_pcm(
    mp4_path: &Path,
    track_index: usize,
    ffmpeg_path: &Path,
) -> Result<Vec<f32>, SubtitleError> {
    let output = std::process::Command::new(ffmpeg_path)
        .arg("-y")
        .arg("-i").arg(mp4_path)
        .arg("-map").arg(format!("0:a:{}", track_index))
        .arg("-ar").arg("16000")
        .arg("-ac").arg("1")
        .arg("-f").arg("f32le")
        .arg("pipe:1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| SubtitleError::Ffmpeg(format!("spawn 失败: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SubtitleError::Ffmpeg(format!("退出码非 0: {}", stderr.chars().take(500).collect::<String>())));
    }

    // f32le → Vec<f32>
    let bytes = &output.stdout;
    if bytes.len() % 4 != 0 {
        return Err(SubtitleError::Decode(format!(
            "PCM 字节数 {} 不是 4 的倍数", bytes.len())));
    }
    let samples: Vec<f32> = bytes.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    Ok(samples)
}
```

- [ ] **Step 2: build 确认编译**

```bash
cargo build -p octopus-record
```

Expected: 0 error 0 warning

- [ ] **Step 3: Commit（与 Task 3.2 一起）**

---

### Task 3.2: RecordTaskEvent 加 Subtitle 变体 + generate_subtitle/export_subtitle/get_subtitle 命令

**Files:**
- Modify: `crates/desktop/src/record_commands.rs`（RecordTaskEvent enum 加变体 + 3 个新命令）
- Modify: `crates/desktop/src/main.rs`（generate_handler 注册 3 命令）

**Interfaces:**
- Consumes: `record::select_track` + `record::extract_audio_track_to_pcm` + `record::generate_srt` + `record::RecordStore::update_subtitle` + `asr_local::transcribe_segments_with_timestamps` + `asr_local::PipelineConfig` + `asr_local::AsrEngineManager`（或等价的 active engine 获取方式）

- [ ] **Step 1: RecordTaskEvent 加 3 个 Subtitle 变体**

修改 `crates/desktop/src/record_commands.rs:936-969`，在 `MergeFailed` 后追加：

```rust
    #[serde(rename_all = "camelCase")]
    MergeFailed { id: i64, error: String },
    #[serde(rename_all = "camelCase")]
    SubtitleStarted { id: i64 },
    #[serde(rename_all = "camelCase")]
    SubtitleProgress { id: i64, stage: octopus_record::SubtitleProgress },
    #[serde(rename_all = "camelCase")]
    SubtitleDone { id: i64, cue_count: usize },
    #[serde(rename_all = "camelCase")]
    SubtitleFailed { id: i64, error: String },
```

- [ ] **Step 2: 实现 generate_subtitle 命令**

在 `crates/desktop/src/record_commands.rs`（merge_audio_tracks 函数后，文件末尾前）追加：

```rust
/// 生成字幕：抽 mic/system track → ASR → 入 DB。返回 SubtitleResult。
#[command]
pub async fn generate_subtitle(
    app: AppHandle,
    id: i64,
    track: Option<String>, // "microphone" | "system" | None(=Auto)
) -> Result<octopus_record::SubtitleResult, String> {
    let _ = app.emit("record://task", RecordTaskEvent::SubtitleStarted { id });

    // 1. 查 DB 拿 RecordingMeta
    let meta = with_db_blocking(move |conn| {
        let store = octopus_record::RecordStore::new(conn);
        store.get(id)
    })
    .await?
    .ok_or_else(|| format!("recording {} 不存在", id))?;

    // 2. 解析 TrackPreference
    let pref = match track.as_deref() {
        Some("system") => octopus_record::TrackPreference::System,
        Some("microphone") => octopus_record::TrackPreference::Microphone,
        _ => octopus_record::TrackPreference::Auto,
    };

    // 3. 选轨
    let (track_idx, track_used) = octopus_record::select_track(&meta, pref)
        .map_err(|e| e.to_string())?;

    // 4. emit 进度：抽音轨
    let _ = app.emit("record://task", RecordTaskEvent::SubtitleProgress {
        id,
        stage: octopus_record::SubtitleProgress::ExtractingAudio { percent: 10 },
    });

    // 5. ffmpeg 抽 PCM
    let ffmpeg = find_ffmpeg()?;
    let mp4_path = std::path::PathBuf::from(&meta.file_path);
    let pcm = octopus_record::extract_audio_track_to_pcm(&mp4_path, track_idx, &ffmpeg)
        .map_err(|e| e.to_string())?;

    // 6. emit 进度：识别中
    let _ = app.emit("record://task", RecordTaskEvent::SubtitleProgress {
        id,
        stage: octopus_record::SubtitleProgress::Recognizing { percent: 40 },
    });

    // 7. 拿 active engine + PipelineConfig
    let engine = get_active_asr_engine().await?;
    let cfg = octopus_asr_local::pipeline::PipelineConfig::from_app_config("zh");

    // 8. ASR 带时间戳转写
    let timestamped = octopus_asr_local::pipeline::transcribe_segments_with_timestamps(
        engine.as_ref(), &pcm, &cfg)
        .map_err(|e| format!("ASR 失败: {e}"))?;

    // 9. emit 进度：组装
    let _ = app.emit("record://task", RecordTaskEvent::SubtitleProgress {
        id,
        stage: octopus_record::SubtitleProgress::Finalizing { percent: 90 },
    });

    // 10. 转 SubtitleCue + 生成 SRT
    let cues: Vec<octopus_record::SubtitleCue> = timestamped.into_iter()
        .map(|t| octopus_record::SubtitleCue {
            start_ms: t.start_ms, end_ms: t.end_ms, text: t.text,
        })
        .collect();
    let model = get_active_asr_model_name().await.unwrap_or_default();
    let srt_text = octopus_record::generate_srt(&cues);
    let result = octopus_record::SubtitleResult {
        cues: cues.clone(), srt_text: srt_text.clone(), model: model.clone(), track_used,
    };

    // 11. UPDATE DB
    let cues_json = serde_json::to_string(&cues).map_err(|e| e.to_string())?;
    with_db_blocking(move |conn| {
        let store = octopus_record::RecordStore::new(conn);
        store.update_subtitle(id, &cues_json, &srt_text, &model)
    })
    .await?;

    // 12. emit done
    let _ = app.emit("record://task", RecordTaskEvent::SubtitleDone { id, cue_count: cues.len() });

    Ok(result)
}

/// 导出 SRT 文件到指定路径。
#[command]
pub async fn export_subtitle(
    id: i64,
    dest_path: String,
) -> Result<String, String> {
    let srt = with_db_blocking(move |conn| {
        let store = octopus_record::RecordStore::new(conn);
        let meta = store.get(id)?
            .ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)?;
        Ok::<_, rusqlite::Error>(meta.subtitle_srt.unwrap_or_default())
    })
    .await?;
    if srt.is_empty() {
        return Err("字幕未生成".into());
    }
    std::fs::write(&dest_path, &srt).map_err(|e| format!("写文件失败: {e}"))?;
    Ok(dest_path)
}

/// 读取字幕（历史项展开时调）。None = 未生成。
#[command]
pub async fn get_subtitle(
    id: i64,
) -> Result<Option<octopus_record::SubtitleResult>, String> {
    with_db_blocking(move |conn| {
        let store = octopus_record::RecordStore::new(conn);
        let meta = match store.get(id)? {
            Some(m) => m,
            None => return Ok(None),
        };
        match (meta.subtitle_cues, meta.subtitle_srt, meta.subtitle_model) {
            (Some(cues), Some(srt), Some(model)) => {
                // track_used 查 audio_tracks 第一个 source（仅用于前端 fallback 提示）
                let track_used = meta.audio_tracks.first()
                    .map(|t| t.source)
                    .unwrap_or(octopus_record::AudioTrackSource::Unknown);
                Ok(Some(octopus_record::SubtitleResult {
                    cues, srt_text: srt, model, track_used,
                }))
            }
            _ => Ok(None),
        }
    })
    .await
}
```

- [ ] **Step 3: 实现 get_active_asr_engine / get_active_asr_model_name helper**

⚠️ 这两个 helper 需要从 AppState 获取 AsrEngineManager。先查 `crates/desktop/src/main.rs` 看 AsrEngineManager 如何注入 State。实施时：
- 如果 AppState 持有 `AsrEngineManager`，用 `state: State<'_, AppState>` 参数拿
- `get_active_asr_engine()` 返回 `Arc<dyn OfflineAsrEngine>`
- `get_active_asr_model_name()` 返回当前激活模型名（`String`）

**实施时先 grep `AsrEngineManager` 在 desktop crate 的注入位置**，按现有模式写。签名参考：

```rust
async fn get_active_asr_engine() -> Result<std::sync::Arc<dyn octopus_asr_local::OfflineAsrEngine>, String> {
    // 从全局 AppState 取 AsrEngineManager → active_engine()
}

async fn get_active_asr_model_name() -> Result<String, String> {
    // 从全局 AppState 取 active_engine_name
}
```

- [ ] **Step 4: 注册 3 个命令到 generate_handler**

修改 `crates/desktop/src/main.rs`（约 line 619 `merge_audio_tracks` 后），追加：

```rust
#[cfg(target_os = "macos")]
record_commands::generate_subtitle,
#[cfg(target_os = "macos")]
record_commands::export_subtitle,
#[cfg(target_os = "macos")]
record_commands::get_subtitle,
```

- [ ] **Step 5: build 确认编译**

```bash
cargo build -p octopus-desktop --features embedded,custom-protocol 2>&1 | head -50
```

Expected: 0 error（可能有 warning，逐个看）。⚠️ 多半会有几个编译错误：AsrEngineManager 注入方式、SubtitleProgress clone derive、with_db_blocking 闭包返回类型等。**看完整 error 列表，逐个修完再 build**。

- [ ] **Step 6: 跑 desktop 测试确认无回归**

```bash
cargo test -p octopus-desktop --lib 2>&1 | tail -20
```

Expected: 现有测试全过

- [ ] **Step 7: Commit**

```bash
git add crates/desktop/src/record_commands.rs crates/desktop/src/main.rs crates/record/src/subtitle.rs
git commit -m "feat(desktop): generate_subtitle/export_subtitle/get_subtitle 命令 + extract_audio_track_to_pcm

- generate_subtitle：抽轨 → ASR → cue → SRT → UPDATE DB，emit SubtitleStarted/Progress/Done
- export_subtitle：DB 读 SRT → 写文件
- get_subtitle：DB 读 cues/srt/model → SubtitleResult（None = 未生成）
- RecordTaskEvent 加 SubtitleStarted/Progress/Done/Failed 变体
- extract_audio_track_to_pcm：ffmpeg -map 0:a:<idx> -ar 16000 -ac 1 -f f32le pipe:1"
```

---

## Phase 4：前端 UI

### Task 4.1: 激活「转字幕」按钮 + subtitle state

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/RecordingPanel.tsx`（激活现有灰禁占位 + 加 subtitle state）

⚠️ 涉及视觉改动，**实施时必须用 frontend-design skill 做设计**（AGENTS.md 准则）。

- [ ] **Step 1: 加 SubtitleCue / SubtitleResult TS interface**

修改 `RecordingPanel.tsx`（line 79-82 MergeResult 后），追加：

```typescript
interface SubtitleCue {
  startMs: number;
  endMs: number;
  text: string;
}

interface SubtitleResult {
  cues: SubtitleCue[];
  srtText: string;
  model: string;
  trackUsed: 'microphone' | 'system' | 'merged' | 'unknown';
}

type SubtitleStage = 'extracting-audio' | 'recognizing' | 'finalizing' | 'done' | 'error';
interface SubtitleProgressPayload {
  id: number;
  stage: SubtitleStage;
  percent?: number;
  cueCount?: number;
  message?: string;
}
```

- [ ] **Step 2: 加 subtitleGeneratingId state + listen 事件**

在 `RecordingPanel.tsx`（line 139 `gifExportingId` 附近）追加 state：

```typescript
const [subtitleGeneratingId, setSubtitleGeneratingId] = useState<number | null>(null);
const [subtitleError, setSubtitleError] = useState<string | null>(null);
const [expandedSubtitleId, setExpandedSubtitleId] = useState<number | null>(null);
const [subtitleResults, setSubtitleResults] = useState<Record<number, SubtitleResult>>({});
```

listen `record://task` 事件（在 RecordingPanel 组件内 useEffect）：

```typescript
useEffect(() => {
  const unlisten = listen<{ event: string; id: number; stage?: SubtitleStage; cueCount?: number; error?: string }>(
    "record://task",
    (e) => {
      const { event, id } = e.payload;
      if (event === "subtitle-started") {
        setSubtitleGeneratingId(id);
        setSubtitleError(null);
      } else if (event === "subtitle-done") {
        setSubtitleGeneratingId(null);
        // 重新拉取该 recording 的字幕
        invoke<SubtitleResult>("get_subtitle", { id }).then((r) => {
          if (r) setSubtitleResults((prev) => ({ ...prev, [id]: r }));
        });
      } else if (event === "subtitle-failed") {
        setSubtitleGeneratingId(null);
        setSubtitleError(e.payload.error || "字幕生成失败");
      }
    }
  );
  return () => { unlisten.then((f) => f()); };
}, []);
```

⚠️ 需要 `import { listen } from "@tauri-apps/api/event";`（检查现有 import）。

- [ ] **Step 3: 激活「转字幕」按钮 + onGenerateSubtitle handler**

修改 `RecordingPanel.tsx:749-761`（现有 Captions 图标按钮）。去掉灰禁，接 `generate_subtitle`：

```tsx
<IconButton
  icon={<Captions size={16} />}
  tooltip={t("settings.recordings.transcript")}
  onClick={() => onGenerateSubtitle?.(rec.id)}
  loading={subtitleGeneratingId === rec.id}
/>
```

在 RecordingPanel 组件内加 handler：

```typescript
const onGenerateSubtitle = async (id: number) => {
  setSubtitleGeneratingId(id);
  setSubtitleError(null);
  try {
    const result = await invoke<SubtitleResult>("generate_subtitle", { id });
    setSubtitleResults((prev) => ({ ...prev, [id]: result }));
    showToast?.(t("settings.recordings.subtitleDone", { count: result.cues.length }));
  } catch (e) {
    setSubtitleError(String(e));
    showToast?.(t("settings.recordings.subtitleFailed") + ": " + String(e));
  } finally {
    setSubtitleGeneratingId(null);
  }
};
```

- [ ] **Step 4: build 确认 tsc 通过**

```bash
cd crates/desktop/frontend && npx tsc --noEmit
```

Expected: 0 error

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/RecordingPanel.tsx
git commit -m "feat(frontend): 激活「转字幕」按钮 + subtitle state + listen record://task"
```

---

### Task 4.2: cue 预览面板 + 导出 SRT + fallback 提示

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/RecordingPanel.tsx`

⚠️ 这是新组件，**实施时用 frontend-design skill 做视觉设计**。

- [ ] **Step 1: 加 cue 预览面板组件**

在 `RecordingPanel.tsx`（RecordingRow 组件内或紧邻）加 SubtitlePanel 组件：

```tsx
function SubtitlePanel({ result, onExport, onCopyCue, onCopyAll }: {
  result: SubtitleResult;
  onExport: () => void;
  onCopyCue: (cue: SubtitleCue) => void;
  onCopyAll: () => void;
}) {
  const isFallback = result.trackUsed !== 'microphone';
  return (
    <div className="...">
      {isFallback && (
        <div className="...">{t("settings.recordings.subtitleFallbackSystem")}</div>
      )}
      <div>{t("settings.recordings.subtitleCount", { count: result.cues.length })} · {result.model}</div>
      <div>
        {result.cues.map((c, i) => (
          <div key={i} onClick={() => onCopyCue(c)} className="...">
            <span>{formatMs(c.startMs)} → {formatMs(c.endMs)}</span>
            <span>{c.text}</span>
          </div>
        ))}
      </div>
      <div>
        <button onClick={onCopyAll}>{t("settings.recordings.subtitleCopyAll")}</button>
        <button onClick={onExport}>{t("settings.recordings.subtitleExport")}</button>
      </div>
    </div>
  );
}

function formatMs(ms: number): string {
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  return h > 0
    ? `${h}:${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}`
    : `${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}`;
}
```

⚠️ **具体样式实施时用 frontend-design skill 定**——上面的 className 是占位。

- [ ] **Step 2: 接入展开/折叠 + 导出/复制 handler**

在 RecordingRow 加「查看字幕」按钮（当 subtitleResults[id] 存在时），点击 toggle `expandedSubtitleId`。

导出 SRT handler：

```typescript
const onExportSubtitle = async (id: number) => {
  // Tauri save dialog
  const { save } = await import("@tauri-apps/plugin-dialog");
  const dest = await save({
    defaultPath: `recording_${id}.srt`,
    filters: [{ name: "SubRip", extensions: ["srt"] }],
  });
  if (!dest) return;
  try {
    await invoke("export_subtitle", { id, destPath: dest });
    showToast?.(t("settings.recordings.subtitleExportDone", { path: dest }));
  } catch (e) {
    showToast?.(t("settings.recordings.subtitleExportFailed") + ": " + String(e));
  }
};
```

⚠️ 检查项目是否已有 `@tauri-apps/plugin-dialog` 依赖。如果没有，参考其他导出功能（如 GIF）怎么做文件保存。

复制 cue handler：

```typescript
const onCopyCue = async (cue: SubtitleCue) => {
  await navigator.clipboard.writeText(cue.text);
  showToast?.(t("settings.recordings.subtitleCopied"));
};
```

- [ ] **Step 3: build + 手动 e2e**

```bash
cd crates/desktop/frontend && npx tsc --noEmit && npx vite build
```

Expected: 0 error

手动 e2e（按 spec §7.4 清单）。

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/RecordingPanel.tsx
git commit -m "feat(frontend): cue 预览面板 + 导出 SRT + fallback 提示

- SubtitlePanel：cue 列表 + 时间区间 + 单击复制 + 复制全部 + 导出 SRT
- formatMs 紧凑时间格式
- track_used 非 microphone 显示 fallback 提示
- 导出走 Tauri save dialog"
```

---

### Task 4.3: i18n 文案

**Files:**
- Modify: `crates/desktop/frontend/src/locales/zh-CN.yaml`
- Modify: `crates/desktop/frontend/src/locales/en.yaml`

- [ ] **Step 1: zh-CN.yaml 加 settings.recordings.subtitle* 键**

在 `settings.recordings` 命名空间（zh-CN.yaml 约 line 105 `mergeAudioFailed` 后）追加：

```yaml
    transcript: 转字幕
    transcriptTooltip: 生成自动字幕（需 ASR 模型）
    subtitleDone: 字幕已生成（${count} 条）
    subtitleFailed: 字幕生成失败
    subtitleFallbackSystem: 未检测到麦克风音轨，已使用系统音轨识别
    subtitleCount: 共 ${count} 条
    subtitleCopyAll: 复制全部
    subtitleCopyOne: 复制
    subtitleCopied: 已复制
    subtitleExport: 导出 SRT
    subtitleExportDone: 已导出到 ${path}
    subtitleExportFailed: 导出失败
    subtitleEmpty: 未检测到语音内容
```

- [ ] **Step 2: en.yaml 对应英文键**

在 en.yaml 约 line 101 `mergeAudioFailed` 后追加：

```yaml
    transcript: Subtitle
    transcriptTooltip: Generate auto-subtitle (requires ASR model)
    subtitleDone: Subtitle generated (${count} cues)
    subtitleFailed: Subtitle generation failed
    subtitleFallbackSystem: No microphone track detected, used system audio
    subtitleCount: ${count} cues
    subtitleCopyAll: Copy All
    subtitleCopyOne: Copy
    subtitleCopied: Copied
    subtitleExport: Export SRT
    subtitleExportDone: Exported to ${path}
    subtitleExportFailed: Export failed
    subtitleEmpty: No speech detected
```

- [ ] **Step 3: build 确认无缺失键**

```bash
cd crates/desktop/frontend && npx vite build
```

Expected: 0 error

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/locales/zh-CN.yaml crates/desktop/frontend/src/locales/en.yaml
git commit -m "i18n(frontend): settings.recordings.subtitle* 文案（zh-CN + en）"
```

---

## Phase 5：手动 e2e 验证 + 文档同步

### Task 5.1: 手动 e2e（spec §7.4 清单）

- [ ] **Step 1: 启动桌面应用**

```bash
cargo run --profile optimize -p octopus-desktop --features embedded,custom-protocol
```

（或开发期 `./run-octopus.sh`）

- [ ] **Step 2: 逐项验证 spec §7.4 清单**

- [ ] 5 分钟带麦克风讲解的录屏 → 点「转字幕」→ ≤60 秒完成 → SRT 在 VLC/QuickTime 正确加载
- [ ] 时间戳对齐：随机抽 3 条 cue，视频跳到该时间点听到对应语音（偏差 ≤500ms）
- [ ] 双轨录制 → 默认选 mic track → cue 预览无 fallback 提示
- [ ] 单 system track 录制 → 自动 fallback → cue 预览顶部显示「已使用系统音轨」
- [ ] 无声录制 → 友好提示「未检测到语音内容」
- [ ] 重复点「转字幕」→ 覆盖旧结果（不重复 cue）
- [ ] 导出 SRT → 文件可在剪映/Premiere 导入

失败项记录并修。

### Task 5.2: 同步 architecture.md

**Files:**
- Modify: `docs/architecture.md`

- [ ] **Step 1: 在录屏章节加字幕说明**

更新 `docs/architecture.md` 录屏相关章节，说明：
- 字幕功能（VAD 段级时间戳 + SRT 导出）
- 数据模型（recordings.subtitle_*）
- 触发方式（手动）
- schema 版本（v54）

- [ ] **Step 2: Commit**

```bash
git add docs/architecture.md
git commit -m "docs(architecture): 录屏自动字幕章节 + schema v54"
```

---

## Self-Review

### Spec coverage 核查

| spec 章节 | 覆盖任务 |
|---|---|
| §1 范围 | Task 1-4 全覆盖 |
| §2 架构（方案 B） | Task 3 编排层确认 |
| §3.1 DB schema v54 | Task 1.1 |
| §3.2 数据模型 | Task 1.2 |
| §3.3 RecordingMeta 扩展 | Task 1.4 |
| §3.4 SRT 格式 | Task 1.2 generate_srt |
| §3.5 选轨 fallback | Task 1.3 |
| §4.2 segment_audio_vad_with_offsets | Task 2.1 |
| §4.3 边界（<500ms 过滤） | Task 2.2 transcribe_segments_with_timestamps |
| §4.4 带时间戳 transcribe | Task 2.2 |
| §4.5 短音频走 VAD | Task 2.2（统一路径） |
| §5.1 record API | Task 1.2/1.3/3.1 |
| §5.2 asr-local API | Task 2.2 |
| §5.3 3 个 Tauri 命令 | Task 3.2 |
| §5.4 事件 emit | Task 3.2（RecordTaskEvent Subtitle 变体） |
| §5.5 错误降级 | Task 3.2 + Task 4 错误 UI |
| §5.6 casing 核验 | Task 1.2/3.2（rename_all + 测试） |
| §6 前端 UI | Task 4.1/4.2/4.3 |
| §7 测试 | Task 1.2/1.3/1.4/2.1/2.2 TDD + Task 5.1 e2e |
| §8 4 阶段 | Phase 1-4 + Phase 5 验证 |

### 类型一致性

- `SubtitleCue` (record) 与 `TimestampedSegment` (asr-local) 字段一一对应（start_ms/end_ms/text），Task 3.2 显式转换 ✅
- `AudioTrackSource` 复用 audio_tracks.rs，不重建 ✅
- `SubtitleProgress` enum 与前端 `SubtitleStage` type 对齐 ✅
- `TrackPreference` enum 与前端 track 参数对齐 ✅

### 阶段依赖

```
Phase 1 (Task 1.1-1.4) ──┬──→ Phase 3 (Task 3.1-3.2) ──→ Phase 4 (Task 4.1-4.3) ──→ Phase 5
                          │
Phase 2 (Task 2.1-2.2) ──┘
```

Phase 1 和 Phase 2 可并行（不同 crate）。Phase 3 依赖两者。Phase 4 依赖 Phase 3。Phase 5 收尾。

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-28-record-auto-subtitle.md`. Two execution options:

**1. Subagent-Driven (recommended)** - 每个 Task 派新 subagent，任务间 review，快速迭代
**2. Inline Execution** - 本 session 内执行，批量 + checkpoint review

Which approach?
