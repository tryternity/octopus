# 录屏自动字幕（Record Auto-Subtitle）— 设计规格（spec）

> **Status: 📝 设计阶段**（2026-07-28，brainstorming 完成，待写实施 plan）
>
> **本 spec 范围**：MVP——录屏停止后用户手动触发，抽 mic track → ASR → 生成 VAD 段级时间戳字幕 cue 列表，存 DB 并可导出标准 SRT 文件。
>
> **不在本 spec 范围**：视频播放器叠加字幕、字幕编辑器、词级时间戳（强制对齐）、多语言/双语字幕、多说话人分离、停止即自动生成、字幕样式定制、Windows/Linux 录屏 helper。

## 0. 决策回顾（brainstorming 确认）

| 决策项 | 结论 | 理由 |
|---|---|---|
| **生成时机** | 手动后处理（不自动） | 用户可控、不阻塞视频就绪、ASR 失败不影响主流程、CPU 不暴躁 |
| **选轨策略** | 默认 mic，可选 system | 教程/会议讲解 = mic 主战场；无 mic 自动 fallback system + 前端提示 |
| **输出格式** | 先做 SRT 导出 + cue 预览 | MVP 边界清晰，播放器叠加留下个 spec |
| **时间戳精度** | VAD 段级 | 工作量最小，能打通全链路；词级留下次升级 |
| **ASR 模型** | 跟随 Settings 配置 | 用户透明，复用现有模型管理 |
| **MVP 范围** | cue 预览 + 导出 SRT | 不做字幕编辑（导出 SRT 后外部工具改） |
| **整体方案** | 方案 B（desktop 编排） | record 保持纯净，与 merge_audio_tracks 等现有模式一致 |
| **短音频分段** | 统一走 VAD | 即使 5s 音频也能切出 1~3 段，简化分支 |

## 1. 范围与边界

### 1.1 MVP 范围（IN）

- 手动触发：录制停止后，历史项菜单加「生成字幕」入口
- 默认 mic track，UI 可选 system（单轨录制时自适应）
- ASR 模型跟随用户 Settings 配置
- VAD 段级时间戳（一句字幕 ≈ 一个语音段，可能不规整，后期可升级）
- DB 存 cue 列表（结构化）+ SRT 全文（便于直接导出）
- 历史项可展开 cue 列表预览，每 cue 可复制，整体可导出 SRT 文件到磁盘
- 进度可见（抽音轨/识别中/完成）+ 失败优雅降级（不阻塞视频已就绪）

### 1.2 MVP 范围外（OUT，留给后续 spec）

- 视频播放器叠加字幕（octopus 内的字幕播放器）
- 字幕编辑（cue 文本增删改、时间调整）—— 用 SRT 导出后在剪映/Premiere 改
- 词级时间戳（强制对齐）
- 多语言字幕、双语字幕
- 多说话人分离
- 自动字幕生成（停止即跑）—— 当前手动触发
- 字幕样式定制（字体/颜色/位置）—— SRT 标准不携带样式
- Windows / Linux 录屏 helper（仍仅 macOS）

### 1.3 成功标准

1. 一段含麦克风讲解的 5 分钟录屏，点「生成字幕」后 ≤ 60 秒出完整 SRT
2. SRT 在 QuickTime / VLC / 剪映里能正确加载，时间戳对齐可接受（每条 cue 的起始时间与视频对应语音段偏差 ≤ 500ms）
3. 重复点「生成字幕」是幂等的（覆盖旧结果，不重复 cue）
4. 无 mic track（纯系统音频录制）→ 走 system track 也能跑，但有 toast 提示「未检测到麦克风音轨，使用系统音轨」
5. 完全无声的录制 → 友好提示「未检测到语音内容」，不崩溃

## 2. 架构总览

### 2.1 数据流

```
[1] 用户点「生成字幕」(前端)
      ↓ invoke generate_subtitle { recording_id, track?: "microphone"|"system" }
[2] desktop/record_commands.rs::generate_subtitle
      ├─ 从 DB 查 RecordingMeta（含 audio_tracks JSON）
      ├─ record::select_track(meta, pref) → (track_index, track_used)
      ├─ 解析 mp4 file_path + 选定 track index
      └─ emit("record://subtitle-progress", ExtractingAudio)
           ↓ 调用 record crate
[3] crates/record/src/subtitle.rs (新增模块)
      ├─ extract_audio_track_to_pcm(mp4_path, track_index, ffmpeg_path)
      │    └─ ffmpeg -i xxx.mp4 -map 0:a:<idx> -ar 16000 -ac 1 -f f32le pipe:1
      │    └─ 读 stdout → Vec<f32> (16k mono PCM)
      └─ 返回 PCM 给 desktop
           ↓ desktop 调用 asr-local
[4] crates/asr-local/src/pipeline.rs (扩展)
      ├─ segment_audio_vad_with_offsets(samples)
      │    └─ 改造分段：返回 Vec<VadSegment { offset_samples, samples }>
      ├─ 逐段 transcribe_each_segment (复用现有) → (offset_samples, text)
      ├─ 后处理每段文本 (corrector + ITN + hans，复用现有)
      └─ 组装 Vec<TimestampedSegment { start_ms, end_ms, text }>
           ↓ 返回 desktop
[5] desktop/record_commands.rs
      ├─ TimestampedSegment → SubtitleCue（字段一一对应）
      ├─ record::generate_srt(&cues) → srt_text
      ├─ 组装 SubtitleResult { cues, srt_text, model, track_used }
      ├─ emit("record://subtitle-progress", Finalizing)
      ├─ UPDATE recordings SET subtitle_cues=?, subtitle_srt=?, subtitle_model=? WHERE id=?
      ├─ emit("record://subtitle-progress", Done { cue_count })
      └─ 返回 SubtitleResult 给前端
[6] 前端展开历史项 → 显示 cue 列表 + 导出 SRT 按钮
```

### 2.2 分层职责

| 层 | crate | 职责 | 复用/新增 |
|---|---|---|---|
| ASR 核心 | `asr-local` | PCM → 带时间戳段列表（文本 + 后处理 + 时间区间） | **扩展**：pipeline 加 timestamp 函数 |
| 录屏业务 | `record` | mp4 → PCM（ffmpeg 抽轨）；SRT 格式生成；选轨；字幕数据模型 | **新增**：subtitle.rs 模块 |
| 命令层 | `desktop` | Tauri 命令编排 + DB 读写 + 事件 emit | **新增**：generate/export/get 命令 |
| 展示 | `frontend` | 历史项 cue 预览 + 导出按钮 | **新增**：UI 组件 |

### 2.3 依赖方向（采用方案 B）

```
infra ← record ← desktop
infra ← asr-local ← desktop
            ↑
    record 不依赖 asr-local
    desktop 编排两者
```

- `crates/record/src/subtitle.rs` 只做：**mp4 → PCM（ffmpeg 抽轨）+ SRT 格式生成 + 选轨 + 字幕数据模型**（无 ASR 依赖）
- `crates/asr-local/src/pipeline.rs` 扩展：**PCM → Vec<TimestampedSegment>**（带时间戳，复用 VAD/transcribe/后处理）
- `crates/desktop/src/record_commands.rs::generate_subtitle`：编排——抽 PCM → 调 ASR → 转 SubtitleCue → 入 DB → emit

**复用现有模式**：与 `merge_audio_tracks`（desktop 调 record + ffmpeg amix）一致。

## 3. 数据模型

> **⚠️ v2 架构变更（2026-07-28 e2e 后）**：存储模型从「DB 三列」改为「**SRT 文件**」。
> 原因：DB 存字幕是过度设计——字幕本质是文件（给外部工具用），存 DB 后还要导出，多此一举。
> 文件方案更简单：`.srt` 与 mp4 同目录同名，直接可被 VLC/剪映/Premiere 加载。
>
> **v2 决策**：
> - SRT 文件位置：与 mp4 同目录同名，命名 `xxx.N.srt`（N 从 1 递增，每次生成都新建不覆盖）
> - cue 预览来源：读最新的 `.srt` 文件解析回 cue 列表（SRT 是唯一真相源）
> - **回退 schema v54→v53**：删 db.sql 的 `subtitle_cues`/`subtitle_srt`/`subtitle_model` 三列 + RecordingMeta 三字段 + `update_subtitle` 方法 + `get_subtitle` 命令
> - 新增 `read_subtitle` 命令：读最新 `.srt` 文件解析为 cue 列表给前端
>
> 下方 §3.1（DB schema v54）**已废弃**，保留作为历史记录。实际实现以本段 v2 决策为准。

### 3.1 存储：SRT 文件（v2，不存 DB）

字幕不存数据库，直接生成 `.srt` 文件与 mp4 同目录同名：

- 命名：`<mp4_stem>.<N>.srt`（N 从 1 递增，每次生成都新建不覆盖）
- 最新版本：N 最大的那个（`latest_srt_path` 扫描取 max N）
- cue 预览来源：读最新 `.srt` 文件解析（`parse_srt`）
- schema 无变更（v1 曾加 DB 三列 v54，v2 已回退到 v53）

### 3.2 Rust 结构（分两 crate）

**字幕数据模型 + SRT/选轨在 record crate 定义**（避免 record 依赖 asr-local）：

```rust
// crates/record/src/subtitle.rs

// 字幕数据模型（跨 Tauri 边界的 DTO）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]   // Tauri 边界 camelCase
pub struct SubtitleCue {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleResult {
    pub cues: Vec<SubtitleCue>,
    pub srt_text: String,
    pub model: String,
    pub track_used: AudioTrackSource,  // 用于前端 fallback 提示
    /// LLM 润色结果（None=未尝试润色）。详见 subtitle-llm-polish spec。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polish_outcome: Option<String>,  // "polished"/"fallbackRatio"/"noLlmConfig"/"failed:msg"
}

// 进度阶段（emit 给前端）
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "stage", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum SubtitleProgress {
    ExtractingAudio { percent: u32 },    // 抽音轨 0~30%
    Recognizing { percent: u32 },        // ASR 中 30~40%
    Polishing { percent: u32 },          // LLM 润色 40~90%（subtitle-llm-polish spec 加）
    Finalizing { percent: u32 },         // 组装写文件 90~100%
    Done { cue_count: usize },           // 完成
    Error { message: String },           // 失败
}

// 选轨偏好
pub enum TrackPreference { Auto, Microphone, System }
```

**v2 新增文件读写函数**（record crate）：

```rust
pub fn next_srt_path(mp4_path: &Path) -> PathBuf      // 算下一个 xxx.N.srt（扫已有取 max N + 1）
pub fn latest_srt_path(mp4_path: &Path) -> Option<PathBuf>  // 找最新版本（N 最大）
pub fn parse_srt(text: &str) -> Vec<SubtitleCue>      // 解析 SRT 文本为 cue 列表（容错）
```

**带时间戳段在 asr-local 定义**（内部类型，不跨 Tauri 边界，snake_case）：

```rust
// crates/asr-local/src/pipeline.rs (扩展)

// 内部类型，desktop 编排时转为 SubtitleCue
pub struct TimestampedSegment {          // snake_case，非 DTO
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

pub struct VadSegment {
    pub offset_samples: usize,
    pub samples: Vec<f32>,
}
```

**注意**：现有 `segment_audio_vad` 位于 `crates/asr-local/src/audio.rs`（非 pipeline.rs），签名是 `segment_audio_vad(samples, vad: &mut SileroVad, frame_size, threshold, min_silence, max_speech)` —— 改造时新增 `segment_audio_vad_with_offsets` 同样放在 `audio.rs`，签名参数对齐。

### 3.3 SRT 格式（生成器规格）

标准 SRT 格式（兼容 QuickTime / VLC / 剪映 / Premiere）：

```
1
00:00:01,234 --> 00:00:03,567
第一句字幕文本

2
00:00:04,000 --> 00:00:06,500
第二句字幕文本

```

- 序号从 1 开始
- 时间格式 `HH:MM:SS,mmm`（毫秒用逗号，SRT 标准）
- 每条 cue 之间空行分隔
- 文件末尾保留一个换行

### 3.5 选轨规则（在 record/subtitle.rs 实现）

```rust
fn select_track(meta: &RecordingMeta, preference: TrackPreference) -> Result<(usize, AudioTrackSource)> {
    // 返回 (track_index, track_source)，source 用于前端 fallback 提示
    match preference {
        Auto | Microphone => meta.audio_tracks.iter()
            .find(|t| t.source == AudioTrackSource::Microphone)
            .map(|t| (t.index, AudioTrackSource::Microphone))
            .or_else(|| fallback_system_or_first(meta)),   // mic 没有则 fallback
        System => meta.audio_tracks.iter()
            .find(|t| t.source == AudioTrackSource::System)
            .map(|t| (t.index, AudioTrackSource::System))
            .ok_or(Error::NoSystemTrack),
    }
    .ok_or(Error::NoAudioTrack)
}
```

**fallback 规则**：用户选 Auto/Microphone 但实际没有 mic track → 自动降级到 system track（再不行降级到第一条 track），并在 `SubtitleResult.track_used` 字段标明实际用轨，前端据此提示「未检测到麦克风音轨，已使用系统音轨」。

## 4. 核心算法——VAD 段级时间戳

### 4.1 现状

现有 VAD 分段函数在 `crates/asr-local/src/audio.rs`（不是 pipeline.rs）：

```rust
// crates/asr-local/src/audio.rs
pub fn segment_audio_vad(
    samples: &[f32],
    vad: &mut SileroVad,
    frame_size: usize,      // 480
    threshold: f32,          // 0.4
    min_silence: usize,      // 500
    max_speech: usize,       // 25000
) -> Vec<Vec<f32>>          // ← 纯音频块，无 offset
```

pipeline.rs 的 `transcribe_segments`（私有函数）调用它，遍历分段逐个 `engine.transcribe(seg)`，连接文本时**丢弃了每段在原音频里的偏移量**。短音频（≤480k samples = 30s）跳过 VAD 直连引擎。

### 4.2 改造目标

在 `audio.rs` 新增带 offset 的分段函数（**不改原函数**，避免影响 `transcribe_segments`）：

```rust
// crates/asr-local/src/audio.rs (新增)
pub fn segment_audio_vad_with_offsets(
    samples: &[f32],
    vad: &mut SileroVad,
    frame_size: usize,
    threshold: f32,
    min_silence: usize,
    max_speech: usize,
) -> Vec<VadSegment>                        // ← 带 offset
```

**算法**：
1. 复用原 `segment_audio_vad` 的内部状态机逻辑（静音/语音边界判定），但记录每段在源音频中的起始样本偏移
2. `offset_samples = 段起始位置`，`samples = samples[start..end].to_vec()`
3. 转毫秒：`start_ms = offset_samples / 16.0`（16k 采样率 = 16 样本/毫秒）；`end_ms = (offset_samples + len) / 16.0`

⚠️ **实现注意**：现有 `segment_audio_vad` 内部维护静音/语音状态机。要拿 offset 必须改其内部循环（或在循环里同时维护「已消费样本计数器」）。**不修改原函数签名**，而是把核心循环抽成一个内部 helper（接收闭包回调决定产出 `Vec<f32>` 还是 `(offset, Vec<f32>)`），两个公开函数共享同一个 helper。这样：
- 原 `segment_audio_vad` 行为 100% 不变（回归测试保护）
- 新 `segment_audio_vad_with_offsets` 复用同一套边界判定逻辑

### 4.3 时间戳精度与边界

VAD 分段基于「语音/静音边界」：一段连续语音（中间无明显停顿）= 一条 cue。

**边界处理**：
- 超长段（>10s 无停顿）→ 保持一条 cue（不强制拆分，超长是演讲/朗读的特征）
- 极短段（<500ms 噪声）→ 过滤掉（VAD 误判，避免字幕闪烁）
- 段间静音 → 不生成 cue（静音不出字幕）

### 4.4 带时间戳的 transcribe（pipeline.rs 新增）

```rust
// crates/asr-local/src/pipeline.rs (新增)
pub fn transcribe_segments_with_timestamps(
    engine: &dyn OfflineAsrEngine,           // ← 注意是 trait 对象，与 transcribe_batch 一致
    samples: &[f32],
    cfg: &PipelineConfig,                    // ← 注意是 PipelineConfig（不是 AsrConfig）
) -> Result<Vec<TimestampedSegment>> {
    // 1. VAD 分段（带 offset）—— VAD 初始化失败时降级：整段一条 cue
    let vad = crate::config::create_silero_vad();
    let segments = match vad {
        Ok(mut v) => crate::audio::segment_audio_vad_with_offsets(
            samples, &mut v, 480, 0.4, 500, 25000),
        Err(e) => {
            log::warn!("VAD 初始化失败，整段作为单条 cue: {}", e);
            vec![VadSegment { offset_samples: 0, samples: samples.to_vec() }]
        }
    };

    // 2. 逐段 transcribe + 后处理（复用 transcribe_batch 的同套逻辑）
    let mut result = Vec::with_capacity(segments.len());
    for seg in &segments {
        let raw = engine.transcribe(&seg.samples, &cfg.language)?;
        // 后处理（与 transcribe_batch §58-79 同套：corrector + ITN + hans）
        let text = postprocess_segment(raw, engine, cfg);

        // 过滤 <500ms 段（噪声）+ 空文本段
        let dur_samples = seg.samples.len();
        let dur_ms = (dur_samples as f64 / 16.0).round() as u64;
        if dur_ms < 500 || text.trim().is_empty() { continue; }

        let start_ms = (seg.offset_samples as f64 / 16.0).round() as u64;
        let end_ms = ((seg.offset_samples + dur_samples) as f64 / 16.0).round() as u64;
        result.push(TimestampedSegment { start_ms, end_ms, text });
    }
    Ok(result)
}
```

**后处理复用**：`transcribe_batch` 的 corrector + ITN + hans 逻辑（§58-79）应抽成一个私有 helper `postprocess_segment(raw_text, engine, cfg) -> String`，让 `transcribe_batch` 和 `transcribe_segments_with_timestamps` 共享——避免后处理逻辑重复。这是「顺手改进工作面」的合理重构（spec brainstorming skill 要求），不改变 `transcribe_batch` 行为。

**关键复用**：`engine.transcribe(seg, language)`、corrector、ITN、hans 全部复用现有实现，零重复。

### 4.5 短音频路径（≤30s）

**统一走 VAD 分段**。短音频如果整段一条 cue 没意义，VAD 即使对 5s 音频也能切出 1~3 段，符合字幕场景。这样也简化了分支逻辑（无需判断音频长度）。

### 4.6 时间戳换算约定

```
sample_rate = 16000 Hz
1 ms = 16 samples
ms = samples / 16 (然后 round)
```

## 5. 接口设计

### 5.1 record crate 公开 API

`crates/record/src/subtitle.rs`（新增模块，纯录屏业务，无 ASR 依赖）：

```rust
// 字幕数据模型（§3.2）
pub struct SubtitleCue { pub start_ms: u64, pub end_ms: u64, pub text: String }
pub struct SubtitleResult { pub cues: Vec<SubtitleCue>, pub srt_text: String, pub model: String, pub track_used: AudioTrackSource }
pub enum SubtitleProgress { ... }
pub enum TrackPreference { Auto, Microphone, System }

// 选轨
pub fn select_track(meta: &RecordingMeta, pref: TrackPreference) -> Result<(usize, AudioTrackSource)>;

// 抽音轨 → 16k mono PCM
pub fn extract_audio_track_to_pcm(
    mp4_path: &Path,
    track_index: usize,
    ffmpeg_path: &Path,
) -> Result<Vec<f32>>;

// SRT 格式化
pub fn generate_srt(cues: &[SubtitleCue]) -> String;
```

**实现要点**：
- `extract_audio_track_to_pcm` 用 ffmpeg：`-map 0:a:<idx> -ar 16000 -ac 1 -f f32le pipe:1`，读 stdout → `Vec<f32>`
- 复用 `crates/desktop/src/record_commands.rs::probe_ffmpeg` 路径解析（或将该函数下沉到 record crate，复用度更高）
- `generate_srt` 纯格式化：cue 序号从 1 开始，时间 `HH:MM:SS,mmm`，cue 间空行

### 5.2 asr-local 扩展 API

`crates/asr-local/src/pipeline.rs`（扩展，加时间戳能力）：

```rust
pub struct TimestampedSegment { pub start_ms: u64, pub end_ms: u64, pub text: String }

pub fn transcribe_segments_with_timestamps(
    samples: &[f32],
    cfg: &AsrConfig,
) -> Result<Vec<TimestampedSegment>>;
```

**实现**：内部调 `crate::audio::segment_audio_vad_with_offsets` + 逐段 `engine.transcribe` + 共享的 `postprocess_segment`（corrector + ITN + hans，§4.4）。

**注意**：入参是 `engine: &dyn OfflineAsrEngine` + `cfg: &PipelineConfig`（与现有 `transcribe_batch` 完全一致的签名风格）。desktop 命令层需要先获取一个 `AsrEngineManager` 实例拿到 `OfflineAsrEngine` 实现 + 从 `app_config` 构造 `PipelineConfig`（用 `PipelineConfig::from_app_config(language)`）后传入。

### 5.3 Tauri 命令（desktop/record_commands.rs）

**命令 1：生成字幕**（v2 + LLM 润色）

```rust
#[tauri::command]
pub async fn generate_subtitle(
    app: AppHandle,
    engine_manager: State<'_, Arc<AsrEngineManager>>,
    id: i64,
    track: Option<String>,        // "microphone" | "system" | None(=Auto)
    polish: Option<PolishOption>, // None=不润色；Some=润色（详见 subtitle-llm-polish spec）
) -> Result<SubtitleResult, String> {
    // 1-8. 查 DB → select_track → extract PCM → ASR transcribe_segments_with_timestamps
    // 8.5. LLM 润色（polish=Some 时，详见 subtitle-llm-polish spec）
    // 9. emit(Finalizing)
    // 10. 转 SubtitleCue + generate_srt → SubtitleResult
    // 11. 写 SRT 文件：next_srt_path(mp4) → xxx.N.srt（不存 DB）
    // 12. emit(Done { cue_count })
}
```

**命令 2：读取最新字幕**（v2：从文件解析）

```rust
#[tauri::command]
pub async fn read_subtitle(id: i64) -> Result<Option<SubtitleResult>, String> {
    // 查 DB 拿 mp4 路径 → latest_srt_path → 读文件 + parse_srt → SubtitleResult
    // 不存在 → None
}
```

**命令 3：在 Finder 显示 SRT 文件**（v2：替代导出）

```rust
#[tauri::command]
pub async fn reveal_subtitle(id: i64) -> Result<String, String> {
    // latest_srt_path → open -R（Finder 高亮）
}
```

### 5.4 Tauri 事件（emit 给前端）

事件名 `record://task`（复用现有录屏事件流），payload 是 `RecordTaskEvent` enum：

```typescript
// 前端 listen("record://task", (msg) => { const e = msg.payload; ... })
// subtitle 相关变体（外层 kebab-case tag + 变体字段 camelCase）：
type SubtitleTaskEvent =
  | { event: "subtitle-started"; id: number }
  | { event: "subtitle-progress"; id: number; stage: SubtitleProgressPayload }
  | { event: "subtitle-done"; id: number; cueCount: number }
  | { event: "subtitle-failed"; id: number; error: string };

// stage 是 SubtitleProgress enum 序列化（嵌套在 progress 变体里）
type SubtitleProgressPayload =
  | { stage: "extracting-audio"; percent: number }
  | { stage: "recognizing"; percent: number }
  | { stage: "polishing"; percent: number }      // LLM 润色阶段
  | { stage: "finalizing"; percent: number }
  | { stage: "done"; cueCount: number }
  | { stage: "error"; message: string };
```

**事件名遵循现有 `record://` 前缀**（与 `record://event` 一致），payload 是 enum 外层 kebab-case tag + 变体 camelCase（AGENTS.md casing 规范）。

### 5.5 错误处理与降级

| 失败场景 | 行为 |
|---|---|
| ffmpeg 未找到 | emit Error + 前端 toast「未找到 ffmpeg，无法提取音轨」|
| ffmpeg 抽轨失败（mp4 损坏/无指定 track） | emit Error + toast「音轨提取失败：{原因}」|
| 录制无任何 audio track | 命令直接返回 Err（前端按钮置灰 + tooltip「该录制无音轨」）|
| ASR 模型未加载/失败 | emit Error + toast「语音识别失败：{原因}」|
| VAD 分段为空（完全无声） | 返回空 cues + srt_text = ""，前端显示「未检测到语音内容」|
| 字幕已存在，重复点「生成字幕」 | 覆盖（UPDATE 覆盖旧值，幂等） |

### 5.6 序列化 casing 核验

| 层 | casing | 实例 |
|---|---|---|
| `SubtitleCue` DTO（Tauri 返回值） | camelCase | `startMs / endMs / text` |
| `SubtitleResult` DTO | camelCase | `cues / srtText / model / trackUsed` |
| `SubtitleProgress` 事件 enum | 外层 kebab + 内层 camelCase | `{stage:"recognizing", percent:45}` |
| `TimestampedSegment`（asr-local 内部，非 DTO） | snake_case | `start_ms / end_ms`（不跨 Tauri 边界） |

⚠️ AGENTS.md casing 教训：前端写 `srtText` 但后端无 rename_all 会序列化 `srt_text` → 前端 undefined。所有跨 Tauri 边界的 struct 必须 `#[serde(rename_all = "camelCase")]`。

## 6. 前端 UI

按 AGENTS.md「涉及前端 UI 必须用 frontend-design skill」准则，本节是设计意图层面的描述，具体视觉设计在实施阶段（Phase 4）用 frontend-design skill 落实。

### 6.1 入口位置

录屏历史列表现有的每个录制项（网格/列表视图），在「...」菜单或卡片操作区加入「生成字幕」入口：

- **未生成字幕时**：显示「生成字幕」按钮（或菜单项）
- **已生成字幕时**：显示「查看字幕」（展开 cue 列表）+「重新生成」+「导出 SRT」
- **无 audio track 时**：按钮置灰 + tooltip「该录制无音轨」

### 6.2 字幕生成流程 UX

**触发**：点击「生成字幕」
**进度反馈**：卡片上叠加一个轻量的进度状态条（不阻塞整个历史列表，只在该卡片上）

```
阶段 1：🎵 提取音轨...  [▬▬▬░░░░░] 20%
阶段 2：🎤 识别中...    [▬▬▬▬▬░░░] 65%
阶段 3：✨ 生成字幕...  [▬▬▬▬▬▬▬░] 92%
完成：✅ 字幕已生成（共 23 条）    [查看字幕] [导出 SRT]
```

- 进度通过 listen `record://subtitle-progress` 更新
- 失败时显示「❌ 字幕生成失败：{message}」+「重试」按钮
- 生成期间禁用「生成字幕」按钮（防重复点击）

### 6.3 cue 列表预览

字幕生成成功后，点击「查看字幕」展开 cue 列表面板：

```
┌─────────────────────────────────────────────────┐
│  字幕（共 23 条）         模型：sensevoice      │
│  ─────────────────────────────────────────────  │
│  1  00:00:01 → 00:00:03   大家好今天给大家介绍  │
│     ─────────────────────────────────────────  │
│  2  00:00:04 → 00:00:07   如何使用这个新功能    │
│     ─────────────────────────────────────────  │
│  3  00:00:08 → 00:00:12   首先我们来看一下界面  │
│  ...                                            │
│                                                 │
│  [复制全部]  [导出 SRT]  [重新生成]            │
└─────────────────────────────────────────────────┘
```

- 每条 cue 显示：序号、时间区间（`HH:MM:SS` 起止，紧凑格式）、文本
- 单击 cue：复制该条文本到剪贴板
- 「复制全部」：复制整段纯文本（不含 SRT 时间戳，只文本拼接）
- 「导出 SRT」：触发 save dialog → 调 `export_subtitle` → toast 成功
- 「重新生成」：确认弹窗（覆盖现有字幕？）→ 再走一遍 generate

### 6.4 时间显示格式

预览面板用紧凑格式（节省横向空间）：`00:00:01 → 00:00:03`
SRT 文件用标准格式：`00:00:01,234 --> 00:00:03,567`

### 6.5 fallback 提示

如果 `track_used` 字段显示用了 system track（而非用户预期的 microphone），cue 预览面板顶部加一行温和提示：

```
ℹ️ 未检测到麦克风音轨，已使用系统音轨识别
```

### 6.6 前端 interface（TS）

```typescript
// crates/desktop/frontend/src/types/record.ts (扩展)
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

type SubtitleProgress =
  | { stage: 'extracting-audio'; percent: number }
  | { stage: 'recognizing'; percent: number }
  | { stage: 'finalizing'; percent: number }
  | { stage: 'done'; cueCount: number }
  | { stage: 'error'; message: string };
```

### 6.7 i18n

新增 zh-CN / en 文案键：
- `record.subtitle.generate` / `record.subtitle.view` / `record.subtitle.regenerate`
- `record.subtitle.export` / `record.subtitle.copyAll` / `record.subtitle.copyOne`
- `record.subtitle.progress.*`（5 个阶段文案）
- `record.subtitle.error.*`（错误文案）
- `record.subtitle.fallback.systemTrack`
- `record.subtitle.empty`（无语音内容）

## 7. 测试策略

遵循 AGENTS.md TDD 准则。分三层：纯逻辑单测（TDD 先行）、集成测试、手动 e2e。

### 7.1 record crate 单测（纯逻辑，TDD）

**文件**：`crates/record/src/subtitle.rs` 内联 `#[cfg(test)] mod tests`

| 测试 | 覆盖点 | TDD |
|---|---|---|
| `test_generate_srt_basic` | 3 条 cue → 标准 SRT 格式（序号、`HH:MM:SS,mmm`、空行分隔） | ✅ 先写 |
| `test_generate_srt_empty` | 0 条 cue → 空字符串（不崩溃） | ✅ 先写 |
| `test_generate_srt_single` | 1 条 cue → 末尾换行正确 | ✅ 先写 |
| `test_generate_srt_time_format` | 边界时间戳：0ms、超过 1 小时、毫秒四舍五入 | ✅ 先写 |
| `test_select_track_microphone` | 双轨 meta → Auto/Microphone 选 mic track | ✅ 先写 |
| `test_select_track_system` | 双轨 meta → System 选 system track | ✅ 先写 |
| `test_select_track_fallback` | 无 mic 的 meta → Auto fallback 到 system + 返回 track_used | ✅ 先写 |
| `test_select_track_no_audio` | 空 audio_tracks → 返回 NoAudioTrack 错误 | ✅ 先写 |
| `test_select_track_only_system` | 只有 system track → Auto 选 system | ✅ 先写 |

**`extract_audio_track_to_pcm` 不写单测**（依赖外部 ffmpeg + 真实 mp4，归 e2e）。

### 7.2 asr-local crate 单测（VAD + 时间戳，TDD）

**文件**：`crates/asr-local/src/audio.rs` + `crates/asr-local/src/pipeline.rs` 内联 `#[cfg(test)] mod tests`

| 测试 | 覆盖点 | TDD |
|---|---|---|
| `test_segment_vad_with_offsets_basic` | 合成 PCM（语音段 + 静音段交替）→ offset 正确累加 | ✅ 先写 |
| `test_segment_vad_with_offsets_all_silence` | 全静音 → 返回空 Vec | ✅ 先写 |
| `test_segment_vad_with_offsets_all_speech` | 全语音 → 单段 offset=0 | ✅ 先写 |
| `test_transcribe_timestamps_ms_conversion` | 已知 offset_samples → start_ms/end_ms 正确换算（samples/16） | ✅ 先写 |
| `test_transcribe_timestamps_filters_empty` | 含空文本段（ASR 输出空）→ 过滤掉 | ✅ 先写 |
| `test_transcribe_timestamps_filters_short` | <500ms 段 → 过滤（噪声） | ✅ 先写 |
| `test_transcribe_timestamps_real_model` | 真实模型 + 真实 wav → 输出非空 cue（回归测试，参考 Paraformer fbank 测试模式） | 事后（需模型文件） |

### 7.3 desktop crate 测试

**文件**：`crates/desktop/src/record_commands.rs` 内联测试

| 测试 | 覆盖点 | 类型 |
|---|---|---|
| DB schema v53→v54 | 升常量后现有 `init_schema_fresh_db_builds_current_version` 自动验证（无需额外测试） | 集成（现有） |
| `RecordingMeta` 序列化/反序列化 | subtitle_cues JSON ↔ Vec<SubtitleCue> 往返 | 单测 |
| `SubtitleCue` casing | 序列化输出 `startMs`（非 `start_ms`） | 单测（防 Task 4.1 casing bug） |

### 7.4 手动 e2e 验证清单（不自动化）

- [ ] 5 分钟带麦克风讲解的录屏 → 点「生成字幕」→ ≤60 秒完成 → SRT 在 VLC/QuickTime 正确加载
- [ ] 时间戳对齐：随机抽 3 条 cue，视频跳到该时间点听到对应语音（偏差 ≤500ms）
- [ ] 双轨录制 → 默认选 mic track → cue 预览无 fallback 提示
- [ ] 单 system track 录制 → 自动 fallback → cue 预览顶部显示「已使用系统音轨」
- [ ] 无声录制 → 友好提示「未检测到语音内容」
- [ ] 重复点「生成字幕」→ 覆盖旧结果（不重复 cue）
- [ ] 导出 SRT → 文件可在剪映/Premiere 导入

### 7.5 测试不覆盖项（明确边界）

- **不测**：ffmpeg 二进制行为（外部依赖，e2e 验证）
- **不测**：ONNX 模型加载（asr-local 已有覆盖）
- **不测**：前端 UI 视觉（手动 e2e）
- **不测**：性能基准（无量化目标，MVP 不做 benchmark）

## 8. 实施分阶段

把整个工作切成 4 个阶段，每阶段独立可验证、可 commit。

### Phase 1：DB schema + 数据模型（地基）

**范围**：migration、Rust 结构、SRT 生成器、选轨逻辑。纯逻辑、纯测试。

| 任务 | 文件 | 验证 |
|---|---|---|
| 1.1 DB schema v53→v54（**机制已简化，无迁移函数**）：db.sql 加 3 列 + 升 `CURRENT_SCHEMA_VERSION` + 升注释 | `crates/infra/src/db.sql` + `crates/infra/src/db.rs` | 现有 `init_schema_fresh_db_builds_current_version` 测试自动验证 v54 |
| 1.2 `SubtitleCue` / `SubtitleResult` / `SubtitleProgress` / `TrackPreference` 定义 | `crates/record/src/subtitle.rs`（新建） | 编译通过 |
| 1.3 `generate_srt()` 纯格式化函数 | `crates/record/src/subtitle.rs` | 4 个 TDD 测试 |
| 1.4 `select_track()` 选轨逻辑 | `crates/record/src/subtitle.rs` | 5 个 TDD 测试 |
| 1.5 `RecordingMeta` 扩展 3 字段 + 序列化适配 | `crates/record/src/store.rs` | 序列化往返测试 + casing 测试 |
| 1.6 record crate `lib.rs` 导出新模块 | `crates/record/src/lib.rs` | `cargo build -p octopus-record` |

**Phase 1 出口**：`cargo test -p octopus-record` + `cargo test -p octopus-infra` 全过；DB schema v54；SRT 生成器 + 选轨逻辑单测覆盖。

### Phase 2：VAD 时间戳能力（asr-local 扩展）

**范围**：给 pipeline 加时间戳能力。纯 ASR 逻辑，可独立测试。

| 任务 | 文件 | 验证 |
|---|---|---|
| 2.1 `VadSegment` + `segment_audio_vad_with_offsets` | `crates/asr-local/src/audio.rs`（新增） | 3 个 TDD 测试（basic/all_silence/all_speech） |
| 2.2 `TimestampedSegment` + `transcribe_segments_with_timestamps` | `crates/asr-local/src/pipeline.rs`（新增） | 3 个 TDD 测试（ms_conversion/filters_empty/filters_short） |
| 2.3 抽 `postprocess_segment` helper（让 `transcribe_batch` 与新函数共享 corrector/ITN/hans） | `crates/asr-local/src/pipeline.rs`（顺手重构） | `transcribe_batch` 回归测试不破 |
| 2.4 短音频统一走 VAD 分段（§4.5） | 同上 | 测试覆盖短音频路径 |

**Phase 2 出口**：`cargo test -p octopus-asr-local` 全过；带时间戳 transcribe 可用；原 `transcribe_segments` 不受影响。

### Phase 3：编排层（desktop 命令 + ffmpeg 抽轨）

**范围**：Tauri 命令、ffmpeg 抽 PCM、DB 读写、事件 emit。

| 任务 | 文件 | 验证 |
|---|---|---|
| 3.1 `extract_audio_track_to_pcm` | `crates/record/src/subtitle.rs` | 编译通过（e2e 验证） |
| 3.2 `probe_ffmpeg` 下沉到 record crate（或保持 desktop 调用） | `crates/record/src/subtitle.rs` 或 `crates/desktop/...` | 复用度评估 |
| 3.3 `generate_subtitle` 命令 | `crates/desktop/src/record_commands.rs` | 编译 + 手动调用 |
| 3.4 `export_subtitle` 命令 | 同上 | 编译 + 手动调用 |
| 3.5 `get_subtitle` 命令 | 同上 | 编译 + 手动调用 |
| 3.6 `record://subtitle-progress` 事件 emit | 同上 | 前端能 listen |
| 3.7 capabilities/default.json 加新命令权限 | `crates/desktop/capabilities/default.json` | invoke 不被拒 |

**Phase 3 出口**：3 个 Tauri 命令可从前端调用；ffmpeg 抽轨 + ASR + DB 入库全链路通；事件正确 emit。

### Phase 4：前端 UI（frontend-design skill）

**范围**：历史项入口、进度反馈、cue 预览面板、导出/复制交互、i18n。

| 任务 | 文件 | 验证 |
|---|---|---|
| 4.1 TS interface（SubtitleCue/SubtitleResult/SubtitleProgress） | `frontend/src/types/record.ts` | tsc 通过 |
| 4.2 invoke 封装（generate/export/get + listen progress） | `frontend/src/lib/record.ts` 或同等位置 | tsc 通过 |
| 4.3 历史项「生成字幕」入口 + 状态切换 | RecordHistory 组件 | 手动 e2e |
| 4.4 进度状态条（卡片内叠加） | 同上 | 手动 e2e |
| 4.5 cue 预览面板（展开/折叠、单击复制、复制全部、导出、重新生成） | 新组件 | 手动 e2e |
| 4.6 fallback 提示（track_used 显示） | cue 预览面板 | 手动 e2e |
| 4.7 i18n 文案（zh-CN + en） | `locales/` | 无未翻译键 |
| 4.8 错误降级 UI（无音轨/无声/ffmpeg 缺失等） | 多处 | 手动 e2e |

**Phase 4 出口**：完整手动 e2e 7 项全过；tsc + vite build 0 error。

### 阶段依赖与并行性

```
Phase 1 (DB + 模型) ──┬──→ Phase 3 (编排) ──→ Phase 4 (前端)
                       │
Phase 2 (VAD 时间戳) ──┘
```

- Phase 1 和 Phase 2 可并行（不同 crate，无相互依赖）
- Phase 3 依赖 Phase 1 + Phase 2
- Phase 4 依赖 Phase 3

### 整体出口（spec 验收标准）

对齐 §1 成功标准：
1. 5 分钟带麦录屏 → 点「生成字幕」→ ≤60 秒完成 SRT
2. SRT 在 QuickTime / VLC / 剪映正确加载，时间戳对齐 ≤500ms 偏差
3. 幂等（重复生成覆盖）
4. 无 mic → system fallback + 提示
5. 无声录制 → 友好提示，不崩溃
6. `cargo test` 全过 + tsc/vite 0 error

## 9. 调研事实基础（brainstorming 调研结论）

本 spec 基于以下代码现状调研（2026-07-28）：

| 维度 | 现状 | 对字幕功能的影响 |
|---|---|---|
| mp4 音轨结构 | 双轨 AAC 48k stereo（mic=track0, system=track1） | ASR 应喂 mic track；需 ffmpeg 按轨抽 PCM |
| 音轨元数据 | DB `audio_tracks` JSON 列已存 source/codec/rate/channels | 选轨逻辑可直接查 DB，无需重新 ffprobe |
| 录后 hook | ffmpeg 路径解析 + spawn 基础设施完备（merge/gif 已验证） | 抽音轨命令链路可直接复用 |
| ASR 输入格式 | 16k mono f32 PCM（hound+rubato 已能从 wav 产出） | ffmpeg `-ar 16000 -ac 1 -f f32le` 即可对接 |
| 离线 transcribe | `transcribe_batch` (pipeline.rs:46-80) 整段→纯文本 | 可用但无时间戳 |
| 流式 transcribe | `StreamingSession::accept_samples` chunk→累积文本 | 可用但无时间戳（本 spec 不用流式） |
| VAD 分段 | `audio::segment_audio_vad` (audio.rs) 返回纯音频块，丢弃 offset | **改造点**：新增 `segment_audio_vad_with_offsets` |
| SRT/VTT 生成 | 完全没有 | 需新建（§3.4 + §5.1） |
| 时间戳格式化 | 完全没有 | 需新建（§3.4） |
| DB 字幕存储 | 无 transcript/subtitle 列（当前 schema v53） | 需加 schema v53→v54（§3.1，机制已简化为只改 db.sql + 升常量） |

关键代码引用：
- 音轨写入：`crates/record/native/macos/Sources/OctopusSckHelperLib/ScreenCaptureRecorder.swift:362-401`
- 协议 payload：`crates/record/src/protocol.rs:60-74`
- 录后处理 hook：`crates/desktop/src/record_commands.rs:997-1147`（merge_audio_tracks）
- 离线 pipeline：`crates/asr-local/src/pipeline.rs:46-146`（`transcribe_batch` + `transcribe_segments`）
- **VAD 分段（改造点）**：`crates/asr-local/src/audio.rs::segment_audio_vad`（5 参数：`samples, vad, frame_size, threshold, min_silence, max_speech`）
- 后处理逻辑（corrector + ITN + hans）：`crates/asr-local/src/pipeline.rs:58-79`（在 `transcribe_batch` 内联，需抽 helper）
- 配置类型：`crates/asr-local/src/pipeline.rs:18-39`（`PipelineConfig`，非 AsrConfig）
- ffmpeg 路径解析：`crates/desktop/src/record_commands.rs:781-799`
