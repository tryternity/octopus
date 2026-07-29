# 屏幕录制音频录后合并 — 设计规格（spec）

> **Status: ✅ 已实现**（2026-07-27，分支 `feat/record-followup`，Phase 1-4 完成，HEAD `b930b572`）。Phase 5 e2e 待用户验证。

## 实现注记（Implementation Notes）

实施过程中与原 spec 的偏差回写至此处。

### 2026-07-27 实施完成（Phase 1-4）

| Task | 实现 | 偏差 |
|---|---|---|
| 1.1 AudioTrack + infer | `crates/record/src/audio_tracks.rs`，7 测试全过 | 无（与 spec 完全一致） |
| 1.2 DB migration v51→v52 | `crates/infra/src/db.rs::migrate_v51_to_v52`，165 infra 测试全过 | 多加了 `init_schema_upgrades_v51_db_to_v52` 端到端测试（合理增强） |
| 1.3 RecordStore 改造 | `crates/record/src/store.rs`，4 个 SQL + 3 新测试 | row_to_meta 列 index 逐行核对（audio_tracks=10，后续 +1） |
| 2.1 probe_ffprobe | `crates/desktop/src/record_audio_probe.rs` | `which` 探测用 `.output()` 单次（非 brief 的两次） |
| 2.2 write_audio_tracks_metadata | 同上文件 | 用 `.mp4.meta.tmp` 临时文件 + rename 覆盖 |
| 2.3 stop_and_store_inner 集成 | `crates/desktop/src/record_commands.rs` | MetaFields 加 mic_device_name（两条路径都 resolve_mic_device_name 重解析，幂等） |
| 3.1 merge_audio_tracks | 同上 + `merged_output_path` | **amix 非 amerge**（spike 发现 mic mono + system stereo）；命令签名去掉 `State<AppState>`（Pre-Flight 修正，项目无 AppState） |
| 4.1 前端 | `RecordingPanel.tsx` + i18n | **blocker fix**：`RecordingMeta` 无 `rename_all`，前端 `audioTracks` 改 `audio_tracks` 对齐 snake_case；`MergeResult` 后端加 `rename_all="camelCase"` |

**关键决策（实施期裁定）**：
- `RecordingMeta` struct **故意不加** `#[serde(rename_all = "camelCase")]`——它有 16 个字段全是 snake_case（与 SQL 列名一致），加 rename_all 要改 16 处前端访问点。新字段继续用 snake_case。**约定已在 struct 上加注释明示**（`crates/record/src/store.rs:7`），防止重踩 Task 4.1 blocker。
- `MergeResult` **加了** `rename_all="camelCase"`——只有 2 字段，且是 Tauri 命令返回值，符合 camelCase 惯例。
- 合并不监听 `record://merge-*` 事件——与 GIF export 模式一致（await invoke + toast 即可），事件留作多窗口同步备用。

---


>
> **本 spec 范围**：
> 1. 录制阶段保持当前双轨智能 add 顺序（不动 helper 录制路径）
> 2. 录制停止后用 ffprobe 探测实际音轨 → 写入 DB + mp4 metadata
> 3. 录屏管理 UI hover 显示音轨信息
> 4. 双轨 recording 提供「合并音轨」按钮 → ffmpeg amix → 另存新文件
>
> **不在本 spec 范围**：实时混音（已 archive）、视频轨逻辑、helper 录制路径修改、自动合并（用户明确选手动）。
>
> **关联文档**：
> - 实时混音旧方向（archive）：`specs/archived/2026-07-27-screen-record-audio-mix-redesign-realtime.md`
> - SCK 格式 spike 报告（archive，未来重做实时混音参考）：`research/archived/2026-07-27-sck-single-stream-spike.md`
> - 原录屏设计：`specs/2026-07-25-screen-record-design.md`
> - 原录屏 plan（含上一轮混音失败诊断）：`plans/2026-07-25-screen-record.md` §「Task 8 后续」

---

## 0. 决策回顾

### 0.1 问题陈述

录屏开启「系统音频 + 麦克风」时，输出 mp4 有 2 条音频轨，但默认播放器（QuickTime Player）只播 track 1，track 2（系统音频）听不到。

上一轮实时混音 5 次失败 + Phase 0 spike 确认 SCK 无单流能力后，用户决定换方向：**保留双轨（不动录制），录后按需合并**。

### 0.2 brainstorming 决策清单

| 维度 | 决策 | 理由 |
|---|---|---|
| **录制阶段** | 保持当前双轨智能 add 顺序（不动 helper） | 实时混音 5 次失败 + spike 证伪单流；双轨是当前最稳的选择 |
| **元数据存储** | DB + mp4 双写 | DB 供前端快读，mp4 metadata 跟随文件走（拷贝/分享后还在） |
| **元数据生成** | ffprobe 读 mp4 实际轨道 | 准确反映 helper 实际产出（避免配置 vs 实际不符） |
| **元数据写入 mp4** | ffmpeg `-c copy -metadata`（不动 helper） | 复用现成 ffmpeg infra，零 Swift 改动，零实时录制路径风险 |
| **合并触发** | 手动按钮（hover 双轨卡片） | 用户完全掌控；自动合并会每次等待 |
| **合并产物** | 另存为新文件 `xxx_merged.mp4` | 不丢失原文件，可对比；DB 新增一条记录指向 merged 文件 |
| **合并技术** | ffmpeg `-filter_complex amix`（非 amerge） | spike 发现 mic 是 mono、system 是 stereo，amerge 要求声道相同会失败；amix 自动处理声道差异更稳 |

### 0.3 排除的方案（含理由）

| 方案 | 排除理由 |
|---|---|
| **实时单轨混音（AVAudioEngine）** | 5 轮失败 + Phase 0 spike 证伪 SCK 单流；AVAudioSourceNode 拉模型 vs SCK 推模型桥接无现成参考，风险高。详见 archive spec |
| **AVAssetWriter 写 metadata** | 要改 helper 实时录制路径（ScreenCaptureRecorder.swift:370），增加崩溃风险；ffmpeg 后处理更简单 |
| **自动合并（录后无感）** | 用户明确选手动；自动会每次双轨录屏等 10-30s |
| **合并后覆盖原文件** | 不可逆；合并出问题原文件没了 |
| **加 mp4 解析 Rust crate** | 新依赖 + 自定义 udta key 解析支持未验证；复用 ffprobe 子进程零依赖 |

### 0.4 调研结论（背景）

详见 brainstorming 阶段调研报告。关键事实：

| 事实 | 影响 |
|---|---|
| recordings 表 16 列，无 ALTER migration 历史（演进模式：改 db.sql + bump user_version） | 加 `audio_tracks` 列需新增 `migrate_v51_to_v52` + 改 db.sql |
| `RecordingMeta` 有 Serialize 无 Deserialize | 前端用 invoke 拿序列化结果，不直接反序列化 |
| 前端 hover UI 在 `RecordingPanel.tsx` 的 `RecordingRow`，行内 Tailwind `group-hover:opacity-*`，无独立 overlay 组件 | 加音轨标签仿现有 source_type 标签模式（行 604-613） |
| ffmpeg infra 完备：`probe_ffmpeg` / `find_ffmpeg` / `export_gif`（record_commands.rs:736-869） | 合并命令直接复用 export_gif 模式 |
| ffmpeg 路径：`~/.octopus/bin/ffmpeg` → 系统 PATH（不 bundled） | 合并功能依赖用户有 ffmpeg（与 GIF 导出同依赖） |
| ffprobe 读自定义 metadata：`-of json` 下 `format.tags.<key>` + `streams[i].codec_type/tags` | Rust 端 spawn ffprobe + serde_json 解析 |

---

## 1. 总体架构

### 1.1 数据流

```
┌─ 录制阶段（不动）──────────────────────────────────────────┐
│  helper 双轨智能 add（当前行为，commit f8bbe8ed）          │
│  → mp4（可能 0/1/2 音轨）                                  │
└────────────────────────────────────────────────────────────┘
                          ↓
┌─ 录制停止后（新增）────────────────────────────────────────┐
│  stop_and_store_inner                                      │
│    ├─ ffprobe <mp4> -of json                               │
│    │   → 解析 streams[audio].tags + format.tags            │
│    │   → 构造 audio_tracks: AudioTrack[]                   │
│    ├─ INSERT recordings (..., audio_tracks=JSON)           │
│    └─ ffmpeg -c copy -metadata octopus_audio_tracks=JSON   │
│        → 临时 mp4 → 覆盖原 mp4（写 metadata 进容器）       │
└────────────────────────────────────────────────────────────┘
                          ↓
┌─ 前端展示（新增）──────────────────────────────────────────┐
│  RecordingPanel → RecordingRow hover                       │
│    显示音轨标签：[🎤 Mic] [🔊 System] 或 [Single: Mic]     │
│    双轨时显示「合并音轨」按钮                              │
└────────────────────────────────────────────────────────────┘
                          ↓（用户点合并）
┌─ 合并阶段（新增）──────────────────────────────────────────┐
│  merge_audio_tracks(id)                                    │
│    ├─ ffmpeg -i <mp4> -filter_complex amix → merged.mp4   │
│    ├─ ffprobe merged.mp4 → 写 audio_tracks metadata        │
│    └─ INSERT recordings (file_path=merged.mp4, ...)        │
│      （新记录，原记录保留）                                 │
└────────────────────────────────────────────────────────────┘
```

### 1.2 数据结构

#### DB schema 改动（v51 → v52）

`recordings` 表新增 1 列：

```sql
audio_tracks TEXT NOT NULL DEFAULT '[]'
```

存 JSON 序列化的 `AudioTrack[]`。

#### AudioTrack 结构（新增）

```rust
// crates/record/src/store.rs（或新模块 audio_tracks.rs）
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrack {
    /// 轨道序号（0-based，对应 mp4 streams 里的 audio index）
    pub index: u32,
    /// 来源：microphone / system / unknown
    pub source: AudioTrackSource,
    /// 编码格式（如 "aac"）
    pub codec: String,
    /// 采样率（Hz）
    pub sample_rate: u32,
    /// 声道数
    pub channels: u32,
    /// 设备名（仅 microphone，可空）
    pub device_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioTrackSource {
    Microphone,
    System,
    /// 合并产出的单轨（merge_audio_tracks 命令产出）
    Merged,
    Unknown,
}
```

**来源判断逻辑**（ffprobe 输出 + 录制配置交叉）：
- ffprobe 给每条 audio stream 的 `tags.title` / `tags.handler_name` 可能是 "Mic" / "System" 等，但**不可靠**（取决于编码器）。
- **可靠方案**：用录制时的配置（`record_system_audio` / `record_microphone`）+ helper 的 add 顺序（mic 先 = track 0，system 后 = track 1）推断：
  - 配置只开 mic → track 0 = microphone
  - 配置只开 system → track 0 = system
  - 配置都开 → track 0 = microphone, track 1 = system
- ffprobe 只确认实际 track 数 + codec/sample_rate/channels（用于 hover 显示技术细节）。

#### mp4 metadata 写入

key: `octopus_audio_tracks`，value: 同 DB 存的 JSON。写到 mp4 udta atom（ffmpeg 默认）。

### 1.3 改动面

| 层 | 改动 |
|---|---|
| **DB schema** | recordings 加 `audio_tracks` 列（migration v51→v52） |
| **Rust infra** | `AudioTrack` struct + migrate + RecordStore SQL 改造 |
| **Rust desktop** | 新增 ffprobe 解析 + mp4 metadata 写入 + `merge_audio_tracks` 命令 |
| **Swift helper** | **不动**（保留 Task 1.1 lib/exec 拆分成果） |
| **前端** | RecordingMeta interface 加字段 + RecordingRow 加音轨标签 + 合并按钮 |

---

## 2. 详细设计

### 2.1 DB migration（v51 → v52）

仿 `migrate_v48_to_v49`（`db.rs:389-425`）范式：

```rust
fn migrate_v51_to_v52(conn: &Connection) -> Result<()> {
    // 幂等检查
    let has_col: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('recordings') WHERE name='audio_tracks'",
        [], |row| row.get(0),
    )?;
    if has_col {
        return Ok(());
    }
    conn.execute("ALTER TABLE recordings ADD COLUMN audio_tracks TEXT NOT NULL DEFAULT '[]'", [])?;
    Ok(())
}
```

`init_schema` if-else 链加：`if v == 51 { migrate_v51_to_v52(conn)?; return Ok(()); }`

`db.sql:440-457` 的 CREATE TABLE 同步加 `audio_tracks TEXT NOT NULL DEFAULT '[]'`（全新库幂等）。

bump `CURRENT_SCHEMA_VERSION` 从 51 → 52。

### 2.2 RecordStore 改造

`crates/record/src/store.rs`：

- `RecordingMeta` struct 加 `pub audio_tracks: Vec<AudioTrack>` 字段
- `insert` SQL 加 `audio_tracks` 列 + 参数（`serde_json::to_string(&meta.audio_tracks)?`）
- `get` / `list` / `row_to_meta` 的 SELECT 加 `audio_tracks` 列 + 解析（`serde_json::from_str(&row.get::<_, String>(...)?)`，解析失败兜底 `vec![]`）
- `RecordingMeta` derive 加 `Deserialize`（合并命令需要反序列化前端传回的数据，或后端内部用）

### 2.3 ffprobe 解析（新增模块）

新文件 `crates/desktop/src/record_audio_probe.rs`（或加到 `record_commands.rs`）：

```rust
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    streams: Vec<FfprobeStream>,
    format: Option<FfprobeFormat>,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    #[serde(rename = "codec_type")]
    codec_type: String,
    #[serde(rename = "codec_name")]
    codec_name: Option<String>,
    #[serde(rename = "sample_rate")]
    sample_rate: Option<String>,  // ffprobe 给字符串
    channels: Option<u32>,
    tags: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct FfprobeFormat {
    tags: Option<Value>,
}

/// 跑 ffprobe 解析 mp4 实际音轨。
pub async fn probe_audio_tracks(
    ffprobe_path: &Path,
    mp4_path: &Path,
) -> Result<Vec<RawAudioTrack>, RecordError> {
    let output = tokio::process::Command::new(ffprobe_path)
        .arg("-v").arg("quiet")
        .arg("-print_format").arg("json")
        .arg("-show_format")
        .arg("-show_streams")
        .arg(mp4_path)
        .output().await?;
    // 检查 exit code
    if !output.status.success() {
        return Err(RecordError::FfprobeFailed(...));
    }
    let parsed: FfprobeOutput = serde_json::from_slice(&output.stdout)?;
    // 过滤 audio streams，转 RawAudioTrack
    let mut tracks = Vec::new();
    let mut audio_index = 0;
    for s in parsed.streams.iter() {
        if s.codec_type == "audio" {
            tracks.push(RawAudioTrack {
                index: audio_index,
                codec: s.codec_name.clone().unwrap_or_default(),
                sample_rate: s.sample_rate.as_ref().and_then(|sr| sr.parse().ok()).unwrap_or(0),
                channels: s.channels.unwrap_or(0),
            });
            audio_index += 1;
        }
    }
    Ok(tracks)
}

/// ffprobe 给的原始音轨信息（无 source 判断，source 需配置推断）。
pub struct RawAudioTrack {
    pub index: u32,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
}
```

**ffprobe 路径解析**：与 ffmpeg 同目录（`~/.octopus/bin/ffprobe` 或系统 PATH）。新增 `probe_ffprobe()` 仿 `probe_ffmpeg()`。

### 2.4 audio_tracks 组装（配置 + ffprobe 交叉）

在 `stop_and_store_inner`（`record_commands.rs:479-580`）里，组装 `RecordingMeta` 时：

```rust
// 1. 从 RecordingRequest 拿配置
let system_enabled = request.audio.system.enabled;
let mic_enabled = /* nativeMicrophoneEnabled 等价物，需 Rust 端判断 */;

// 2. ffprobe 读实际轨道
let raw_tracks = probe_audio_tracks(&ffprobe, &mp4_path).await.unwrap_or_default();

// 3. 交叉推断 source
let audio_tracks = infer_audio_tracks(raw_tracks, system_enabled, mic_enabled, &request.audio.microphone.device_name);
```

`infer_audio_tracks` 逻辑（**纯函数，可单测**）：

```rust
pub fn infer_audio_tracks(
    raw: Vec<RawAudioTrack>,
    system_enabled: bool,
    mic_enabled: bool,
    mic_device_name: Option<&str>,
) -> Vec<AudioTrack> {
    // 按 helper add 顺序映射 source
    // helper 顺序：mic 先（track 0），system 后（track 1）
    raw.into_iter().enumerate().map(|(i, r)| {
        let source = match (i, mic_enabled, system_enabled) {
            (0, true, _) => AudioTrackSource::Microphone,    // 都开 or 只开 mic，track 0 = mic
            (0, false, true) => AudioTrackSource::System,    // 只开 system
            (1, true, true) => AudioTrackSource::System,     // 都开，track 1 = system
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
```

### 2.5 mp4 metadata 写入（ffmpeg 后处理）

在 `stop_and_store_inner` 入库后，spawn 一次 ffmpeg 写 metadata：

```rust
// audio_tracks_json = serde_json::to_string(&audio_tracks)?
// 临时文件 → 成功后覆盖原文件
let tmp_path = mp4_path.with_extension("mp4.meta.tmp");
let status = tokio::process::Command::new(&ffmpeg)
    .arg("-y")
    .arg("-i").arg(&mp4_path)
    .arg("-c").arg("copy")  // 不重编码
    .arg("-metadata").arg(format!("octopus_audio_tracks={}", audio_tracks_json))
    .arg(&tmp_path)
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status().await?;

if status.success() {
    std::fs::rename(&tmp_path, &mp4_path)?;
} else {
    // 失败不阻断——DB 已有 audio_tracks，mp4 metadata 只是 nice-to-have
    let _ = std::fs::remove_file(&tmp_path);
    log::warn!("[record] ffmpeg metadata write failed, DB-only fallback");
}
```

**注意**：`-c copy` 秒级完成（不重编码），对用户感知低。

### 2.6 合并命令（merge_audio_tracks）

新 Tauri 命令 `crates/desktop/src/record_commands.rs`：

```rust
#[tauri::command]
pub async fn merge_audio_tracks(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    id: i64,
) -> Result<MergeResult, String> {
    // 1. 查 DB 拿原 recording
    let meta = with_db_blocking(&state, |db| RecordStore::new(db).get(id))
        .map_err(|e| e.to_string())?
        .ok_or("recording not found")?;

    // 2. 校验是双轨
    let audio_tracks = meta.audio_tracks;
    if audio_tracks.len() < 2 {
        return Err("not a multi-track recording".into());
    }

    // 3. ffmpeg amix 合并（设计期初稿写 amerge，实施期改为 amix——详见 §0.2 决策清单 + 实现注记）
    let input = resolve_recording_path(&meta.file_path);
    let output = merged_output_path(&input);  // xxx.mp4 → xxx_merged.mp4
    let ffmpeg = find_ffmpeg().map_err(|e| e.to_string())?;

    app.emit("record://merge-started", serde_json::json!({"id": id}))?;

    let status = tokio::process::Command::new(&ffmpeg)
        .arg("-y")
        .arg("-i").arg(&input)
        .arg("-filter_complex")
        .arg("[0:a:0][0:a:1]amix=inputs=2:duration=longest:dropout_transition=0[a]")  // amix 比 amerge 更稳（处理声道差异）
        .arg("-map").arg("0:v")
        .arg("-map").arg("[a]")
        .arg("-c:v").arg("copy")       // 视频不重编码
        .arg("-c:a").arg("aac")        // 音频重编码为 AAC
        .arg("-b:a").arg("192k")
        .arg(&output)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())        // 失败时读 stderr 诊断
        .status().await?;

    if !status.success() {
        let _ = std::fs::remove_file(&output);
        app.emit("record://merge-failed", serde_json::json!({"id": id, "error": "ffmpeg failed"}))?;
        return Err("ffmpeg amerge failed".into());
    }

    // 4. ffprobe merged.mp4 → 新 audio_tracks（应单轨，source = merged）
    let merged_raw = probe_audio_tracks(&ffprobe, &output).await?;
    let merged_tracks = vec![AudioTrack {
        index: 0,
        source: AudioTrackSource::Merged,  // 新增 enum 变体
        codec: "aac".into(),
        sample_rate: merged_raw[0].sample_rate,
        channels: merged_raw[0].channels,
        device_name: None,
    }];

    // 5. 写 mp4 metadata
    write_audio_tracks_metadata(&ffmpeg, &output, &merged_tracks).await?;

    // 6. INSERT 新 recording 记录（指向 merged.mp4）
    let mut new_meta = meta.clone();
    new_meta.file_path = output.to_string_lossy().to_string();
    new_meta.audio_tracks = merged_tracks;
    new_meta.title = format!("{} (merged)", meta.title);  // 标题加 (merged) 标识
    let new_id = with_db_blocking(&state, |db| RecordStore::new(db).insert(&new_meta, None))
        .map_err(|e| e.to_string())?;

    app.emit("record://merge-done", serde_json::json!({"id": id, "new_id": new_id}))?;
    Ok(MergeResult { new_id, file_path: new_meta.file_path })
}
```

**amix vs amerge 取舍**：
- `amerge`：严格合并，要求输入声道数相同（mic mono + system stereo 会失败）
- `amix`：自动处理声道差异（mono 自动 duplicate 到 stereo），更稳
- 选 `amix`（spec §0.4 spike 发现 mic 是 mono，system 是 stereo，amerge 会失败）

### 2.7 前端改动

`crates/desktop/frontend/src/pages/Settings/RecordingPanel.tsx`：

#### A. RecordingMeta interface 加字段（行 46-63）

```typescript
interface RecordingMeta {
  // ... 现有字段
  audioTracks: AudioTrack[];
}

interface AudioTrack {
  index: number;
  source: 'microphone' | 'system' | 'merged' | 'unknown';
  codec: string;
  sampleRate: number;
  channels: number;
  deviceName?: string;
}
```

#### B. RecordingRow Meta row 加音轨标签（行 580-614 附近）

仿现有 source_type 标签（行 604-613）模式，加一个音轨标签组：

```tsx
{meta.audioTracks.length > 0 && (
  <div className="flex gap-1 items-center">
    {meta.audioTracks.map(t => (
      <span key={t.index} className="text-[10px] px-1.5 py-0.5 rounded bg-muted">
        {t.source === 'microphone' && '🎤'}
        {t.source === 'system' && '🔊'}
        {t.source === 'merged' && '🎵'}
        {t.source === 'unknown' && '?'}
        {t.source === 'microphone' && t.deviceName ? ` ${t.deviceName}` : ''}
      </span>
    ))}
  </div>
)}
```

#### C. 双轨时显示合并按钮（行 640-742 hover 操作区）

仿 GIF Export 按钮（行 682-694）加一个 Merge Audio 按钮：

```tsx
{meta.audioTracks.length >= 2 && (
  <button
    onClick={() => onMergeAudio(meta.id)}
    disabled={mergingId === meta.id}
    className="opacity-0 group-hover:opacity-50 hover:!opacity-100 transition disabled:opacity-30"
    title={t('Merge audio tracks (mic + system) into single track')}
  >
    {mergingId === meta.id ? <Loader2 className="animate-spin" /> : <MergeIcon />}
  </button>
)}
```

合并进度通过 listen `record://merge-started/done/failed` 事件更新 `mergingId` 状态。

### 2.8 错误处理 + 降级

| 场景 | 处理 |
|---|---|
| ffprobe 不存在 / 失败 | `audio_tracks = []`（DB 存空数组），mp4 metadata 跳过；前端显示「音轨未知」 |
| ffmpeg 写 metadata 失败 | 不阻断（DB 已有），log warn，mp4 metadata 缺失 |
| ffmpeg 合并失败 | 删 merged.mp4（避免半残文件），emit failed，前端 toast 错误 |
| 合并时原文件被移动/删除 | ffmpeg 失败，前端 toast「原文件找不到」 |
| 合并产出 0 音轨 | ffprobe 校验 merged 音轨数 ≥ 1，否则删 merged + 报错 |

---

## 3. 测试策略

按 AGENTS.md「TDD 优先」准则。

### 3.1 可单测（TDD 先行）

| 层 | 测试内容 | 方式 |
|---|---|---|
| **`infer_audio_tracks`**（§2.4 纯函数） | 配置 × ffprobe 输入 → source 映射正确（4 场景：只 mic / 只 system / 都开 / 都关） | Rust unit test，构造 `Vec<RawAudioTrack>` + 配置 flag |
| **`merged_output_path`**（§2.6） | `xxx.mp4` → `xxx_merged.mp4`；已含 `_merged` 时不重复加 | Rust unit test |
| **`AudioTrack` serde 往返** | struct → JSON → struct 一致；camelCase 映射正确 | Rust unit test |
| **DB migration v51→v52**（§2.1） | 旧 schema（无 audio_tracks 列）→ migrate → 新列存在 + 默认值 `'[]'`；幂等（跑两次不报错） | Rust unit test，in-memory SQLite |
| **RecordStore insert/get/list with audio_tracks**（§2.2） | 写入 audio_tracks JSON → 读回一致；旧记录（默认 `'[]'`）读回空 vec | Rust unit test |

### 3.2 只能 e2e（事后补录）

| 层 | 为何 | 验证 |
|---|---|---|
| ffprobe 解析真实 mp4 | 需真实 ffmpeg/ffprobe + mp4 文件 | 手动跑：录一段 → 跑 ffprobe → 看输出 |
| ffmpeg amix 合并 | 需真实 ffmpeg + 双轨 mp4 | 手动跑：合并 → 听音 + ffprobe 单轨 |
| 前端 hover 显示 + 合并按钮 | UI 交互 | 用户 e2e |

### 3.3 验收标准

| # | 检查项 | 通过标准 |
|---|---|---|
| **A1** | ffprobe 单测 | `infer_audio_tracks` 4 场景全过；`merged_output_path` 边界过；AudioTrack serde 往返过；migration 幂等过 |
| **A2** | RecordStore 单测 | insert/get/list 带 audio_tracks 全过 |
| **A3** | 录制后 mp4 有 metadata | `ffprobe -show_format <mp4>` 看到 `format.tags.octopus_audio_tracks` 含正确 JSON |
| **A4** | 前端 hover 显示音轨 | 双轨卡片显示 `[🎤 Mic] [🔊 System]`；单轨显示对应标签 |
| **A5** | 合并成功 | 点合并按钮 → 等 10-30s → 新增 `_merged.mp4` + DB 新记录；merged.mp4 单轨 + 听到两路 |
| **A6** | 合并失败处理 | ffmpeg 故障时（如改 PATH）→ toast 错误 + 无半残文件 |

---

## 4. 实施顺序（plan 骨架）

```
Phase 1: DB + 数据结构（TDD 先行）
  ├─ Task 1.1: AudioTrack struct + serde 单测（已完成的 lib/exec 拆分复用）
  ├─ Task 1.2: DB migration v51→v52 + 幂等单测
  └─ Task 1.3: RecordStore insert/get/list 改造 + audio_tracks 往返单测

Phase 2: ffprobe 解析 + audio_tracks 组装
  ├─ Task 2.1: probe_ffprobe() + probe_audio_tracks()（spawn ffprobe + serde 解析）
  ├─ Task 2.2: infer_audio_tracks() 纯函数 + 4 场景单测
  └─ Task 2.3: stop_and_store_inner 集成 ffprobe + 写 DB

Phase 3: mp4 metadata 写入
  └─ Task 3.1: write_audio_tracks_metadata()（ffmpeg -c copy -metadata）+ stop_and_store_inner 集成

Phase 4: 合并命令
  ├─ Task 4.1: merged_output_path() + 单测
  ├─ Task 4.2: merge_audio_tracks 命令（ffmpeg amix + ffprobe + INSERT 新记录）
  └─ Task 4.3: 错误处理 + 降级

Phase 5: 前端
  ├─ Task 5.1: RecordingMeta interface 加 audioTracks
  ├─ Task 5.2: RecordingRow 加音轨标签
  └─ Task 5.3: 合并按钮 + 事件监听 + toast

Phase 6: e2e + 文档
  ├─ Task 6.1: 用户 e2e（录双轨 → 看元数据 → 合并 → 听音）
  └─ Task 6.2: 文档同步（architecture.md / 原 plan Task 8 后续 / z-sync）
```

---

## 5. 关键风险

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| ffmpeg/ffprobe 用户没装 | 中 | 合并功能不可用；metadata 缺失 | DB 兜底 `[]`；前端合并按钮灰禁 + 引导文案（仿 GIF 导出） |
| `-c copy -metadata` 不被某些 mp4 muxer 接受 | 低 | metadata 写失败 | 不阻断；DB 已有；log warn |
| amix 声道处理意外（mic mono duplicate 后音量不对） | 低 | 合并后音质差 | 实现期实测；可加 `volume` filter 微调 |
| helper 实际 add 顺序与推断逻辑不符（如权限拒绝降级） | 低 | source 标签错 | ffprobe 实际 track 数交叉验证；都开但只 1 轨 → source=unknown |
| migration v51→v52 在已 v52 库重跑 | 低 | 报错 | 幂等检查（pragma_table_info） |

---

## 附录 A：保留的 Task 1.1 成果（lib/exec 拆分）

Phase 0 阶段完成的 Task 1.1（commit `0cc3a3d7`）——Swift helper 拆成 `OctopusSckHelperLib`（library）+ `OctopusSckHelper`（executable wrapper）+ `OctopusSckHelperTests`（testTarget）——**保留**。虽然本 spec 不再需要 helper 内的纯函数单测（混音方向已废），但 lib/exec 拆分对未来 helper 任何 TDD 改造仍有价值，且 revert 反而增加无谓 churn。

后续若发现 lib/exec 拆分给打包/构建带来问题，可单独 revert（不影响本 spec 其他部分）。
