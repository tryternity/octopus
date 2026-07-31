# SayIt vs Octopus 语音识别对比

- **日期**：2026-07-31
- **目的**：评估 SayIt 框架，对比 ASR 架构异同，识别可借鉴点
- **SayIt 路径**：`/Users/wudarui/workspace/agent/SayIt`

## 定位差异（根本）

| | **Octopus** | **SayIt** |
|---|---|---|
| **核心定位** | 多功能桌面工具集（ASR + OCR + 剪贴板 + vault + 翻译 + ActionBar） | **专注语音输入法**（说话→AI 润色→插入光标） |
| **平台** | macOS 为主（cpal/CGEvent/Tauri 2） | Windows 为主（Win32 钩子/WebView2） |
| **架构** | 纯本地 Rust（Tauri 2 全集成） | 客户端 Rust + **可选 Python 服务器**（vLLM GPU 推理） |
| **技术栈** | Rust + React + TypeScript（Tauri 2） | Rust + React + TypeScript（Tauri 2）+ Python FastAPI（服务器） |
| **许可证** | 私有 | AGPL-3.0 |

## ASR 引擎对比

### 本地引擎矩阵

| 引擎 | **Octopus** | **SayIt** |
|---|:---:|:---:|
| Whisper | ✅ | ✅ |
| SenseVoice | ✅ | ✅ |
| Paraformer | ✅（流式+CIF） | ✅ |
| Zipformer | ✅（CTC+Transducer） | ❌ |
| Qwen3-ASR | ✅ | ✅（ONNX） |
| FireRedASR2 | ✅（CTC+AED） | ✅（CTC+AED） |
| Moonshine | ✅ | ❌ |
| FunASR-Nano | ❌ | ✅（speech-LLM） |

### 推理库差异（关键）

| 维度 | **Octopus** | **SayIt** |
|---|---|---|
| **ONNX 封装** | **自研**（onnx-infra crate，Session 管理/特征提取/VAD 全自写） | **sherpa-onnx crate**（静态链接，现成 recognizer） |
| **特征提取** | 自写 mel filterbank + fbank + whisper normalize（feature.rs/fbank.rs） | sherpa-onnx 内置 |
| **VAD** | silero_vad_v4 内嵌（include_bytes!），双实例（检测+过滤） | silero_vad（sherpa-onnx 内置）+ 服务器 FunASR fsmn-vad |
| **维护成本** | 高（特征提取/VAD/维度推断全自维护） | 低（sherpa-onnx 封装） |
| **灵活性** | 高（可深度调优，如 whisper normalize 3 约束、paraformer 5 步 fbank） | 中（受 sherpa-onnx API 约束） |

### 云端引擎

| Provider | **Octopus** | **SayIt** |
|---|:---:|:---:|
| 阿里云 | ✅（Fun-ASR/Paraformer/Qwen-ASR 三协议） | ❌ |
| 字节跳动 | ✅（bigmodel_async） | ✅（doubao seed-asr 流式+实时） |
| 腾讯云 | ✅（HMAC-SHA1 签名） | ❌ |
| 百度 | ✅ | ❌ |
| 千问 | ❌（通过阿里云） | ✅（qwen3-asr-flash/omni） |
| 小米 | ❌ | ✅（mimo-v2.5-asr） |

### 流式 vs 离线

| 维度 | **Octopus** | **SayIt** |
|---|---|---|
| **本地流式** | ✅ **真流式**（StreamingSession，zipformer/paraformer 边说边出字） | ❌（本地离线，长音频 VAD 切分批处理） |
| **云端流式** | ✅（4 provider WS 流式） | ✅（doubao/qwen realtime，partial 实时字幕） |
| **离线伪流式** | ✅（VadSegmentedPipeline，VAD 切段+强制截断+overlap） | ✅（服务器 VAD 智能切分+vLLM 批量推理） |
| **流式尾音冲刷** | ✅（Active Flush，静音≥0.5s padding 冲刷） | ❌ |
| **vLLM 加速** | ❌ | ✅（服务器模式，RTF 0.01） |

**Octopus 优势**：本地真流式（StreamingSession 边说边出字），SayIt 本地是离线的。

## 其他维度对比

| 维度 | **Octopus** | **SayIt** |
|---|---|---|
| **音频采集** | cpal（Rust 原生，看门狗自动重连） | Web Audio API（AudioWorklet+ScriptProcessorNode 兜底） |
| **降噪** | RNNoise + DeepFilterNet3（可选） | 浏览器 noiseSuppression |
| **热词** | 有界纠错器（HotwordIndex，防全词典过纠）+ 方言模糊 | 分引擎偏置（部分直传 context，部分后处理纠错） |
| **热词同步** | ✅（.sync/hotword git 同步） | ❌ |
| **LLM 润色** | 多段增量润色 + CM6 编辑器 + dirty range | 单段润色 + 应用感知 prompt 路由 |
| **翻译** | ✅（Opus-MT/m2m100/云端 5 家） | ❌（仅靠润色 prompt） |
| **ITN 数字归一** | ✅（itn.rs） | ✅（textPostProcess.ts，纯文本不依赖 AI） |
| **简繁归一** | ✅（hans.rs） | ❌ |
| **快捷键** | toggle 式（按一次开始/停止） | **PTT（按键说话）+ toggle + 免提** |
| **粘贴** | 结束后检测焦点 | **开始时预探测缓存目标窗口** |
| **历史记录** | DB（含音频？） | SQLite（含音频，可重新识别） |
| **多段编辑** | ✅ CompactEditor（CM6） | ❌（单段） |
| **应用感知** | ActionBar app_bundle_ids | ✅ polish prompt 路由 + 统计自适应 |

## 值得借鉴的点

### 🔥 高价值

#### 1. 预探测目标窗口（录音开始时缓存 frontmost app）
- **SayIt**：PTT 按下时缓存目标窗口 hwnd，识别完成后用缓存注入，避免录音期间切窗口粘错
- **Octopus 现状**：录音结束后才检测焦点窗口——用户录音时切窗口会粘错
- **借鉴**：开始录音时缓存 frontmost bundle id + window，结束时用缓存（或检测切换给提示）
- **实现成本**：低（coordinator 开始录音处加 1 行缓存 + 粘贴时比对）
- **文件**：`crates/desktop/src/engine/coordinator/session.rs`（开始）+ `paste.rs`（结束）

#### 2. Push-to-Talk（按键说话）模式
- **SayIt**：按住键说话、松开识别（Win32 全局钩子，支持单键 PTT 如 ShiftRight/Space/鼠标侧键）
- **Octopus 现状**：toggle 式（按一次开始、再按一次停止），无 PTT
- **借鉴**：PTT 对短句输入更自然（不用记"再按一次停"）。macOS 可用 CGEvent 全局监听实现
- **实现成本**：中（需全局 keydown/keyup 监听 + coordinator 新增 PTT 状态）
- **参考**：SayIt `client/src-tauri/src/keyboard/mod.rs`（Win32 SetWindowsHookExW）

#### 3. 应用感知 polish 路由
- **SayIt**：按当前 App 自动选不同润色预设（代码编辑器→技术文档 prompt，聊天→口语 prompt），还有统计自适应
- **Octopus 现状**：polish 是全局统一 prompt
- **借鉴**：Octopus 已有 app_bundle_ids 基础设施（ActionBar），可扩展到 polish——按 frontmost app 选不同润色 prompt
- **实现成本**：中（polish prompt 按 app 路由 + 设置 UI 配置）

### ⭐ 中价值

#### 4. 免提模式（Hands-free 常驻录音）
- **SayIt**：常驻录音，VAD 自动切段，5 分钟超时停
- **Octopus 现状**：手动 toggle，无持续监听
- **借鉴**：长会议/口述场景有用。但需权衡隐私 + CPU/内存
- **实现成本**：中（coordinator 新增 hands-free 状态 + VAD 自动切段已有）

#### 5. 录音音量告警状态机
- **SayIt**：基于 RMS/峰值检测「未检测到声音」「请靠近麦克风」「麦克风被静音」，悬浮窗实时告警
- **Octopus 现状**：有 cpal 看门狗（断推检测），但无音量告警
- **借鉴**：用户录音没声音时给即时反馈（而不是录完才发现是空的）
- **实现成本**：低（audio callback 里算 RMS + 前端告警）

### 💡 有启发但 Octopus 已覆盖

- **热词**：Octopus 有界纠错器更严谨（防全词典过纠）
- **LLM 润色**：Octopus 多段增量润色 + CM6 编辑器更强
- **ITN 数字归一**：两者都有
- **多引擎矩阵**：引擎数接近（7 vs 6）
- **流式 partial**：Octopus 本地真流式已实现（StreamingSession）

## Octopus 独有优势（SayIt 没有）

- **真流式本地识别**（StreamingSession，zipformer/paraformer 边说边出字）
- **多段编辑器**（CompactEditor CM6，dirty range 增量编辑）
- **翻译**（Opus-MT/m2m100/云端）
- **vault/OCR/剪贴板/ActionBar/slash 命令**（多功能工具集）
- **macOS 原生**（CGEvent/输入源切换/CoreML/ScreenCaptureKit）
- **热词 git 同步**（.sync/hotword 跨设备）

## 建议的借鉴优先级

| 优先级 | 借鉴点 | 理由 |
|---|---|---|
| **P0** | 预探测目标窗口 | 实现简单，修真实 bug（录音时切窗口粘错） |
| **P1** | Push-to-Talk 模式 | 交互升级，短句输入更自然 |
| **P1** | 应用感知 polish 路由 | 基础设施已有（app_bundle_ids），扩展成本低 |
| **P2** | 免提模式 | 长场景有用，隐私/资源需权衡 |
| **P2** | 录音音量告警 | 用户体验提升，实现成本低 |

---

## 附：SayIt 关键文件路径

| 职责 | 路径 |
|---|---|
| 服务器 ASR（vLLM） | `server/backend/app/asr.py` |
| 服务器 LLM 润色 | `server/backend/app/llm.py` |
| 客户端本地 ASR（sherpa-onnx） | `client/src-tauri/src/models/local_asr.rs` |
| 客户端云 ASR | `client/src-tauri/src/providers/`（registry.rs/asr_doubao_realtime.rs 等） |
| 客户端音频采集 | `client/src/services/audio.ts` |
| 客户端 PTT 编排 | `client/src/services/recorder/RecorderOrchestrator.ts` |
| 客户端键盘钩子 | `client/src-tauri/src/keyboard/mod.rs` |
| 客户端粘贴注入 | `client/src-tauri/src/commands/paste.rs` + `inject/mod.rs` |
| 客户端热词 | `client/src/services/hotwords/` |
| 客户端文本后处理 | `client/src/services/textPostProcess.ts` |
| 应用感知 prompt 路由 | `client/src/services/personalization/` |
