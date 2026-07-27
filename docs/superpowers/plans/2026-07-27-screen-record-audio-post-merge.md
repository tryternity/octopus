# 屏幕录制音频录后合并 实施计划（plan）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 录屏双轨音频保留（不动录制）+ 录后 ffprobe 探测写元数据 + UI 显示音轨 + 手动 ffmpeg amix 合并另存新文件。

**Architecture:** 录制停止后用 ffprobe 读 mp4 实际音轨 → 配置交叉推断 source → 写 DB（recordings.audio_tracks JSON 列）+ ffmpeg `-c copy -metadata` 写 mp4 udta atom。前端 RecordingRow hover 显示音轨标签，双轨时显示「合并音轨」按钮 → ffmpeg `amix`（处理 mic mono + system stereo 声道差异）合并 → ffprobe 新文件 → INSERT 新 recording 记录。

**Tech Stack:** Rust（rusqlite + tokio::process + serde_json）/ SQLite（schema v51→v52）/ ffmpeg + ffprobe 子进程 / React + TypeScript + Tailwind。

**关联文档：**
- spec：`docs/superpowers/specs/2026-07-27-screen-record-audio-post-merge.md`
- 旧方向（archive）：`docs/superpowers/specs/archived/2026-07-27-screen-record-audio-mix-redesign-realtime.md`
- 上一轮 Task 1.1 成果（lib/exec 拆分，保留）：commit `0cc3a3d7`

## Global Constraints

- **不动 Swift helper 录制路径**（双轨 add 顺序保持 commit `f8bbe8ed` 行为）。Phase 0 已完成的 lib/exec 拆分（`0cc3a3d7`）保留。
- **DB schema 演进模式**：改 `crates/infra/src/db.sql`（CREATE TABLE 加列，IF NOT EXISTS 幂等）+ 新增 `migrate_v51_to_v52` 函数（`crates/infra/src/db.rs`，仿 `migrate_v48_to_v49:389-425` 幂等范式）+ `init_schema` if 链加分支。**没有 `CURRENT_SCHEMA_VERSION` 常量**——版本演进靠 `PRAGMA user_version = N` + if 链。
- **ffmpeg/ffprobe 查找路径**：`~/.octopus/bin/ffmpeg`（和 ffprobe）→ 系统 PATH。**不 bundled**。复用 `probe_ffmpeg()`（`record_commands.rs:736`）模式新增 `probe_ffprobe()`。
- **amix 而非 amerge**：spike 发现 mic 是 mono、system 是 stereo，amerge 要求输入声道相同会失败；amix 自动处理。
- **合并产物另存新文件**：`xxx.mp4` → `xxx_merged.mp4`，INSERT 新 recording 记录，原记录保留。
- **错误降级**：ffprobe/ffmpeg 失败不阻断主流程——DB 兜底 `audio_tracks=[]`，mp4 metadata 缺失仅 log warn。
- **测试纪律**：纯函数（`infer_audio_tracks`、`merged_output_path`、`AudioTrack` serde、migration 幂等、RecordStore 往返）TDD 先行；ffmpeg/ffprobe 真实子进程只能 e2e。
- **Rust 改动验证**：每步 `cargo build -p <crate>` 0 error 0 warning；改完 `cargo test -p <crate> --lib` 全过。
- **前端验证**：`cd crates/desktop/frontend && pnpm tsc --noEmit && pnpm build` 0 error。
- **app 启动由用户跑**（`./run-octopus.sh --no-lto`），AI 不代跑。
- **`config/` 是软链接，读写用绝对路径 `/Users/wudarui/.octopus/`**。

## File Structure

| 文件 | 改动 | 职责 |
|---|---|---|
| `crates/record/src/audio_tracks.rs` | **新增** | `AudioTrack` / `AudioTrackSource` struct + `infer_audio_tracks()` 纯函数 |
| `crates/record/src/store.rs` | **改** | `RecordingMeta` 加 `audio_tracks` 字段 + 4 个 SQL 改造 |
| `crates/record/src/lib.rs` | **小改** | pub mod audio_tracks |
| `crates/infra/src/db.sql:440-457` | **小改** | recordings CREATE TABLE 加 `audio_tracks` 列 |
| `crates/infra/src/db.rs` | **改** | `migrate_v51_to_v52` + init_schema 分支 |
| `crates/desktop/src/record_audio_probe.rs` | **新增** | `probe_ffprobe` + `probe_audio_tracks` + `write_audio_tracks_metadata` |
| `crates/desktop/src/record_commands.rs` | **改** | stop_and_store_inner 集成 + `merge_audio_tracks` 命令 |
| `crates/desktop/src/lib.rs` | **小改** | mod record_audio_probe + 注册 merge_audio_tracks 命令 |
| `crates/desktop/frontend/src/pages/Settings/RecordingPanel.tsx` | **改** | interface 加字段 + 音轨标签 + 合并按钮 |

---

## Phase 1: DB + 数据结构（TDD 先行）

### Task 1.1: AudioTrack struct + infer_audio_tracks 纯函数

**Files:**
- Create: `crates/record/src/audio_tracks.rs`
- Modify: `crates/record/src/lib.rs`

**Interfaces:**
- Produces: `AudioTrack`、`AudioTrackSource`、`RawAudioTrack`、`infer_audio_tracks()`

- [x] **Step 1: 写失败的测试（4 场景 + serde 往返）**

在 `crates/record/src/audio_tracks.rs` 末尾的 `#[cfg(test)] mod tests` 里：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn raw(idx: u32, codec: &str, sr: u32, ch: u32) -> RawAudioTrack {
        RawAudioTrack { index: idx, codec: codec.into(), sample_rate: sr, channels: ch }
    }

    #[test]
    fn infer_only_mic_track0_is_microphone() {
        let tracks = infer_audio_tracks(
            vec![raw(0, "aac", 48000, 1)],
            false,  // system_enabled
            true,   // mic_enabled
            Some("UGREEN MIC"),
        );
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].source, AudioTrackSource::Microphone);
        assert_eq!(tracks[0].device_name.as_deref(), Some("UGREEN MIC"));
    }

    #[test]
    fn infer_only_system_track0_is_system() {
        let tracks = infer_audio_tracks(
            vec![raw(0, "aac", 48000, 2)],
            true, false, None,
        );
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].source, AudioTrackSource::System);
        assert_eq!(tracks[0].device_name, None);
    }

    #[test]
    fn infer_both_mic_first_system_second() {
        // helper add 顺序：mic 先（track 0），system 后（track 1）
        let tracks = infer_audio_tracks(
            vec![raw(0, "aac", 48000, 1), raw(1, "aac", 48000, 2)],
            true, true, Some("MacBook Mic"),
        );
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].source, AudioTrackSource::Microphone);
        assert_eq!(tracks[0].device_name.as_deref(), Some("MacBook Mic"));
        assert_eq!(tracks[1].source, AudioTrackSource::System);
        assert_eq!(tracks[1].device_name, None);
    }

    #[test]
    fn infer_empty_when_no_raw() {
        let tracks = infer_audio_tracks(vec![], true, true, None);
        assert!(tracks.is_empty());
    }

    #[test]
    fn infer_unknown_when_config_disagrees_with_actual() {
        // 配置都开但实际只 1 轨（如 mic 权限拒绝降级）→ 超出配置预期的标 unknown
        let tracks = infer_audio_tracks(
            vec![raw(0, "aac", 48000, 2), raw(1, "aac", 48000, 2), raw(2, "aac", 48000, 2)],
            true, true, None,
        );
        assert_eq!(tracks[2].source, AudioTrackSource::Unknown);
    }

    #[test]
    fn audio_track_serde_roundtrip_camel_case() {
        let t = AudioTrack {
            index: 1,
            source: AudioTrackSource::Microphone,
            codec: "aac".into(),
            sample_rate: 48000,
            channels: 2,
            device_name: Some("UGREEN".into()),
        };
        let json = serde_json::to_string(&t).unwrap();
        // camelCase + lowercase enum
        assert!(json.contains("\"sampleRate\""));
        assert!(json.contains("\"deviceName\""));
        assert!(json.contains("\"microphone\""));
        let back: AudioTrack = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn audio_track_source_merged_serializes() {
        let s = serde_json::to_string(&AudioTrackSource::Merged).unwrap();
        assert_eq!(s, "\"merged\"");
    }
}
```

- [x] **Step 2: 跑测试确认失败**

```bash
cargo test -p octopus-record --lib audio_tracks
```

Expected: FAIL "could not find `audio_tracks`"。

- [x] **Step 3: 实现**

`crates/record/src/audio_tracks.rs`:

```rust
//! 录屏音轨元数据 + 配置推断。

/// ffprobe 给的原始音轨信息（无 source 判断）。
#[derive(Debug, Clone, PartialEq)]
pub struct RawAudioTrack {
    pub index: u32,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
}

/// 写入 DB / mp4 metadata / 发给前端的音轨描述。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrack {
    pub index: u32,
    pub source: AudioTrackSource,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioTrackSource {
    Microphone,
    System,
    /// merge_audio_tracks 命令产出的合并单轨
    Merged,
    Unknown,
}

/// 按 helper add 顺序（mic 先 track 0，system 后 track 1）+ 配置推断每轨 source。
///
/// spike 验证 helper 双轨 add 顺序（commit f8bbe8ed）：都开时 microphoneAudioInput
/// 先 add（track 1），systemAudioInput 后 add（track 2）。所以 track index 0/1 →
/// mic/system。只开一边时该轨是 track 0。
pub fn infer_audio_tracks(
    raw: Vec<RawAudioTrack>,
    system_enabled: bool,
    mic_enabled: bool,
    mic_device_name: Option<&str>,
) -> Vec<AudioTrack> {
    raw.into_iter().enumerate().map(|(i, r)| {
        let source = match (i, mic_enabled, system_enabled) {
            (0, true, _) => AudioTrackSource::Microphone,
            (0, false, true) => AudioTrackSource::System,
            (1, true, true) => AudioTrackSource::System,
            _ => AudioTrackSource::Unknown,
        };
        AudioTrack {
            index: r.index,
            source,
            codec: r.codec,
            sample_rate: r.sample_rate,
            channels: r.channels,
            device_name: match source {
                AudioTrackSource::Microphone => mic_device_name.map(String::from),
                _ => None,
            },
        }
    }).collect()
}

#[cfg(test)]
mod tests {
    // 上面 Step 1 的测试贴这里
}
```

`crates/record/src/lib.rs` 加：

```rust
pub mod audio_tracks;
pub use audio_tracks::{AudioTrack, AudioTrackSource, RawAudioTrack, infer_audio_tracks};
```

- [x] **Step 4: 跑测试确认通过**

```bash
cargo test -p octopus-record --lib audio_tracks
```

Expected: 7 个 test 全过。

- [x] **Step 5: Commit**

```bash
git add crates/record/src/audio_tracks.rs crates/record/src/lib.rs
git commit -m "feat(record): AudioTrack 元数据 + infer_audio_tracks 配置推断纯函数"
```

---

### Task 1.2: DB migration v51 → v52

**Files:**
- Modify: `crates/infra/src/db.sql`（recordings CREATE TABLE 加列）
- Modify: `crates/infra/src/db.rs`（migrate_v51_to_v52 + init_schema 分支）

**Interfaces:**
- Produces: schema v52，recordings 表有 `audio_tracks` 列

- [x] **Step 1: 写失败的测试（幂等性 + 默认值）**

在 `crates/infra/src/db.rs` 的 `#[cfg(test)] mod tests` 末尾加：

```rust
#[test]
fn migrate_v51_to_v52_adds_audio_tracks_column() {
    let conn = Connection::open_in_memory().unwrap();
    // 先建 v51 schema（跑 INIT_SQL）
    conn.execute_batch(INIT_SQL).unwrap();
    // 模拟 v51 状态：移除 audio_tracks 列（db.sql 已含时手动 ALTER 删）
    // 实际上：本测试在新加列**之前**跑会失败（find column 失败），加列后跑成功
    conn.execute("PRAGMA user_version = 51", []).unwrap();
    // 删除 audio_tracks 列（sqlite 不支持 DROP COLUMN before 3.35，用重建表法或跳过）
    // 简化：直接测 migrate 幂等性——先跑一次，列应存在；再跑一次，不报错
    migrate_v51_to_v52(&conn).unwrap();
    let has_col: bool = conn
        .prepare("SELECT COUNT(*) > 0 FROM pragma_table_info('recordings') WHERE name='audio_tracks'")
        .unwrap()
        .query_row([], |r| r.get(0))
        .unwrap();
    assert!(has_col, "audio_tracks 列应存在");

    // 默认值 '[]'
    let default: String = conn
        .prepare("SELECT audio_tracks FROM recordings LIMIT 1")
        .ok()
        .and_then(|mut s| s.query_row([], |r| r.get::<_, String>(0)).ok())
        .unwrap_or_else(|| "'[]'".into());
    // 表空时拿不到行，验证列存在即可

    // 幂等：再跑一次不报错
    migrate_v51_to_v52(&conn).unwrap();
}

#[test]
fn fresh_db_has_audio_tracks_column() {
    // 全新库直接跑 INIT_SQL，audio_tracks 列应存在（db.sql 已加）
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(INIT_SQL).unwrap();
    let has_col: bool = conn
        .prepare("SELECT COUNT(*) > 0 FROM pragma_table_info('recordings') WHERE name='audio_tracks'")
        .unwrap()
        .query_row([], |r| r.get(0))
        .unwrap();
    assert!(has_col, "全新库应直接有 audio_tracks 列");
}
```

- [x] **Step 2: 跑测试确认失败**

```bash
cargo test -p octopus-infra --lib migrate_v51_to_v52
```

Expected: FAIL "cannot find function `migrate_v51_to_v52`"。

- [x] **Step 3: 改 db.sql**

`crates/infra/src/db.sql` 的 recordings CREATE TABLE（行 440-457）加列：

```sql
CREATE TABLE IF NOT EXISTS recordings (
    id INTEGER PRIMARY KEY,
    file_path TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    duration_ms INTEGER NOT NULL,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    fps INTEGER NOT NULL,
    codec TEXT NOT NULL,
    has_system_audio INTEGER NOT NULL DEFAULT 0,
    has_microphone INTEGER NOT NULL DEFAULT 0,
    audio_tracks TEXT NOT NULL DEFAULT '[]',  -- 新增：JSON 序列化的 AudioTrack[]
    source_type TEXT NOT NULL,
    file_size INTEGER NOT NULL,
    has_thumbnail INTEGER NOT NULL DEFAULT 0,
    is_favorite INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    deleted_at TEXT DEFAULT NULL
);
```

- [x] **Step 4: 实现 migrate_v51_to_v52**

在 `crates/infra/src/db.rs` 的 `migrate_v50_to_v51` 后追加：

```rust
/// v51→v52：recordings 加 audio_tracks 列（JSON 序列化的 AudioTrack[]）。
///
/// 用于「双轨保留 + 录后合并」方案（spec 2026-07-27-screen-record-audio-post-merge.md）。
/// 幂等：PRAGMA table_info 检查列不存在才 ALTER。
fn migrate_v51_to_v52(conn: &Connection) -> Result<()> {
    let has_audio_tracks = conn
        .prepare("SELECT 1 FROM pragma_table_info('recordings') WHERE name = 'audio_tracks'")?
        .exists([])?;
    if !has_audio_tracks {
        let has_table = conn
            .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='recordings'")?
            .exists([])?;
        if has_table {
            conn.execute(
                "ALTER TABLE recordings ADD COLUMN audio_tracks TEXT NOT NULL DEFAULT '[]'",
                [],
            )?;
            log::info!("schema v52: recordings 补 audio_tracks 列");
        }
    }
    conn.execute("PRAGMA user_version = 52", [])?;
    log::info!("schema upgraded to v52 (recordings.audio_tracks)");
    Ok(())
}
```

- [x] **Step 5: 改 init_schema**

`init_schema`（`db.rs:488`）的 if 链：

把：
```rust
if v >= 51 {
    // ... 自愈逻辑
    return Ok(());
}
```
改为：
```rust
if v >= 52 {
    // v52+ 最新，自愈检查（同 v51 逻辑：表缺失重跑 db.sql）
    let has_recordings = conn
        .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='recordings'")?
        .exists([])?;
    if !has_recordings {
        log::warn!("schema v52+ 但 recordings 表缺失，重跑 db.sql 自愈");
        conn.execute_batch(INIT_SQL)
            .context("self-heal: execute_batch INIT_SQL for missing recordings table")?;
    }
    return Ok(());
}
```

然后在 v50 分支前加 v51 分支：

```rust
if v == 51 {
    migrate_v51_to_v52(conn)?;
    return Ok(());
}
if v == 50 {
    migrate_v50_to_v51(conn)?;
    return Ok(());
}
// ... 现有 v49/v48/... 分支不变
```

- [x] **Step 6: 跑测试确认通过**

```bash
cargo test -p octopus-infra --lib migrate_v51_to_v52
cargo test -p octopus-infra --lib fresh_db_has_audio_tracks_column
```

Expected: 2 个 test 全过。

- [x] **Step 7: 跑全部 infra 测试确认无回归**

```bash
cargo test -p octopus-infra --lib
```

Expected: 全过。

- [x] **Step 8: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(infra): schema v52——recordings 加 audio_tracks 列"
```

---

### Task 1.3: RecordStore 改造（RecordingMeta + 4 个 SQL）

**Files:**
- Modify: `crates/record/src/store.rs`

**Interfaces:**
- Consumes: `AudioTrack`（Task 1.1）
- Produces: `RecordingMeta.audio_tracks` 字段，insert/get/list/row_to_meta 支持

- [x] **Step 1: 写失败的测试（往返 + 旧记录兼容）**

在 `crates/record/src/store.rs` 的 `#[cfg(test)] mod tests` 里改 `sample_meta` 加 `audio_tracks: vec![]`，并加新测试：

```rust
fn sample_meta_with_tracks(id: i64) -> RecordingMeta {
    let mut m = sample_meta(id);
    m.audio_tracks = vec![
        AudioTrack {
            index: 0,
            source: AudioTrackSource::Microphone,
            codec: "aac".into(),
            sample_rate: 48000,
            channels: 1,
            device_name: Some("UGREEN".into()),
        },
        AudioTrack {
            index: 1,
            source: AudioTrackSource::System,
            codec: "aac".into(),
            sample_rate: 48000,
            channels: 2,
            device_name: None,
        },
    ];
    m
}

#[test]
fn insert_and_get_with_audio_tracks() {
    let conn = test_db();
    let store = RecordStore::new(&conn);
    let meta = sample_meta_with_tracks(2001);
    store.insert(&meta, None).unwrap();
    let got = store.get(2001).unwrap().unwrap();
    assert_eq!(got.audio_tracks.len(), 2);
    assert_eq!(got.audio_tracks[0].source, AudioTrackSource::Microphone);
    assert_eq!(got.audio_tracks[1].source, AudioTrackSource::System);
}

#[test]
fn audio_tracks_default_empty_for_legacy_rows() {
    // 旧记录（audio_tracks 列默认 '[]'）读回应是空 vec
    let conn = test_db();
    let store = RecordStore::new(&conn);
    // 直接 INSERT 不带 audio_tracks（模拟旧客户端写入）
    conn.execute(
        "INSERT INTO recordings (id, file_path, title, duration_ms, width, height, fps, codec,
         has_system_audio, has_microphone, source_type, file_size, has_thumbnail, is_favorite, created_at)
         VALUES (3001, '/x.mp4', '', 1000, 100, 100, 30, 'h264', 0, 0, 'display', 0, 0, 0, '2026-01-01T00:00:00Z')",
        [],
    ).unwrap();
    let got = store.get(3001).unwrap().unwrap();
    assert!(got.audio_tracks.is_empty());
}

#[test]
fn list_returns_audio_tracks() {
    let conn = test_db();
    let store = RecordStore::new(&conn);
    store.insert(&sample_meta_with_tracks(1), None).unwrap();
    let list = store.list(&ListFilter { limit: 100, offset: 0, include_deleted: false, favorites_only: false }).unwrap();
    assert_eq!(list[0].audio_tracks.len(), 2);
}
```

- [x] **Step 2: 跑测试确认失败**

```bash
cargo test -p octopus-record --lib store
```

Expected: FAIL（RecordingMeta 缺 audio_tracks 字段）。

- [x] **Step 3: 改 RecordingMeta struct**

`crates/record/src/store.rs`：

```rust
use crate::audio_tracks::{AudioTrack, AudioTrackSource};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordingMeta {
    pub id: i64,
    pub file_path: String,
    pub title: String,
    pub duration_ms: i64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: String,
    pub has_system_audio: bool,
    pub has_microphone: bool,
    #[serde(default)]
    pub audio_tracks: Vec<AudioTrack>,
    pub source_type: String,
    pub file_size: u64,
    pub has_thumbnail: bool,
    pub is_favorite: bool,
    pub created_at: String,
    pub deleted_at: Option<String>,
}
```

⚠️ 加 `Deserialize`（原只有 Serialize）—— merge 命令需要。`#[serde(default)]` 让旧 JSON（无此字段）反序列化兜底空 vec。

- [x] **Step 4: 改 4 个 SQL（insert/get/list/row_to_meta）**

**insert**（行 43-58）：

```rust
pub fn insert(&self, meta: &RecordingMeta, thumbnail: Option<&[u8]>) -> RecordResult<()> {
    let audio_tracks_json = serde_json::to_string(&meta.audio_tracks)
        .unwrap_or_else(|_| "[]".into());
    self.conn.execute(
        "INSERT INTO recordings
         (id, file_path, title, duration_ms, width, height, fps, codec,
          has_system_audio, has_microphone, audio_tracks, source_type, file_size,
          has_thumbnail, is_favorite, created_at, deleted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, NULL)",
        rusqlite::params![
            meta.id, meta.file_path, meta.title, meta.duration_ms,
            meta.width, meta.height, meta.fps, meta.codec,
            meta.has_system_audio as i32, meta.has_microphone as i32,
            audio_tracks_json,
            meta.source_type, meta.file_size,
            thumbnail.is_some() as i32, meta.is_favorite as i32,
            meta.created_at,
        ],
    )?;
    // ... thumbnail 部分不变
}
```

**get**（行 70-82）+ **list**（行 84-109）的 SELECT 都加 `audio_tracks` 列（在 `has_microphone` 后）：

```sql
SELECT id, file_path, title, duration_ms, width, height, fps, codec,
       has_system_audio, has_microphone, audio_tracks, source_type, file_size,
       has_thumbnail, is_favorite, created_at, deleted_at
FROM recordings ...
```

**row_to_meta**（行 190-209）：

```rust
fn row_to_meta(&self, row: &rusqlite::Row<'_>) -> rusqlite::Result<RecordingMeta> {
    let audio_tracks_json: String = row.get(10)?;
    let audio_tracks = serde_json::from_str(&audio_tracks_json).unwrap_or_default();
    Ok(RecordingMeta {
        id: row.get(0)?,
        file_path: row.get(1)?,
        title: row.get(2)?,
        duration_ms: row.get(3)?,
        width: row.get(4)?,
        height: row.get(5)?,
        fps: row.get(6)?,
        codec: row.get(7)?,
        has_system_audio: row.get::<_, i32>(8)? != 0,
        has_microphone: row.get::<_, i32>(9)? != 0,
        audio_tracks,
        source_type: row.get(11)?,
        file_size: row.get(12)?,
        has_thumbnail: row.get::<_, i32>(13)? != 0,
        is_favorite: row.get::<_, i32>(14)? != 0,
        created_at: row.get(15)?,
        deleted_at: row.get(16)?,
    })
}
```

- [x] **Step 5: 改 sample_meta 测试辅助**

`sample_meta`（行 225-244）加 `audio_tracks: vec![]`。

- [x] **Step 6: 跑测试确认通过**

```bash
cargo test -p octopus-record --lib store
```

Expected: 全过（含新 3 个测试 + 旧的 11 个）。

- [x] **Step 7: 改 record_commands.rs 组装点**

`crates/desktop/src/record_commands.rs` 的 `stop_and_store_inner`（行 534-551）组装 `RecordingMeta { ... }` 加 `audio_tracks: vec![]`（占位，Task 2.3 再填真实值）：

```rust
let meta = RecordingMeta {
    // ... 现有字段
    audio_tracks: vec![],  // Task 2.3 填充
    // ...
};
```

跑 `cargo build -p octopus-desktop` 确认 0 error。

- [x] **Step 8: Commit**

```bash
git add crates/record/src/store.rs crates/desktop/src/record_commands.rs
git commit -m "feat(record): RecordingMeta 加 audio_tracks + RecordStore SQL 改造"
```

---

## Phase 2: ffprobe 解析 + audio_tracks 组装

### Task 2.1: probe_ffprobe + probe_audio_tracks

**Files:**
- Create: `crates/desktop/src/record_audio_probe.rs`
- Modify: `crates/desktop/src/lib.rs`

**Interfaces:**
- Produces: `probe_ffprobe()`、`probe_audio_tracks()`

- [x] **Step 1: 实现 probe_ffprobe**

新文件 `crates/desktop/src/record_audio_probe.rs`（无法纯单测——需真实 ffprobe + mp4，跳过 TDD，e2e 验证）：

```rust
//! 录屏音频元数据探测——ffprobe 读 mp4 实际轨道 + ffmpeg 写 metadata。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use serde::Deserialize;
use octopus_record::RawAudioTrack;
use octopus_infra::octopus_config_home;

/// 探测 ffprobe 路径。仿 probe_ffmpeg（record_commands.rs:736）：~/.octopus/bin/ffprobe → PATH。
pub fn probe_ffprobe() -> Option<PathBuf> {
    let home_bin = octopus_config_home().join("bin").join("ffprobe");
    if home_bin.exists() {
        return Some(home_bin);
    }
    // 系统 PATH
    let status = std::process::Command::new("which")
        .arg("ffprobe")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    if status.success() {
        // 拿到路径（which 输出在 stdout，重定向 null 了，重跑一次拿路径）
        let out = std::process::Command::new("which")
            .arg("ffprobe")
            .output()
            .ok()?;
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    None
}

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    streams: Vec<FfprobeStream>,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    #[serde(rename = "codec_type")]
    codec_type: String,
    #[serde(rename = "codec_name")]
    codec_name: Option<String>,
    #[serde(rename = "sample_rate")]
    sample_rate: Option<String>,
    channels: Option<u32>,
}

/// 跑 ffprobe 解析 mp4 实际音轨。
pub async fn probe_audio_tracks(ffprobe: &Path, mp4: &Path) -> Result<Vec<RawAudioTrack>, String> {
    let output = tokio::process::Command::new(ffprobe)
        .arg("-v").arg("quiet")
        .arg("-print_format").arg("json")
        .arg("-show_streams")
        .arg("-select_streams").arg("a")  // 只看音频流
        .arg(mp4)
        .output()
        .await
        .map_err(|e| format!("ffprobe spawn 失败: {e}"))?;

    if !output.status.success() {
        return Err(format!("ffprobe 退出码非 0: {}", String::from_utf8_lossy(&output.stderr)));
    }

    let parsed: FfprobeOutput = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("ffprobe JSON 解析失败: {e}"))?;

    let mut tracks = Vec::new();
    for (i, s) in parsed.streams.iter().enumerate() {
        if s.codec_type != "audio" { continue; }
        tracks.push(RawAudioTrack {
            index: i as u32,
            codec: s.codec_name.clone().unwrap_or_default(),
            sample_rate: s.sample_rate.as_ref().and_then(|sr| sr.parse().ok()).unwrap_or(0),
            channels: s.channels.unwrap_or(0),
        });
    }
    Ok(tracks)
}
```

- [x] **Step 2: 注册 module**

`crates/desktop/src/lib.rs` 加：

```rust
#[cfg(target_os = "macos")]
mod record_audio_probe;
```

注意：record_commands.rs 是 `#![cfg(target_os = "macos")]`，record_audio_probe 也 gate macOS（octopus-record 只在 macOS 引入）。

- [x] **Step 3: Build 验证**

```bash
cargo build -p octopus-desktop
```

Expected: 0 error。

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/record_audio_probe.rs crates/desktop/src/lib.rs
git commit -m "feat(desktop): probe_ffprobe + probe_audio_tracks——ffprobe 解析 mp4 音轨"
```

---

### Task 2.2: write_audio_tracks_metadata（ffmpeg 后处理）

**Files:**
- Modify: `crates/desktop/src/record_audio_probe.rs`

**Interfaces:**
- Consumes: `AudioTrack`、`find_ffmpeg`
- Produces: `write_audio_tracks_metadata()`

- [x] **Step 1: 实现**

在 `crates/desktop/src/record_audio_probe.rs` 加：

```rust
use octopus_record::AudioTrack;

/// 用 ffmpeg -c copy -metadata 把 audio_tracks JSON 写进 mp4 udta atom。
///
/// 失败不阻断主流程——调用方吞掉错误仅 log warn（DB 已有 audio_tracks）。
/// 流程：临时文件 → 成功覆盖原文件；失败删临时文件。
pub async fn write_audio_tracks_metadata(
    ffmpeg: &Path,
    mp4: &Path,
    tracks: &[AudioTrack],
) -> Result<(), String> {
    let json = serde_json::to_string(tracks).unwrap_or_else(|_| "[]".into());
    let tmp = mp4.with_extension("mp4.meta.tmp");

    let status = tokio::process::Command::new(ffmpeg)
        .arg("-y")
        .arg("-i").arg(mp4)
        .arg("-c").arg("copy")
        .arg("-metadata").arg(format!("octopus_audio_tracks={}", json))
        .arg(&tmp)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|e| format!("ffmpeg spawn 失败: {e}"))?;

    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err("ffmpeg metadata 写入失败（退出码非 0）".into());
    }

    std::fs::rename(&tmp, mp4).map_err(|e| format!("覆盖原文件失败: {e}"))?;
    Ok(())
}
```

- [x] **Step 2: Build 验证**

```bash
cargo build -p octopus-desktop
```

Expected: 0 error。

- [x] **Step 3: Commit**

```bash
git add crates/desktop/src/record_audio_probe.rs
git commit -m "feat(desktop): write_audio_tracks_metadata——ffmpeg -c copy -metadata 写 mp4 udta"
```

---

### Task 2.3: stop_and_store_inner 集成 ffprobe + 写 DB + 写 mp4 metadata

**Files:**
- Modify: `crates/desktop/src/record_commands.rs`（stop_and_store_inner 行 479-580）

**Interfaces:**
- Consumes: `probe_audio_tracks`、`write_audio_tracks_metadata`、`infer_audio_tracks`、`probe_ffprobe`

- [x] **Step 1: 改 stop_and_store_inner**

在 `RecordingMeta { ... }` 组装前（行 534 前），加音轨探测逻辑：

```rust
// 探测音轨元数据（失败兜底空 vec，不阻断）
let audio_tracks = probe_recording_audio_tracks(
    &abs_path,
    has_system_audio,
    has_microphone,
    fields.mic_device_name.as_deref(),  // MetaFields 需加这个字段（见 Step 2）
).await;

let meta = RecordingMeta {
    id: recording_id,
    file_path: file_path.clone(),
    // ... 其他字段不变
    audio_tracks: audio_tracks.clone(),
    // ...
};
```

入库成功后（行 558 后，Finder reveal 前），写 mp4 metadata：

```rust
// 写 mp4 metadata（失败不阻断——DB 已有 audio_tracks，mp4 metadata 是 nice-to-have）
if !audio_tracks.is_empty() {
    // probe_ffmpeg 在 record_commands.rs，Task 2.3 Step 3 改为 pub(crate)
    if let Some(ffmpeg) = crate::record_commands::probe_ffmpeg() {
        if let Err(e) = crate::record_audio_probe::write_audio_tracks_metadata(
            &ffmpeg, &abs_path, &audio_tracks,
        ).await {
            log::warn!("[record] mp4 metadata 写入失败（不影响录制）: {e}");
        }
    }
}
```

- [x] **Step 2: MetaFields 加 mic_device_name**

`MetaFields` struct（搜 `struct MetaFields` 定位）加字段：

```rust
pub(crate) struct MetaFields {
    pub recording_id: i64,
    pub width: u32,
    pub height: u32,
    pub source_type: String,
    pub has_system_audio: bool,
    pub has_microphone: bool,
    pub mic_device_name: Option<String>,  // 新增
}
```

所有构造 `MetaFields { ... }` 的地方（grep `MetaFields {` 找）都加这个字段。值从 `resolve_mic_device_name()`（`record_commands.rs:265-289` 附近）的结果来。

- [x] **Step 3: probe_ffmpeg 改 pub(crate)**

`record_commands.rs:736` 的 `fn probe_ffmpeg() -> Option<PathBuf>` 改 `pub(crate) fn probe_ffprobe` —— 等等，是 `probe_ffmpeg` 改可见性：

```rust
pub(crate) fn probe_ffmpeg() -> Option<std::path::PathBuf> {
    // ... 原实现不变
}
```

Step 1 的代码里用 `crate::record_commands::probe_ffmpeg()`（同 crate 跨 module 可见）。

- [x] **Step 4: 加 probe_recording_audio_tracks 辅助函数**

在 `record_audio_probe.rs` 加：

```rust
use octopus_record::{AudioTrack, infer_audio_tracks};

/// 完整流程：ffprobe 读 mp4 → 配置交叉推断 source。失败返回空 vec。
pub async fn probe_recording_audio_tracks(
    mp4: &Path,
    system_enabled: bool,
    mic_enabled: bool,
    mic_device_name: Option<&str>,
) -> Vec<AudioTrack> {
    let ffprobe = match probe_ffprobe() {
        Some(p) => p,
        None => {
            log::debug!("[record] ffprobe 不可用，audio_tracks 兜底空");
            return vec![];
        }
    };
    match probe_audio_tracks(&ffprobe, mp4).await {
        Ok(raw) => infer_audio_tracks(raw, system_enabled, mic_enabled, mic_device_name),
        Err(e) => {
            log::warn!("[record] ffprobe 解析失败: {e}");
            vec![]
        }
    }
}
```

- [x] **Step 5: Build + test**

```bash
cargo build -p octopus-desktop
cargo test -p octopus-record --lib  # 无回归
```

Expected: 0 error，测试全过。

- [x] **Step 6: Commit**

```bash
git add crates/desktop/src/record_commands.rs crates/desktop/src/record_audio_probe.rs
git commit -m "feat(record): stop_and_store_inner 集成 ffprobe 音轨探测 + mp4 metadata 写入"
```

---

## Phase 3: 合并命令

### Task 3.1: merged_output_path + merge_audio_tracks 命令

**Files:**
- Modify: `crates/desktop/src/record_audio_probe.rs`（加 merged_output_path）
- Modify: `crates/desktop/src/record_commands.rs`（加 merge_audio_tracks 命令）
- Modify: `crates/desktop/src/lib.rs`（注册命令）

**Interfaces:**
- Produces: `merge_audio_tracks` Tauri 命令

- [x] **Step 1: 写失败的测试（merged_output_path 纯函数）**

在 `record_audio_probe.rs` 加测试 module：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn merged_output_path_appends_merged_suffix() {
        let p = PathBuf::from("/tmp/2026-07-27_10.30.00_123.mp4");
        let m = merged_output_path(&p);
        assert_eq!(m.file_name().to_str().unwrap(), "2026-07-27_10.30.00_123_merged.mp4");
    }

    #[test]
    fn merged_output_path_no_double_suffix() {
        // 已含 _merged 不重复加
        let p = PathBuf::from("/tmp/x_merged.mp4");
        let m = merged_output_path(&p);
        assert_eq!(m.file_name().to_str().unwrap(), "x_merged.mp4");
    }

    #[test]
    fn merged_output_path_preserves_dir() {
        let p = PathBuf::from("/Users/wudarui/.octopus/recordings/abc.mp4");
        let m = merged_output_path(&p);
        assert_eq!(m.parent(), p.parent());
    }
}
```

- [x] **Step 2: 跑测试确认失败**

```bash
cargo test -p octopus-desktop --lib merged_output_path
```

Expected: FAIL "cannot find `merged_output_path`"。

- [x] **Step 3: 实现 merged_output_path**

`record_audio_probe.rs`:

```rust
/// 合并产物路径：`xxx.mp4` → `xxx_merged.mp4`（同目录）。已含 _merged 不重复加。
pub fn merged_output_path(input: &Path) -> PathBuf {
    let file_name = input.file_name().and_then(|n| n.to_str()).unwrap_or("output.mp4");
    let stem = file_name.trim_end_matches(".mp4");
    let already_merged = stem.ends_with("_merged");
    let new_name = if already_merged {
        file_name.to_string()
    } else {
        format!("{stem}_merged.mp4")
    };
    input.with_file_name(new_name)
}
```

- [x] **Step 4: 跑测试确认通过**

```bash
cargo test -p octopus-desktop --lib merged_output_path
```

Expected: 3 个 test 全过。

- [x] **Step 5: 实现 merge_audio_tracks 命令**

`record_commands.rs` 加（仿 `export_gif:818-869` 模式）：

```rust
use octopus_record::audio_tracks::{AudioTrack, AudioTrackSource};

#[derive(serde::Serialize)]
pub struct MergeResult {
    pub new_id: i64,
    pub file_path: String,
}

/// 把双轨 mp4（mic + system）用 ffmpeg amix 合并成单轨，另存为新文件 + INSERT 新 DB 记录。
///
/// amix（非 amerge）：自动处理声道差异（mic mono 自动 duplicate 到 stereo）。
/// 进度：emit record://merge-started/done/failed。
#[command]
pub async fn merge_audio_tracks(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<MergeResult, String> {
    use octopus_infra::paths::resolve_recording_path;

    // 1. 查 DB 拿原 recording
    let meta = with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        store.get(id)?.ok_or(RecordError::NotFound(id))
    })
    .await?;

    if meta.audio_tracks.len() < 2 {
        return Err("不是多音轨录屏，无需合并".into());
    }

    let input = resolve_recording_path(&meta.file_path);
    if !input.exists() {
        return Err(format!("源文件不存在: {}", input.display()));
    }

    let ffmpeg = find_ffmpeg().await?;
    let output = crate::record_audio_probe::merged_output_path(&input);

    let _ = app.emit("record://merge-started", serde_json::json!({ "id": id }));

    // 2. ffmpeg amix 合并（视频 -c copy 不重编码，音频重编码 AAC）
    let status = tokio::process::Command::new(&ffmpeg)
        .arg("-y")
        .arg("-i").arg(&input)
        .arg("-filter_complex")
        .arg("[0:a:0][0:a:1]amix=inputs=2:duration=longest:dropout_transition=0[a]")
        .arg("-map").arg("0:v")
        .arg("-map").arg("[a]")
        .arg("-c:v").arg("copy")
        .arg("-c:a").arg("aac")
        .arg("-b:a").arg("192k")
        .arg(&output)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .status()
        .await
        .map_err(|e| format!("ffmpeg spawn 失败: {e}"))?;

    if !status.success() {
        let _ = std::fs::remove_file(&output);
        let _ = app.emit(
            "record://merge-failed",
            serde_json::json!({ "id": id, "error": "ffmpeg amix 失败" }),
        );
        return Err("ffmpeg amix 失败（退出码非 0）".into());
    }

    // 3. 探测 merged 文件音轨（应单轨）
    let merged_tracks = match crate::record_audio_probe::probe_ffprobe() {
        Some(ffprobe) => {
            match crate::record_audio_probe::probe_audio_tracks(&ffprobe, &output).await {
                Ok(raw) if !raw.is_empty() => {
                    vec![AudioTrack {
                        index: 0,
                        source: AudioTrackSource::Merged,
                        codec: raw[0].codec.clone(),
                        sample_rate: raw[0].sample_rate,
                        channels: raw[0].channels,
                        device_name: None,
                    }]
                }
                _ => vec![AudioTrack {
                    index: 0, source: AudioTrackSource::Merged,
                    codec: "aac".into(), sample_rate: 48000, channels: 2,
                    device_name: None,
                }],
            }
        }
        None => vec![AudioTrack {
            index: 0, source: AudioTrackSource::Merged,
            codec: "aac".into(), sample_rate: 48000, channels: 2,
            device_name: None,
        }],
    };

    // 4. 写 mp4 metadata（失败不阻断）
    if let Err(e) = crate::record_audio_probe::write_audio_tracks_metadata(
        &ffmpeg, &output, &merged_tracks,
    ).await {
        log::warn!("[record] merged mp4 metadata 写入失败: {e}");
    }

    // 5. INSERT 新 recording 记录
    let file_size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
    let new_id = chrono::Utc::now().timestamp_millis();
    let new_meta = RecordingMeta {
        id: new_id,
        file_path: output.to_string_lossy().to_string(),
        title: if meta.title.is_empty() {
            "merged".into()
        } else {
            format!("{} (merged)", meta.title)
        },
        duration_ms: meta.duration_ms,
        width: meta.width,
        height: meta.height,
        fps: meta.fps,
        codec: meta.codec.clone(),
        has_system_audio: true,
        has_microphone: true,
        audio_tracks: merged_tracks,
        source_type: meta.source_type.clone(),
        file_size,
        has_thumbnail: false,
        is_favorite: false,
        created_at: now_iso(),
        deleted_at: None,
    };

    with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        store.insert(&new_meta, None)
    })
    .await?;

    let result = MergeResult {
        new_id,
        file_path: new_meta.file_path.clone(),
    };
    let _ = app.emit(
        "record://merge-done",
        serde_json::json!({ "id": id, "new_id": new_id, "path": result.file_path }),
    );
    Ok(result)
}
```

- [x] **Step 6: 注册命令**

`crates/desktop/src/lib.rs` 的 `invoke_handler!` 里加 `merge_audio_tracks`（grep `export_gif` 找位置）。

- [x] **Step 7: Build + test**

```bash
cargo build -p octopus-desktop
cargo test -p octopus-desktop --lib merged_output_path
```

Expected: 0 error，3 个 test 全过。

- [x] **Step 8: Commit**

```bash
git add crates/desktop/src/record_audio_probe.rs crates/desktop/src/record_commands.rs crates/desktop/src/lib.rs
git commit -m "feat(record): merge_audio_tracks 命令——ffmpeg amix 合并双轨 + 另存新文件"
```

---

## Phase 4: 前端

### Task 4.1: RecordingMeta interface + 音轨标签 + 合并按钮

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/RecordingPanel.tsx`

**Interfaces:**
- Consumes: 后端 `RecordingMeta.audio_tracks`、`merge_audio_tracks` 命令、`record://merge-*` 事件

- [x] **Step 1: 加 AudioTrack interface + 扩展 RecordingMeta**

`RecordingPanel.tsx` 行 46-63 附近：

```typescript
interface AudioTrack {
  index: number;
  source: 'microphone' | 'system' | 'merged' | 'unknown';
  codec: string;
  sampleRate: number;
  channels: number;
  deviceName?: string;
}

interface RecordingMeta {
  // ... 现有字段保留
  audioTracks: AudioTrack[];
}
```

- [x] **Step 2: 加音轨标签到 Meta row**

在 RecordingRow 的 Meta row 区（行 580-614，source_type 标签附近）加：

```tsx
{meta.audioTracks && meta.audioTracks.length > 0 && (
  <div className="flex gap-1 items-center text-[10px]">
    {meta.audioTracks.map((t, i) => (
      <span
        key={i}
        className="px-1.5 py-0.5 rounded bg-muted text-muted-foreground"
        title={`${t.codec} ${t.sampleRate}Hz ${t.channels}ch`}
      >
        {t.source === 'microphone' && `🎤${t.deviceName ? ` ${t.deviceName}` : ''}`}
        {t.source === 'system' && '🔊'}
        {t.source === 'merged' && '🎵 merged'}
        {t.source === 'unknown' && '? unknown'}
      </span>
    ))}
  </div>
)}
```

- [x] **Step 3: 加合并按钮到 hover 操作区**

在 RecordingRow 的 hover 按钮 区（行 640-742，仿 GIF Export 按钮行 682-694）加：

```tsx
{meta.audioTracks && meta.audioTracks.length >= 2 && (
  <button
    onClick={(e) => {
      e.stopPropagation();
      onMergeAudio(meta.id);
    }}
    disabled={mergingId === meta.id}
    className="opacity-0 group-hover:opacity-50 hover:!opacity-100 transition disabled:opacity-30 disabled:cursor-not-allowed"
    title={mergingId === meta.id ? t('Merging...') : t('Merge audio tracks (mic + system) into single track')}
  >
    {mergingId === meta.id ? (
      <Loader2 className="w-4 h-4 animate-spin" />
    ) : (
      <MergeIcon className="w-4 h-4" />
    )}
  </button>
)}
```

`MergeIcon` 用 lucide-react 的 `MergeIcon` 或 `Combine`（确认 lucide 版本支持，否则用 `Music` 占位）。`Loader2` 项目已用（GIF 导入）。

- [x] **Step 4: 加 onMergeAudio + mergingId 状态**

在 RecordingPanel 组件顶层（仿 `onExportGif` 模式）加：

```typescript
const [mergingId, setMergingId] = useState<number | null>(null);

const onMergeAudio = async (id: number) => {
  setMergingId(id);
  try {
    await invoke<MergeResult>("merge_audio_tracks", { id });
    // 成功——刷新列表（新记录加入）
    await refreshRecordings();
    toast.success(t('Audio tracks merged'));
  } catch (e) {
    toast.error(`${t('Merge failed')}: ${e}`);
  } finally {
    setMergingId(null);
  }
};

// mount 时监听 merge 事件（多窗口同步备用）
useEffect(() => {
  const unlistenStarted = listen("record://merge-started", (e) => {
    if (e.payload?.id) setMergingId(e.payload.id);
  });
  const unlistenDone = listen("record://merge-done", () => {
    refreshRecordings();
    setMergingId(null);
  });
  const unlistenFailed = listen("record://merge-failed", (e) => {
    setMergingId(null);
    toast.error(`${t('Merge failed')}: ${e.payload?.error || 'unknown'}`);
  });
  return () => {
    unlistenStarted.then(f => f());
    unlistenDone.then(f => f());
    unlistenFailed.then(f => f());
  };
}, []);
```

`MergeResult` interface：

```typescript
interface MergeResult {
  newId: number;
  filePath: string;
}
```

把 `onMergeAudio` 传给 RecordingRow props。

- [x] **Step 5: 前端验证**

```bash
cd crates/desktop/frontend
pnpm tsc --noEmit
pnpm build
```

Expected: 0 error。

- [x] **Step 6: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/RecordingPanel.tsx
git commit -m "feat(frontend): RecordingRow 显示音轨标签 + 合并按钮 + 事件监听"
```

---

## Phase 5: e2e + 文档

### Task 5.1: 用户 e2e 验证

- [x] **Step 1: 通知用户 build + 跑**

```bash
cargo build --release -p octopus-desktop  # 或 ./run-octopus.sh --no-lto
```

用户在终端跑 app。

- [x] **Step 2: 录双轨**

用户录屏（开系统音频 + 开麦克风），≥10s，停止。

- [x] **Step 3: 验证音轨标签**

录屏管理 hover 录制卡片，应看到 `[🎤 Mic xxx] [🔊]` 标签。

- [x] **Step 4: 验证 DB + mp4 metadata**

```bash
# DB
sqlite3 ~/.octopus/octopus.db "SELECT id, audio_tracks FROM recordings ORDER BY id DESC LIMIT 1"
# 应看到 [{"index":0,"source":"microphone",...},{"index":1,"source":"system",...}]

# mp4 metadata（需 ffprobe）
ffprobe -show_format <录的mp4> | grep octopus_audio_tracks
# 应看到 TAG:octopus_audio_tracks=[...]
```

- [x] **Step 5: 验证合并**

点合并按钮，等 10-30s，应：
- 录屏管理出现新记录（标题带 `(merged)`）
- ffprobe 新记录的 mp4 显示单 audio track
- 听音：merged.mp4 同时听到麦克风 + 系统音频

```bash
ffprobe -show_streams <merged.mp4> | grep -c codec_type=audio
# 应输出 1
```

- [x] **Step 6: 决策门**

全过 → Phase 5.2 文档同步。失败 → 诊断 + 修。

### Task 5.2: 文档同步

**Files:**
- Modify: `docs/superpowers/specs/2026-07-27-screen-record-audio-post-merge.md`（实现注记）
- Modify: `docs/superpowers/plans/2026-07-25-screen-record.md`（Task 8 后续）
- Modify: `docs/architecture.md`（音轨章节）

- [x] **Step 1: spec 回填实现注记**

在 spec「实现注记」章节填：
- 实际实现偏差（如 merged_output_path 实际命名、ffmpeg amix 参数微调）
- e2e 验收结果

- [x] **Step 2: 更新原 plan Task 8 后续**

`docs/superpowers/plans/2026-07-25-screen-record.md` §「Task 8 后续」改为：

```markdown
## Task 8 后续：录后合并方案（2026-07-27，已实现）

实时混音 5 轮失败后改方向：双轨保留 + 录后按需合并。
- 录制：双轨智能 add 顺序（保持 f8bbe8ed 行为）
- 元数据：ffprobe 读 mp4 → DB + mp4 metadata 双写
- 合并：手动按钮 → ffmpeg amix → 另存新文件
详见 specs/2026-07-27-screen-record-audio-post-merge.md + plans/2026-07-27-screen-record-audio-post-merge.md
```

- [x] **Step 3: 更新 architecture.md**

音轨章节更新为「双轨 + 录后合并」描述。

- [x] **Step 4: Commit**

```bash
git add docs/
git commit -m "docs(record): 录后合并方案同步——spec 注记 + plan Task 8 + architecture"
```

- [x] **Step 5: 跑 z-sync-superpowers**

调用 `z-sync-superpowers` skill 确认所有 spec/plan 一致。

---

## 总结

- Phase 1：DB + 数据结构（AudioTrack + migration v52 + RecordStore）TDD
- Phase 2：ffprobe 解析 + audio_tracks 组装 + mp4 metadata 写入
- Phase 3：合并命令（merged_output_path + merge_audio_tracks）
- Phase 4：前端（音轨标签 + 合并按钮 + 事件）
- Phase 5：e2e + 文档同步

**执行节奏**（Subagent-Driven）：
- Phase 1-4 全 subagent 连续执行（不需用户硬件）
- Phase 5.1 e2e 用户跑
- Phase 5.2 文档 subagent

## 实施偏差（review plan）

实施期回填（Phase 1-4 全部完成，2026-07-27）：

| Task | 偏差 | 裁定 |
|---|---|---|
| 1.1 | 无 | 与 brief 完全一致 |
| 1.2 | 多加 `init_schema_upgrades_v51_db_to_v52` 端到端测试（验证 if 链接通 migrate） | 合理增强，保留 |
| 1.3 | row_to_meta 列 index 逐行核对（audio_tracks 插入后后续全部 +1） | reviewer 独立验证全对 |
| 2.1 | `probe_ffprobe` 的 `which` 用单次 `.output()`（非 brief 的两次 status+output） | implementer 改进，更高效 |
| 2.1 | 模块注册在 `main.rs` 而非 brief 说的 `lib.rs`（desktop 是 bin crate 无 lib.rs） | 项目结构，brief 错 |
| 2.2 | 临时文件 `.mp4.meta.tmp` + rename 覆盖（崩溃窗口留 Task 2.3 调用方处理） | 设计接受 |
| 2.3 | MetaFields 加 `mic_device_name`，两条路径都用 `resolve_mic_device_name` 重解析（幂等） | 已知边界：录屏中改 ASR 麦克风配置可能不一致 |
| 3.1 | **Pre-Flight 修正**：命令签名去掉 `State<'_, AppState>`（项目无此类型），用 `with_db_blocking` | brief 错 |
| 3.1 | brief 测试代码 `m.file_name().to_str()` API 错（`file_name()` 返回 `Option`），修正为 `.unwrap().to_str().unwrap()` | brief 错 |
| 3.1 | `cargo test --lib` 在 desktop bin crate 报错（无 lib target），去掉 `--lib` | brief 错 |
| 4.1 | **blocker**：前端 `audioTracks`（camelCase）与后端 `RecordingMeta` 无 rename_all（snake_case）不一致，运行时 undefined | fix：前端改 `audio_tracks` 对齐 snake_case；`MergeResult` 后端加 `rename_all="camelCase"` |
| 4.1 | lucide 用 `Combine`（非 brief 的 `MergeIcon`，lucide-react 无此名） | 实测确认 |
| 4.1 | 刷新列表复用 `loadList`，加 `onMerged={loadList}` prop（GIF 不刷新因不产新记录） | 合理 |

**关键裁定**：
- `RecordingMeta` struct **不加** `rename_all="camelCase"`——16 字段全 snake_case 与 SQL 列名一致，加 rename_all 要改 16 处前端。新字段继续 snake_case。
- `MergeResult` **加** `rename_all="camelCase"`——2 字段，Tauri 返回值惯例。
- 实时混音方向（AVAudioEngine）已 archive（spike 确认 SCK 无单流 KVC + AVAudioSourceNode 桥接无参考）。
