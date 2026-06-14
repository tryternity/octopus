# 设计文档：octopus-desktop 桌面应用（V2）

> 基于 Tauri 2.x 构建的独立桌面语音识别应用，支持流式识别（边说边识别）和 VAD 伪流式分段识别。

## 0. 背景

octopus-desktop V1 已完成基础功能：全局快捷键、录音 overlay、离线识别、自动粘贴。
V2 新增：

- **流式识别**：Paraformer/Zipformer 引擎支持边说边识别，实时显示识别文本
- **结果展示窗口**：类输入法样式的浮动窗口，可拖拽、多行滚动显示
- **VAD 标点**：基于 VAD 静音检测自动插入逗号/句号
- **VAD 伪流式**：非流式引擎（SenseVoice/Whisper/Qwen3-ASR）使用 VAD 分段识别，体验接近流式
- **可编辑结果窗口**：用户可直接编辑识别文本，编辑内容实时同步到 record.txt
- **文本持久化**：识别文本写入 `~/.octopus/record.txt`，清空时归档到 `~/.octopus/history.txt`（最多 20 条）

## 1. 目标与约束

### 1.1 功能范围

| 功能 | V1 | V2 |
|------|----|----|
| 全局快捷键 + Overlay | ✅ | ✅ |
| 离线识别（全量录音→一次识别） | ✅ | ✅ |
| 自动粘贴输出 | ✅ | ✅ |
| 三种引擎模式 | ✅ | ✅ |
| 流式识别（Paraformer/Zipformer） | ❌ | ✅ |
| 结果展示窗口（可拖拽、多行滚动） | ❌ | ✅ |
| VAD 静音检测标点 | ❌ | ✅ |
| VAD 伪流式（非流式引擎分段识别） | ❌ | ✅ 已实现 |
| 可编辑结果窗口 | ❌ | ✅ contenteditable |
| 文本持久化（record.txt + history.txt） | ❌ | ✅ |

### 1.2 引擎识别模式

| 引擎 | 离线模式 | 流式模式 | 伪流式模式 |
|------|----------|----------|------------|
| Paraformer | — | ✅ 天然流式 | — |
| Zipformer | — | ✅ 天然流式 | — |
| SenseVoice | ✅ | — | ✅ VAD 分段 |
| Whisper | ✅ | — | ✅ VAD 分段 |
| Qwen3-ASR | ✅ | — | ✅ VAD 分段 |

## 2. 架构概览

```
┌──────────────────────────────────────────────────────────────────┐
│                    octopus-desktop (Tauri 2.x)                   │
│                                                                  │
│  ┌──────────┐  ┌───────────┐  ┌──────────┐  ┌────────────────┐  │
│  │ Tray Icon │  │ Shortcut  │  │ Overlay  │  │ Result Window  │  │
│  │          │  │ (global)  │  │ (状态提示) │  │ (识别结果显示)  │  │
│  └──────────┘  └─────┬─────┘  └──────────┘  └────────────────┘  │
│                      │                                           │
│                      └───────────┬───────────────────────────┘  │
│                                  │                               │
│  ┌───────────────────────────────▼────────────────────────────┐  │
│  │                   Coordinator（状态机）                      │  │
│  │                                                             │  │
│  │  ┌─────────────────┐  ┌──────────────┐  ┌──────────────┐  │  │
│  │  │ StreamingStage  │  │ Recording +  │  │ VadSegmented │  │  │
│  │  │ (流式 tick 驱动) │  │ Processing   │  │ (伪流式 tick) │  │  │
│  │  └─────────────────┘  └──────────────┘  └──────────────┘  │  │
│  └─────────────────────────────────────────────────────────────┘  │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │               TranscriptionEngine 抽象层                    │  │
│  │                                                             │  │
│  │  ┌──────────────┐  ┌────────────┐  ┌────────────┐         │  │
│  │  │  Embedded    │  │ WebSocket  │  │   gRPC     │         │  │
│  │  │ (octopus-asr) │  │ Remote     │  │   Remote   │         │  │
│  │  └──────────────┘  └────────────┘  └────────────┘         │  │
│  └─────────────────────────────────────────────────────────────┘  │
│                                                                  │
│  ┌──────────┐  ┌───────────┐  ┌──────────┐  ┌──────────────┐  │
│  │  Audio   │  │   Paste   │  │  Config  │  │ Streaming    │  │
│  │ (cpal)   │  │ (enigo +  │  │ (octopus  │  │ Session      │  │
│  │          │  │ clipboard) │  │  -asr)   │  │ (统一包装)    │  │
│  └──────────┘  └───────────┘  └──────────┘  └──────────────┘  │
└──────────────────────────────────────────────────────────────────┘
```

## 3. 项目结构

```
crates/desktop/
├── Cargo.toml
├── tauri.conf.json
├── capabilities/
│   └── default.json                # 包含 result_window 权限
├── icons/
├── src/
│   ├── main.rs                     # Tauri setup：managers、tray、result_window
│   ├── config.rs                   # DesktopConfig + is_streaming_engine()
│   ├── engine.rs                   # trait TranscriptionEngine
│   ├── engine_embedded.rs          # octopus-asr 进程内调用（使用 AsrEngineManager）
│   ├── engine_ws.rs                # WebSocket 远程
│   ├── engine_grpc.rs              # gRPC 远程（feature flag）
│   ├── audio.rs                    # 麦克风录音管理 + SharedAudioState
│   ├── coordinator.rs              # 核心状态机（Streaming/Recording/Processing/Pasting）
│   ├── streaming_engine.rs         # StreamingSession 统一包装
│   ├── result_window.rs            # 结果展示窗口管理
│   ├── overlay.rs                  # 录音状态 overlay
│   ├── paste.rs                    # 自动粘贴
│   ├── shortcut.rs                 # 全局快捷键
│   └── tray.rs                     # 系统 tray icon
└── dist/
    ├── overlay/
    │   └── index.html              # overlay 页面
    └── result/
        └── index.html              # 结果展示页面
```

## 4. 状态机（Coordinator）

### 4.1 流式模式状态流转（Paraformer/Zipformer）

```
快捷键 Toggle → Idle
                    │
                    │ start() + 创建 StreamingSession
                    ▼
              ┌──────────────┐
              │   Streaming   │◄──────────────────┐
              │  (tick 驱动)   │                    │
              └──────┬───────┘                    │
                     │ Toggle (停止)               │
                     ▼                             │
              ┌──────────────┐                    │
              │   Pasting    │────────────────────┘
              └──────────────┘  PasteDone

Cancel → 任何时候回到 Idle
```

**Streaming 阶段详情：**

- 每 600ms tick 排空音频缓冲区 → 送入 StreamingSession
- VAD 检测静音间隔 > 0.5s → 下次有语音时插入逗号
- result window 实时更新识别文本
- Toggle 停止时 → finish() 获取最终文本 → 追加句号 → 粘贴

### 4.2 离线模式状态流转（SenseVoice/Whisper/Qwen3-ASR）

**当前实现：**

```
Idle → Recording → Processing → Pasting → Idle
```

**V2 计划（VadSegmented 伪流式）：**

```
Idle → VadSegmented → Pasting → Idle
                    ↓ (Toggle 停止，仍有识别中)
                    → WaitingCompletion → Pasting → Idle
```

### 4.3 Command 变体

| 命令 | 说明 |
|------|------|
| `Toggle` | 切换录音状态（开始/停止） |
| `Cancel` | 取消当前操作 |
| `StreamingTick` | 流式识别定时 tick |
| `VadSegmentedTick` | VAD 伪流式定时 tick |
| `TranscriptionDone { text, seq }` | 转录完成（seq 用于乱序拼接） |
| `PasteDone` | 粘贴完成 |

### 4.4 Stage 变体

```rust
enum Stage {
    Idle,
    /// 流式识别：边录边识别
    Streaming {
        engine: StreamingSession,
        accumulated_text: String,
        streaming_active: Arc<AtomicBool>,
        vad: Option<SileroVad>,
        silence_duration: f64,
    },
    /// VAD 伪流式：tick 驱动分段识别
    VadSegmented { vad, audio_buffer, overlap_tail, accumulated_text, ... },
    /// 等待所有识别完成
    WaitingCompletion { accumulated_text, active_count, ... },
    /// 粘贴中
    Pasting,
}
```

## 5. StreamingSession（流式引擎统一包装）

### 5.1 设计

```rust
pub enum StreamingSession {
    Paraformer {
        engine: Mutex<StreamingParaformer>,
        accumulated: Mutex<String>,  // Paraformer 返回增量，需内部累加
    },
    Zipformer(Mutex<StreamingZipformer>),  // Zipformer 天然返回全文
}
```

### 5.2 接口

| 方法 | 说明 |
|------|------|
| `new(engine_name)` | 根据引擎名创建 session |
| `accept_samples(samples, was_silent)` | 送入音频，返回累积文本。`was_silent` 触发逗号插入 |
| `finish()` | 冲刷剩余音频，追加最终句号，返回完整文本 |
| `reset()` | 重置引擎状态（不重新加载模型） |

### 5.3 统一语义

- **对外**：始终返回累积全文
- **Paraformer**：内部维护 `accumulated` 字段，将增量追加
- **Zipformer**：直接返回引擎结果（已是累积全文）
- **标点**：`was_silent=true` 时在文本前插入逗号；`finish()` 时追加句号

## 6. 结果展示窗口（Result Window）

### 6.1 窗口属性

| 属性 | 值 |
|------|-----|
| 尺寸 | 520×100 px |
| 透明 | ✅ `transparent(true)` |
| 无边框 | ✅ `decorations(false)` |
| 置顶 | ✅ `always_on_top(true)` |
| 不抢焦点 | ✅ `focused(false)` |
| 初始位置 | 屏幕顶部居中（y=80） |
| 可拖拽 | ✅ 顶部 drag handle + `startDragging()` JS API |

### 6.2 Tauri 事件

| 事件 | 方向 | 说明 |
|------|------|------|
| `show-result` | Rust → JS | 显示窗口 + 设置文本 |
| `update-result` | Rust → JS | 更新文本（流式/伪流式模式） |
| `clear-result` | Rust → JS | 清空文本 + 隐藏窗口（同时归档到 history） |
| `hide-result` | Rust → JS | 隐藏窗口 |
| `result-edited` | JS → Rust | 用户编辑文本后同步到 record.txt |

### 6.3 前端特性

- 3 行高度（max-height 63px），超出后滚动
- 自动滚动到底部
- 白色背景，8px 圆角，阴影
- Esc 键隐藏窗口
- 拖拽通过顶部 drag handle 区域 + `currentWindow.startDragging()`
- **可编辑**：`contenteditable="true"`，用户可直接修改识别文本
- 聚焦时浅蓝背景提示，编辑时 300ms 防抖发送 `result-edited` 事件
- 流式更新时若用户正在编辑，追加新文本而非覆盖

### 6.4 拖拽实现要点

- **不可用** `data-tauri-drag-region`（Tauri 2 透明窗口中不可靠）
- **必须** 使用 JS API `currentWindow.startDragging()`
- **必须** 在 `capabilities/default.json` 中添加 `core:window:allow-start-dragging` 权限
- 窗口标签：`"result_window"`，必须在 capabilities 的 `windows` 数组中声明

### 6.5 文本持久化

识别文本通过 `~/.octopus/record.txt` 持久化，确保程序异常退出后文本不丢失。

**同步机制：**

| 触发时机 | 操作 |
|----------|------|
| 流式/伪流式识别更新（`update_result`） | 覆盖写入 record.txt |
| 最终文本粘贴（`start_pasting`） | 覆盖写入 record.txt |
| 用户编辑（JS `input` 事件） | 300ms 防抖后发送 `result-edited` → Rust 写入 record.txt |
| 清空（`clear_result`） | record.txt 归档到 history.txt → 删除 record.txt |

**history.txt 归档规则：**

- 格式：`--- YYYY-MM-DD HH:MM:SS ---\n文本内容\n`
- 每条以时间戳分隔行开头
- 最多保留 **20 条**历史记录
- 超出时删除最早的记录

## 7. VAD 标点机制

### 7.1 原理

使用 SileroVad 在流式 tick 中分析音频块，检测语音/静音比例：

- 每 512 采样点（32ms）计算一次 VAD 概率
- 语音概率 ≥ 0.5 → 语音块
- 语音概率 < 0.5 → 静音块
- 语音比例 = 语音块数 / 总块数

### 7.2 标点规则

| 条件 | 动作 |
|------|------|
| 语音比例 < 0.3 且之前累积静音 ≥ 0.5s | `was_silent = true`，下次有语音时插入逗号 |
| 语音比例 ≥ 0.3 | 重置静音计时器 |
| finish() 时文本不以标点结尾 | 追加句号 `。` |

### 7.3 常量

```rust
const VAD_SPEECH_THRESHOLD: f32 = 0.5;           // VAD 语音概率阈值
const VAD_CHUNK_SIZE: usize = 512;                // VAD 分块大小（采样点）
const PUNCTUATION_SILENCE_THRESHOLD: f64 = 0.5;   // 静音标点阈值（秒）
const STREAMING_TICK_INTERVAL_MS: u64 = 600;      // 流式 tick 间隔（毫秒）
```

## 8. VAD 伪流式（已实现）

### 8.1 目标

让非流式引擎（SenseVoice/Whisper/Qwen3-ASR）也能"边说边识别"。

### 8.2 核心逻辑

- 每 300ms tick 排空音频 → 累积缓冲区 → VAD 检测语音/静音
- **切分策略**（阈值来自 `config.yaml`）：静音边界切分（主）+ 连续超时强制切断（兜底）
  - 静音切分：检测到语音后静音 ≥ `segment_silence`（默认 500ms）→ 切分，**无 overlap**（静音是自然语句边界，下一段从干净开始）
  - 强制切断：连续语音缓冲达 `segment_duration`（默认 20s）仍未静音 → 强制切断，**保留末尾 `segment_overlap`（200ms）作下一段 overlap**（语句被硬切，需重叠保连贯）
- 多段识别并发执行，按 seq 序号保证拼接顺序
- 所有识别结果实时追加到 result window；原生 + 润色双份持久化到 SQLite（`transcriptions` 表）

### 8.3 状态机

```
Idle → VadSegmented（Toggle 开始，tick 驱动）
VadSegmented → VadSegmented（Tick 循环，分段识别）
VadSegmented → WaitingCompletion（Toggle 停止，尚有识别进行中）
VadSegmented → Pasting（Toggle 停止，无进行中识别）
WaitingCompletion → WaitingCompletion（TranscriptionDone，仍有进行中）
WaitingCompletion → Pasting（active_count == 0）
Pasting → Idle（PasteDone）
```

### 8.4 顺序保证

- `next_seq`：每次发送识别时递增
- `completed_results`：按 seq 缓存结果
- `completed_seq`：只消费连续序号，保证文本拼接顺序

## 9. 窗口管理

### 9.1 窗口列表

| 窗口标签 | 用途 | 显示时机 |
|----------|------|----------|
| `main` | 不可见主窗口 | 始终存在（不可见） |
| `recording_overlay` | 录音状态提示 | 离线模式录音/识别中 |
| `result_window` | 识别结果展示 | 流式模式始终显示；离线模式识别完成后 |

### 9.2 流式模式窗口策略

- **不显示 overlay**（避免双窗口）
- **显示 result window**（从开始到粘贴完成）
- result window 实时更新识别文本

### 9.3 离线模式窗口策略（VadSegmented 伪流式）

- **不显示 overlay**（与流式模式一致，避免双窗口）
- **显示 result window**（从开始到粘贴完成）
- result window 显示"正在聆听…"占位文本，识别结果实时更新
- 粘贴完成后清空并归档到 history.txt

## 10. 配置

```yaml
# ~/.octopus/config.yaml
engine_mode: embedded          # embedded | websocket | grpc
remote_url: "ws://127.0.0.1:3000/ws/stream"
grpc_endpoint: "http://127.0.0.1:50051"

# ASR 引擎选择
asr_engine: paraformer-zh      # 引擎名决定识别模式：
                                #   paraformer-zh / zipformer-zh → 流式
                                #   sensevoice / whisper / qwen3-asr → 离线/伪流式
language: auto                 # auto | zh | en | ja | ko

# 快捷键
shortcut: "CmdOrCtrl+Shift+Space"

# 粘贴方式
paste_method: clipboard        # clipboard | direct | none

# 麦克风
microphone: ""                 # 空 = 系统默认

# VAD 伪流式分段识别参数（非流式引擎生效）
segment_duration: 20.0         # 连续语音强制切断阈值（秒），超过此值未静音则强制切分（带 overlap）
segment_silence: 500           # 静音触发切分的时长阈值（毫秒），检测到语音后静音超过此值即切分（无 overlap）
segment_overlap: 200           # 强制切断时保留下一段的 overlap 时长（毫秒），仅强制切断生效（静音切分不带）
```

### 10.1 引擎模式判断

`config.is_streaming_engine()` 通过 `octopus_asr::config::resolve_engine_category()` 判断：
- `Paraformer` / `Zipformer` → 流式模式
- 其他 → 离线模式（V2 转为 VadSegmented 伪流式）

## 11. 依赖清单

### Rust 依赖

| 依赖 | 用途 |
|------|------|
| `tauri` 2.x | 桌面应用框架 |
| `tauri_plugin_global_shortcut` 2.x | 全局快捷键 |
| `tauri_plugin_single_instance` 2.x | 单实例 |
| `tauri_plugin_clipboard_manager` 2.x | 剪贴板 |
| `tauri_plugin_store` 2.x | 配置持久化 |
| `octopus-asr` | ASR 推理 (embedded feature) |
| `cpal` | 麦克风录音 |
| `rubato` | 音频重采样 |
| `enigo` | 键盘模拟/粘贴 |
| `tokio` | 异步运行时 |
| `anyhow` | 错误处理 |

## 12. 与 V1 的差异总结

| 方面 | V1 | V2 |
|------|----|----|
| Coordinator 状态 | Recording → Processing → Pasting | Streaming / VadSegmented / WaitingCompletion / Pasting |
| 流式引擎 | 不支持 | StreamingSession 统一包装 |
| 结果展示 | overlay 状态提示 | result window（可拖拽、可编辑、多行滚动） |
| 标点 | 无 | VAD 静音检测标点 |
| 离线引擎体验 | 全量录音后一次性识别 | VAD 伪流式分段识别 |
| 文本持久化 | 无 | record.txt 实时同步 + history.txt 归档（20 条） |
| 窗口管理 | overlay only | result window（所有模式共用） |
| 分段参数 | 硬编码 | config.yaml 可配置（duration/silence/overlap） |
