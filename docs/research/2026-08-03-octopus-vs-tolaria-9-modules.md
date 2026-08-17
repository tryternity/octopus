# Octopus 9 大模块 vs Tolaria 知识库竞品调研报告

- **日期**：2026-08-03（2026-08-04 合并 main 后更新）
- **作者**：调研工作流（9 个并行子代理 + 主代理汇总）
- **范围**：ASR / 截图 / 剪贴板 / 翻译 / Action Bar / Terminal / 录屏 / OCR / 密码箱 共 9 大模块
- **对照来源**：`/Users/wudarui/.tolaria/`（个人知识库，2026-08 截止）+ 必要的 web 验证
- **目的**：找出 octopus 独特的价值和不足的地方
- **分支**：`research/tolaria-comparison`（worktree `.worktrees/research-tolaria-comparison`）
- **方法**：每个模块由一个独立 subagent 同时深读 octopus 源码与 tolaria 对应文档，按统一模板（现状 → 对比矩阵 → 独特价值 → 不足 → 改进方向）产出，最后汇总。所有论断均带 `file:line` 或外部 URL 锚定证据。
- **更新记录**：
  - 2026-08-03：初版（基于 HEAD `8322cb94`）
  - 2026-08-04（第 1 次）：合并 `origin/main`（fast-forward 37 commit 到 `b9d75ff0`）后修订 VAD v4→v6 升级 + `stitch.rs` 拆分两处已落地事项
  - 2026-08-04（第 2 次）：根据讨论修订剪贴板加密策略——字段级加密 → 整库加密（绑定 sync）
  - **2026-08-04（第 3 次）**：双向同步到 `origin/main` HEAD `14b29713`（含 main 推过来的 6 commit「too_many_arguments struct 治理」+ 第八~十三轮审查修复）后，全面复核 9 大模块描述与代码一致性。结论：**报告内容总体仍准确**，唯一需要精化的是 VAD v6 描述（补充官方 16k_op15 精简版、ONNX 签名破坏性变更细节、commit `3f6fd519` 锚定）。其余模块（ASR 引擎清单 7 族+4 家、ActionBar agent claude/codex/gemini/pi、OCR PP-OCRv6-small、vault folder 软删同步）核对后均与代码一致。main 带来的多是 refactor（stitch 拆分 / 内联资源集中化 / too_many_arguments struct 化）+ fix（hotword spawn_blocking / vault folder 软删同步 P0 / rAF 背压等），不引入新功能模块。
  - **2026-08-05（第 4 次）**：剪贴板模块两项落地后同步报告状态——① Paste Stack（P1 粘贴队列）已实现；② ConcealedType 检测（P3）已实现。
  - **2026-08-05（第 5 次）**：ConcealedType 检测从 macOS 扩展到 Windows + Linux 跨平台（`ExcludeClipboardContentFromMonitorProcessing` / `x-kde-passwordManagerHint`），§0.3 / §3.4 / §3.5 对应「仅 macOS」描述更新为「跨平台」。
  - **2026-08-10（第 6 次，本次）**：截图模块大幅补齐——① 3 种模糊（Pixelate/Gaussian Stackblur/Redact）；② 平铺水印（density+angle+color，6 config 字段）；③ 工具栏归组（形状/线条合并，6 标注按钮）；④ ImagePreview 图片编辑器同步归组 + 水印。§0.4 / §2.1 / §2.2（标注工具+马赛克+水印行）/ §2.4 第 3 项 / §2.5 第 5 项 全部更新。

---

## 0. 总览（先读这一节）

### 0.1 一句话定位

**Octopus 是一个 "本地优先、模块聚合、纯 Rust + Tauri 2" 的桌面工具集**——它把 ASR、OCR、翻译、剪贴板、截图、录屏、终端、Action Bar、密码箱 9 个独立工具用同一套 SQLite schema、同一套 `octopus-sync` git 基础设施、同一套 ONNX Runtime 推理后端、同一套 Tauri 命令 + 事件边界整合到一个 app 里。tolaria 知识库里的对应工具绝大多数是**单点极致的独立产品**（CleanShot X 只做截图、KeePassXC 只做密码箱、WezTerm 只做终端……），octopus 的根本差异是「**聚合 + 一体化联动**」。

### 0.2 跨模块共通的 3 个独特价值（任何竞品都难以复制的）

1. **DB 激活语义统一 4 域**。`models.is_enabled`（激活，每域唯一）+ `is_available`（可用），4 个域（asr/llm/ocr/translate）经同一套 `get_active_model(domain)` / `switch_active_model(domain, id)` API 管理，运行期 `ACTIVE_ENGINES: LazyLock<RwLock<HashMap<domain, Arc<ResolvedEngine>>>>` 缓存（`crates/asr-local/src/config.rs:366-455`）。tolaria 里没有任何一个工具做到「同一界面里切换语音/翻译/OCR/润色 的激活模型」。

2. **统一 SQLite schema（v58）+ FTS5 trigram + git sync**。`clipboard_history` 表吞并了原 `transcriptions` 表，统一存 text/voice/ocr/image/file 五类，FTS5 trigram 索引对 CJK 子串友好（`crates/infra/src/db.sql:63-120`）。vault + 热词都走 `octopus-sync`（256 桶分片 + md5 增量 + SSH 私有库守卫）。这是「装一个 app 等于装一整套相互联动的工具」的基础设施。

3. **跨模块 pipeline 短路径**。截图 → OCR → CompactEditor 双 tab → FTS5 全文搜索 → 翻译润色；ASR → `paste-text` 直写终端 PTY 绕过键盘模拟；Finder 选中 → Action Bar → agent CLI（`{{voice}}` 录音填占位符）；录音 → 录后 ASR → LLM 润色字幕。tolaria 里把这些场景拆给了至少 4 个独立工具，octopus 一气呵成。

### 0.3 跨模块共通的 5 个系统性不足

| 不足 | 影响 | 出现的模块 |
|---|---|---|
| **平台覆盖严重不均** | Action Bar（`window.rs:19-24` `#[cfg(not(target_os = "macos"))] return`）、Terminal（删 ConPTY）、录屏（Swift helper 仅 mac）、密码箱 Auto-Type（仅 `macos.rs`）只跑 macOS | Action Bar / Terminal / 录屏 / Vault |
| **无加密 / 无 secret 检测** | `clipboard_history` 全明文（与 Maccy/EcoPaste 同档位，加 sync 前不构成缺口）；跨平台 concealed hint 检测已于 2026-08-05 补齐（`watcher.rs` 检测三平台密码管理器标记：macOS `org.nspasteboard.ConcealedType` / Windows `ExcludeClipboardContentFromMonitorProcessing` / Linux `x-kde-passwordManagerHint`，命中静默跳过）；vault 的 `keychain.rs:14-30` 坦承 machine-key 是 obfuscation 而非真加密（受 adhoc 签名限制） | 剪贴板 / Vault |
| **无云同步 / 无团队流** | 剪贴板零依赖 sync crate；所有模块都不做团队共享 / 云分享链接（CleanShot Cloud、Cap 分享、Bitwarden 组织都没有） | 剪贴板 / 截图 / 录屏 / Vault |
| **无浏览器扩展 / 系统级集成** | 没有任何 Native Messaging、IME 输入法、Passkey provider、桌面搜索集成 | Vault / Action Bar / 翻译 |
| **生态/社区/文档弱** | 私有仓 + 零社区分发，相对 sherpa-onnx 12.9k★ / Pot 19.1k★ / kiss-translator 11.6k★ / WezTerm 全平台 | 全部 |

### 0.4 一图速览：9 模块独特价值 + 最大不足 + P0 改进

| 模块 | 最独特的价值 | 最大的不足 | P0 改进方向 |
|---|---|---|---|
| 1. ASR | 本地 7 族 + 云端 4 家国内服务商统一 trait，热词单记录 + git 软删，Silero VAD v6（2026-08-04 升级） | 无说话人分离、无词级时间戳 / SRT、Whisper 仅 small.en | 词级时间戳导出 + 集成 sherpa-onnx 分离 |
| 2. 截图 | 三平台原生贴图 + 自研滚动 NCC 拼接 + **3 种模糊（Pixelate/Gaussian Stackblur/Redact）+ 平铺水印（density+angle）**（2026-08-10 新增）+ **截图翻译（OCR→translate_window 只读浮窗，2026-08-11 新增）** | 无窗口/元素截图入口、贴图能力弱于 PixPin | 贴图能力补齐 |
| 3. 剪贴板 | text/voice/ocr/image/file 统一表 + FTS5 trigram，与 ASR/OCR 联动；粘贴队列（2026-08-05 新增） | 无云同步、无 macro、富文本仅标记不存原文（明文存储与 Maccy/EcoPaste 同档位，加 sync 前不构成缺口） | sync 接入（同步前先做 sqlite3mc 整库加密） |
| 4. 翻译 | Opus-MT + m2m100 + CloudLlm 5+1 provider 三引擎统一 trait + 兜底降级 + **OCR→translate pipeline（截图翻译 + ActionBar 选中翻译 → translate_window 浮窗，2026-08-11 新增）** | 无 glossary、双语对照弱 | glossary 表 |
| 5. Action Bar | OCR/ASR/translate/clipboard/terminal/vault 全栈聚合 + agent CLI 启动 + CompactEditor tab | 仅 macOS、无扩展生态、跨屏焦点问题 | AX 直读选中文本 + 扩展注册中心 |
| 6. Terminal | OSC agent 状态感知（Claude/Codex/Gemini/Pi 工作相位）+ ASR 直写 PTY + WKWebView 稳定性细节 | 无 GPU 渲染、无 SSH、无 AI 命令、无 shell history sync | Atuin 集成 + russh 远程 |
| 7. 录屏 | 与 ASR 联动（录后字幕 + LLM 润色）、Swift helper 子进程解耦、双轨 source 标注 | 无实时转写、无点击效果、无 AI 摘要、无云分享 | 录制中实时转写（杀手锏） |
| 8. OCR | 纯 Rust + ONNX 与 ASR 共用后端，PP-OCRv6-small + QR + 启发式 Markdown + **word-level box + 图片文字拖选层**（2026-08-13 新增） | 无 ML 版面/表格/公式、无 VLM OCR、无 PDF 多页 | PP-Structure 接入 + VLM OCR（PaddleOCR-VL） |
| 9. 密码箱 | Auto-Type 跨应用填充（macOS 原生）+ git sync 复用 + Argon2id 双向护栏 | 无 Passkey、无浏览器扩展、无 YubiKey、无 SSH Agent | Passkey 提供方（战略生死线） |

### 0.5 阅读指南

下面 9 节是按模块独立的深度报告，每节结构一致：

1. **现状**：octopus 该模块的实际实现（文件 + 行号锚定）
2. **竞品对比矩阵**：维度 × 竞品的表格
3. **独特价值**：相对竞品的差异化优势
4. **不足 / 缺失**：相对竞品的差距
5. **建议改进方向**：按性价比排序的行动清单

如果只关心某个模块，直接跳到对应章节。

---


## 1. ASR 语音识别模块

### 1.1 Octopus 现状

octopus 的 ASR 子系统分布在两个 crate：`crates/asr-local`（离线推理库，约 14 个引擎模块 + VAD/降噪/热词/纠错/ITN/简繁）与 `crates/asr-cloud`（4 家国内云服务商的 WSS 流式协议层 + 批引擎适配）。三端（desktop/cli/server）共享同一激活语义与 pipeline。

**功能矩阵**

| 维度 | 实现 |
|------|------|
| **离线引擎（7 族）** | Whisper / SenseVoice-原版（FunASR 4 输入 ONNX，非 sherpa 简化版）/ Paraformer / Qwen3-ASR / Zipformer（CTC + Transducer 自动判别）/ Moonshine / FireRedASR2-AED CTC（`crates/asr-local/src/config.rs:77-96` EngineCategory 枚举；`crates/infra/src/db.sql:388-402` seed 14 条本地条目） |
| **流式引擎** | Paraformer 流式 + Zipformer CTC/Transducer 流式（`crates/asr-local/src/streaming/`），通过 `StreamingSession` 枚举统一 `accept_samples / flush / finish / reset` 语义（`streaming_engine.rs:14-34`） |
| **云端引擎（4 家）** | 阿里云 DashScope（Fun-ASR / Paraformer-realtime / Qwen3-ASR-flash 三族，WSS）/ 字节豆包（Doubao-ASR 1.0/2.0 + SeedASR 2.0）/ 腾讯云（HMAC-SHA1 签名，16k_zh 系列 + 方言 + 多语种）/ 百度（START 帧鉴权，dev_pid 1537/15372/17372 等）—— 见 `crates/asr-cloud/src/{aliyun,bytedance,tencent,baidu}_stream.rs` + `db.sql:502-510` 的 model 预设 |
| **VAD** | Silero VAD **v6**（2026-08-04 从 v4 升级，feat `3f6fd519` + 4 个修复/重构 commit），编译期内嵌字节（`octopus_infra::resources::silero_vad_v6_onnx()`，**官方 16k_op15 精简版 1.23MB**，比完整版 2.3MB 小 46%，已删 8kHz 分支）+ 磁盘 `~/.octopus/models/vad.onnx` 覆盖；按 path 全局缓存 `Arc<Mutex<Session>>`（`audio/vad.rs`）。v6 相对 v4：噪声错误率 -16%、人声 prob 0.90 vs 0.15-0.32、静音 prob 0.002 vs 0.04；ONNX 签名破坏性变更——`state` 单 tensor `[2,1,128]`（v4 是 h/c 双 `[2,1,64]`）+ 输入需 context 拼接 `[1,576] = context(64) + samples(512)`、窗口必须 512 样本（漏拼 context 导致 prob 恒近零，详见 [spec](../superpowers/specs/archived/2026-08-04-silero-vad-v6-upgrade-design.md)）。`StreamingRunner` 用 32ms / 512 样本块、阈值 0.5、静音≥0.5s 触发标点 |
| **降噪** | 可插拔 `FrameDenoise` trait，`denoise_mode` 选 RNNoise（`nnnoiseless` 纯 Rust，48k/480 样本）或 DeepFilterNet3（libDF v0.5.6 + tract 0.19，48k 全频带）。**已弃用**第三方 `dfn3.onnx`（压语音，gain≈0.10），改自控 fork `tryternity/DeepFilterNet@v0.5.6`（`Cargo.toml:26-34`） |
| **热词** | 「有界热词纠错」——候选仅来自用户热词表 `HotwordIndex`（DB `hotword_words` 单记录表，v57 迁移；set 级软删 + tombstone sync，v58）。空表即 no-op，根治旧全词典 n-gram 过纠（"开始语音识别"→"开始于饮食别"）。`find_candidates` 用 `select_nth_unstable_by` 取 top-6（2026-08-02 优化，避免全 clone + 全排序） |
| **方言 / 模糊拼音** | DB 表 `fuzzy_dialect_rules`（v56 迁移），基础规则常开（平翘舌 zh/ch/sh→z/c/s + 前后鼻音）+ syllable/initial/special_hu 三组可配。索引 key 与查询对称归一化（`text/hotword.rs:34-89`） |
| **`asr_correct`** | pipeline 阶段开关，v55 迁移强制翻 true。`language == "en"` 自动跳过（`streaming_runner.rs:179-182`）。Qwen3-ASR 可经 `skip_corrector()` 短路 |
| **ITN 数字归一化** | `chinese2digits` crate，纯 Rust 无原生依赖；黑名单保护 + 从后向前 `replace_range`（2026-08-02 优化，O(N·K)→O(N)）。解决 Zipformer/Moonshine/Whisper 输出"二零二六年七月二十六日"痛点 |
| **LLM polish** | 不在 asr-local，留端 pipeline（`streaming_runner.rs:18` 注释）。desktop 经停顿阈值 `pause_polish_threshold_ms` 触发，调 `octopus_llm::polish` |
| **简繁归一** | CC-BY 3.0 对照表编译期嵌入，按 `output_simplified` 归一化 |

**架构亮点**

- **DB 激活语义（Task 1-7 重构）**：`models` 表 `is_enabled` 表「激活」（每域仅 1 个=1），`is_available` 表「可用」。4 域（asr/llm/ocr/translate）经 `get_active_model(domain)` / `switch_active_model(domain, id)` 统一查询；删除 `app_config` 的 4 个激活字段。运行时 `ACTIVE_ENGINES: LazyLock<RwLock<HashMap<domain, Arc<ResolvedEngine>>>>` 缓存，热路径零 DB 开销（`config.rs:366-455`）
- **多引擎统一 trait**：`OfflineAsrEngine: Send + Sync`（`engines/engine.rs:15-26`）+ `AsrEngineManager`（缓存上限默认 2，server 可放大；`switch_model` / `get_engine` 双模式，保护 active 不被淘汰）。云端经 `CloudBatchEngine: impl OfflineAsrEngine` 与本地统一喂 `transcribe_batch`（`asr-cloud/src/lib.rs:22`）
- **per-chunk 特征归一化**：`normalize_whisper_features`（`engines/feature.rs:148-174`）合并自 zipformer + qwen3_asr 两份实现，**快路径** `as_slice_mut()` 单遍扁平迭代（log10 + find-max 合并），替代原 3 趟嵌套 `[[i,j]]` 索引；**调用约定**流式引擎必须 per-chunk 独立归一化（参考 sherpa-onnx `online-recognizer-transducer-impl.h`），不是全局归一
- **LFR 堆叠**：m=7/n=6 → 560 维（paraformer/sensevoice 共用），纯 fbank 模式给 firered（`engines/fbank.rs:38-58`）；C1 修复改用 mel 空间 filterbank 权重对齐 kaldi_native_fbank
- **VAD LSTM 预热**：`preroll_vad` 喂 10 帧静音让 Silero 状态稳定，避免开头 prob 漂移导致标点检测不准（`streaming_runner.rs:85-92`）
- **开口前静音门控**：`seen_speech` 标志在首个 has_speech tick 前丢弃样本，避免启动噪声触发 spurious token（实测 paraformer 首 chunk 在 ~0.6s 噪声上 alpha_sum≈1.3 误 fire 出"嗯"）（`streaming_runner.rs:183-189`）
- **Whisper 工程细节**：int8 三件套（encoder + dec_init + dec_past）、动态 max_tokens（`seconds × 6 + 10).min(448)`，防短音频静音段幻听）、Mel center=True reflect 填充对齐 PyTorch `torch.stft`、特殊 token 强制查询（.en 整体偏移 -1）
- **manifest 完整性**：sha256 + size JSON map 存 DB `models.secret_key`，desktop/cli 共用，detect 损坏/缺失
- **跨平台 EP**：`ort` 2.0.0-rc.12，按 `cfg(target_os)` 编入 CoreML/CUDA/DirectML（`Cargo.toml:47-54`）；qwen3-asr 含动态算子自动跳过 CoreML 走 CPU（`config.rs:580-591`）

**三端暴露**

- **desktop**（Tauri）：录音 coordinator + tray + 设置页模型管理 + Action Bar + 停顿润色
- **cli**：`--model` 多模型路径（`resolve_engine_any` 不限激活），本地/云端分流后统一 `transcribe_batch`
- **server**：WS↔`StreamingRunner` 桥接 + `TranscriptEvent`→JSON 序列化（`server/src/pipeline.rs`），`AsrEngineManager::new_with_capacity` 多模型并发

### 1.2 竞品对比矩阵

| 维度 | Octopus | sherpa-onnx | faster-whisper | transcribe-rs | transcribe.cpp | moss-transcribe | CapsWriter | FireRedASR2S |
|------|---------|-------------|----------------|---------------|----------------|-----------------|------------|--------------|
| **语言/栈** | Rust + ONNX Runtime | C++ + ONNX | Python + CTranslate2 | Rust + ONNX/whisper.cpp | C++17 + ggml | C++17 + ggml | Python + ONNX/GGUF | Python |
| **License** | (闭源/私库) | Apache-2.0 | MIT | MIT | MIT | MIT | MIT | 见仓库 |
| **本地引擎覆盖** | 7 族（Whisper/SenseVoice-原版/Paraformer/Qwen3-ASR/Zipformer×2/Moonshine/FireRedASR2） | 最全（Zipformer/Whisper/SenseVoice/Paraformer/Qwen3/Moonshine/Dolphin/FireRed/Telespeech 等，含 sherpa nano 简化版） | 仅 Whisper 家族 + Distil | 9 引擎（Parakeet/Canary/Cohere/Moonshine/SenseVoice/GigaAM/Whisper/Whisperfile/OpenAI） | **16 家族 60+ 变体**（Parakeet/Whisper/Qwen3/Cohere/Voxtral/Granite/SenseVoice/FunASR-Nano/Nemotron/GigaAM/MedASR/MOSS 等） | 1（MOSS-Transcribe-Diarize 0.9B） | 4（Qwen3-ASR/Fun-ASR-Nano/SenseVoice/Paraformer，ONNX+GGUF 混合） | FireRedASR2-LLM/AED + VAD + LID + Punc 四件套 |
| **流式** | Paraformer + Zipformer CTC/Transducer | Zipformer Transducer 全系 + 多种 online 模型 | 否（仅 LocalAggressive 等第三方流式） | Moonshine streaming | Nemotron Streaming / Moonshine Streaming / Voxtral Realtime / Multitalker Parakeet | 否（端到端单次） | 否（明确不做，按住说话松开上屏） | 否（AED 60s 上限） |
| **VAD** | Silero **v6**（2026-08-04 升级，16k_op15 精简版 1.23MB，噪声错误率 -16% vs v4）+ 磁盘覆盖 | Silero + 自研 + 多种 | Silero（vad_filter） | Silero（feature） | 各家族自带 | Whisper 编码器隐式分段 | sherpa-onnx VAD | **FireRedVAD**（F1 97.57%，100+ 语言，仍为 SOTA） |
| **热词** | 有界纠错（HotwordIndex + 拼音首字母/模糊拼音/方言规则 DB 化） | 有限（部分模型 context） | 无 | 无 | 无 | 无 | **三层**（音素 RAG hot.txt + 正则 hot-rule.txt + 服务端 hot-server.txt，3s 热重载） | 无 |
| **说话人分离** | **无** | **有**（speaker embedding + 聚类，3D-Speaker/NeMo） | 无（WhisperX 外挂） | 无 | MOSS Transcribe-Diarize 内联 | **核心卖点**（inline `[Sxx]` 标记 + SRT/ASS/JSON） | 无 | 无 |
| **降噪** | RNNoise + DeepFilterNet3 双后端可切 | 语音增强 + 源分离（Spleeter/UVR） | 无 | 无 | 无 | 无 | 无 | 无 |
| **标点 / 纠错** | 自带 ITN + 简繁 + 有界热词纠错；LLM polish 留端 | 标点恢复模型（ct-transformer 等） | 无 | 无 | 无 | 无 | ITN + LLM 角色（DeepSeek/Claude/Gemini/Ollama） | **FireRedPunc**（F1 78.90%，FunASR-Punc 62.77%） |
| **云端** | **4 家**（阿里/字节/腾讯/百度，WSS 流式，统一 trait） | 无（纯离线） | 无 | OpenAI API 远程 | 无 | 无 | 无 | 无 |
| **平台** | macOS/Linux/Windows（Tauri 桌面 + cli + server） | 全平台（含 Android/iOS/HarmonyOS/WebAssembly/嵌入式 ARM/RISC-V/NPU） | 跨平台 Python | 跨平台 Rust | macOS Metal / Vulkan / CUDA / CPU | 同 transcribe.cpp | **仅 Windows** | 仅 Linux Ubuntu 22.04 测试 |
| **活跃度** | 私有项目，高频迭代（schema v58，多份 2026-08 spec） | 12.9k★，v1.13.2（2026-05） | 主流 | 208★ | 1.5k★ | 32★ | 6.4k★ | 工业级 SOTA |
| **模型下载** | HF + ModelScope 模板（`{huggingface}`/`{modeloscope}` 运行时变量，`download_mirror` 配 hf-mirror）+ manifest sha256 校验 | HF + manual | HF Hub 自动 | 手动 | HF + GGUF | HF + 百度网盘 | GitHub + 百度网盘手动 | ModelScope（推荐）+ HF |

### 1.3 Octopus 独特价值

1. **本地 + 云端 4 家服务商统一 trait**。这是 tolaria 知识库里**唯一**把阿里 DashScope / 字节豆包 / 腾讯云 / 百度实时 ASR 的 WSS 协议层都自己实现并统一到 `OfflineAsrEngine` 的项目（`asr-cloud/src/{aliyun,bytedance,tencent,baidu}_stream.rs`，每家 7-36KB 协议代码）。sherpa-onnx、faster-whisper、transcribe.rs/cpp 都是纯本地或仅 OpenAI 远程。对国内用户而言，这意味着「本地模型不够强时一键切云端，pipeline/热词/ITN 全保留」。

2. **DB 激活语义 + 多引擎缓存管理**。4 域（asr/llm/ocr/translate）共用 `ResolvedEngine` 结构 + `ACTIVE_ENGINES` 内存缓存 + `switch_active_model(domain, id)` 一致 API（`config.rs:328-455`）。`AsrEngineManager` / `StreamingSessionManager` 各自带 max_cache 驱逐策略（保护 active），这是 transcribe-rs 的 `SpeechModel` trait 没有的运维层。

3. **流式 per-chunk Whisper 特征归一化**。`normalize_whisper_features` 的 `as_slice_mut()` 单遍扁平迭代 + 显式 per-chunk 调用约定（`feature.rs:143-174`），把 sherpa-onnx C++ 里 `online-recognizer-transducer-impl.h` 的关键正确性约束在 Rust 侧落地，且有 bench（`benches/fbank.rs` + `benches/streaming_paraformer.rs`）。

4. **热词单记录 + 软删 git 同步**。`hotword_words` 表（v57）每词一条 + UUID v5 确定性 ID + 原始拼音；`hotword_sets` 加 `is_deleted` epoch + `UNIQUE(name,is_deleted)`（v58），tombstone 经 sync merge 传播，根治跨设备删除复活。这是 CapsWriter 的 `hot.txt` 平文件 + 3s 热重载所没有的工程化。

5. **与 LLM 润色/翻译/OCR 一体化的 Tauri 单 app**。octopus 不是单一 ASR 工具，而是 ASR + LLM + OCR + Translate 4 域聚合的桌面工具集——停顿润色、Action Bar、CompactEditor 翻译都复用同一激活模型 DB 与 config。tolaria 里的 Handy / CapsWriter / YuHuang 都是单一 ASR 场景。

6. **可插拔双降噪后端**。RNNoise（纯 Rust，无 unsafe）+ DeepFilterNet3（libDF + tract）模式可切，且改用自控 fork 防上游删库。这是 transcribe.rs/cpp / faster-whisper 都没有的——它们依赖输入已是干净音频。

### 1.4 Octopus 不足 / 缺失

1. **无说话人分离**。这是相对 sherpa-onnx / moss-transcribe / WhisperX 最显著的缺口。会议纪要、播客转写场景下「谁在何时说了什么」无法产出。tolaria 里 `ai-speech-06-speaker-diarization.md`、`voicefilter-paper.md`、`wavlm-speech-separation-paper.md` 三篇都指向这一能力，说明作者关注但未实现。

2. **无词级时间戳导出 / 字幕管线**。`OfflineAsrEngine::transcribe` 只返回 `String`（`engine.rs:17`），不返回 segments/word timestamps。CapsWriter 拖拽转 SRT、faster-whisper 的 `word_timestamps=True`、moss-transcribe 的 SRT/ASS/JSON 导出都无法实现。也没有 ffmpeg 集成做音视频→字幕一站式。

3. **Whisper 仅支持 small.en，不支持 Large v3 / Turbo**。引擎硬编码 `N_MELS=80` + 静态 80×201 filterbank，遇到 128 mel 会提前 fail（`architecture.md` whisper 行）。相对 faster-whisper（large-v3 batch=8 int8 比 openai 快 25 倍）、transcribe.cpp（12 Whisper 变体含 v3-turbo）竞争力不足。

4. **无 macOS Dictation 替代 / 系统级语音输入法集成**。YuHuang 做了 fcitx5 原生插件 + 三区悬浮草稿窗 + LCP 增量提交，CapsWriter 有 Push-to-Talk 全局热键 + 管理员权限输入。octopus desktop 录音 coordinator 偏「应用内工具」，没有看到系统级 IME 集成或全局热键的明确实现。

5. **无 FireRedVAD / FireRedLID / FireRedPunc 等高质量子模型**。FireRedASR2S 把 VAD（F1 97.57%）/ LID / Punc 都做到 SOTA。**2026-08-04 更新**：octopus 已从 Silero v4 升级到 v6（噪声错误率 -16%、人声灵敏度 0.90 vs 0.15-0.32），VAD 质量显著改善，但相对 FireRedVAD 仍有差距（v6 无公开 F1 对比，FireRedVAD F1 97.57% / False Alarm 2.69% 是当前公开 SOTA）。LID / Punc 仍用自带 ITN + LLM polish 凑。

6. **流式仅 Paraformer + Zipformer**。SenseVoice / Qwen3-ASR / Whisper 都是非流式。对「边说边出字」体验，YuHuang 的双模型流水线（流式 paraformer-zh-streaming 实时草稿 + SenseVoiceSmall 离线修正 + LCP 增量提交）更优雅。

7. **无量化 / GGUF 路径**。全部走 ONNX Runtime，模型动辄数百 MB ~ GB。CapsWriter 用 ONNX encoder + GGUF decoder 混合（Fun-ASR-Nano 800M 显存 1-2GB），transcribe.cpp 的 q4_k 把 MOSS 3.4GB→511MB 逐字节一致。octopus 的 int8 是 ONNX 图内量化，没有 K-quant 这种激进压缩。

8. **无嵌入式 / 移动端 / WebAssembly 支持**。sherpa-onnx 覆盖 Android/iOS/HarmonyOS/RISC-V/NPU，octopus 仅桌面三平台。

9. **正确性强但生态薄**。无 HF Space 在线 demo、无 Discord、无 12 语言绑定。相对 sherpa-onnx（12.9k★）和 faster-whisper，社区认知度为零。

### 1.5 建议改进方向（按性价比排序）

| 优先级 | 方向 | 预估工作量 | 收益 |
|--------|------|-----------|------|
| **P0** | **词级时间戳 + SRT/ASS/JSON 导出**：扩 `OfflineAsrEngine::transcribe` 返回 `Transcription { segments, words }`（对齐 faster-whisper / transcribe-rs 的 `Transcription` 结构），pipeline 末尾加字幕导出。Whisper/FireRed-AED/Paraformer 都有原生时间戳能力 | 中（1-2 周） | 解锁会议纪要、播客字幕、视频配音场景；补齐相对 CapsWriter 最大缺口 |
| **P0** | **集成 sherpa-onnx 说话人分离**：要么 FFI 调 sherpa-onnx Rust 绑定（`docs.rs/sherpa-onnx`，含 ActivityDetector/speaker embedding/diarization），要么移植 3D-Speaker ECAPA-TDNN ONNX。先做「ASR 文本 + 说话人标签」联合输出 | 大（2-3 周） | 补齐相对 sherpa/WhisperX/moss 最显著缺口；会议场景必备 |
| **P1** | **双模型流式流水线**（学 YuHuang）：流式 paraformer-zh-streaming 出实时草稿 + SenseVoice/Qwen3-ASR 离线修正 + LCP 增量提交 + 三区悬浮窗。把当前单流式 session 升级为「快模型 + 准模型」双轨 | 中（1-2 周） | 边说边出字体验质变；当前流式质量受限于 Paraformer/Zipformer |
| **P1** | **支持 Whisper Large v3 / Turbo**：放开 `N_MELS=128`，动态 filterbank，对齐 faster-whisper 的 mel 频谱。或直接接 transcribe-rs 的 whisper.cpp 绑定走 GGML 路径 | 中（1 周） | 多语种 + 高准确率场景；当前 small.en 仅英语 |
| **P2** | **FireRedVAD 替换 Silero**：F1 97.57%、False Alarm 2.69%（v6 无公开 F1 对比，但 v6 噪声错误率较 v4 仅 -16%，相对 FireRedVAD 仍有差距），且 FireRedVAD 支持流式 + mVAD（语音/歌唱/音乐）。已有 firered 引擎，加 VAD 模块成本低。**2026-08-04 更新**：v6 升级已部分缓解，此项紧迫度下降，但仍是有意义的下一步 | 小（3-5 天） | 标点触发与分段准确度提升，直接改善热词/纠错命中 |
| **P2** | **GGUF / K-quant 路径**（学 transcribe.cpp + CapsWriter）：用 llama.cpp + ggml-rs 跑 Qwen3-ASR / Fun-ASR-Nano / MOSS，q4_k 把 2GB→500MB 逐字节一致。可与 ONNX 并存作为「轻量档」 | 大（3-4 周） | 显存/内存减半，低端设备可用；移动端前置 |
| **P2** | **macOS 系统级语音输入**（Push-to-Talk 全局热键 + 模拟输入）：参考 CapsWriter 的 pynput 思路用 rdev + enigo（Rust 已有生态），或做 InputMethodKit 插件 | 中（1-2 周） | 从「应用内工具」升级为「系统级输入法替代」 |
| **P3** | **开源 + HF Space demo + 多语言绑定**：公开核心 asr-local crate（如 transcribe-rs 的 MIT 路径），提供 Python 绑定 + 在线 demo | 中 | 社区认知度；当前是私有项目零传播 |
| **P3** | **ffmpeg 集成字幕管线**：cli 加 `octopus asr --srt video.mp4`，自动抽音→降噪→VAD→ASR→字幕 | 小（3-5 天） | 一站式视频字幕，CapsWriter 已有此能力 |

**总体判断**：octopus 的 ASR 子系统在「多引擎统一管理 + 国内 4 家云端 + 工程化激活/缓存/热词同步」上是 tolaria 知识库里最完整的，工程纪律（spec 驱动、迁移链、bench、审计）也最强。但相对 sherpa-onnx / transcribe.cpp 的引擎覆盖广度、相对 faster-whisper / WhisperX 的词级时间戳 + 分离、相对 YuHuang 的双模型流式体验、相对 CapsWriter 的系统级输入集成，都有明显代差。P0 两项（时间戳 + 分离）是性价比最高的补齐方向。


---

## 2. 截图模块

### 2.1 Octopus 现状

Octopus 的截图能力集中在独立 crate `octopus-capx` 与 `desktop/src/record/screenshot_commands/`（区域 / 滚动 / 共享 helper 三个子模块），底层依赖 **xcap 0.9.6**（`crates/capx/Cargo.toml:7`），在 macOS 上 xcap 实际走的是 `CGWindowListCreateImage` 而非 ScreenCaptureKit。整套流程通过 Tauri 命令暴露给前端 React 选区 + Canvas 标注层。

**捕获能力**

- 全屏 / 多显示器：`capture_all_monitors` 用 `std::thread::scope` 给每块屏一个 spawned thread 并行截图，双屏 4K 从串行 ~800ms 降到 ~400ms（`crates/capx/src/capture.rs:21`）。
- 区域截图：前端选区确定后通过 IPC 二进制 Raw body 把 Canvas 合成 PNG 直接发后端（`crates/desktop/src/record/screenshot_commands/area.rs:530`）。
- 滚动截图：`start_scroll_recording` 30ms 截帧 + tokio watch 通道 + Canvas-Anchored NCC + Sobel 拼接引擎。**2026-08-04 重构**：原 `crates/capx/src/stitch.rs` 单文件（123KB）已拆分为 `crates/capx/src/stitch/` 模块目录（共 2966 行 / 5 个文件）：`mod.rs`（编排，927 行）+ `canvas_heal.rs`（画布常数尾自愈，613 行）+ `fallback_chain.rs`（降级链 5 层，839 行）+ `graybuf.rs`（灰度缓冲 + 投影，370 行）+ `ncc_match.rs`（NCC 匹配 + 抛物线精修，217 行）。含 `content_tail` 暗尾裁剪、`eff_strip_h` 自适应、画布常数尾每帧自愈等六轮迭代修复的鲁棒性逻辑（`docs/features/screenshot.md` §4）。
- 窗口截图：macOS 走 `capture_window_region` + `find_window_id_by_pid`（`capture.rs:425` / `:346`），用于滚动模式排除 overlay 窗口；目前未作为独立「窗口截图」入口暴露给用户。
- 多屏：每屏独立 Tauri 窗口，按物理坐标三级 fallback 匹配 xcap capture（精确坐标 → 同分辨率 → 索引，`area.rs:179`）。

**坐标处理**

AGENTS.md 明确写了「物理/逻辑坐标转换 ⚠️ 已踩坑 6+ 次」（`AGENTS.md:411`）。规则：xcap `Monitor::position()/size()` 是物理像素，需 ÷ `scale_factor`；`CGEvent::location()`、Tauri `inner_position()` 是逻辑 points 不再除；macOS Cocoa frame Y 轴翻转。建议捷径：已知 `CGDirectDisplayID` 时直接 `CGDisplay::new(id).bounds()` 拿逻辑 CGRect。

**标注**

工具栏（`frontend/src/components/Annotation/`，Screenshot 与 RecordAnnotation 共用）：**2026-08-10 归组**——形状（矩形/椭圆/菱形合并）+ 线条（直线/箭头/画笔/荧光/序号合并）+ 文字 + 模糊 + 水印 + 橡皮擦 / 撤销重做 / 清空 + OCR + 二维码识别。子模式在 ToolPropsPopover 浮层切换（图标随当前子模式变化）。**模糊 3 种**（Pixelate 像素化 / Gaussian Stackblur 纯 JS 像素操作 / Redact 黑条），blurMode 字段切换。**平铺水印**（density 密度 + angle 旋转 + color 颜色 + opacity 透明度，6 config 字段，工具栏水印按钮弹浮层输入）。ImagePreview 图片编辑器工具栏同步归组 + 水印。标注在选区内 Canvas clip 绘制，工具栏位置三选算法（下方 → 上方 → 内部底部）。

**OCR-on-screenshot**

`ocr_screenshot` 闭环（`area.rs:322`）：spawn_blocking 内 PNG 解码 → 图片入库 → PaddleOCR（PP-OCRv5 / v6-small）识别 → insert_ocr_item → 主线程 `open_compact_editor_tabs` 同时打开图片 + 文本双 tab → emit `ocr-screenshot://result` 把文本块推回 ImagePreview 叠加显示。`OcrLockGuard` 全局互斥。

**Translate-on-screenshot**（2026-08-11 已实现）

截图工具栏「翻译」按钮 → `translate_screenshot` 命令（同 `ocr_screenshot` 的 Raw body PNG + OcrLockGuard 互斥，尾部换成 `translate_window::show_at_mouse`）→ OCR → `do_translate_streaming(Float)` 流式翻译 → `translate_window` 只读浮窗（`emit_to` 定向推送译文）。ActionBar 选中翻译也于 2026-08-11 改走同一浮窗（原走 CompactEditor contrast tab）。详见 [spec](../superpowers/specs/archived/2026-08-11-screenshot-translate-float-window-design.md)。

**贴图（pin_window）**

三平台原生浮窗（`crates/desktop/src/ui/pin_window`）：macOS 自定义 `PinNSWindow` + `NSTrackingArea` hover；Windows Win32 `WS_EX_TOPMOST|LAYERED` + `UpdateLayeredWindow`；Linux GTK3 + Cairo。支持拖拽、滚轮缩放（鼠标位置为锚点）、hover 红色关闭按钮，单窗内存 < 5MB。`pin_screenshot`（`area.rs:648`）走自定义二进制协议（label + 4 个 f64 几何 + PNG bytes）。

**输出 / 快捷键**

输出：剪贴板 + DB（WebP/JPEG q85/q100）+ 系统保存对话框（PNG）。全局快捷键 `register_screenshot_shortcut` 默认 `Alt+S`，托盘菜单「截图」，可配置 + 热重载（`area.rs:80`）。

**macOS 专属**

`CGWindowListCreateImage` 截图、`CGEventSourceButtonState` FFI 检测右键取消（scroll 鼠标穿透态前端收不到右键，`area.rs:35-48`）、`save_frontmost_app`/`activate_prev_app` 在 scroll 模式把焦点交给下层应用让其能滚动（`scroll.rs:134`）。

### 2.2 竞品对比矩阵

| 维度 | Octopus | CleanShot X | Snapzy | eSearch | PixPin | HushSnap | ScreenCapture (xland) | Inkeys |
|---|---|---|---|---|---|---|---|---|
| 平台 | macOS/Win/Linux | macOS | macOS | Win/Linux/macOS | Win/macOS | Windows | Windows | Windows |
| 区域截图 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | —（批注） |
| 全屏 | ✅ 多屏并行 | ✅ | ✅ | ✅ | ✅ | ✅ 点击全屏 | ✅ | — |
| 窗口截图 | ⚠️ 仅内部用 | ✅ 可换背景 | ✅ Smart Element | ❌ | ✅ UI 元素识别 | ❌ | ❌ | — |
| 滚动截图 | ✅ Canvas-NCC 自研 | ✅ 全应用通用 | ✅ 子系统 | ✅ 模板匹配 + 任意方向 | ✅ + 超长 200 万像素 | ❌ | ✅ 自动滚动 | ❌ |
| 多屏 | ✅ 并行 + 三级匹配 | ✅ | ✅ | ✅ | ✅ | 单屏为主 | ✅ | ✅ |
| 标注工具 | **归组 6 按钮**（形状/线条/text/blur/水印/eraser）+ 撤销 | 15+（含聚光灯/取色/旋转） | 15 种 + 8 种模糊 + Mockup | 6 种 + 模糊 | 11 种 + 聚光灯 + 水印 | 6 种 + 马赛克 | 11 种 | 画笔为主 + 形状吸附 |
| 马赛克 | ✅ **3 种**（Pixelate/Gaussian/Redact，2026-08-10） | ✅ 像素化+模糊 | ✅ 8 种效果 | ✅ | ✅ + 智能擦除 | ✅ | ✅ | ❌ |
| 水印 | ✅ **平铺**（density+angle+color，2026-08-10） | ❌ | ❌ | ❌ | ✅ 单水印 | ❌ | ❌ | ❌ |
| 序号标注 | ✅ | ✅ Counter | ✅ Counter | ❌ | ✅ 序列号 | ❌ | ✅ 标号 | ❌ |
| 贴图 / 浮动 | ✅ 三平台原生 | ✅ 锁定+透明度+方向键 | ✅ Pin + 锁定穿透 | ✅ Ding + CSS 滤镜 | ✅ 文本/文件/LaTeX | ✅ 置顶图 | ❌ | 窗口定格 |
| OCR | ✅ PaddleOCR 离线 | ✅ Vision 设备端 24+ 语种 | ✅ Vision + 远程端点 | ✅ PaddleOCR + 在线 | ✅ + 公式识别 | ✅ PP-OCRv6 50 语种 | ❌ | ❌ |
| 翻译 | ✅ 截图翻译 + ActionBar 选中翻译 → translate_window 浮窗（2026-08-11） | ❌ | ❌ | ✅ 多引擎 + 屏幕翻译 | ✅ 内置 | ❌ | ❌ | ❌ |
| 二维码 | ✅ 多码全识别 | ✅ QR 读取 | ✅ QR 检测 | ✅ | ✅ 自动识别 | ❌ | ❌ | ❌ |
| 录屏 | ✅ 同模块 SCK | ✅ MP4/GIF + 摄像头 + 点击捕获 | ✅ + 视频编辑器 + Follow Mouse | ✅ + WebCodecs 帧级编辑 | ✅ 普通/快速 + 标注 | ❌ | ✅ GIF/MP4 | — |
| AI 特性 | OCR/二维码/VLM 预留 | Raycast Chat 集成 | Vision OCR + OpenAI 端点 + 抠图 | AI Vision 多模态 | 公式识别 | — | — | — |
| 全局快捷键 | ✅ 可配置 + 热重载 | ✅ 每模式独立 | ✅ + 冲突检测 | ✅ Alt+C | ✅ Ctrl+1/2 | ✅ Alt+Q | ✅ + CLI 参数 | ✅ |
| 输出格式 | PNG/WebP/JPEG/剪贴板/DB | + HEIC/WebP/Cloud | PNG/JPG/WebP + Cloud | + GIF/Anki | + PDF/AVIF | PNG/JPEG/BMP | 文件/剪贴板 | 画板文件 |
| 开源 | ✅（私有仓） | ❌ $29 买断 | ✅ BSD-3 | ✅ GPL-3.0 | ❌ 部分会员 | ✅ GPL-3.0 | 自定义许可 | ✅ GPL-3.0 |

### 2.3 Octopus 独特价值

1. **跨平台原生贴图 + 同模块录屏**：CleanShot/Snapzy/HushSnap/Inkeys 都只覆盖单平台。Octopus 一份代码三平台原生贴图窗口（NSWindow / Win32 LAYERED / GTK3），且截图与录屏共用选区 + 标注 + 坐标换算（`screenshot_geometry.rs`），是矩阵中唯一同时做到「跨平台 + 截图 + 录屏 + 贴图 + OCR + 二维码」一体化的方案。
2. **滚动拼接引擎的工程深度**：Canvas-Anchored NCC + 自写 Sobel + 每帧画布常数尾自愈 + 矮选区 `eff_strip_h` 自适应，经历过 release 实测的六轮回归（`docs/features/screenshot.md` §4–§5）。eSearch 也是模板匹配但只做一次性裁固定元素；PixPin 商业实现不开源。Octopus 的工程笔记本身就有移植价值。
3. **截图 → 剪贴板历史 → 编辑器闭环**：截图确认后 PNG SHA-256 去重入库、OCR 文本进剪贴板历史、CompactEditor 直接打开图片+文本双 tab 绑定编辑。竞品里只有 Snapzy 的 History + Annotate sidecar 做到了「可恢复编辑」，但缺少 OCR 文本与图片的强绑定视图。
4. **IPC 零编码二进制传输**：前端 `canvas.toBlob` → Raw body → 后端，去掉 base64 round-trip（双屏省 ~3s）；前端拿 RGBA 直接 `createImageBitmap(ImageData)` 走 GPU 路径（`area.rs:559`、`docs/features/screenshot.md` §9）。这是 Rust+Tauri 栈相对 Electron（eSearch/Screenity）的天然优势。
5. **隐私 + 离线**：OCR/二维码全本地 ONNX Runtime 推理，与 HushSnap/eSearch 同档位，优于需 Cloud 的 CleanShot Pro 和默认调远程的 Snapzy OCR 端点。

### 2.4 Octopus 不足 / 缺失

1. ~~**截图翻译 UI 缺失**~~ **✅ 已解决（2026-08-11）**：~~架构文档明确标注「数据通路已支持，UI 后续」~~。截图工具栏「翻译」按钮 + ActionBar 选中翻译均走 `translate_window` 只读浮窗（OCR→流式翻译→`emit_to` 定向推送）。详见 [spec](../superpowers/specs/archived/2026-08-11-screenshot-translate-float-window-design.md)。**仍缺**：eSearch 的「屏幕翻译」（贴图窗内文字替换为译文 + 定时翻译适合视频）——这是更重的图片翻译功能，留后续。
2. **没有窗口截图入口**：`capture_window_region` + `find_window_id_by_pid` 实现完备，但只用于滚动模式排除 overlay，用户层没有「截活动窗口」「Smart Element 检测」这类一键操作（CleanShot/Snapzy/PixPin 都有 UI 元素识别）。
3. **~~标注工具相对单薄~~（2026-08-10 大幅补齐）**：~~缺 PixPin 的水印、放大镜；缺 Snapzy 的 8 种模糊效果~~ → **已补齐 3 种模糊**（Pixelate/Gaussian Stackblur/Redact）+ **平铺水印**（density+angle+color）。工具栏归组精简（形状/线条合并，6 个标注按钮）。仍缺：CleanShot/Snapzy 的聚光灯、取色器、Mockup 背景、旋转翻转、多图合并、放大镜。
4. **贴图能力弱于 PixPin**：Octopus pin 只支持拖拽 + 缩放 + 关闭。PixPin 贴图支持透明度调节、锁定、鼠标穿透、取色、缩略图模式、文本/文件/LaTeX 贴图、批量操作、阴影颜色状态指示——是矩阵中贴图功能最全的。Octopus 贴图甚至没有透明度滑块和键盘方向键微调（CleanShot 有）。
5. **无云分享 / 团队流**：CleanShot Cloud、Snapzy BYOS（S3/R2/GDrive）都没有。若面向团队场景是缺口；若定位纯本地工具则可忽略。
6. **无 Quick Access Overlay**：截图后没有 CleanShot/Snapzy/HushSnap 都有的「右下角浮动卡片 → 拖拽到应用 / 一键 OCR / 编辑」快速动作面板。Octopus 截图后只能进标注工具栏或直接确认，少了一层「先看一眼再决定」的轻交互。
7. **滚动截图限制**：`start_scroll_recording` 大段 `#[cfg(target_os = "macos")]` gate，依赖 Quartz 全局鼠标追踪 + `set_ignore_cursor_events` 穿透 + 激活下层 app——这套在 Windows/Linux 上是否等价可用文档没明确，存在平台能力不均风险（架构文档 §截图不可统一 段也承认是 macOS 专属）。
8. ~~**`stitch.rs` 单文件 123KB**：拼接引擎密度过高，降级链五层 + 多个互相耦合的自愈机制，长期维护成本高，新人接手门槛大。~~ **✅ 已解决（2026-08-04）**：已拆分为 `stitch/{mod,canvas_heal,fallback_chain,graybuf,ncc_match}.rs` 五模块（2966 行），每模块职责单一、可独立单测。详见 [stitch-refactor spec](../superpowers/specs/archived/2026-08-04-stitch-refactor-design.md)。

### 2.5 建议改进方向（按性价比排序）

1. ~~**截图翻译 UI 接线**~~ **✅ 已完成（2026-08-11）**：截图工具栏「翻译」按钮 + ActionBar 选中翻译 → `translate_window` 只读浮窗（非原设想的 CompactEditor contrast——产品决策改为独立浮窗，行为统一）。详见 [spec](../superpowers/specs/archived/2026-08-11-screenshot-translate-float-window-design.md) + [plan](../superpowers/plans/archived/2026-08-11-screenshot-translate-float-window.md)。
2. **暴露窗口 / UI 元素截图入口**（高收益中投入）：`capture_window_region` + `find_window_id_by_pid` 现成。加一个工具栏按钮或快捷键，截图模式下按 `W` 切到「窗口模式」，用 `CGWindowListCopyWindowInfo` 列出可见窗口让用户点选。Snapzy 的 Smart Element（AX 查询自动检测 UI 元素）是更进一步的方案但 macOS 限定。
3. **贴图能力补齐**（中收益低投入）：pin_window 加透明度调节（`Ctrl+滚轮`）、锁定模式（`L` 键 + `setIgnoresMouseEvents`）、方向键微调位置。PixPin 的功能集是直接抄的模板，三平台原生实现已经在手里，主要是 UI/快捷键层补全。
4. **Quick Access Overlay**（中收益中投入）：截图确认后不立即关窗，右下角弹一张可拖拽缩略卡片，提供「复制 / 保存 / OCR / 翻译 / pin / 编辑」六动作。HushSnap 的「缩略图 → 左键 OCR / 悬停按钮 / 拖放」交互是很好的范本。这一层能显著降低「截完才发现要 OCR」的返工。
5. **~~标注工具扩充~~（部分完成 2026-08-10）**：~~模糊变体（高斯 / 智能擦除）可作为 Nice-to-have~~ → **已做 3 种模糊**（Pixelate/Gaussian Stackblur/Redact）+ **平铺水印** + **工具栏归组**。仍可扩充：聚光灯（高亮一区域其他变暗）、取色器（`C` 键复制 HEX/RGB）、文字段落数样式预设、智能擦除（自动检测敏感区域）。
6. ~~**`stitch.rs` 拆分 + 抽象**（低收益高投入但减债）：把 123KB 拆成 `strip_extract.rs` / `ncc_match.rs` / `fallback_chain.rs` / `canvas_heal.rs`，每层加单测。~~ **✅ 已完成（2026-08-04）**：拆分为 `stitch/{mod, canvas_heal, fallback_chain, graybuf, ncc_match}.rs`（命名与原建议略异——实际多了 `graybuf.rs` 抽象灰度缓冲，无独立 `strip_extract`）。下一步可继续：每模块补单测覆盖（stitch-refactor plan task 5 已开始补 `FallbackStep` 单测，可继续推进）。
7. **滚动截图跨平台验证**（中收益中投入）：把 macOS 专属的 `CGEvent` 鼠标追踪 + `set_ignore_cursor_events` 在 Windows/Linux 上找等价（Win32 `SetWindowDisplayAffinity` + 低级鼠标钩子，RecEasy 已有参考；X11/Wayland 较难）。或退一步：非 mac 平台只支持「手动滚动 + 自动拼接」，不做穿透激活下层 app。
8. **截图历史浏览器**（低收益低投入）：Snapzy/PixPin/CleanShot 都有「按时间/类型过滤的历史」恢复可编辑状态。Octopus 剪贴板历史已含图片条目，只需在 CompactEditor 加「截图历史」入口即可复用。

**关键文件索引**

- `crates/capx/Cargo.toml` — 依赖声明（xcap 0.9.6 / image 0.25 / imageproc 0.25）
- `crates/capx/src/capture.rs:21` `capture_all_monitors` 并行截图；`:297` `capture_region_excluding_window`；`:425` `capture_window_region`；`:346` `find_window_id_by_pid`
- `crates/capx/src/stitch/` — 滚动拼接引擎模块目录（2026-08-04 重构，Canvas-Anchored NCC + Sobel）：`mod.rs`（编排）/ `canvas_heal.rs`（画布自愈）/ `fallback_chain.rs`（降级链）/ `graybuf.rs`（灰度缓冲）/ `ncc_match.rs`（NCC 匹配）
- `crates/desktop/src/record/screenshot_commands/area.rs:103` `start_screenshot`；`:322` `ocr_screenshot`；`:421` `scan_qrcode_screenshot`；`:648` `pin_screenshot`；`:80` `register_screenshot_shortcut`
- `crates/desktop/src/record/screenshot_commands/scroll.rs` — 滚动截图 580 行 `start_scroll_recording` + ESC 全局快捷键动态注册
- `crates/desktop/src/record/screenshot_geometry.rs` — 坐标换算纯逻辑（可单测）
- `docs/features/screenshot.md` — 完整模块文档
- `AGENTS.md:411-433` — 物理/逻辑坐标转换 gotcha

---

## 3. 剪贴板模块

### 3.1 Octopus 现状

octopus 把「剪贴板历史」做成了 ASR / OCR / 截图 / 文件搬运的**统一中转表**——这是它与所有纯剪贴板管理器最大的结构性差异。

**统一表 schema（`crates/infra/src/db.sql:63-120`）。** `clipboard_history` 表用 `item_type` 枚举区分 `text` / `voice` / `ocr` / `image` / `file` 五类（`crates/clipboard/src/model.rs:5-12`），数据按三层模型拆开：

- `content`（TEXT）——voice/ocr/text 存全文，image/file 存空串（`db.sql:69`）
- `ref_data`（TEXT）——image 存 `MD5(RGBA)` blob_hash，file 存 JSON 路径数组（`db.sql:70`）
- `meta_info`（TEXT JSON）——按类型存不同 schema：image `{w,h,size}`、voice `{engine,model,char_count,asr_mode,polished,polish_model,duration_ms}`、ocr `{engine,model,char_count}`、text `{char_count}`、file `{files:[{size,type}]}`（`model.rs:39-67`）
- `segments`（仅 voice 段 JSON）、`is_favorite` / `is_rich`（标记 HTML/RTF 富文本，`watcher.rs:196-198`）/ `is_deleted`（软删标记）

该表**吞并了原 `transcriptions` 表**（v17 DROP，`docs/architecture.md:185`），所有 ASR 数据以 `item_type='voice'` 入库，热词挖掘的 `list_recent_text` 直接复用此表（INV-C1）。

**FTS5 trigram 全文索引（`db.sql:100-120`）。** `clipboard_history_fts` 虚表用 `tokenize='trigram'`，对 CJK 子串友好（不像 unicode61 需要分词器）。查询逻辑分两段（`store.rs:123-145`）：≥3 字符包成 `"phrase"` 走 `MATCH` 短语查询；<3 字符降级 `content LIKE '%x%'`。三个触发器 `clip_fts_ai/ad/au` 在事务内增量同步，无需周期 rebuild（架构注释说 `rebuild_fts_index` 已作为死代码删除）。查询额外加 `ORDER BY created_at DESC, id DESC` 二级排序消除毫秒戳同秒不稳。

**图片双存储 + 引用计数（`image.rs` + `store.rs:468-530`）。** 原图存文件系统 `~/Documents/octopus/screens/<hash>.jpg`（2026-07-29 从 DB BLOB 改 FS），缩略图 240×240 存 `image_data.thumb`（`db.sql:89-98`）。编码链按 `IMAGE_SAVE_QUALITY`（默认 `jpeg:100`，可配 webp/jpeg 多级降级）+ `THUMB_SAVE_QUALITY`（默认 `jpeg:5`）。`hash_rgba` 用 MD5（2026-07-29 从 SHA-256 换 MD5，剪贴板去重无需密码学强度）。删除条目时 `delete_image_if_unreferenced` 检查引用计数，归零才删文件 + DB 行（`store.rs:521-530`）。

**软删回收站（仅 voice，`store.rs:249-297`）。** 2026-07-29 策略反转后：voice 软删（`UPDATE is_deleted=1`，500 条上限 `VOICE_TRASH_MAX`，超出按 `created_at ASC` 物理删最老的），text/ocr/image/file 一律物理 DELETE。**回收站概念不暴露给用户**——无 trash tab、无还原命令（`restore_item` / `empty_trash` 已删，`store.rs:258`）。`is_deleted` 仅是 voice 的内部标记，给热词挖掘 + bigram 语料留数据（INV-C1：`list_recent_text` 故意不过滤此列）。短 voice（content <5 字符）直接物理删（对 bigram 语料无价值）。

**自动清理（`cleanup.rs`）。** scheduler 每 10 分钟在 CPU 空闲（<30%）时跑 `run_cleanup`：① 按天数（默认 30 天，`clipboard_max_age_days`）物理删非收藏超龄项；② 按数量（默认 1000 条，`clipboard_max_items`）超出时先清回收站再清活跃项；③ 孤立 blob 回收。全部物理删（容量管理不走软删）。

**Listener（`watcher.rs` + `clipboard_queue.rs`）。** 基于 `clipboard-rs 0.3`（features=`image,wayland`，`Cargo.toml:9`），**非 arboard**。`ClipboardWatcherContext::start_watch()` 跑在独立线程，三端机制不同（`docs/architecture.md:183`）：macOS 轮询 `NSPasteboard.changeCount`（500ms）、Windows 事件驱动 `AddClipboardFormatListener`、Linux X11 XFixes 事件、Wayland 两级 MIME+text 轮询。变化回调只 `enqueue()` 一个 `()` 信号到 mpsc channel（<1μs），后台 worker 串行消费 `handle_clipboard_change`——避免 watcher 线程被 WebP 编码阻塞导致连续复制丢通知（2026-07-21 P0-5 重构）。类型判断优先级 `files > image > text`，非三者静默跳过避免 Adobe/Office 专有格式触发 error 日志（`watcher.rs:184-186`）。

**OCR 联动（`clipboard_commands.rs:415-512`）。** 三处入口（截图工具栏 / 图片预览 / 剪贴板图片条目）识别文本后统一走 `insert_ocr_clipboard_item` → 新建 `item_type='ocr'` 条目 → 前端 `openCompactEditorTab(id)` 打开绑定 tab 编辑。`ocr_image` 命令用全局 `OcrLockGuard::try_acquire()` 互斥（防多任务并发推理），PaddleOCR PP-OCRv6-small，返回 `{text, blocks:[{text,x,y,w,h,score}]}`。还有 `scan_qrcode_image`（zxing-cpp）。

**Pin / favorite / 富文本。** `toggle_favorite`（`store.rs:232`）；`is_rich` 标记同时含 HTML/RTF 的文本条目（`watcher.rs:196-198` 用 `handle.has(Html) || has(Rtf)`），但**仅作标记，不单独存储 HTML/RTF 原文**——content 只存纯文本。

**快捷键 + 快速粘贴（`clipboard_window.rs:247-265` + `clipboard_commands.rs:188-229`）。** 全局快捷键 `clipboard_shortcut`（默认 `Alt+C`，`config.rs:301-303`）经 `tauri_plugin_global_shortcut` 注册，热重载。`paste_clipboard_item` 双击条目：写剪贴板（设 suppress flag 防 watcher 回环）→ hide 窗口 → sleep 300ms 等焦点稳定 → `focus.restore_focus()` + `simulate_paste()`（macOS osascript 发 Cmd+V）。

**无云同步。** `octopus-sync`（通用 git 同步 crate）仅被 vault + 热词使用（`docs/architecture.md:23,191` 的 `vault_sync` 任务），**clipboard crate 与 desktop/clipboard 目录完全不引用 sync**——grep 确认零依赖。这是与 EcoPaste-Pro / UniClipboard 最显著的能力缺口。

### 3.2 竞品对比矩阵

| 维度 | Octopus | CopyQ | Ditto | Maccy | EcoPaste | EcoPaste-Pro | UniClipboard | Paster | ortu | mini-clipboard | VloamClip |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 平台 | Win/Mac/Linux | Win/Mac/Linux | Win | Mac | Win/Mac/Linux(x11) | 同左 | Win/Mac/Linux+iOS/Android | Win/Linux | Win/Mac/Linux | Mac | Mac/Win |
| 技术栈 | Rust+Tauri | Qt6/C++ | C/C++ | Swift | Tauri v2+React | 同左+插件 | Tauri 2+React | Qt6/C++ | Tauri v2+SvelteKit | Swift | Wails v3(Go) |
| License | 闭源? | GPL-3.0 | GPL-3.0 | MIT | Apache-2.0 | Apache-2.0 | AGPL-3.0 | GPLv3(未开源) | MIT | Apache-2.0 | 闭源(买断) |
| text | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| image | ✅ JPEG/MD5 去重 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ 流式 | ✅ | ✅ blob 去重 | ✅ | ❌ 仅文本 |
| rich(HTML/RTF) | ⚠️ 仅标记不存原文 | ✅ 多 MIME | ✅ RTF/HTML | ✅ | ✅ XSS 过滤 | ✅ 12 类 | ❌ | ✅ | ⚠️ | ✅ RTF/HTML | ❌ |
| file | ✅ JSON 路径数组 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
| 语音/ASR | ✅ **独有**(voice 类型) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| OCR | ✅ **独有**(PaddleOCR) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ Tesseract+Paddle | ❌ | ❌ | ❌ |
| 搜索 | FTS5 trigram+LIKE | 正则+实时 | 通配符+正则 | 实时过滤 | 模糊 | FTS5+空格分词+通配符 | **加密 FTS** | 关键词高亮 | FTS5+模糊重排 | 关键词+类型+来源 | FTS5 |
| pin/favorite | ✅ favorite | ✅ tab+pin | ✅ groups | ✅ pin | ✅ favorite+note | ✅+智能分组 | ✅ | ✅ 文件夹 | ✅ pin+用户组 | ✅ pinboard | ✅ frequent |
| 加密 | ❌ | ✅ 标签页(QCA) | ✅ DB(SQLite3MC) | ❌(过滤密码) | ❌ | ⚠️ 仅凭据 | ✅ **E2EE** XChaCha20 | ❌ | ✅ **字段级** AES-256-GCM | ❌ | ❌ |
| 同步 | ❌ | ⚠️ 文件夹 | ⚠️ LAN | ❌ | ❌ | ✅ WebDAV+transfer | ✅ **P2P** iroh/QUIC | ⚠️ LAN+远程 | ❌(roadmap) | ❌ | ❌ |
| macro/脚本 | ❌ | ✅ **ECMAScript** | ✅ ChaiScript | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ snippets+transforms | ❌ | ❌ |
| 常用优先 | ❌(按时间) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ **按频率** |
| 粘贴队列 | ✅ **paste stack**（2026-08-05） | ❌(buffers 5 个) | ✅ 5 buffers | ❌ | ❌ | ❌ | ❌ | ✅ 快速插入 | ✅ **paste stack** | ❌ | ✅ **FIFO/LIFO** |
| 预览 | ✅ 图片缩略图+全图 | ✅ F4 | ✅ | ✅ | ✅ | ✅ CodeMirror | ✅ Quick Panel | ✅ 贴图 | ✅ | ✅ Quick Look | ✅ 悬停空格 |
| 回收站 | ⚠️ 仅 voice 内部 | ✅ | ❌ | ❌ | ❌ | ✅ 7/15/30 天 | ❌ | ✅ 可恢复 | ❌ | ❌ | ❌ |
| 热键/快速粘贴 | ✅ Alt+C+osascript | ✅ 可自定义 | ✅ Ctrl-\` | ✅ ⇧⌘C | ✅ | ✅ Win+V 接管 | ✅ | ✅ Ctrl+1..5 | ✅ 可重绑 | ✅ ⇧⌘P | ✅ Alt+C |

### 3.3 Octopus 独特价值

1. **多类型统一表是结构性的护城河。** text/voice/ocr/image/file 共用一张 `clipboard_history` 表 + 一个 FTS5 索引（`db.sql:66-107`），意味着用户搜索「上周那段关于 X 的话」时，ASR 转写、OCR 识别、手动复制的文本**一并命中**——这是 CopyQ / Maccy / EcoPaste 全部做不到的（它们没有 voice/ocr 类型）。这是 octopus 作为「桌面工具集合」而非「纯剪贴板管理器」的红利。
2. **FTS5 trigram 对 CJK 友好。** `tokenize='trigram'`（`db.sql:106`）不需中文分词器就能做子串匹配（搜「剪贴板」能命中「剪贴板模块」），这是 SQLite FTS5 在中文场景的较优解；多数竞品（Maccy/Ditto）只有 LIKE 或简单子串，CopyQ 走正则。EcoPaste-Pro 才在 2026-05 加了空格分词+通配符。
3. **图片 OCR 闭环。** 图片条目 → `ocr_image` → `insert_ocr_clipboard_item` → 绑定 tab 编辑（`clipboard_commands.rs:415-512`），文本自动进 FTS5 可搜索——Paster 也有此能力（PaddleOCR+Tesseract），但 octopus 的 OCR 结果**直接进统一历史表**而非旁路存储，与 ASR/文本同列同搜。
4. **工程细节扎实。** watcher→queue→worker 三段解耦避免阻塞 NSPasteboard 通知（`clipboard_queue.rs`）、JPEG q100 + nearest resize 把大图编码从 6s 压到 55ms（`image.rs:30-37`）、MD5 去重 + 引用计数回收、FTS5 触发器事务内同步——这些是生产级剪贴板工具的硬功夫。

### 3.4 Octopus 不足 / 缺失

1. **本地明文存储，与竞品同档位（Maccy / EcoPaste / CopyQ 默认明文），仅在加 sync 时成为问题。** `clipboard_history` 表明文存储，无应用层加密。但：① 字段级 AES-GCM 加密会破坏 FTS5 trigram 索引（加密列无法 tokenize）——这是 ortu 那套方案无法移植的核心障碍；② 运行时浮窗/历史可见，加密只能防「离线磁盘取证」，威胁模型与 FileVault/BitLocker 全盘加密重叠，应用层 ROI 差；③ 行业惯例：本地优先剪贴板工具默认明文 + 文件权限 0600 + 全盘加密覆盖本地威胁。**真正需要加密的时机是接入 sync 时**（同步前必须加密，否则 git repo 明文外泄）——见 §3.5 P2。~~Maccy/CopyQ 至少做的「过滤密码管理器 concealed type」octopus 也没做~~（**2026-08-05 已补齐跨平台 concealed hint 检测**——macOS ConcealedType + Windows ExcludeClipboardContentFromMonitorProcessing + Linux x-kde-passwordManagerHint，见 §3.5 P3）。
2. **无云同步 / 无跨设备。** vault 和热词都用 `octopus-sync` git 同步，**唯独 clipboard 不用**（grep 确认零依赖）。EcoPaste-Pro 的 transfer 插件 + UniClipboard 的 iroh P2P 都已成熟。octopus 已有 sync 基建却没接到 clipboard，是低成本就能补的能力。
3. **无 macro / 脚本 / snippets。** CopyQ 的 ECMAScript 引擎、Ditto 的 ChaiScript、ortu 的 snippets+transforms（`{{date}}` 变量、JSON 美化、Base64 编解码）octopus 都没有。对开发者用户这是高价值功能。
4. **无常用优先排序。** 列表只按 `created_at DESC` 排（`store.rs:104`），不像 VloamClip 按使用频率排序。高频话术只能靠 `is_favorite` 手动收藏。
5. **~~无粘贴队列~~（2026-08-05 已实现 Paste Stack）。** Cmd+点击多选入栈 + Cmd+Shift+V 全局热键逐条弹出粘贴（FIFO）+ 队列 Tab（拖拽排序/单条删除/清空）+ hover overlay 详情。对标 ortu paste stack / VloamClip FIFO 队列。LIFO 模式后续可加。
6. **富文本仅标记不存原文。** `is_rich` 标记了 HTML/RTF 存在（`watcher.rs:196-198`），但 content 只存纯文本——复制带格式的文档再粘贴会丢格式。CopyQ/Ditto/EcoPaste 都保留多 MIME 原文。
7. **回收站不暴露给用户。** 仅 voice 内部软删（给热词挖掘用），text/image 删除即物理删，误删不可恢复。Paster/EcoPaste-Pro 有可恢复回收站。
8. **Wayland 支持弱。** clipboard-rs 的 Wayland 是两级轮询（MIME+text 500ms），不如 arboard/wl-clipboard-rs 走协议事件优雅（ortu 文档明确指出此点）。
9. **~~无密码管理器感知~~（2026-08-05 跨平台已补齐）。** `watcher.rs::handle_clipboard_change` 开头检测三平台密码管理器 hint（macOS `org.nspasteboard.ConcealedType` / Windows `ExcludeClipboardContentFromMonitorProcessing` / Linux `x-kde-passwordManagerHint`），命中静默跳过——对标 Maccy/CopyQ。clipboard-rs 0.3.4 四后端的 `ContentFormat::Other` 统一支持任意类型字符串检测。

### 3.5 建议改进方向

**P1（补齐 sync）。** 复用现有 `octopus-sync`（git）或新加 WebDAV/transfer 插件（参考 EcoPaste-Pro 的双插件拆分：`tauri-plugin-eco-webdav` 备份 + `tauri-plugin-transfer` 实时同步）。**必须做防回环**（传输回写指纹，避免接收→回写剪贴板→watcher 重复入库→重复推送）。凭据走系统钥匙串不明文落库。统一表的好处是同步一张表即同步 text/voice/ocr/image/file 全类型——这是相对竞品的同步优势。**⚠️ 前置依赖**：同步前必须先做整库加密（见下 P2），否则 git repo 明文外泄。

**P1（粘贴队列）✅ 2026-08-05 已实现。** 内存 `Mutex<VecDeque<String>>` FIFO 队列（不持久化，重启清空——粘贴队列是临时操作流）。交互：剪贴板浮窗 Cmd+点击多选条目（绿色高亮 + 序号 badge ①②③）→ 底部「入栈」按钮 → 自动切到队列 Tab → 用户切到目标应用按 `Cmd+Shift+V`（`paste_stack_shortcut`，全局热键）逐条弹出粘贴（`pop_and_paste`：pop front → 写剪贴板 + suppress flag → restore_focus → simulate_paste）。队列 Tab（Cmd+8）：拖拽排序（`@dnd-kit/sortable`，WKWebView HTML5 DnD 不可靠）、单条删除、清空全部、hover overlay 详情（独立 hoverIndex + 按 id 查完整 ClipboardItem）。详见 [paste-stack spec](../superpowers/specs/archived/2026-08-05-paste-stack-design.md) + [paste-queue-tab spec](../superpowers/specs/archived/2026-08-05-paste-queue-tab-design.md)。**未做**：LIFO 模式、去重（当前重复入栈会保留两条）。

**P2（常用优先 + snippets）。** ① 加 `paste_count` 列 + 复制时 `touch_created_at` 同时 `paste_count += 1`，提供「按频率」排序选项（参考 VloamClip Frequent tab）；② snippets 表（已有 `prompts` 表可借鉴），支持 `{{date}}` / `{{clipboard}}` 变量 + transforms（trim/UPPER/Base64/JSON pretty）。

**P2（富文本原文）。** `is_rich` 已标记，加 `rich_data` 列（或新表 `clipboard_rich`）存 HTML/RTF 原文，粘贴时按目标应用决定格式。HTML 渲染预览必须加 XSS 过滤（dompurify，参考 EcoPaste v0.5.0）。

**P2（回收站对用户可见）。** 把 voice 的 `is_deleted` 软删模式扩到全类型，加 trash tab + 还原命令 + 清空回收站。容量上限按 EcoPaste-Pro 的 7/15/30 天可配。

**P2（整库加密——sync 的前置依赖，本地场景可选）。** 用 **sqlite3mc / SQLCipher 整库加密**（Ditto 用的就是 SQLite3MC；`rusqlite` 加 `bundled-sqlite3mc` feature 即可），**不是字段级 AES-GCM**。理由：① 性能透明（页级 AES，~5-15% DB op 开销）；② FTS5 trigram 索引照常工作（索引也加密，搜索不受影响——字段级加密做不到这点）；③ 应用代码 0 行改动（连接时传 key 即可）。**密钥管理复用 vault 现成基建**：`crates/vault/src/keychain.rs` 的 HKDF-SHA256(machine_id || username || 常量) 派生方案（注释坦言是 obfuscation 而非真加密，但比纯硬编码好一档——攻击者需要知道 machine_id + username，不只逆向二进制）。**触发时机**：纯本地场景靠 FileVault/BitLocker + 文件权限 0600 已足够；**接入 sync（P1）时此项变为强制前置**，否则同步出去的就是明文。

**P3（concealed type 检测——做个好公民）✅ 跨平台 2026-08-05 已实现。** `watcher.rs::handle_clipboard_change` 开头（files/image/text 分支前）用 `CONCEALED_HINTS` 常量数组（`#[cfg]` 门控各平台 hint）+ `handle.has(ContentFormat::Other(...))` 检测，命中静默 return。覆盖三平台密码管理器复制场景：macOS `org.nspasteboard.ConcealedType`（1Password/Bitwarden/iCloud Keychain/KeePassXC）/ Windows `ExcludeClipboardContentFromMonitorProcessing`（MS 官方 format）/ Linux `x-kde-passwordManagerHint`（KeePassXC 事实约定）。clipboard-rs 0.3.4 四后端（macos/win/x11/wayland）的 `ContentFormat::Other` 均支持任意类型字符串检测，零新依赖。octopus autotype 的 `suppress_next` 不变（macOS 双重保险，Win/Linux autotype 未实现）。详见 [spec](../superpowers/specs/archived/2026-08-05-macos-concealed-type-skip.md)。

**P3（智能分组）。** 参考 ortu 的规则分类器（URL/Code/JSON/Shell/Email/Secret/Path + 置信度打分）或 EcoPaste-Pro 的 12 类扩展识别（加颜色/Markdown/Windows 指令），把扁平历史变成可治理的资料库。octopus 已有 `item_type` 一级分类，可在其上加二级 `subtype`。

> **修订记录（2026-08-04）**：原 P0「secret 检测 + 字段级加密」已删除——字段级 AES-GCM 会破坏 FTS5 trigram 索引（无法 tokenize 加密列），运行时浮窗可见使加密只能防「离线磁盘取证」（与 FileVault 全盘加密威胁模型重叠，应用层 ROI 差）。secret 分类器（30 个正则探测器）误报率高、维护成本重、对运行时可见的剪贴板价值有限，一并删除。改为 P2 整库加密（绑定 sync 时机）+ P3 concealed type 检测（零成本好公民行为）。

> **修订记录（2026-08-05）**：剪贴板模块两项落地——① **Paste Stack（P1 粘贴队列）已实现**：FIFO 队列 + Cmd+Shift+V 全局热键逐条弹出 + 队列 Tab（@dnd-kit 拖拽排序 + hover overlay）；② **macOS ConcealedType 检测（P3）已实现**：`watcher.rs` 检测 `org.nspasteboard.ConcealedType` 静默跳过密码管理器复制。§0.4 总览表、§3.2 对比矩阵（粘贴队列 ❌→✅）、§3.4 不足（第 1/5/9 项）、§3.5（P1/P3 标 ✅）均已同步。Windows/Linux 的 concealed hint 检测仍是 follow-up。

---

## 4. 翻译模块

### 4.1 Octopus 现状

Octopus 的翻译能力集中在 `octopus-translation` crate（`crates/translation/`）和 desktop 层的 `action_bar/action_bar_commands/translate.rs`，采用「三引擎统一 trait + 全局缓存 + 流式分段」的架构。

**统一 trait + 缓存**。三引擎统一实现 `TranslationEngine` trait——`async fn translate(&self, text, source_lang, target_lang) -> Result<String>` + `fn name(&self) -> &str`（`crates/translation/src/engine.rs:7-11`）。trait 于 2026-07-17 改造为 `#[async_trait]` 以支持云端 HTTP 调用，本地引擎实现里 `.await` 立即返回。全局 `HashMap<String, Arc<dyn TranslationEngine>>` 缓存（`engine.rs:14-15`）：m2m100 按 `local:{spec}` 缓存、opus-mt 按 `local:opus-mt-{src}-{tgt}` 方向键缓存（`engine.rs:49-66`），云端引擎每次按 DB 行即时构造、不入缓存（避免配置编辑后命中陈旧实例）。

**三引擎**：
- **Opus-MT**（MarianMT，~30M/方向，中英互译）：`crates/translation/src/opus_mt.rs`。按方向加载子目录 `~/.octopus/models/translate/opus-mt/{zh-en,en-zh}/`，各含 encoder/decoder int8 ONNX + tokenizer.json（`opus_mt.rs:31-46`）。greedy 解码配 **repetition_penalty=1.3 + no_repeat_ngram_size=3** 防重复（MarianMT 训练用 beam search，greedy 易陷入模式循环），penalty 逻辑抽为纯函数 `apply_penalties` 并有 6 个单测守护边界（`opus_mt.rs:338-367`、test 模块 369-465）。输入预处理 **`normalize_cjk_spaces`** 移除 CJK 邻接空格——opus-mt tokenizer（WhitespaceSplit+Metaspace）对带空格中文产生句中独立 `▁` token 偏离训练分布，致 decoder 过早 EOS、译文截断为第一段（`opus_mt.rs:306-332`，含 5 个单测）。tokenizer 加载时删除 Xenova 导出的 `precompiled_charsmap=null` 字段规避 tokenizers 0.21 panic（`opus_mt.rs:222-243`）。
- **m2m100-418M**（ONNX int8，100+ 语言互译）：`crates/translation/src/m2m100.rs`。encoder/decoder quantized ONNX，decoder greedy 解码带「8-token 重复检测」硬截断（`m2m100.rs:121-127`），长文本按句子切分打包为不超过 900 tokens 的 chunk（`m2m100.rs:137-196`）。
- **CloudLlmEngine**（OpenAI 兼容云端 LLM）：`crates/translation/src/cloud.rs`。覆盖 **5+1 家服务商**——OpenAI / DeepSeek / 阿里云百炼 / 智谱 BigModel / Moonshot(Kimi) / MiniMax，差异仅在 DB models 行的 `provider`/`source`(base_url)/`secret_key`/`model_name`（`cloud.rs:1-4,16-39`）。内部复用 `octopus_llm::chat_text_with_prompt`（`reqwest::blocking`），blocking 调用由外层 `tauri::async_runtime::block_on` 在 worker 线程隔离。翻译 prompt 复用 CopyTranslator 风格：`"Translate the following text from {src} to {tgt}. Only output the translation, without any explanation or extra text."`，语言代码映射成英文全称（`cloud.rs:57-76`）。`is_thinking` 模型翻译时由 octopus-llm 自动关闭 thinking。

**策略解析 + 引擎加载分离**。`TranslateStrategy` 解析（`action_bar/action_bar_commands/translate.rs:30-56`）只决定路径不预加载引擎：`resolve_active_engine("translate")` 取激活模型 → 按 `entry.is_local_or_builtin()` 分流 `LocalModel` / `CloudModel`。未激活 / 云端缺 secret_key → **`FallbackLlm` 分支**（复用激活润色 LLM 兜底翻译，避免到 translate 时才报错）。真正的引擎加载延迟到 `do_translate`（`translate.rs:75-124`）：opus-mt 按文本方向 `load_opus_mt(src, tgt)`、m2m100 按 `local:{model_name}` spec、云端按 ResolvedEngine 字段即时构造。

**API key 加密**（vault feature on 时）：`models.secret_key` 的云端行以 `v1:<base64>` 加密存储，`do_translate` 经 `try_decrypt_secret_global` 透明解密（`translate.rs:100-103`），vault 未解锁或密文损坏时直接 Err 而非把密文当 bearer 发出去（`crates/desktop/src/vault/vault_secret_access.rs:1-30,47-60`）。本地 manifest JSON / 未迁移明文 → no-op 原样返回。

**流式翻译 + 双语对照**。`do_translate_streaming` 按换行切分段落逐段翻译，每段完成 emit 累积结果（`translate.rs:266-304`）。双事件名隔离（2026-07-17 修复跨窗口泄漏）：`Result` target 走 `translate-progress`/`translate-done`（payload 裸 String），`CompactEditor { session_id }` target 走 `compact-editor://translate-progress`/`done`（payload `{ sessionId, text }`），前端按 sessionId 路由到具体 tab（`translate.rs:126-184`）。**双语对照**通过 CompactEditor 的 `mode="contrast"`（左原文右译文）实现——`compact_editor_commands.rs:28,57-64` 携带 `translated_text` + `translate_session_id` 字段。后端额外缓存 done 终止态（`TRANSLATE_RESULTS` HashMap，上限 64，`translate.rs:204-261`）兜底 Tauri v2 fire-and-forget 事件在新窗口 listener 注册前丢失的竞态。

**不足**：
- **无 glossary / 术语表 / memory**——`grep -rn "glossary\|术语"` 在 translation crate 零命中，全仓无翻译术语持久化机制（对比 kiss-translator 的「自定义 AI 术语词典」、Pot 的生词本导出）。
- **双语对照仅限 CompactEditor contrast 模式**，无网页级段落级双语对照（kiss-translator 的核心卖点）。
- ~~**无 OS 级划词翻译 / 屏幕翻译 / 截图翻译**~~ **部分已解决（2026-08-11）**：~~translation crate 纯文本输入输出，不与 OCR / 截屏 / 系统选区集成~~。截图翻译（`translate_screenshot` 命令 OCR→翻译）+ ActionBar 选中翻译已接入 `translate_window` 浮窗。**仍缺**：OS 级划词翻译（监听系统选区）、屏幕翻译（贴图窗内文字替换为译文，eSearch 做法）。
- **云端翻译方向固定中英互译**——`detect_translate_direction` 仅按是否含 CJK 二分 zh/en（`translate.rs:58-67`），m2m100 虽支持 100+ 语言但上层路由不暴露语言选择。

### 4.2 竞品对比矩阵

| 维度 | **Octopus** | kiss-translator | Pot (pot-app) | esearch | Paster | STranslate |
|---|---|---|---|---|---|---|
| **本地引擎** | Opus-MT (MarianMT int8) + m2m100-418M (ONNX int8) | 无（纯调用方） | Ollama 离线（插件） | 无（Google 免费） | Argos Translate（中英） | 无（系统服务） |
| **云端多 provider** | 5+1 家 OpenAI 兼容（OpenAI/DeepSeek/Aliyun/BigModel/Moonshot/MiniMax） | 传统4家(Google/MS/腾讯/火山)+AI(OpenAI/Gemini/Claude/Ollama/DeepSeek/OpenRouter)+DeepL | OpenAI/Gemini/智谱+百度/腾讯/火山/有道/DeepL/Bing/Yandex 等 15+ | Google/DeepL/百度/ChatGPT/自定义 | 无（仅离线） | 多家国产+DeepL |
| **OCR/截图翻译** | ✅ 截图翻译 + ActionBar 选中翻译 → translate_window 浮窗（2026-08-11） | ❌ | ✅ 截图OCR+截图翻译+外部HTTP调用 | ✅ 截屏→OCR→翻译+**屏幕翻译**（贴图替换） | ✅ 截图OCR→离线翻译 | ✅ 图片翻译 |
| **双语对照** | △（仅 CompactEditor contrast 左右栏） | ✅✅ 网页段落级双语对照（核心卖点） | ❌ | △（屏幕翻译贴图） | ❌ | ❌ |
| **流式** | ✅ 按换行分段 emit 增量 | ✅ 流式传输+聚合批量 | ❌ | ❌ | ❌ | ❌ |
| **glossary/memory** | ❌ | ✅ 自定义AI术语词典+上下文记忆 | ✅ 生词本导出(Anki/欧路/有道/扇贝) | ✅ 翻译结果存Anki | ❌ | ❌ |
| **API key 管理** | ✅ vault `v1:` 加密+透明解密+未解锁即 Err | △（浏览器 storage，syncCrypto 同步加密） | △（本地 SQLite） | △（本地配置） | N/A | △ |
| **划词翻译** | ❌ | ✅ Alt+S 弹窗+多服务对比 | ✅ 划词+输入+剪贴板监听 | ✅ 识屏选词 | △（剪贴板触发） | ✅ |
| **跨平台** | ✅ Tauri (Win/macOS/Linux，CoreML/CUDA/DirectML EP) | ✅ 浏览器扩展+油猴（含 Android Kiwi/iOS Orion） | ✅ Tauri (Win/macOS/Linux+Wayland) | ✅ Electron (Win/Linux/macOS) | △ Win+Linux（macOS 不成熟） | △ 仅 Windows (WPF) |
| **License** | 待确认（项目内） | GPL-3.0 | GPL-3.0 | GPL-3.0 | GPLv3（代码未公开，仅分发二进制） | 待确认 |
| **活跃度** | 内部项目 | 11.6k★ v2.0.28 | 19.1k★ v3.0.7（2026-07 更新） | v15.3.3 个人维护 | v1.3.0 闭源 | 社区活跃 |

### 4.3 Octopus 独特价值

1. **三引擎统一 trait + 5+1 云 LLM，本地/云端/兜底三级降级**——`TranslationEngine` async trait 让 Opus-MT / m2m100 / CloudLlmEngine 三者对上层完全同构，`TranslateStrategy` 三分支（LocalModel / CloudModel / FallbackLlm）保证「无激活翻译模型也能用润色 LLM 兜底」的可用性（`translate.rs:30-56,110-123`）。这是竞品里独有的设计——Pot/kiss-translator 的多引擎是并列选择，Octopus 是带兜底的级联。

2. **本地离线 Opus-MT/m2m100 + ASR 转写翻译联动**——opus-mt ~30M/方向极致轻量、m2m100 100+ 语言单模型，两者均 ONNX int8 + 平台 EP 加速（CoreML/CUDA/DirectML，`Cargo.toml` target cfg）。与 octopus-asr-local 的转写结果可直接喂给 `do_translate` 形成「语音→转写→翻译」链路（`do_translate` 注释明确提到 coordinator 终翻路径复用，`translate.rs:69-74`）。竞品里只有 esearch/Paster 有离线翻译，但都是 Argos Translate（质量低于 Opus-MT）。

3. **Opus-MT greedy 防重复工程化**——`apply_penalties` 纯函数 + 6 单测守护 off-by-one 边界、`normalize_cjk_spaces` 5 单测覆盖 CJK/Latin/混合/首尾场景（`opus_mt.rs:369-465`）。这种「把训练-推理分布偏差（带空格中文致 EOS 截断）做成可测预处理」的工程深度，在开源翻译工具里罕见。

4. **vault 加密的 API key 管理**——`v1:` 前缀加密 + 透明解密 chokepoint + 未解锁即 Err 不泄密（`vault_secret_access.rs:1-30`），安全性高于竞品的本地明文/浏览器 storage。

### 4.4 Octopus 不足 / 缺失

1. **无 glossary / 术语表 / 翻译记忆**——全仓零术语持久化。kiss-translator 有「自定义 AI 术语词典 + 上下文记忆」，Pot 有生词本导出 Anki/欧路。Octopus 的 `is_thinking` 自动关闭和 CopyTranslator 风格 prompt 已是 LLM 翻译的工程化，但缺术语一致性保证（专业领域翻译痛点）。

2. **无双语对照 UI 的网页级段落对照**——CompactEditor 的 `mode="contrast"` 是左右双栏（`compact_editor_commands.rs:28`），不是 kiss-translator 那种「原文段落与译文段落交错嵌入」的沉浸式对照。对阅读长文场景体验弱于 kiss-translator / FluentRead / Read Frog。

3. ~~**无 OS 级划词翻译 / 屏幕翻译 / 截图翻译**~~ **截图翻译已实现（2026-08-11）**：截图翻译（`translate_screenshot`）+ ActionBar 选中翻译接入 `translate_window` 浮窗。**仍缺**：OS 级划词翻译（监听系统选区，Pot 做法）、屏幕翻译（贴图窗内文字替换为译文，eSearch 做法）。

4. **云端翻译方向硬编码中英互译**——`detect_translate_direction` 仅按 CJK 二分 zh/en（`translate.rs:58-67`），m2m100 支持 100+ 语言但上层不暴露。即使 m2m100 已下载，用户也无法选日语/韩语等其他方向。

5. **无多引擎并行翻译对比**——Pot 的「多接口并行翻译结果并列展示」、kiss-translator 的「划词弹窗多服务对比」在 Octopus 缺失，translate 是单引擎串行。

### 4.5 建议改进方向

1. **加 glossary / 术语表**（P0）——复用现有 DB schema 扩展能力（`models`/`prompts`/`fuzzy_dialect_rules` 等表已证明 schema 演进机制成熟）。建 `translate_glossary` 表（source_term / target_term / context / domain），CloudLlmEngine 的 prompt 注入术语表（参考 kiss-translator 的 systemPrompt 占位符 `{{glossary}}`），本地引擎可在 decode 后做术语替换后处理。

2. ~~**OCR→translate pipeline 拼接**（P0）~~ **✅ 已完成（2026-08-11）**：截图工具栏「翻译」按钮（`translate_screenshot` 命令）+ ActionBar 选中翻译（`auto_translate` 分支）均接入 `translate_window` 浮窗，复用 `do_translate_streaming` 流式 emit。详见 [spec](../superpowers/specs/archived/2026-08-11-screenshot-translate-float-window-design.md)。

3. **暴露多语言选择 + 接入 NLLB-200**（P1）——`detect_translate_direction` 扩展为 UI 可选语言列表，m2m100 已支持 100+ 语言只需上层放开。可考虑接 NLLB-200-distilled-600M（比 m2m100 质量更高、200 语言，社区 2026 仍推荐用于低资源语言）作为第三本地引擎选项，与 Opus-MT（少语言高质量）/ m2m100（多语言兜底）形成梯度。

4. **沉浸式双语对照**（P1）——CompactEditor contrast 模式扩展为段落级交错对照（原文段 / 译文段交替渲染），参考 CopyTranslator / FluentRead。对长文阅读场景价值大。

5. **多引擎并行对比**（P2）——`translate_text` 支持 `engines: Vec<String>`，并行 spawn 多个 `do_translate`，emit 时带 engine_name，前端并列展示。对标 Pot 的多接口并行。

6. **划词翻译 / 屏幕翻译**（P2）——OS 级全局快捷键（已有 hotkey 基建）+ 选区获取（macOS Accessibility / Windows UIAutomation / X11 selection）+ 弹窗，与 ActionBar 现有 UI 复用。

7. **翻译缓存持久化**（P2）——当前仅内存 session 缓存（`TRANSLATE_RESULTS` 上限 64）。可落 DB（复用 `clipboard_history` 的 FTS5 基建）做翻译历史 + 命中缓存省云端调用，对标 kiss-translator 的翻译缓存 + history。

**Sources（web 研究）**:
- [Picovoice – Open-Source Translation Models 2025](https://picovoice.ai/blog/open-source-translation/)（Opus-MT 按方向优化质量最佳）
- [Medium – Pretrained Models for NMT](https://medium.com/@kalyanks/pretrained-language-models-for-neural-machine-translation-b2cdd2b22e78)（Opus-MT 最轻量、NLLB-200 词表最大 256.2K）
- [Hacker News – NLLB200 vs M2M100 vs Opus MT](https://hn.algolia.com/?query=Deep%20Learning%20Translation%3A%20NLLB%20200%20vs.%20M2M100%20vs.%20Opus%20MT&type=story)（m2m100 多语言但质量低于 Opus-MT）
- [arXiv 2403.03923](https://arxiv.org/html/2403.03923v2)（NLLB/TI 鲁棒性优于 Opus）
- [Meta NLLB-200](https://ai.meta.com/blog/nllb-200-high-quality-machine-translation/)（200 语言单模型 SOTA）

---

## 5. Action Bar 模块

### 5.1 Octopus 现状

octopus 的 Action Bar（后端代码里也叫「命令面板」「AI 命令面板」）是一个**由选中文本/文件 + 全局热键触发的搜索驱动型命令浮窗**，位置是悬浮在 macOS 桌面之上、鼠标上方弹出的单例 480×76 透明无边框窗口（`crates/desktop/src/action_bar/action_bar_window.rs:8,22`）。后端共 9 个 Rust 文件、约 3621 行，组织在 `crates/desktop/src/action_bar/` 下，按职责拆成 `action_bar_window.rs`（浮窗 show/hide）+ `action_hotkey.rs`（Quick Execute 全局快捷键）+ `agent_adapter.rs`（CLI agent 适配器）+ `terminal_launcher.rs`（Terminal.app fallback）+ `action_bar_commands/` 子目录（7 个子模块，详见 `2026-07-29-action-bar-split.md`）。

**触发机制（核心特征）**

- 主入口是 `trigger_action_bar`（`window.rs:18`），由全局快捷键 `⌘⇧Space` 唤起（`action_bar_window.rs:172` 的 `register_action_bar_shortcut`，toggle 语义：可见时再按则隐藏）。
- 触发后 `detect_selection`（`context.rs:131`）一次性采集「选中 + 鼠标坐标」，然后按 `Selection` enum 四分支路由（`window.rs:44-77`）：
  - **Text**：先 `gather_context`（App 名/bundleId/窗口标题/前后文）写 `PENDING_CONTEXT`，再 `show_action_bar_at_mouse_with_pos`（鼠标上方 −240/−42 偏移，含碰撞检测防溢出屏边）。
  - **File/Folder**（Finder 选中）：AppleScript 拿 `selection` POSIX 路径（`finder_selection.rs`），同样鼠标上方弹出。
  - **None**：清空 `PENDING_CONTEXT`，主屏水平居中 + 垂直 1/5 处弹出（`show_action_bar_centered`，仿 Alfred/Wox）。
- 选中检测有 3 条路径（`context.rs:131-282`）：Finder AppleScript、Sublime 插件直读 `sel_start/sel_end`（绕过 Sublime 4 的 `copy_with_empty_selection` 陷阱）、其余通过模拟 `Cmd+C` + 轮询 `NSPasteboard.changeCount` 判断是否真有选中。changeCount 按 dispatch 路径动态超时（CGEvent 80ms / Osascript 300ms，针对微信 WKWebView 异步写入调优，`context.rs:60,221`）。剪贴板检测后立即恢复原值（防 `Cmd+C` 污染），并用 `CHANGE_COUNT_BASELINE` 隔离恢复写入自身递增的污染（`context.rs:53,269`）。
- **第二个入口**：每个菜单项可配 `global_shortcut`（Quick Execute，`action_hotkey.rs:33`），按热键绕过浮窗直接执行 → 结果展示在 CompactEditor（不粘贴替换）。文件选中 + agent + `need_voice` 时分支走 `trigger_agent_voice_core(hide_action_bar=false)`（`action_hotkey.rs:222`）。
- **第三个入口**：斜杠命令 `/cmd [params]`（输入框打头 `/`，前端 normalize 顿号 `、`→`/`，`architecture.md:381`），由 `search_slash_commands` fuzzy 匹配 trigger_keyword。

**动作清单（6 种 `action_type` + 搜索 + slash）**

DB `action_bar_items` 表存菜单项，自引用 `parent_id` 两级菜单，6 种 `action_type`（`architecture.md:353,712`）：

1. `ai`：LLM 调用。`action_data == "auto_translate"` 走翻译（本地 opus-mt/m2m100、云端 OpenAI 兼容、`FallbackLlm` 三策略，`translate.rs:30-56`）；其他 action_data 当 prompt，支持 `@文件名` 引用 `~/.octopus/.sync/prompts/command/<name>.md`（`prompt_files.rs:32`）。结果在 CompactEditor 临时 tab 展示，翻译走 contrast 模式（`context.rs:327`）。AI 操作 10s 后端超时（`script.rs:341,443`）。
2. `script`：第一行 magic comment 分发——`#shell`/`#osascript`/`#powershell`/`#python`/`#node`/`#deno`/`#bun`/`#javascript`/`#typescript`（`script.rs:100-161`）。JS/TS 自动探测运行时优先级（node→bun→deno / bun→deno→npx tsx）。选中文本经环境变量 `OCTOPUS_TEXT` 注入，>200KB 写临时文件 + `_____ULTRA_LONG_TEXT_____:/path` marker（`script.rs:166-203`）。Package 脚本（绝对路径）额外注入 `OCTOPUS_PACKAGE_DIR`。同步 60s 超时、异步无超时。执行记录入 `script_runs` 表（可列表/清理/批量删）。
3. `url`：模板替换 `{query}`/`{text}` + `url_encode_param` 全编码。空 action_data（选中文本即 URL）仅放行 http/https，其余补 `https://`（`script.rs:453-471`），防 smb/file/vnc 触发系统操作。
4. `agent`：**最独特的类型**——渲染 prompt（`{{voice}}`/`{{text}}`/`{{files}}` 占位符，`prompt_files.rs:18`）→ 经 `agent_adapter::resolve_effective_adapter`（菜单指定 → 系统默认 → 第一个可用三层 fallback）→ 启动 CLI agent（claude/codex/gemini/pi）到**内嵌终端窗口**，失败 fallback Terminal.app（`script.rs:521-559`）。`need_voice=true`（agent 且 prompt 含 `{{voice}}`，`items.rs:20`）时走 `trigger_agent_voice_core` 触发音录，ASR 结果填入占位符。
5. `copy_path`：路径格式化 plain/url/quoted 写剪贴板（`prompt_files.rs:159`）。
6. `submenu`：父菜单，承载子项。

附加搜索能力：独立 crate `octopus-search` 提供 7 个 Tab（全部/应用/文件/书签/动作/命令/斜杠），nucleo-matcher 四级 fuzzy（exact > prefix > pinyin > fuzzy），应用索引带本地化别名 + base64 icon + bundle_id，命令索引扫 PATH + LLM 补关键字（`architecture.md:381,385`）。

**Agent LLM 命令的 Tab Bug**

「ActionBar agent 命令建不出 tab」的根因在 `AGENTS.md:445`：跨窗口 `listen()` 的回调引用不稳定（`useCallback(fn, [tabs, activeId])` 每次 setTabs 都变），导致 effect cleanup/re-register 间隙 listener 处于未注册态，后端 `emit_to` 撞上间隙即丢。修复模式是用 ref 持有最新回调、effect `[]` 依赖挂一次 + `target: { kind: "WebviewWindow", label }` 精确匹配。

**CompactEditor / Sessions / Tabs 联动**

action bar 的执行结果（翻译/润色/摘要/解释/脚本 stdout）经 `action_bar_show_result_internal`（`context.rs:291`）打开 CompactEditor 临时 tab，按 action 映射 label（翻译/润色/摘要/解释）。翻译走 contrast 模式 + 流式分段翻译 emit `compact-editor://translate-progress|done` 携 sessionId（`translate.rs:144-176`），后端缓存 done 终态兜底 Tauri 事件丢失（`TRANSLATE_RESULTS` HashMap 上限 64，`translate.rs:211`）。前端 mount 后调 `get_translate_result(sessionId)` 主动拉取。

**跨模块联动（强项）**

- **translate**：opus-mt 本地 / OpenAI 兼容云端 / 润色 LLM 兜底三策略；选 CJK 检测方向。
- **OCR / ASR**：agent + `need_voice` 联动 ASR 录音，结果填入 `{{voice}}`；窗口上下文经 `gather_context`（Pages osascript/lsof/pdftotext/officecli/mdfind/subl fallback 链，统一 500ms 超时，`architecture.md:381`）。
- **clipboard**：`octopus-clipboard` 模块共用（read_text/write_text/suppress_next/set_image/write_files）。
- **terminal**：内嵌终端（`ui::terminal_window::open_terminal_with_command`）+ Terminal.app fallback。
- **vault**：云端翻译/ASR 的 secret_key 走 `vault_secret_access::try_decrypt_secret_global` 透明解密（`translate.rs:100`）。
- **sync**：`~/.octopus/.sync/prompts/` 同步的 prompt 文件被 action bar 的 `@引用` 消费。

**悬浮窗机制 / 焦点协调（macOS 工程难点）**

- 单例 `action_bar_window`，应用启动时建（`visible=false`），show/hide toggle。
- macOS 用 `before_floating_window_show(app, false)`（不隐藏终端）+ `activate_self_no_raise`（只 `NSApplication::activate()`，不抬其他窗口）+ `NSWindow makeKeyAndOrderFront` 强制拿 key（`action_bar_window.rs:39-77`）。
- 150ms/350ms 双时点 `check_and_consolidate_focus` 巩固焦点（防 Sublime `subl --command` 延迟抢焦）；已 dismiss 的窗口跳过（防对隐藏窗口夺焦把用户带回 octopus，`action_bar_window.rs:132-158`）。
- 已知跨屏限制：`activate()` 会把 app 抬到副屏前台（TODO，`action_bar_window.rs:48-55`）；根治方案是 tauri-nspanel NSPanel（调研 §3）。
- `AGENTS.md:443`：click-through poller 的 `BAR_W` 必须与前端容器同宽，曾因 520→720 不同步致按钮点不到（注意：这是 `result_window.rs` 的 gotcha，action_bar_window 本身不是 click-through poller 模型）。

**自定义（DB 驱动 + app 感知）**

- ActionBarItem 全字段 DB 化：title/icon/action_type/action_data/is_async/write_output_to_clipboard/agent/accepts（text/file/any）/trigger_keyword/is_enabled/need_voice/app_bundle_ids/global_shortcut。CRUD 命令在 `items.rs`，同级上限 35 项（Alt+1-9 + a-z 定位符）。
- **per-app 绑定**（v49）：`app_bundle_ids` JSON 数组，全局项 + 当前 app 专属项叠加显示，与 `accepts` 维度独立 AND（`architecture.md:361`）。多选器 UI `AppPicker.tsx` 调 `list_all_apps`。
- **prompt 外部引用**（v26）：`@文件名` → `~/.octopus/.sync/prompts/command/<name>.md`，设置页 PromptEditor 提供「内联/引用文件」切换 + hover 预览 + 「查看更多」调 CompactEditor 全文编辑。
- **Extension Package**（2026-07-10）：`~/.octopus/extensions/` 文件夹 + `config.yaml`，导入后存 DB（`action_data` 存脚本绝对路径，`action_type=script`），`spawn_script` 检测绝对路径前缀（`architecture.md:357`）。
- **Agent 适配器**（v42 起 DB 驱动）：Pi/Claude 由 `db.sql` seed，用户可自定义 adapter（detect_binary + command_template + `{prompt}`/`{files}`/`{files_at}`/`{cwd}` 占位符）。

### 5.2 类似工具对比

| 维度 | **Octopus Action Bar** | **PopClip**（macOS） | **SnipDo**（Windows） | **eSearch**（跨平台） | **Raycast**（macOS/Win/iOS） | **Manggo / Pot**（跨平台） |
|---|---|---|---|---|---|---|
| 触发 | 全局热键 `⌘⇧Space` + Quick Execute 热键 + 斜杠命令 | 选中文本自动弹出 | 选中文本自动弹出 | `Alt+C` 框选截屏 | 全局热键唤出 launcher | 划词/截图热键 |
| 选中检测 | Cmd+C+changeCount / Finder AppleScript / Sublime 插件 / AX 上下文 | macOS 原生选中事件 | Windows 选中事件 | 框选 + OCR | 无（启动器模型） | 划词 + OCR |
| 动作类型 | ai/url/script/agent/copy_path/submenu + 7 Tab 搜索 | URL/Key Press/Service/Shortcut/Shell/AppleScript/JS-TS（7 种） | 80+ 内置动作（搜索/拼写/翻译/字典） | OCR/搜索/翻译/贴图/以图搜图/二维码/录屏/屏幕翻译 | 应用启动/文件搜索/AI Commands/Snippets/Quicklinks/计算器/扩展 | 划词翻译/截图 OCR/翻译替换/输入框转译 |
| AI / Agent | **agent 类型启动 CLI agent（claude/codex/pi）+ need_voice 联动 ASR**；ai 类型走 LLM；翻译三策略 | 第三方扩展（ChatGPT/Claude/Ollama/Grok） | 第三方/社区扩展 | 多引擎翻译（Google/DeepL/百度/ChatGPT） | **Pro AI Chat + AI Commands**（OpenAI/Anthropic/Perplexity 等） | 多服务商翻译（小牛/百度/腾讯） |
| 自定义 | DB 全字段 + per-app 绑定 + Extension Package + `@prompt` 引用 | Snippet YAML + `.popclipext` Package + 数字签名 | pack 自定义 | 配置文件 | React+TS 扩展 SDK + Store | 服务商配置 |
| 跨模块联动 | **translate/OCR/ASR/clipboard/terminal/vault 全栈聚合** | 经扩展间接 | 弱 | 截图+OCR+翻译一体 | 启动器生态（剪贴板/书签/窗口管理） | 翻译+OCR 一体 |
| 结果展示 | CompactEditor 多 tab + sessions + 流式翻译 | 直接粘贴/替换/打开 | 弹窗/粘贴 | 主窗口编辑 + 贴图 | launcher Detail/Preview | 浮层/替换 |
| 平台 | macOS only（大量 objc2 AppKit 调用） | macOS only（≥11） | Windows only | Win/Linux/macOS | macOS（Win 开发中/iOS app） | Win/macOS/Linux |
| License | 闭源（项目内） | 商业买断 | 免费（MS Store） | GPL-3.0 | 免费 + Pro $8/月 + Team $12 | 闭源商业（Pro 付费） |

PopClip 2026.7（2026-10-27 发）新增 iCloud Sync（扩展/动作布局/设置跨 Mac 同步）+ 扩展自动更新 + 18 语言 UI（[popclip.app](https://www.popclip.app/)、[Reddit r/macapps](https://www.reddit.com/r/macapps/comments/1v54yeb/popclip_20267_released/)）。SnipDo 在 Microsoft Store 免费、80+ 动作（[snipdo-app.com](https://snipdo-app.com/)）。Raycast Pro $8/月含 AI，Team $12/用户/月（[raycast.com/pricing](https://www.raycast.com/pricing)），AI Command Bar 跨 Mac/Win/iOS（[raycast.com/core-features/ai](https://www.raycast.com/core-features/ai)）。

### 5.3 Octopus 独特价值

1. **agent 类型 + CLI 启动是 PopClip/SnipDo 都没有的能力**——PopClip 的 Shell/AppleScript 动作只能跑短脚本，octopus 的 agent 动作能把选中文件 + 用户口述指令（`{{voice}}`）渲染成 prompt，在内嵌终端启动 claude/codex/gemini/pi 做长任务（如 `make-ppt.prompt.md` 制作 PPT），定位是「桥接器」不碰文件系统。
2. **OCR / ASR / translate / clipboard / terminal / vault 一体聚合**——竞品要么专注文本操作（PopClip/SnipDo）、要么专注截图 OCR（eSearch）、要么专注翻译（Manggo）、要么是通用启动器（Raycast）。octopus 把这些能力作为 action bar 的下游模块统一调度，单一浮窗即可触发任意组合（截图→OCR→翻译→CompactEditor；选中文本→润色 LLM；Finder 选中→agent 启动）。
3. **上下文增强（gather_context）**——`build_enriched_text`（`context.rs:484`）把 App 名/类型/窗口标题/前后文拼成 LLM 情境块，比 PopClip 的纯选中文本输入质量更高（对齐 VoxFlow/Moly 的 AX 上下文增强方向）。
4. **CompactEditor tab 会话 + 流式翻译**——结果不是简单粘贴替换，而是多 tab 持久会话，翻译分段流式 emit + sessionId 路由 + done 缓存兜底事件丢失，比竞品的「一次弹窗」模型更适合长结果/对照阅读。
5. **斜杠命令 + 搜索驱动**——从纯菜单条升级为命令面板（仿 Wox/Raycast），fuzzy + pinyin + 应用/文件/书签/命令/动作多 source，单一入口覆盖「搜索应用 + 执行菜单 + 跑命令」。
6. **per-app 绑定**——`app_bundle_ids` 让菜单按前台 app 动态过滤（Xcode 看 Code Review、Safari 看 Summarize），比 PopClip 的 include/exclude 更精细（PopClip 是 App 级开关，octopus 是菜单项级绑定）。

### 5.4 Octopus 不足 / 缺失

1. **仅 macOS**——`trigger_action_bar` 直接 `#[cfg(not(target_os = "macos"))]` return（`window.rs:19-24`），Cmd+C+changeCount / Finder AppleScript / NSPasteboard / objc2 AppKit 全是 macOS 专属。Windows/Linux 用户无法使用。
2. **无扩展生态/插件市场**——Extension Package 是本地文件夹导入，没有 PopClip 的 370+ 官方目录 + 数字签名、Raycast 的 Store + Team 共享、Wox 的插件商店。社区扩展生态空白。
3. **JS-TS magic comment 已支持但无社区分发**——`#node`/`#deno`/`#bun`/`#javascript`/`#typescript` 都有（`script.rs:127-156`），但缺 PopClip Snippet YAML 那种「一段文本即可安装」的低门槛分发。
4. **文档主要在 architecture.md/specs/plans，无面向最终用户的扩展开发指南**——KoBar 的 `for-agents/SKILL.md`、PopClip 的 Developer Reference 这类「教第三方写扩展」的文档缺失。
5. **跨屏焦点抬窗口问题未根治**（`action_bar_window.rs:48-55` TODO）——副屏唤出时会把 app 抬到副屏最前；tauri-nspanel NSPanel 方案是已知根治路径但未实施。
6. **可视化编排缺失**——Alfred Workflow 画布、Raycast 的多步 Command 这类零代码编排能力没有；多步骤场景只能靠用户手写 script/agent prompt。
7. **外观自定义弱**——固定 480px 宽 / 90% 透明 / backdrop-blur-2xl（`architecture.md:381`），无 PopClip 的 vibrancy 主题编辑、Raycast 数百主题。
8. **MCP 集成缺失**——Wox 已有 STDIO + StreamableHTTP 双端 MCP，octopus 的 script 动作尚未演化为 MCP tool 调用（调研 §10.7 已识别为 P2 方向）。

### 5.5 建议改进方向

按价值/成本排序：

**P0（与现有二期计划对位，最高价值）**

1. **跨平台（Windows/Linux）**——detect_selection 的 Cmd+C+changeCount 在 Windows 可改用 `WM_CLIPBOARDUPDATE` + UI Automation；Finder AppleScript 改 Windows Explorer COM。这是扩大用户基数的关键，但工作量极大（整个 objc2 AppKit 焦点协调层需重写）。建议先评估 Win 需求再投入。
2. **AX 直读选中文本**（调研 §12 P0）——用 Accessibility API 直读 `AXSelectedText`，绕过 Cmd+C 污染剪贴板 + 解决微信 WKWebView 异步写入的 300ms 超时问题。需处理 Electron/Chrome `--force-renderer-accessibility` 桥接。
3. **扩展市场 / 注册中心**（调研 §5.3 KoBar 模式）——`kobar.json` 清单 + `registry.json` GitHub Actions 聚合，社区在 GitHub 仓库放清单即可被发现。低成本启动生态。
4. **Snippet YAML 分发格式**（调研 §11 PopClip 模式）——一段 YAML 文本即可安装的动作，降低第三方贡献门槛。octopus 已有 Extension Package 文件夹格式，加 Snippet 单文件格式作为轻量补充。

**P1（差异化增强）**

5. **一键直达热键扩展到所有动作类型**（调研 §1.1 VoxFlow）——当前 Quick Execute 是每菜单项配 `global_shortcut`，可借鉴 VoxFlow 的 `⌘⇧J=翻译`/`⌘⇧K=总结` 语义化热键（高频动作绕过菜单）。
6. **密码框检测 + 采集超时安全边界**（调研 §1.5 VoxFlow PRIVACY.md）——上下文增强在密码管理器/银行 App 中应自动禁用，目前只有 per-app 绑定但无自动检测。
7. **截图 OCR fallback 串联**（调研 §2 eSearch）——无选中文本时一键截图→OCR→喂给 action bar，突破「必须先选中」限制（适合 PDF/视频字幕/禁复制页面）。
8. **多引擎翻译可选**（调研 §2.4 eSearch）——当前固定 LLM/本地/云端三策略，可加 DeepL/Google 作为用户可选备选。
9. **智能解析本地动作**（调研 §8.7 Paster）——选中 `#ff5500` 显示颜色、`2+2*3` 显示 8、`2026-07-09` 显示距今天数，不需 LLM 调用的轻量 action_type。

**P2（工程优化）**

10. **tauri-nspanel NSPanel 方案**（调研 §3）——根治焦点抢夺 + `canJoinAllSpaces` 跨 Space/全屏。三期或焦点问题再现时实施。
11. **MCP 集成**（调研 §10.7 Wox）——script 动作演化为 MCP tool 调用，对接任意 MCP server，扩展能力边界。
12. **沙箱插件运行时**（调研 §5.4 KoBar）——HTML/JS 类自定义动作的安全前提（当前 script 类已用环境变量隔离，UI 类需 iframe 沙箱）。
13. **可视化编排**（调研 §10.4 Alfred Workflow）——多步骤动作的拖拽式编辑器，工作量大建议远期。

**P3（生态/合规）**

14. **数字签名 + 未签名警告**（调研 §11.4 PopClip）——开放社区扩展生态时的安全策略。
15. **GPL 风险隔离**——VoxFlow/eSearch 都是 GPL-3.0，只学架构思想不抄代码（调研 §13 警示）。

**关键文件清单**（绝对路径）

- 后端入口：`/Users/wudarui/workspace/agent/octopus/crates/desktop/src/action_bar/mod.rs`
- 浮窗 show/hide + 焦点协调：`/Users/wudarui/workspace/agent/octopus/crates/desktop/src/action_bar/action_bar_window.rs`
- 触发 + 路由 + 检测：`/Users/wudarui/workspace/agent/octopus/crates/desktop/src/action_bar/action_bar_commands/window.rs` + `context.rs`
- 6 种动作执行核心：`/Users/wudarui/workspace/agent/octopus/crates/desktop/src/action_bar/action_bar_commands/script.rs`（`execute_action_bar_inner` 在 `script.rs:347`）
- 翻译策略 + 流式：`/Users/wudarui/workspace/agent/octopus/crates/desktop/src/action_bar/action_bar_commands/translate.rs`
- Agent 适配器 + 三层 fallback：`/Users/wudarui/workspace/agent/octopus/crates/desktop/src/action_bar/agent_adapter.rs`
- 菜单项 CRUD + need_voice 自动判定：`/Users/wudarui/workspace/agent/octopus/crates/desktop/src/action_bar/action_bar_commands/items.rs`
- Quick Execute 全局热键：`/Users/wudarui/workspace/agent/octopus/crates/desktop/src/action_bar/action_hotkey.rs`
- 前端浮窗：`/Users/wudarui/workspace/agent/octopus/crates/desktop/frontend/src/pages/ActionBar/index.tsx`
- 架构说明：`/Users/wudarui/workspace/agent/octopus/docs/architecture.md`（§353-385 Action Bar 段）
- 已有调研：`/Users/wudarui/workspace/agent/octopus/docs/superpowers/specs/research/2026-07-09-action-bar-related-tools-survey.md`
- Gotchas：`/Users/wudarui/workspace/agent/octopus/AGENTS.md:411-447`（坐标转换/焦点/click-through/listener 稳定化/osascript 编码）

---

## 6. Terminal 终端模块

### 6.1 Octopus 现状

Octopus 的终端是内嵌于桌面 app（Tauri 2 + WKWebView）的一个模块，与 ASR/OCR/Clipboard/ActionBar 同进程共存。后端 `crates/pty/`（crate 名 `octopus-pty`）+ 前端 `crates/desktop/frontend/src/pages/Terminal/`。

**PTY 后端（portable-pty，无 fork）**
- `crates/pty/Cargo.toml:9` 直接用 `portable-pty = "0.9"`，**不是 fork**（与 Codux/OxideTerm 同源依赖）。代码注释自称「参考 Terax pty 模块设计」（`crates/pty/src/lib.rs:2`），但 octopus-pty 是独立实现。
- macOS-only：去掉 Windows ConPTY/Job、WSL（`session.rs:8`）。注意 octopus **整体定位是 ASR 工具集**（见 `docs/architecture.md:3`），终端只是「让 ASR 转写文本能直接进 shell」的伴随能力，不像 Codux/OxideTerm 是终端 first 产品。
- **3 线程模型**（`session.rs:11-17`）：reader（阻塞读 master → OSC 解析 → push pending buffer）+ flusher（Condvar 等 pending → 4ms coalesce → `on_data(chunk)`，降低 IPC 压力）+ waiter（`child.wait()` → 等 reader join → flush tail → `on_exit(code)`）。
- **分层约束**：pty crate「纯逻辑无 tauri」，`on_data/on_exit/on_signal` 是 `Send + 'static` 闭包，desktop 层桥接到 Tauri Channel/emit（`session.rs:4-7`）。Cargo.toml 只 5 个依赖（portable-pty/log/anyhow/parking_lot/serde/dirs/libc），刻意保持极简。

**OSC agent 状态感知（核心差异化）**
`crates/pty/src/agent_detect.rs` 是一个 OSC 序列解析状态机，从 PTY 原始字节流提取 3 类序列推断 agent CLI（claude/codex/gemini/pi/opencode/grok）状态：
- **OSC 133;C;\<cmd>**（prompt tracker 标记）：匹配已知 agent 命令名 → `Started{agent}`。支持路径前缀（`/usr/local/bin/claude`）、npx 包装（`npx claude`）、连字符后缀（`claude-enigma`）、引号剥离（`'claude'`/`"claude"`，`agent_detect.rs:307-336`）。
- **OSC 777;notify;octopus;\<agent>;\<event>**（自定义 hook marker）：3-field（Claude 默认）+ 4-field（Codex/Gemini/Pi 命名）格式。`working`/`attention`/`finished` 事件驱动 UI 徽章。bash 无 preexec，靠 OSC 777 自我 arm（`agent_detect.rs:239-255`，auto-arm）。
- **OSC 9**（generic desktop notify）：armed 时映射为 `Attention`，但排除 `OSC 9;4`（taskbar 进度，`agent_detect.rs:217`）。
- 安全：`OSC_MAX=2048` 防 TUI 恶意 payload 撑爆缓冲；`status` 字段防 `Working` 重复 emit；PTY 关闭时 `finish()` 发 `Exited` 清 stale UI（`agent_detect.rs:200-205`）。
- **闭环**：`crates/desktop/src/commands/agent_hooks.rs` 把 OSC 777 marker 注入 Claude（`.claude/settings.json`，用 `terminalSequence` JSON 字段，因 Claude v2.1.139+ 丢了 `/dev/tty`）/ Codex/Gemini/Pi（用 `/dev/tty` 直接 emit）的 hook 配置。`write_atomic`（tmp+rename）+ `OWNED_MARKERS` prune 保证幂等，不破坏用户已有 hook。

**前端渲染（xterm.js + WebGL）**
- `crates/desktop/frontend/src/pages/Terminal/useTerminalSession.ts:22-26`：`@xterm/xterm` + FitAddon + WebLinksAddon + WebglAddon + SearchAddon。**不是自研渲染器**，与 wterm（DOM 渲染）/ Alacritty（OpenGL）/ Ghostty（Metal+shader）路线完全不同。
- WebGL renderer 默认启用，GPU 不可用自动降级 Canvas（`useTerminalSession.ts:87-125`）。context loss（sleep/wake/GPU reset）250ms 后重 attach（值来自 Terax 实测，`WEBGL_RECOVERY_DELAY_MS`）。
- **冷启动光标闪烁修复**（细节深度）：WebglAddon 的 `CursorBlinkStateManager` 构造时若 textarea 未聚焦（`isFocused=false`）不启动 600ms blink 定时器 → 光标「可见但不闪」。用 `requestAnimationFrame` 延迟一帧 `term.focus()` 等 WKWebView 真正可见后再聚焦（`useTerminalSession.ts:243-264`）。同类修复还覆盖 tab 切换（`applyActive`，`useTerminalSession.ts:145-167`）和窗口 focus/visibilitychange（`useTerminalSession.ts:418-427`）。

**多 Tab / 布局（无分屏 pane）**
- `crates/desktop/frontend/src/pages/Terminal/index.tsx`：多 tab 数组，每 tab 一个 TerminalPane（独立 xterm + PTY session）。tab 切换用 `visibility:hidden` 保活（不卸载 xterm，切回 scrollback 保留，`index.tsx:313`）。
- 隐藏 tab 释放 WebGL context（防 WKWebView ~16 上限，`useTerminalSession.ts:457-462`）。
- **两种布局**：顶部 tabs ↔ 左侧 sidebar list（`localStorage` 持久化，`index.tsx:54-58`）。**无分屏（split panes）**——简化版相对 Terax 的 rendererPool + dormantRing + pane 树（`useTerminalSession.ts:3-5`）。
- tab 改名（双击内联编辑）、右键菜单（新建/改名/关闭）、agent 状态徽章（working amber 脉冲 / attention 红色 bell / finished 淡出，`index.tsx:660-676`）。

**Shell 集成（OSC 133 + OSC 7）**
- `crates/pty/src/shell_init.rs`：zsh 用 ZDOTDIR 方案（写到 `~/.cache/octopus/shell-integration/zsh/`，内部 source 用户原 ZDOTDIR，starship/p10k 照常工作）；bash 用 `--rcfile` + `-i`。fish/sh 不注入（`shell_init.rs:212-217`）。
- `crates/pty/src/scripts/zshrc.zsh`：precmd hook 发 `OSC 133;D;<exit>` + `OSC 7;file://host/<urlencoded_pwd>` + `OSC 133;A`；preexec 发 `OSC 133;C;<cmd>`。byte-wise urlencode 保证多字节路径安全。
- **OSC 7 安全过滤**（`osc-handlers.ts:38-82`）：命令执行期间（OSC 133 B→C→D/A）忽略 OSC 7——防 SSH/恶意文件 `cat`/`echo` 伪造 cwd。只有 shell precmd（命令间）发的 OSC 7 才可信。
- **OSC 7 用途**：新 tab 继承当前 cwd（`index.tsx:159-165`）+ 标题显示 basename（优先级 customName > cwdBasename > agentName > 默认，`index.tsx:300-304`）。
- **prompt 检测精度**：OSC 133 A/B/C/D 完整，但仅 zsh；bash 用 `PROMPT_COMMAND` + `PS0`，B/C 标记可能不如 zsh 精确（spec `2026-07-31-terminal-osc7-cwd-design.md:135-137` 承认）。
- 关键修复：`getpwuid_r` 替代非可重入 `getpwuid`（防两 PTY 并发 spawn 竞争静态缓冲区，`shell_init.rs:66-117` + 回归测试 `login_shell_concurrent_safe` 10 线程并发断言）。

**文件拖拽（双入口分治，绕开 WKWebView 限制）**
- 决策：HTML5 Drag and Drop（`draggable`/`dataTransfer`）在 WKWebView + xterm canvas 下**完全不可靠**——`onDrop`/`onDropCapture` 都不触发（spec `2026-08-01-terminal-drag-file-to-terminal-design.md:30-36`）。这是 AGENTS.md 记录的 gotcha。
- 双入口：① **Finder OS 拖入**用 Tauri 原生 `onDragDropEvent`（OS 层事件，绕开 HTML5 DnD）；② **文件树内部拖拽**用 pointer events 模拟（document mouseup + containerRef hit-test，`TerminalPane.tsx:177-196`）。
- `relPath`（纯函数）：子树内相对路径，外部回退绝对。`shellEscape`：含非安全字符单引号包裹 + POSIX 转义 `'\'\''`。
- 写入用 `session.paste`（bracketed paste mode）而非 `session.write`——终端跑 Claude Code 等开启 bracketed paste 的程序时正确识别为一次完整输入（spec `:71-73`）。

**paste-text listener 模式（稳定回调 ref）**
- `TerminalPane.tsx:116-173`：ASR 文本回写场景，后端检测前台是 terminal webview 时 emit `paste-text` 定向到本窗口，仅活跃 pane 直写 PTY（绕过 xterm/键盘模拟，最可靠）。
- **AGENTS.md gotcha**：`session` 对象每次渲染都是新引用（`useTerminalSession` 返回字面量），不能放 effect deps——否则每渲染都 unlisten/listen，间隙丢事件。用 `sessionRef` 持有最新，effect 只挂一次（`TerminalPane.tsx:150-173`）。同款模式覆盖 `onPtyIdRef`/`onCwdRef`/`onConsumeCommandRef`（`:117-142`）和 index.tsx 的 `consumeFirstTabRef`（`:250-276`，对齐 d5a879ed 修复）。
- 多窗口隔离：`listen(..., { target: { kind: "WebviewWindow", label: currentLabel } })` 精确匹配，比裸 string AnyLabel 投递更可靠。

**waiter join 超时 + reap_exited 泄漏修复**
- spec `2026-08-02-terminal-cloud-text-bugfix.md §1.2`（🔴 严重）：shell 起后台进程（`sleep 100 &`/daemon/powerlevel10k）持 PTY slave fd → shell 退出后 master read 永不 EOF → reader 永不退 → `reader_thread.join()` 无界阻塞 → `on_exit` 永不调 → 前端永远显示「运行中」。
- 修复（`session.rs:57-85`）：`JoinHandle::join` 无 stable `try_join`，改 spawn 计时守护线程 + channel + `recv_timeout(READER_JOIN_TIMEOUT=2s)`。超时则 force-finalize（取 pending tail + on_exit），reader 线程泄漏（阻塞在 read，session Drop 时 master fd 关 → read 返 EOF → reader 退；进程级泄漏，OS 回收，benign）。回归测试 `waiter_finalizes_on_reader_hang_grandchild_holds_fd`（`#[ignore]`，real-pty）。
- **waiter spawn 失败清理**（§1.3）：RLIMIT_NPROC 时 reader/flusher 已 spawn，闭包 drop 但 child 不 reap → 僵尸 + flusher 永旋。`.map_err` 闭包内 `done.store(true)` + `cv.notify_all()` 让 flusher 退 + session Drop kill child（`session.rs:341-358`）。
- **服务端 reaper**（§1.5，`lib.rs:33-57`）：`PtyState::reap_exited()` 周期扫描 `is_exited()==true` 的 session，写锁互斥不与 `pty_close` 竞争。desktop `init_pty` spawn 5s 间隔 daemon 线程调用（**不引入 tokio 到 pty crate**，保持「纯逻辑」约束）。兜底前端崩溃/路由切换不调 `pty_close` 的场景。

**rAF 节流累积（防 shell 回显丢失）**
- `useTerminalSession.ts:281-310`：每帧（~16ms）最多 write 一次。`yes` 这种持续高速输出产生大量 onData 回调，rAF 合并防 xterm write buffer 无限积压。
- **关键修复（2026-08-03）**：旧逻辑「覆盖丢中间块」对高速连续输出（yes）无害，但对 **shell 回显致命**——shell 逐字符/小块回显用户输入，快速输入+回删时多个 onData 同帧到达，旧逻辑只保留最后一块 → 丢失前面回显字符 → xterm 显示和 shell 实际接收不一致（用户看到 `clone` 但 shell 收到 `clonne`）。改为**累积拼接**：同帧内多块拼接到 `pendingChunks`，flush 时一次性 write 全部。

**AI 集成（间接，非终端原生 AI）**
- 终端**本身无 AI**（无自然语言转命令、无 LLM 对话、无 AI 补全）。但通过 OSC agent 状态感知，它能**可视化**用户在终端里跑的 AI CLI（Claude Code/Codex/Gemini/Pi）的工作相位——这是与 Codux（统一管理 9+ AI CLI + Token 统计 + 凭证隔离 + 本地记忆）和 OxideTerm（BYOK AI + MCP + RAG + 审批式工具调用）的本质区别：octopus 是「被动观察 agent 状态」，前者是「主动编排 agent」。
- ActionBar 联动：`listen "terminal://new-tab" { cwd, command }` 可从其他模块（如 ActionBar）新建 tab + 写命令（`index.tsx:252-276`），用 ready 回拉消除后端固定 sleep(250ms) 的竞态（§1.6）。

**SSH / 远程（无）**
- 代码搜索确认：octopus 无 SSH/SFTP/远程 PTY（`crates/` 下 ssh 命中仅在 sync/vault 的 git 操作 + pin_window，与终端无关）。这是相对 OxideTerm（纯 Rust SSH russh + SFTP + 端口转发 + RDP/VNC）、WezTerm（libssh2 + SSH Domain mux）、Codux（iroh P2P 跨设备）的明显缺失。

**搜索 / 命令历史**
- 终端内搜索：`SearchOverlay`（`Cmd+F`）+ xterm SearchAddon，增量高亮 + 上/下导航（`SearchOverlay.tsx`）。**无跨会话/跨机器命令历史同步**——不像 Atuin（SQLite + E2E 加密同步 + fuzzy/prefix/fulltext/skim/daemon-fuzzy 5 模式 + AI 命令生成 + MCP Server）。

**主题 / 字体 / Nerd Fonts**
- 主题：终端画布固定深色 `#0c0c0f`（signature 元素，不随主题切换，`useTerminalSession.ts:225-230`）。tab/sidebar 栏用主题 token 浅色/深色自适应。**无主题系统**（不像 Ghostty 300+ 内置主题/Catppuccin、WezTerm 700+ 方案、Alacritty TOML colors 段）。
- 字体：`terminal_font_size`/`terminal_font_family` 存 AppConfig，`Cmd+=/-` clamp 8-32 + persist（`index.tsx:117-124`）。`setFontFamily` 必须 dispose + 重 attach WebGL（字符 atlas 缓存旧字体会字宽错乱，`useTerminalSession.ts:516-532`）。**无 Nerd Fonts 内置支持**（不像 Starship/Catppuccin 方案依赖 Nerd Fonts 图标）。

**热键 / 全局终端 toggle（无）**
- 终端窗口无全局热键 toggle。octopus 的全局热键在 action_bar（`action_hotkey.rs`）/ record（`record_hotkey.rs`），与终端无关。

### 6.2 竞品对比矩阵

| 维度 | Octopus | Alacritty | WezTerm | Ghostty | Zellij | wterm | Codux | OxideTerm | Atuin |
|---|---|---|---|---|---|---|---|---|---|
| **GPU 渲染** | xterm WebGL（WKWebView 内） | OpenGL ES 2.0+（自研） | OpenGL（自研） | Metal + 自定义 shader（自研） | CPU（crossterm） | DOM（无 GPU） | GPUI（Zed） | GPUI + alacritty_terminal | N/A（CLI） |
| **多路复用** | 无（多 tab） | 无（哲学：交 tmux） | 本地/SSH/Unix/TLS Domain | 无（单窗口多 tab） | 核心（浮动/堆叠面板） | 无 | iroh P2P 跨设备 | SSH 连接池 + Grace Period 重连 | N/A |
| **shell integration** | OSC 133+7（zsh/bash） | 无（哲学） | OSC 7/133/1337 | shell-integration = zsh | 自动检测 + 指令 | 无 | 非侵入 wrapper（9+ CLI） | 内置 | preexec/postexec hook（6 shell） |
| **AI** | 被动观察 agent 相位 | 无 | 无 | 无 | 无 | 无 | **原生**（统一管理 + Token + 记忆 + 凭证隔离） | **原生**（BYOK + MCP + RAG + 审批式工具） | **原生**（AI 命令生成 + MCP Server + Agent Hook） |
| **SSH** | 无 | 无 | libssh2 + ~/.ssh/config | 无 | 无 | WebSocket（示例） | iroh P2P（非 SSH） | **russh**（纯 Rust，不依赖 OpenSSL/libssh2）+ Agent + 多跳代理 | 无 |
| **SFTP** | 无 | 无 | 无 | 无 | 无 | 无 | 无 | **有**（+ trzsz 带内传输） | 无 |
| **WASM 插件** | 无 | 无 | 无（Lua 插件） | 无 | **wasmtime**（15 内置插件） | 无 | 无 | **wasmtime**（沙箱 + 宿主 API） | 无 |
| **prompt 检测** | OSC 133（zsh 精确） | 无 | OSC 133 | OSC 133 | 无 | 无 | OSC 133 + agent wrapper | OSC 133 | preexec hook |
| **历史同步** | 无 | 无 | 无 | 无 | 无 | 无 | rusqlite（AI 历史/会话） | 本地 | **E2E 加密跨机器**（SQLite/Postgres） |
| **主题** | 固定深色 | TOML colors | 700+ 方案 + Lua | 300+ 内置（Catppuccin） | KDL | CSS 变量（4 套） | GPUI 原生 | GPUI 原生 | N/A |
| **跨平台** | macOS-only | Linux/macOS/Win/BSD | Linux/macOS/Win/FreeBSD | Linux/macOS | Linux/macOS/Win | Web（Zig WASM） | Linux/macOS/Win + WSL + 移动端 | Linux/macOS/Win | 6 shell 全平台 |
| **license** | （闭源，内部） | Apache-2.0 | MIT | MIT | MIT | （Vercel Labs） | GPL-3.0 | GPL-3.0 | MIT |
| **语言** | Rust + TS（React） | Rust | Rust | Zig | Rust | Zig + JS | Rust + Dart | Rust | Rust |

### 6.3 Octopus 独特价值

1. **嵌入桌面 app 一体化**：终端与 ASR（语音转写）/ OCR / Clipboard / ActionBar / Vault 同进程同窗口。`paste-text` 事件让 ASR 转写文本直写 PTY（绕过 xterm/键盘模拟，最可靠），这是任何独立终端都无法复制的——Alacritty/WezTerm/Ghostty/Zellij 都没有「语音 → 终端」的直通路径。文件拖拽从内置 FileTreePanel（gitignore 感知）发起，相对 cwd 路径自动计算。

2. **OSC agent 状态感知的轻量实现**：与 Codux（重——非侵入 wrapper + per-tool 适配器 + 凭证注入 + Token 统计）不同，octopus 用**零侵入**的 OSC 序列嗅探（`AgentDetector` 状态机）+ hook 配置注入（`agent_hooks.rs`），只读地可视化 agent CLI 的工作相位。用户不需要改工作流，装上 octopus 就能在 tab 徽章上看到 Claude Code 在 working/attention/finished。代码量小（`agent_detect.rs` 583 行含测试），无外部依赖。

3. **稳定性细节修复的深度**：waiter join 超时（`sleep 100 &` 假死）+ reap_exited（前端崩溃泄漏）+ getpwuid_r（并发竞争）+ rAF 累积（shell 回显丢失）+ 冷启动光标闪烁（CursorBlinkStateManager）+ WebGL context loss 重连 + listener 稳定回调 ref（防间隙丢事件）+ ready 回拉（消除 sleep(250ms) 竞态）。这些是 WKWebView + xterm.js + portable-pty 组合在 macOS 上的**已知暗坑集合**，octopus 逐一修复并有回归测试 + spec 文档。这套经验在竞品里几乎没有公开记录（Alacritty/WezTerm 是原生窗口不踩 WKWebView 坑；Codux/OxideTerm 用 GPUI 也不踩）。

4. **OSC 7 安全过滤**：命令执行期间（OSC 133 B→D/A）忽略 OSC 7，防 SSH/恶意文件伪造 cwd。这是相对简单终端（只裸听 OSC 7）的安全增强，spec 明确论证（`2026-07-31-terminal-osc7-cwd-design.md:113-118`）。

### 6.4 Octopus 不足 / 缺失

1. **无 GPU 渲染（自研）**：依赖 xterm.js WebGL addon，受限于 WKWebView 的 OpenGL ES 子集 + ~16 context 上限（需手动释放隐藏 tab 的 context）。吞吐量无法与 Alacritty（vtebench 持续领先）/ Ghostty（Metal + 自定义 shader）/ WezTerm（OpenGL 自研管线）的 native GPU 渲染相比。`yes` 高速输出需 rAF 节流就是症状。

2. **无 SSH / 远程**：完全缺失。无法替代 iTerm2/WezTerm/OxideTerm 用于服务器运维场景。octopus 的定位是本地 ASR 工具，远程不在范围内，但对「想用一个终端搞定一切」的用户是硬伤。

3. **无终端原生 AI**：终端本身不提供自然语言转命令（piz）、AI 对话（Codux/OxideTerm）、AI 命令生成（Atuin AI）。只能被动观察用户自己跑的 AI CLI。ASR → 命令的路径存在（paste-text），但没有「语音/自然语言 → AI 理解 → 生成 shell 命令」的闭环。

4. **无多路复用 / 分屏**：只有多 tab + 两种布局（tabs/sidebar），无 split panes、无远程 mux、无会话恢复。Zellij/WezTerm/Codux/OxideTerm 都有分屏 + 会话持久化。

5. **无 shell history sync**：命令历史只存在 shell 自身（zsh/bash history 文件），无 SQLite 索引、无跨会话/跨机器同步、无 fuzzy 搜索。Atuin 的全套（退出码/耗时/cwd 上下文 + E2E 加密同步 + 5 搜索模式 + AI）完全缺失。

6. **无 WASM 插件 / 配置生态**：无插件系统（Zellij wasmtime 15 内置 + 自定义）、无 Lua 配置（WezTerm）、无 TOML 配置热重载（Alacritty）。主题固定深色，无 Catppuccin/300+ 主题。

7. **macOS-only**：portable-pty 的 Windows ConPTY/WSL 被显式删除（`session.rs:8`）。与 Alacritty/WezTerm/Ghostty/Codux/OxideTerm 的全平台覆盖形成对比。

8. **无图形协议**：不支持 Kitty graphics / iTerm2 image / Sixel（Ghostty/WezTerm 的卖点）。终端内无法显示图片。

### 6.5 建议改进方向

按投入产出比排序：

1. **shell history 集成 Atuin**（低成本高收益）：octopus 已有 OSC 133 prompt 边界检测 + `crates/infra/db` SQLite + sync crate（git 同步基础设施）。可记录每条命令（cmd/exit_code/duration/cwd/timestamp/session_id）到本地 DB，复用 sync crate 做跨机器同步。无需重写 Atuin，甚至可直接 `atuin init zsh` 注入到 `zshrc.zsh`，octopus 只做 UI 浮层（Cmd+R 模糊搜索）。这是补齐「终端工具完整度」最显眼的缺口。

2. **远程 SSH（ russh 路线）**：参考 OxideTerm 的纯 Rust SSH（`russh` + `ring`，不依赖 OpenSSL/libssh2）。octopus 已有 portable-pty 本地 PTY + Tauri 多窗口能力。新增 `crates/ssh/` 封装 russh，终端 tab 类型扩展为「local/ssh」，PTY session 的 reader/writer 桥接到 SSH channel 而非本地 master fd。中期可加 SFTP 复用 FileTreePanel 的 UI。这条路线能让 octopus 从「本地 ASR 工具」升级为「运维 + ASR 工作台」。

3. **AI 命令生成（轻量，复用现有 llm crate）**：octopus 已有 `crates/llm/`（LLM 润色）。加一个「自然语言 → shell 命令」的 prompt 模板 + 安全校验（参考 piz 的三层防护：Prompt 拒绝 + 注入检测正则 + 危险分级二次确认）。终端内 Cmd+I 触发输入框，结果 paste 到 PTY（不自动回车）。比 Codux/OxideTerm 的重 AI 集成轻得多，但补齐「终端原生 AI」的感知。

4. **图形协议（Kitty graphics）**：xterm.js 有 `addon-canvas` + 社区 image addon。如果要在终端内显示 OCR 识别的图片截图（octopus 已有 `crates/ocr/` + `crates/capx/`），Kitty graphics 协议能让 `capx` 截图直接在终端内 inline 显示，形成「截图 → OCR → 终端内显示结果」的闭环。优先级低于前 3 项。

5. **主题系统**：接入 Catppuccin（Latte/Frappé/Macchiato/Mocha 4 风味）+ 跟随系统深浅色。当前固定深色 `#0c0c0f` 是有意的 signature 设计，但提供选项让用户跟随系统主题一致性更好。低成本（xterm theme 对象 + 配置项）。

6. **分屏 pane（中期）**：参考 Terax 的 pane 树（octopus 注释里多次提到「简化版相对 Terax」，`useTerminalSession.ts:3-5`）。若要加，需回填 rendererPool + dormantRing（xterm 实例池化，隐藏 pane 不 dispose）。投入较大，建议在 SSH/AI/history 之后再考虑。

**不建议跟进的方向**：WASM 插件系统（Zellij/OxideTerm 路线）——与 octopus「ASR 工具集」定位不符，维护成本高；自研 GPU 渲染器（Alacritty/Ghostty 路线）——xterm.js WebGL 在 octopus 的吞吐场景下已够用，自研投入产出比极低。

**核心结论**：octopus 终端的独特价值是「嵌入 ASR/OCR/Clipboard 一体化 + OSC agent 状态感知 + WKWebView 稳定性细节」，不是「做一个更好的独立终端」。改进方向应**强化一体化**（history sync 复用 DB/sync、AI 命令复用 llm crate、图形协议联动 ocr/capx）而非追赶独立终端的功能清单（GPU 渲染/WASM 插件/300 主题）。SSH 是唯一值得跳出一体化去补的能力，因为它能扩展 octopus 的可用场景到远程运维。

Sources:
- [lwn.net - Ghostty 1.0](https://lwn.net/Articles/1004377/)
- [ghostty.org/docs/features](https://ghostty.org/docs/features)
- [ghostty.org 1.3.0 release notes](https://ghostty.org/docs/install/release-notes/1-3-0)
- [sw.kovidgoyal.net/kitty/graphics-protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)

---

## 7. 录屏模块

### 7.1 Octopus 现状

Octopus 的录屏模块采用 **「路线 D-Swift」（vendor Swift helper 子进程）** 架构——这是 `docs/superpowers/specs/research/2026-07-25-screen-record-survey.md:24` 经过对 screencapturekit-rs / snow-shot(ffmpeg) / QuickRecorder / openscreen 四套源码勘读后选定的路线。核心思想（抄自 openscreen）：把「原生捕获 + 编码」剥离成独立二进制，主进程纯 Rust 调度。

**Swift helper 子进程协议**（`crates/record/src/protocol.rs` + `crates/record/native/macos/Sources/OctopusSckHelperLib/`）：
- 传输层是 **JSON-over-stdio**：`argv[1] = RecordingRequest`（一次性启动配置），`stdout = HelperEvent` 事件流（按行 JSON），`stdin = pause/resume/stop` 命令流（`OctopusSckHelperLibMain.swift:74-89`）。
- `RecordingRequest`（`protocol.rs:10-17`）含 `Source`（Display/Window/Area 三态 tagged enum）+ `VideoConfig`（fps/codec H264|HEVC/bitrate/hide_system_cursor）+ `AudioConfig`（system + microphone）+ `Outputs.screenPath`。
- `HelperEvent`（`protocol.rs:69-77`）：`ready / recording-started / recording-paused / recording-resumed / recording-stopped / warning / error`，外层 `kebab-case`，字段 `snake_case`（注意：与 Tauri 边界的 `camelCase` 规范相反——helper 协议是内部协议，前端不直接消费；`protocol.rs:204` 测试注释明确这是 AGENTS.md 例外）。
- `RecordSession`（`session.rs`）管理 helper 生命周期：`start()` spawn 后等 `recording-started`（10s 超时），`stop()` 等 `recording-stopped` payload（含精确 `duration_ms`/`file_size`）。三次 P0 修复（`session.rs:91-110` 注释）都在「失败路径必须 `reset_to_idle` SIGKILL helper」——否则 state 卡 Starting/Stopping，后续 start 全撞 AlreadyRunning。

**捕获后端**：`ScreenCaptureRecorder.swift` 用 `SCStream + AVAssetWriter`（**不是** macOS 15 的 `SCRecordingOutput`），因此支持 macOS 13+（`ScreenCaptureRecorder.swift:17` `@available(macOS 13.0, *)`）。Area 裁剪用 `SCStreamConfiguration.sourceRect`（macOS 14+ API，`ScreenCaptureRecorder.swift:327-337`），13.x 误传 area 时 emit warning + fallback 全屏。

**音轨（`RecordingMeta.audio_tracks`）**：双轨独立 `AVAssetWriterInput`——**麦克风先 add（track 0，播放器默认播放），系统音频后 add（track 1）**（`ScreenCaptureRecorder.swift:392-401` 注释解释了这个顺序修复：原 system 先 add 导致播放器默认放静音系统轨）。`audio_tracks.rs:40-65` 的 `infer_audio_tracks` 按 helper add 顺序 + 配置推断每轨 source（Microphone/System/Merged/Unknown）。mic 优先用 SCK 原生 `captureMicrophone`（macOS 14+ 私有 KVC，`ScreenCaptureRecorder.swift:340-357`），用 `responds(to:)` 探测后降级。系统音频走 `capturesAudio=true` + `excludesCurrentProcessAudio=true`（无驱动内录，无需 BlackHole）。后期合并用 ffmpeg `amix`（`postprocess.rs:268-289`，mono/stereo 自动适配）。

**多屏 / 区域 / 窗口**：`--list-displays` / `--list-windows` / `--list-microphones` 子命令（`OctopusSckHelperLibMain.swift:104-224`）。window 列表多层过滤排除状态栏/Dock/控制中心 + octopus 自己的「录制设置」浮窗（`OctopusSckHelperLibMain.swift:151-183`）。Area 选区用 `record_area_picker.rs`——多屏全屏透明覆盖，用户拖框（仿 screenshot 模式）。

**浮窗 pill（`record_control_window.rs`）+ 副屏坐标 gotcha**：display/window 录制时显示 130×38 pill（红点 + 时长 + 暂停/停止），位置在录制所在屏右下角。**关键修复**（`record_control_window.rs:74-106`）：旧代码用 `app.primary_monitor()` 永远定位主屏，且 `Monitor::position()` 返回物理像素未除 scale——双重错误导致副屏录制时 pill 跑到主屏。修复后用 `CGDisplay::new(display_id).bounds()` 拿**逻辑** CGRect（CoreGraphics 原生返回 points，已含 scale）。`pill_bottom_right`（`:113-117`）显式不 `.max(0.0)`——副屏在主屏左/上方时 origin 是负值，clamp 会把 pill 推回主屏（回归测试 `:162-179`）。

**权限模型**（`protocol.rs:81-87`）：`PermissionStatus{Granted,Denied,NotDetermined}` + `PrivacySection{ScreenCapture,Microphone,Accessibility,Automation}`（camelCase 序列化，`protocol.rs:204-222`）。helper 内 `CGPreflightScreenCaptureAccess` / `CGRequestScreenCaptureAccess` + `AVCaptureDevice.requestAccess(for:.audio)`（`ScreenCaptureRecorder.swift:216-241`）。

**其他**：热键 `Cmd+Shift+R` toggle（可配置）+ `Esc` stop（按需注册避免吞掉其他窗口 DOM ESC，`record_hotkey.rs:8-22`）；pause/resume 用 PTS 偏移重写（`ScreenCaptureRecorder.swift:497-559`，非分段录制）；录制完默认 Finder 高亮文件（`control.rs:501`）；GIF 导出 ffmpeg `fps=15,scale=800:-1:flags=lanczos`（`postprocess.rs:112-117`）；字幕生成是**录后**流程（`generate_subtitle`，ffmpeg 抽 16k mono PCM → ASR → SRT，可选 LLM 润色，`postprocess.rs:418-600`）。

### 7.2 竞品对比矩阵

| 维度 | Octopus | quickrecorder | openscreen | CleanShot-X | Snapzy | Cap | screenity | RecEasy | esearch |
|---|---|---|---|---|---|---|---|---|---|
| 平台 | macOS 13+ | macOS 12.3+ | 全平台 | macOS 10.15+ | macOS 13+ | 全平台 | 浏览器 | Windows | 全平台 |
| 后端 | SCStream+AVAssetWriter | SCK+AVFoundation | SCK/WGC/浏览器 | 原生(闭源) | SCK+AVAssetWriter | scap(SCK/D3D) | getUserMedia | mss+ffmpeg | node-screenshots+WebCodecs |
| 系统音频 | ✅ 无驱动 | ✅ 无驱动 | ✅ | ✅ 4.6 新引擎 | ✅ 48kHz | ✅ | ✅ | ✅ loopback | ⚠️ |
| 麦克风 | ✅ 双轨 | ✅ 双轨 | ✅ 双轨 | ✅ | ✅ 独立+混音 | ✅ | ✅ PTT | ✅ 双轨 | ✅ |
| 区域/窗口/全屏 | ✅ 三模式 | ✅ +应用+移动设备 | ✅ 窗口/全屏 | ✅ +滚动 | ✅ +应用 | ✅ | ✅ 标签页/区域/应用 | ✅ +手绘+追踪 | ✅ |
| 多屏 | ✅（副屏 pill bug 已修） | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ 跨 DPI | ✅ |
| 光标效果 | ⚠️ 仅 hide 开关 | ✅ 高亮+放大镜 | ✅ 可编辑光标 | ✅ | ✅ 涟漪 | ✅ 平滑 | ✅ 聚光灯 | ❌ Roadmap | ✅ |
| 点击效果 | ❌ | ✅ | ✅ 主题 | ✅ 自定义 | ✅ +按键 | ✅ | ✅ | ❌ Roadmap | ✅ |
| 字幕/转写 | ✅ 录后 ASR+LLM 润色 | ❌ | ✅ 本地离线 | ❌ | ❌ | ✅ AI 自动 | ❌ (Pro) | ❌ | ❌ |
| AI 摘要 | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ 标题/章节/摘要 | ❌ | ❌ | ✅ Vision |
| 编辑 | ⚠️ 仅 GIF+音轨合并 | ✅ 修剪器 | ✅ 缩放/修剪/标注 | ✅ 内置编辑器 | ✅ 缩放/变速/Follow | ✅ 本地编辑 | ✅ 裁剪/音频 | ❌ | ✅ WebCodecs 帧级 |
| 导出格式 | MP4, GIF | MP4,H265,HEVC-Alpha,MP3,GIF | MP4,GIF | MP4,GIF | MP4,HEVC,GIF | MP4,WebM | MP4,GIF,WebM | MP4 | MP4,WebM,GIF |
| 云分享 | ❌ | ❌ | ❌ | ✅ Cloud | ✅ BYOS S3/R2/GDrive | ✅ 分享链接 | ✅ GDrive | ❌ | ❌ |
| 摄像头 PiP | ❌ | ✅ Presenter Overlay | ✅ | ✅ | ❌ | ✅ | ✅ AI 背景 | ❌ | ✅ 虚拟背景 |
| license | MIT(helper) | AGPL-3.0 | MIT | 闭源商业 | BSD-3 | AGPL-3.0 | GPL-3.0 | MIT | GPL-3.0 |
| 跨平台 | ❌ macOS only | ❌ | ✅ | ❌ | ❌ | ✅ | ✅(浏览器) | ❌ Win | ✅ |

### 7.3 Octopus 独特价值

1. **与 ASR 深度联动（最大差异化）**：`generate_subtitle`（`postprocess.rs:418`）复用 `octopus-asr-local` 的流式 Paraformer/SenseVoice，选轨逻辑（`subtitle.rs:108-144`）优先 mic → fallback system → Merged，且支持 LLM 整段润色（保留 `[[N]]` 边界标记，`subtitle_polish.rs`）。这是 quickrecorder/CleanShot/Snapzy 都没有的——录完即转写，无需第三方服务，完全本地。Cap 虽有 AI 摘要但是云端。

2. **Tauri app 一体化**：录屏不是孤立工具，而是与 ASR/剪贴板/OCR/翻译模块共享 DB（`recordings` 表 v58 schema）、配置（`record_*` key 复用 ASR 的 `microphone` 配置，`control.rs:164-181` 三级回退）、热键体系、浮窗工厂（`build_float_window`）。麦克风设备复用 ASR 精心选过的设备——这是单一用途录屏工具做不到的协同。

3. **双轨音轨 source 标注**：`AudioTrack{source: Microphone/System/Merged}`（`audio_tracks.rs:13-33`）写入 DB + mp4 udta metadata（`write_audio_tracks_metadata`），播放器/后期工具可识别每轨来源——为后期编辑做了数据准备。

4. **Area 标注实时绘入视频**：`record_annotation_window`（always_on_top，SCK 会录到）让用户录制中画的标注直接进视频，Canvas 限制在选区内（被录）、工具栏在选区外（不被录）——这是 esearch「超级录屏」同类思路。

5. **本地优先 + 隐私**：无云依赖、无遥测、无账号——与 screenity 的 privacy-first 同一赛道但桌面原生。

### 7.4 Octopus 不足 / 缺失

1. **无实时转写**：`generate_subtitle` 是**录后**触发（ffmpeg 抽 PCM → ASR），录制中没有流式转写。Cap 的「录制即生成 transcript」、esearch 的实时字幕都更强。这是与 ASR 联动潜力的「半成品」——`octopus-asr-local` 本就是流式引擎，技术上有基础但未接通。

2. **无 AI 摘要 / 章节 / 标题**：Cap 的招牌特性（自动生成 title/summary/chapters）octopus 完全没有。虽有 LLM 润色字幕文本，但没做内容级摘要。

3. **无点击效果 / 光标高亮 / 聚光灯 / 平滑缩放**：`VideoConfig` 只有 `hide_system_cursor` 布尔（`protocol.rs:34`），`ScreenCaptureRecorder.swift:317` `showsCursor = !hideSystemCursor`。无 CleanShot/Snapzy/openscreen 的点击涟漪、光标放大、自动跟焦。Web 研究（Screen Charm 等）确认 SCK 不原生提供这些，需 app 层 CGEvent tap + 后期合成——openscreen 的 cursor helper（SHA256 去重发送光标轨迹）是范本。

4. **编辑能力薄弱**：仅有 `export_gif` + `merge_audio_tracks` 两个后处理命令。无 trim/speed/crop/zoom 关键帧/Follow Mouse——Snapzy/CleanShot/openscreen/esearch 都有可视化时间线编辑器。

5. **无云分享 / 链接分享**：录完只能 Finder 高亮本地文件。Cap/Screenity/Snapzy 都有一键分享链接。

6. **无 WebM / 透明通道输出**：仅 MP4（H264/HEVC）。quickrecorder 有 HEVC with Alpha，screenity/esearch 有 WebM。

7. **macOS 独占**：helper 是 Swift，Windows/Linux 无对应实现（`platform/windows.rs` / `linux.rs` 是占位）。RecEasy(esearch/Cap/openscreen) 跨平台。

8. **无摄像头 PiP / Presenter Overlay / 虚拟背景**：quickrecorder/CleanShot/Snapzy/screenity 都支持。

9. **无滚动录制**：CleanShot/Snapzy/esearch 的招牌特性。

### 7.5 建议改进方向

按「投入产出比 + 与既有架构契合度」排序：

1. **🔴 录制中实时转写（P0，差异化杀手锏）**：`octopus-asr-local` 已是流式引擎，改造 helper 在 `didOutputSampleBuffer`（`.audio` 分支，`ScreenCaptureRecorder.swift:177-180`）旁路一份 PCM 给主进程 → 喂 ASR → emit `subtitle-cue` 事件 → 前端浮层显示。这是把「与 ASR 联动潜力」从半成品变完成品的最小路径，且无竞品（Cap 是云端、esearch 无）。复用既有 `SubtitleCue` 模型。

2. **🟠 点击效果 + 光标轨迹录制（P1）**：抄 openscreen 的 cursor helper（`survey.md:192-196`）——独立子进程 `CGEvent tap` 监听 leftMouseDown/Up + `NSCursor.current` 采样，SHA256 去重发送。录后编辑器（或简化版「点击涟漪烧录」ffmpeg 滤镜）按时间轴合成。`VideoConfig` 加 `cursor_effect` 配置。这是录屏工具的「教程质感」分水岭。

3. **🟠 AI 摘要 / 章节（P1）**：录制完 → 字幕 cues → 喂 LLM 生成 title/summary/chapters → 写入 `RecordingMeta`（加字段）。octopus 已有 LLM 配置体系（`list_subtitle_llms`，`postprocess.rs:688`），复用。直接对标 Cap。

4. **🟡 简易 trim 编辑（P2）**：ffmpeg `-ss/-to` 命令（survey.md:311 已规划），无自研编辑器。前端加时间线 UI 选段 → 调 `trim_recording` 命令。

5. **🟡 WebM 输出（P2）**：`VideoCodec` enum 加 WebM/VP9 变体，helper 改用 AVAssetWriter 的 WebM fileType（macOS 14+）。或 ffmpeg 后处理转码。

6. **🟡 摄像头 PiP（P2）**：helper 加 `AVCaptureSession` 视频输入 → 第三条 `AVAssetWriterInput` → burn-in 或 sidecar 轨。macOS 14+ 用原生 Presenter Overlay（`SCContentFilter(presenting:`).

7. **🟢 跨平台 helper（P3，长期）**：Windows 抄 openscreen 的 `wgc-capture.exe`（WGC + WASAPI loopback + MF encoder，同 JSON-stdio 协议）。`platform/windows.rs` 已占位。

8. **🟢 云分享（P3）**：参考 Snapzy BYOS（S3/R2/GDrive）——比 Cap 的托管云更隐私友好，与 octopus 本地优先定位一致。

**关键文件索引**（均为绝对路径）：
- 协议定义：`/Users/wudarui/workspace/agent/octopus/crates/record/src/protocol.rs`
- 会话控制：`/Users/wudarui/workspace/agent/octopus/crates/record/src/session.rs`
- Swift helper 核心：`/Users/wudarui/workspace/agent/octopus/crates/record/native/macos/Sources/OctopusSckHelperLib/ScreenCaptureRecorder.swift`
- helper 入口：`/Users/wudarui/workspace/agent/octopus/crates/record/native/macos/Sources/OctopusSckHelperLib/OctopusSckHelperLibMain.swift`
- 浮窗 pill（副屏修复）：`/Users/wudarui/workspace/agent/octopus/crates/desktop/src/record/record_control_window.rs`
- 控制命令 + 入库：`/Users/wudarui/workspace/agent/octopus/crates/desktop/src/record/record_commands/control.rs`
- GIF/合并/字幕：`/Users/wudarui/workspace/agent/octopus/crates/desktop/src/record/record_commands/postprocess.rs`
- 音轨推断：`/Users/wudarui/workspace/agent/octopus/crates/record/src/audio_tracks.rs`
- 字幕模型：`/Users/wudarui/workspace/agent/octopus/crates/record/src/subtitle.rs`
- 路线调研（最有价值参考）：`/Users/wudarui/workspace/agent/octopus/docs/superpowers/specs/research/2026-07-25-screen-record-survey.md`

Sources:
- [Cap — Beautiful screen recordings](https://cap.so/)
- [CapSoftware/Cap — GitHub](https://github.com/capsoftware/cap)
- [Apple — Capturing screen content in macOS](https://developer.apple.com/documentation/screencapturekit/capturing-screen-content-in-macos)
- [Recordscript (Tauri + Whisper-rs)](https://news.ycombinator.com/item?id=41244915)

---

## 8. OCR 模块

### 8.1 Octopus 现状

`octopus-ocr` 是统一 OCR 入口，依赖 `octopus-paddle-ocr`（vendor 自 paddle-ocr-rs 的精简版，删 opencv/turbojpeg/clap/reqwest/serde_yaml，保留 det/rec/cls/pipeline/runtime/vision 核心）。推理后端与 ASR 共用 ONNX Runtime：`crates/paddle-ocr/Cargo.toml:7` `ort = "2.0.0-rc.12"` features `["ndarray","download-binaries"]`，跨平台零原生编译。

**引擎 / Pipeline**：PaddleOCR PP-OCRv6-small（主）+ PP-OCRv5（fallback）。三阶段 `det → cls → rec`（`crates/paddle-ocr/src/pipeline/rapid_ocr.rs:80-99`，`RapidOcr::run` 编排，三 stage 各自可关）。模型组在 `crates/ocr/src/paddle_backend.rs:96-121` `build_engine_config` 拼 `det.onnx + rec.onnx + cls.onnx + keys.txt`，cls 缺失则关 `use_cls`：
- PP-OCRv5：det 4.5M + rec 16M + cls 572K + keys 18383 行
- PP-OCRv6-small：det 9.7M + rec 21.5M + keys 18708 行（`ppocrv6_dict.txt`）（`docs/architecture.md:203`）

默认常量 `DEFAULT_OCR_MODEL = "PP-OCRv5"`（`crates/ocr/src/model.rs:3`），激活模型从 DB `models.is_enabled=1` 查（`engine.rs:58-62`）。版本路由靠 model_name 字符串前缀：v6 跳过英文分词（CTC space token 已激活），v5 走 37 万词表贪心分词（`engine.rs:344-429` `segment_english_words`，编译期内嵌 `assets/words_common.txt`）。

**语言**：`LangRec` enum 声明 15 种（ch/ch_doc/en/arabic/chinese_cht/cyrillic/devanagari/japan/korean/ka/latin/ta/te/eslav/th/el，`crates/paddle-ocr/src/config.rs:76-119`），`LangDet` Ch/En/Multi，`LangCls` 仅 Ch。但实际分发只配齐 PP-OCRv5 默认（中英日 + 拼音）与 PP-OCRv6-small（统一 7.7M 参数、50 语言能力）。`OcrVersion` enum **只到 v4/v5**（`config.rs:24-31`），v6 未进 enum。

**输入 / 输出**：图片字节（PNG/JPEG）→ `image::load_from_memory` → `DynamicImage`（`engine.rs:148-157`）。**不支持 PDF / 多页文档**。输出 `OcrBlock { text, x, y, w, h, score }`（`engine.rs:23-32`）+ Markdown（`layout.rs::to_markdown`）。坐标是 det quad 折叠成的轴对齐外接矩形（`engine.rs:278-294`），**丢失旋转信息**。

**布局 / 表格 / 公式**：**无** ML 版面、无表格、无公式。仅启发式后处理（`layout.rs:21-179`）：标题靠框高 / median_h 比（≥1.6→H1，≥1.3→H2）；列表靠文本前缀（`•/-/①/1./1、` 等）；段落靠 y 间距（>median_h×0.8→新段）；多行正文 code fence 包裹不 reflow。无栏分析、阅读顺序、cell、LaTeX。

**QR 码**：`crates/ocr/src/qrcode.rs` 用 `zxing-cpp 0.5`（`Cargo.toml:15`，bundled C++ FFI），多码全识别，**仅 QRCode 格式**（未启用 DataMatrix/PDF417/Aztec/Code128 等）。专门绕开 zxing-cpp 的 `image` feature 避开 avif→rav1e 重依赖链，灰度 Lum 通道直喂 `ImageView`。

**ONNX 推理优化**（`crates/paddle-ocr/src/runtime/session.rs:42-93`）：GraphOptimizationLevel::Level3、intra/inter_threads auto-tune（按物理核 clamp，`session.rs:177-201`）、CPU memory arena 可关、ExecutionProvider 链解析（`runtime/provider.rs:32-68`，CPU/CUDA/DirectML/CANN）。但 **`RuntimeBackend` enum 只有 `OnnxCpu`**（`config.rs:137-142`），session 强制校验 backend==OnnxCpu 否则报错。**无 INT8/FP16 量化、无 CoreML EP**（ProviderPreference enum 无 CoreML）。

**集成**（desktop crate）：`ocr_screenshot`（`area.rs:323-414`，截图工具栏闭环：合成选区→入库→OCR→双开图片+文本 tab→emit blocks 给 ImagePreview 叠加，解码一次共享省 100-300ms）；`ocr_image`（剪贴板图片条目 / 图片预览）；`scan_qrcode_screenshot` / `scan_qrcode_image`（纯识别不入库）。全局 `OcrLockGuard` 互斥（`engine.rs:431-448`），多入口并发后者被拒、前端 4 处给出反馈。idle 60s 守护线程释放模型内存（`engine.rs:42-138`），下次 `run_ocr` 自动重载并补 probe。三入口统一走 `insert_ocr_clipboard_item` 入库。长图（>1600px）chunk 切分（CHUNK_HEIGHT=1280, OVERLAP=200，`engine.rs:37-39, 229-270`），按坐标 `covered_until_y` fold max 去重。

**VLM OCR**：**未实现**，仅 trait 预留。`OcrBackend` trait（`backend.rs:9-24`）注释「VLM=true 跳过 to_markdown」，`engine.rs:99-102` 注释「未来按 source_type 分流：source_type=2 → VlmOcrBackend」——目前所有路径固定路由到 `PaddleOcrBackend`。无 MiniCPM-V / Qwen-VL / DeepSeek-OCR / PaddleOCR-VL 集成；`asr-cloud` 与 `llm` crate 也无 cloud vision API。

### 8.2 竞品对比矩阵

| 维度 | Octopus(paddle-ocr) | ocrs | RapidOCR | Umi-OCR | surya | MonkeyOCR v2 | OvisOCR2 | OpenOCR | deepseek-ocr-rs | tr / trwebocr | unlimited-ocr |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 技术栈 | Rust + ONNX Runtime（ort 2.0-rc.12），vendored paddle-ocr-rs | 纯 Rust（rten 自研 ONNX 引擎） | Python + ONNX/OpenVINO/TensorRT/Paddle/PyTorch/MNN 六后端 | Python + QML(PySide2)，外置引擎插件 | Python，Qwen3.5 650M VLM + EfficientViT | Python，ViT-S/B 28-113M 编码器 + 0.6B LLM | Python，Qwen3.5-0.8B 端到端 VLM | Python + PyTorch/ONNX，UniRec-0.1B | Rust + Candle + Metal/CUDA | C++ libtr.so（CTPN+CRNN）+ Python | Python，DeepSeek-OCR + R-SWA |
| 语言 | 实际配齐中/英/日；v6 具备 50 语言能力但未分发语言包 | 仅拉丁（多语言规划中） | 80+（11 个专用模型） | 中英日韩俄（插件决定） | 91 语言（通过率 87.2%） | 17 语言 SOTA | 多语言端到端 | 中英为主（UniRec40M 英:中≈3:1） | VLM 原生多语种 | 仅中文 | 多语言端到端 |
| 布局分析 | 仅启发式几何 | 几何 find_text_lines | 无 | TBPU 8 方案 + 忽略区域 | 19 区域类型 + 阅读顺序 | 17 语言 SOTA 文档解析 | 端到端 Markdown 自然阅读顺序 | PP-DocLayoutV2 | VLM 原生 grounding | 无 | 端到端 |
| 表格识别 | 无 | 无 | 无 | 双层 PDF（无 cell 识别） | HTML 表格含跨行跨列 | 支持 | HTML `<table>` | OpenDoc 新增（社区反馈仍弱） | VLM 原生 | 无 | 端到端 |
| 段落 / 公式 | 段落靠 y 间距；无公式 | 无 | 无 | TBPU 段落；Pix2Text 插件公式 | `<math>` LaTeX 内嵌 | 公式 SOTA（CDM 90.9%） | LaTeX | UniRec 文本+公式（比 Mathpix +18.8%） | VLM 原生 | 无 | 端到端 |
| VLM | 无（trait 预留） | 无 | 无 | MistralOCR 插件 | 是（650M） | 是（28-113M 视觉骨干） | 是（0.8B） | 否（UniRec 专用） | 是（DeepSeek-OCR/PaddleOCR-VL/DotsOCR） | 无 | 是（端到端） |
| 模型大小 | ~31M（v6-small det+rec+cls）+ 词表 | MB 级（rten） | 标准 PP-OCR ONNX ~10-20M | 外置引擎 | 650M VLM | 0.6B-1.8B | 0.8B | 0.1B + PP-DocLayoutV2 | 9-50GB（重） | ctpn.bin 54M + crnn.bin 43M | 未公开 |
| License | Apache-2.0（继承） | MIT OR Apache-2.0 | Apache-2.0（模型归百度） | MIT | Apache-2.0 代码 / OpenRAIL-M 权重 | Apache-2.0 | Apache-2.0 | Apache-2.0 | Apache-2.0 | ⚠️ Tr 2026-01 删 LICENSE；trwebocr Apache-2.0 但上游已无许可 | MIT |
| 纯 Rust | 是（除 zxing-cpp C++ FFI） | 是（含 rten 引擎） | 否 | 否 | 否 | 否 | 否 | 否 | 是（Candle，但 CUDA alpha） | 否 | 否 |
| PDF 多页 | 无 | 无（CLI 单图） | RapidOCRPDF 子项目 | ✅ PyMuPDF 双层可搜索 PDF | ✅ page_range | ✅ vLLM serving | ✅ max_pixels 2880² | openocr-python 0.1.5+ PDF | 否（图像 prompt） | 无 | ✅ 32K 单次数十页 |
| QR 码 | ✅ zxing-cpp QRCode（其他码制未启用） | 无 | 无 | ✅ zxing-cpp 19 种码制 | 无 | 无 | 无 | 无 | 无 | 无 | 无 |
| 维护 | 活跃（2026-07 重构） | 活跃（v0.12.2 / 2026-03） | 活跃（v3.9.1 / 2026-07） | 活跃（v2.1.5 / 2025-03） | 活跃（v0.21.1 / 2026-07） | 新（2026-07） | 新（2026-07） | 活跃（2026-02） | 活跃（v0.6.0 / 2026-02） | ⚠️ 停维 + 删 LICENSE | 新（2026-06） |
| 形态 | 桌面应用内嵌模块（截图/剪贴板/翻译一体） | Rust 库 + CLI + WASM 扩展 | Python 库 + CLI + 多语言 SDK | 桌面 GUI 软件 + HTTP/CLI | Python 库 + CLI + Streamlit | Python 模型 + vLLM | Python 模型 + vLLM | Python 库 + SDK | Rust 库 + CLI + OpenAI 兼容 HTTP | SDK / HTTP 服务 | Python 模型 |

**Web 补充**：PP-OCRv6 随 PaddleOCR 3.1（2025-07）发布，统一 50 语言单模型，PP-OCRv6_medium 检测 Hmean 86.2% / 识别 83.2%，比 v5 同档检测 +4.9pp / 识别 +5.1pp，medium 识别甚至超 v5_server；1.5M-34.5M 多档。octopus 选 mobile 档最轻量 v6-small。SOTA 小模型赛道：GOT-OCR2.0（580M）仍是边缘 VLM OCR 选择，但 2025-2026 sub-2B VLM（GLM-OCR 0.9B OmniDocBench V1.5 94.62、PaddleOCR-VL 1.6B 报告比 DeepSeek-OCR 快 6×、OvisOCR2 0.8B OmniDocBench v1.6 96.58 端到端首超流水线）精度全面超越 GOT。OCRBench v2 上 IBM Granite Vision 是新晋最佳小模型。

### 8.3 Octopus 独特价值

1. **纯 Rust + ONNX 与 ASR 共用后端**——OCR 与 ASR/VAD 同跑 ort 2.0-rc.12，无 MNN/libtorch/Python 运行时依赖（对比 Tr libtr.so / Umi-OCR PyStand / surya vLLM）。这是对 ocrs（自研 rten）和 rust-paddle-ocr（MNN C++）的差异化：用最主流的 ONNX Runtime + 业界事实标准 PaddleOCR 模型，工程上最稳。

2. **PP-OCRv6-small 最新版**——mobile 档最轻量（~31M），统一 50 语言单模型，比 v5 跨代提升。配合 `use_word_segmentation` 路由（v6 关 / v5 开），同一 backend trait 同时承载两代模型。

3. **截图 / 剪贴板 / 翻译一体**——OCR 不是孤立 SDK，是桌面工作流的一环：截图 OCR → 图片+文本双 tab CompactEditor → FTS5 全文搜索 → 翻译润色。三入口共享 `insert_ocr_clipboard_item`，闭环最短。

4. **QR 一并**——zxing-cpp bundled 一并解决二维码场景，截图识字 + 扫码同源，避免用户装第二个工具。

5. **启发式 Markdown 输出**——`to_markdown` 把零散 det 框变成结构化（标题/列表/段落/code fence），消费端零改动受益。多数纯 OCR 库（ocrs / RapidOCR / tr）不做的事——Umi-OCR 的 TBPU 是同类思路，octopus 是 Rust 轻量实现。

### 8.4 Octopus 不足 / 缺失

1. **无 ML 版面 / 表格 / 公式**——只有几何启发式。栏分析、阅读顺序、表格 cell、公式 LaTeX 全无。文档解析能力远弱于 surya / MonkeyOCR v2 / OvisOCR2 / OpenOCR / deepseek-ocr-rs。

2. **无 VLM OCR**——trait 预留 `source_type=2` 但未实现。错过 PaddleOCR-VL（1.6B，6× 快于 DeepSeek-OCR）、OvisOCR2（0.8B 端到端 96.58 登顶）、MiniCPM-V 等 sub-2B VLM OCR 小型化红利。

3. **无 PDF / 多页文档**——只支持单图。长图 chunk 切分（1280+200 overlap）不是真正的多页文档处理，缺 PDF 解码、双层可搜索 PDF 输出。对比 Umi-OCR（PyMuPDF 双层）、RapidOCRPDF、unlimited-ocr（32K 一次性数十页）、OvisOCR2（PDF 单页推理）。

4. **语言包覆盖未配齐**——`LangRec` enum 声明 15 种，实际只分发 PP-OCRv5（中英日）+ PP-OCRv6-small（具备 50 语言能力但只配 `ppocrv6_dict.txt`）。对比 rust-paddle-ocr（11 专用模型覆盖 100+）、RapidOCR（80+）。

5. **量化缺失**——无 INT8/FP16 量化，macOS 走 CPU 无 CoreML EP（`ProviderPreference` enum 无 CoreML，仅 CPU/CUDA/DirectML/CANN）。rust-paddle-ocr 已有 FP16 模型（推理 +9% / 内存 -8% / 模型减半）。

6. **EP 类型化与版本 enum 不全**——`OcrVersion` 只到 v5（v6 靠字符串前缀）、`RuntimeBackend` 只有 `OnnxCpu`（CUDA/DirectML/CANN 只在 ProviderPreference 链里尝试、session 层强制 CPU 校验）。两处 enum 类型化不完整，反映 v6 与多 EP 是「补丁式」追加。

7. **QR 仅 QRCode 一种码制**——zxing-cpp 19 种码制能力未充分利用。Umi-OCR 同库启用全部 19 种。

8. **OcrBlock 丢失旋转信息**——quad 折叠成轴对齐 x/y/w/h，det 旋转 / 倾斜框方向丢失。ocrs 输出 `RotatedRect` 保留方向。

### 8.5 建议改进方向

1. **PP-Structure / PP-DocLayout 接入**——补 ML 版面。PaddleOCR 官方 PP-StructureV3 已有布局 / 表格 / 公式（LaTeX）/ 阅读顺序四件套，ONNX 格式同样 ort 可跑。优先级：表格 > 公式 > 阅读顺序。OpenOCR 的 PP-DocLayoutV2 是更轻开源替代。

2. **VLM OCR 落地（`source_type=2`）**——实现预留的 `VlmOcrBackend`。首选 PaddleOCR-VL（1.6B、6× 快、CPU 友好、与现有 PaddleOCR 同源）或 OvisOCR2（0.8B OmniDocBench v1.6 96.58 端到端 SOTA）。本地用 Candle（参考 deepseek-ocr-rs），云端走 OpenAI 兼容 API（参考其 `/v1/responses` 折叠多轮为单轮 OCR）。

3. **多语言包配齐**——释放 PP-OCRv6 50 语言能力：分发 korean / latin / cyrillic / arabic / devanagari / japan / th 等专用 rec 模型 + keys（rust-paddle-ocr 已有完整映射表），DB `models` 表 seed 多条供切换。

4. **PDF / 多页文档**——新增 `pdf_ocr` 命令：pdfium-render 或 mupdf-rs 解页 → 逐页 OCR → 双层可搜索 PDF（参考 Umi-OCR）或合并 Markdown（参考 unlimited-ocr 单次长输出）。

5. **EP 类型化补齐 + CoreML + 量化**——`OcrVersion` 加 `PPocrV6`、`RuntimeBackend` 加 `OnnxCuda`/`OnnxCoreMl`/`OnnxDirectMl`，`ProviderPreference` 加 `CoreMl`。macOS 走 CoreML EP（M 系列原生加速）。配合 PP-OCRv6 INT8 量化模型（官方有发布）进一步压体积和延迟。

6. ~~**OcrBlock 保留旋转 + word-level box**——`return_word_box` 路径在 paddle-ocr 已实现但 octopus 默认关~~ **✅ word-level box 已启用（2026-08-13）**：`paddle_backend` 设 `return_word_box: Some(true)`，`OcrBlock.words: Option<Vec<OcrWord>>` 透出到前端，ImagePreview 渲染 HTML 透明文字层（`color: transparent` + `user-select: text`）实现原生拖选（对标 macOS Live Text / PixPin）。**仍缺**：旋转信息（`RotatedRect` 替代轴对齐矩形）。详见 [spec](../superpowers/specs/archived/2026-08-13-image-text-selection-layer-design.md)。

7. **TBPU 式后处理升级**——参考 Umi-OCR 8 种 parser 方案（multi_para 默认 / single_code 代码截图 / 忽略区域），把当前启发式扩展为可切换策略，覆盖多栏与代码截图两个高频痛点。

---

## 9. 密码箱 Vault 模块

### 9.1 Octopus 现状

octopus-vault 是一个纯逻辑 crate（不依赖 tauri/tokio，依赖方向 `infra ← vault ← desktop`，见 `crates/vault/src/lib.rs:13`），由 16 个子模块组成。整体走的是「双层加密 + SQLite 落盘 + git 同步」的本地优先路线。

**加密栈（crypto/）**：分层非常清晰——

- KDF：**Argon2id**，参数 `t=3, m=65536 KiB (64 MiB), p=4`，OWASP 2024 推荐值（`crates/vault/src/crypto/kdf.rs:27-35`）。salt 是 32B 随机，存 `vault_meta.kdf_salt`。安全护栏非常扎实：`from_i64_strict`（`kdf.rs:100-151`）对**远程不可信**同步仓库的 meta.json 做安全下限校验（`memory_kib ≥ 16 MiB`、`iterations ≥ 2`）防弱 KDF 爆破，同时加了**可用性上限**（`memory_kib ≤ 256 MiB`、`iterations ≤ 10`）防 OOM/卡死攻击（`kdf.rs:122-145`）。测试用 `test_params()`（`memory_kib=8`，`kdf.rs:171-178`）走 `from_i64` 本地可信路径，不污染 strict 路径。
- 对称加密：**AES-256-GCM**，密文格式统一 `v1:<base64(nonce[12B]||ct||tag[16B])>`（`crates/vault/src/crypto/symmetric.rs:1-3, 20`），GCM 自带 16B tag 不需独立 HMAC。
- 密钥派生：**HMAC-SHA512 简化 BIP44**（`crates/vault/src/crypto/hierarchy.rs:1-21`），固定 label `octopus/v1/user-vault` / `app-secrets` / `sync` / `send` 派生子 key。Zeroize 卫生极其严格——连 `GenericArray<U64>` 的 chain code 都启用 zeroize feature 清零（`Cargo.toml:30`，`hierarchy.rs:36-52`）。

**Cipher 类型**：`CipherType` enum 定义了 Login=1 / SecureNote=2 / Card=3 / Identity=4 四种（`crates/vault/src/types.rs:13-20`），**但 `CipherData` 实际只有 `Login` 一个变体**（`types.rs:161-166`），其余三种是协议占位、未实现。Login 包含 uris/username/password/totp/password_revision_date（`types.rs:132-142`），uris 匹配策略严格对齐 Bitwarden 官方 `UriMatchType`（`types.rs:79-100`，曾发生过 Exact/StartsWith 反转 bug 已修正）。

**TOTP（`crates/vault/src/totp.rs`）**：RFC 6238，支持裸 Base32 secret 与 `otpauth://` URL 两种输入，支持 SHA1/256/512、digits 6/8、period 任意（`totp.rs:1-12, 63-100`）。放宽了 totp-rs 默认 ≥128bit 限制以接受 RFC 6238 下限的 80bit secret（`totp.rs:8-12, 49-60`）。

**Auto-Type**：实现在 desktop crate（不在纯逻辑的 vault 内），用 **enigo 模拟 CGEvent 键盘事件**（`crates/desktop/src/vault/autotype/macos.rs:1-6, 71-95`），全局热键 `CmdOrCtrl+Shift+S`（`crates/infra/src/config.rs:201-207`），检测浏览器 URL（Chrome/Safari/Firefox/Edge/Brave/Arc，经 AppleScript）匹配 cipher 后注入。有焦点安全校验（防钓鱼注入，校验前台 bundle id ≠ octopus 自身）。**仅 macOS 实现**（`autotype/` 下只有 macos.rs，无 windows.rs / x11.rs）。剪贴板 concealed fallback 兜底。

**Bitwarden 导入**：仅支持 **unencrypted JSON** + type=1（Login）（`crates/vault/src/importer/bitwarden.rs:1-5`），解析 folder/password history/reprompt/fields/uris（`bitwarden.rs:18-100`）。加密导出（`encrypted=true`）不支持。有配套 exporter。

**健康检查（health/）**：**纯本地**，`zxcvbn` 强度评估（score<3 标弱）+ 重复密码分组（`crates/vault/src/health/mod.rs:26-68`），`HealthReport` 含 `scored_count` 修正了 average_score 分母不一致（`mod.rs:20-24`）。**无 HIBP 在线泄露查询**（spec 明确列为 P2，`2026-07-18-password-vault-design.md:42`）。

**密码生成器（generator/）**：Random + EN Passphrase（EFF 7776 词表）+ **ZH Passphrase（jieba 词频 TOP 4096 双字词）**+ PIN（`crates/vault/src/generator/mod.rs:1-9`），中文 passphrase 是相对竞品少见的功能。

**文件夹（storage/folder.rs）**：folder 名用 user_vault_key 加密（`v1:` 前缀密文，`folder.rs:1-8`），软删（is_deleted=1，`folder.rs:31-36`）。UUID v4 作主键（2026-07-21 v44 改造，为 git 同步铺路）。

**Git 同步（sync/）**：复用独立抽离的 `octopus-sync` crate（git wrapper + outline + privacy + store 通用工具，`crates/vault/src/sync/mod.rs:1-41`）。布局 `~/.octopus/.sync/vault/{meta.json, outline.json, ciphers/<2hex>/<uuid>.json, folders/<2hex>/<uuid>.json}`（`sync/mod.rs:6-15`），256 桶分片 + md5 内容指纹增量同步。引擎流程：fetch → merge --ff-only（失败 rebase 兜底）→ 文件系统↔SQLite 双向同步 → commit → push（`sync/engine.rs:1-23`）。GitHub + Gitee 双 remote，**SSH key 认证**（octopus 完全不接触凭证，`sync/mod.rs:2-4`），含**私有库检测守卫**（拒绝公有库）。加密层完全复用 user_vault_key + AES-256-GCM，文件密文格式与 SQLite 一致（`sync/mod.rs:17-18`）。Phase 2 加了每小时自动同步。

**Keychain 集成（keychain.rs）**：**名不副实**——因 octopus-desktop 是 adhoc 签名，macOS Keychain 写入是 session-only（重启丢），现把 `K_machine` 写到 `~/.octopus/machine-key.enc`，用 HKDF-SHA256 派生 file_key 做 AES-256-GCM 加密（`crates/vault/src/keychain.rs:1-39`）。注释非常坦诚：**这是 obfuscation 而非真加密**——派生输入全是公开/硬编码的（machine_id/username/常量），防护等价于文件权限 0600（`keychain.rs:14-30`）。生产签名后应切回真 Keychain。

**明确缺失**：无浏览器扩展 / Native Messaging、无 Passkey / WebAuthn、无 YubiKey、无 SSH Agent、无附件（加密文件存储）、无组织/共享（spec §0.3 明确单用户）、无 HIBP 在线泄露查询、Card/SecureNote/Identity 未实现、Windows/Linux Auto-Type 未实现。

### 9.2 竞品对比矩阵

| 维度 | **Octopus Vault** | **KeePassXC** | **gopass** | **Bitwarden** | **1Password** | **vaultwarden** | **Proton Pass** |
|---|---|---|---|---|---|---|---|
| 存储格式 | SQLite + git 同步（加密 JSON blob） | KDBX 4/3 单文件 | 每 secret 一个 GPG/age 文件 + Git | 服务端 SQL + 客户端缓存 | 服务端 + 客户端 | SQLite/MySQL/PG（Bitwarden 兼容） | 服务端 + 客户端 |
| 加密 | AES-256-GCM | AES-256 / ChaCha20 / Twofish | GPG / age（公钥体系） | AES-256-CBC + HMAC-SHA256 | 闭源（已知 SGX） | 同 Bitwarden | E2EE |
| KDF | **Argon2id 64MiB/3iter/p4** | Argon2id（可调）+ AES-KDF 兼容 | GPG passphrase / age | Argon2id（可配 PBKDF2） | 闭源 + Secret Key | Argon2id | 闭源 |
| TOTP | ✅ RFC 6238 + otpauth URL | ✅ 内置 + QR 导出 | ✅ TOTP/HOTP | ✅ 内置（付费） | ✅ 内置 | ✅（付费） | ✅ 内置 2FA |
| Auto-Type | ✅ **仅 macOS**（enigo + AppleScript） | ✅ 跨平台（X11/Win/mac） | ❌（tessen 仅 Wayland） | ❌ | ❌ | ❌ | ❌ |
| 浏览器扩展 | ❌ | ✅ KeePassXC-Browser（NaCl 端到端加密） | ✅ gopassbridge + jsonapi | ✅ 官方 | ✅ 官方 | ✅ Bitwarden 扩展 | ✅ 官方 |
| Passkey | ❌ | ✅ 1.10.0+（经浏览器扩展） | ❌ | ✅ vault 内 | ✅ | ✅ | ✅ |
| YubiKey | ❌ | ✅ 挑战-响应解锁 | ✅（经 GPG/agent） | ✅ Premium | ✅ | ✅（付费） | ❌ |
| SSH Agent | ❌ | ✅ 内置 | ✅（经 GPG） | ❌ | ❌ | ❌ | ❌ |
| 附件 | ❌ | ✅ 加密附件 | ✅ 二进制文件 | ✅ 付费 | ✅ | ✅（付费） | ✅ |
| 共享/团队 | ❌（单用户） | KeeShare 片段 | ✅ 收件人列表 + Git | ✅ 组织/集合 | ✅ 家庭/团队 | ✅ 组织 | ✅ 共享 vault |
| 同步 | git repo（GitHub/Gitee，SSH） | 用户自选（Syncthing/Dropbox） | Git push/pull | 内置云 + WebSocket | 内置云 | 内置（自托管） | 内置云 |
| Audit | 本地 zxcvbn + 重复检测 | HIBP + 健康报告 | gopass-hibp | 内置 HIBP | Watch Tower | 内置 HIBP | 内置 |
| 跨平台 | mac/win/linux（Auto-Type 仅 mac） | mac/win/linux | mac/win/linux（CLI） | 全平台含移动 | 全平台含移动 | 全平台 | 全平台含移动 |
| License | 私有 | GPL-2/3 | MIT | GPL-3 | 闭源 | AGPL-3 | GPL |

### 9.3 Octopus 独特价值

1. **与 octopus app 一体的「输入态工具」**——vault 不是独立应用，而是嵌在「语音/热词/ASR/Action Bar」一体桌面工具里（`docs/architecture.md` §crates 树）。同一套 SQLite schema（v58）、同一套 `octopus-sync` git 基础设施、同一套 settings 面板。对开发者/极客用户，这意味着「装一个 app 顺带拿到密码箱」，不增加新的同步/配置心智负担。

2. **Auto-Type 跨应用填充（macOS 原生）**——这是 Octopus Vault 唯一真正区别于 Bitwarden/1Password/ProtonPass 桌面端的特性。后者都靠浏览器扩展填网页表单，对原生 app（SSH 客户端、Remote Desktop、本地 GUI 应用）无能为力；Octopus 用 enigo + CGEvent 模拟真实键盘事件（`desktop/src/vault/autotype/macos.rs:1-6`），密码字段（masked input）也能接收，且**完全不依赖浏览器扩展**——对纯 native 应用场景是比 KeePassXC 更轻量的方案（KeePassXC 要装 Qt 全家桶）。

3. **git sync 复用 octopus-sync，与热词模块统一架构**——`crates/vault/src/sync/mod.rs:30-41` 直接 re-export `octopus_sync::{git, outline, privacy, store}`，热词同步（`crates/sync/src/hotword.rs`）走同一套 outline + md5 增量协议。这意味着 vault 同步不是孤岛——隐私脱敏（PAT redact `SafeUrl`）、私有库守卫、HTTPS→SSH 改写、自动同步 scheduler 是 vault 和热词共用的基础设施，维护成本摊薄。

4. **加密工程严谨度高于同期开源项目**——`from_i64_strict` 的双向护栏（防弱 + 防 OOM，`kdf.rs:100-151`）、Zeroize 对 `GenericArray<U64>` chain code 的覆盖、密文移动攻击的显式威胁模型说明（`symmetric.rs:6-11`，单机假设下不实施 AAD 但讲清取舍）、cipher 协议对齐 Bitwarden 官方 UriMatchType 的回归守护（`types.rs:438-461`）——这种安全工程的自我审查深度在小型密码管理器里少见。

5. **中文 passphrase 生成器**——`generator/passphrase_zh.rs` 用 jieba 词频 TOP 4096 双字词，是 KeePassXC/gopass/Bitwarden 都没有的本地化特性。

### 9.4 Octopus 不足 / 缺失

1. **无浏览器扩展 / Native Messaging**——这是相比 KeePassXC-Browser（NaCl box 端到端加密，`passkeys-register`/`get-logins` 等协议动作）、Bitwarden/1Password 官方扩展的最大短板。Auto-Type 虽能填网页，但对 SPA 动态表单、隐藏 iframe、多步登录的鲁棒性远不如 content script 直插 DOM。spec 把浏览器扩展列为 P2（`2026-07-18-password-vault-design.md:36`），目前零代码。

2. **无 Passkey / WebAuthn**——这是 2026 年密码管理器赛道的**核心赛道**。Bitwarden 已支持 vault 内存 passkey（[Bitwarden passkeys blog](https://bitwarden.com/blog/tag/passkeys/)），KeePassXC-Browser 1.10.0+ 支持 passkey 注册/认证（`keepassxc-browser` 笔记 §三），Proton Pass 全力押注（[proton.me/pass](https://proton.me/pass)）。Octopus 完全空白，且 spec §0.3 明确「不实现 passkey 提供方」。中长期这是生存级风险——passkey 替代密码的趋势已定。

3. **无 YubiKey / 硬件二因素**——KeePassXC 把 YubiKey 挑战-响应作为数据库解锁第二因素（`keys/`，PC/SC + libusb），gopass 经 GPG/agent 间接支持。Octopus 解锁只靠主密码 + obfuscated machine-key 文件（`keychain.rs:14-30` 坦言这是 obfuscation）。对安全敏感用户这是硬伤。

4. **无 SSH Agent**——KeePassXC `sshagent/` 把加密数据库变成系统密钥链，对开发者是「密码管理器即基础设施」的关键。Octopus 用户群（开发者/极客）正是 SSH Agent 重度用户，这是高 ROI 缺口。

5. **无附件 / 加密文件存储**——KeePassXC 每条目可挂加密文件，Bitwarden/1Password 付费版都有。Octopus 完全没有（spec §0.2 未列入）。

6. **无共享 / 组织**——spec 明确单用户（`2026-07-18-password-vault-design.md:52-58`）。对个人工具合理，但堵死了团队场景（gopass 的收件人列表、Bitwarden 的组织集合都是成熟方案）。

7. **无 HIBP 在线泄露检查**——`health/` 只做本地 zxcvbn + 重复检测（`health/mod.rs:26-68`），不做 haveibeenpwned 查询。KeePassXC/gopass-hibp/Bitwarden 都内置。

8. **Auto-Type 平台覆盖不全**——只有 `macos.rs`，Windows（SendInput）和 Linux（XTest/Wayland）未实现。跨平台一致性差。

9. **Card/SecureNote/Identity 未实现**——`CipherType` 有四种值但 `CipherData` 只有 Login（`types.rs:161-166`），相比 Bitwarden/KeePassXC 的「全身份资料库」定位窄。

10. **Keychain 集成是 obfuscation**——`keychain.rs:14-30` 如实承认 K_machine 文件加密的派生输入全公开，等价于 0600 文件权限。adhoc 签名限制下的妥协，生产签名前是已知弱点。

### 9.5 建议改进方向

按 ROI / 战略紧迫度排序：

1. **Passkey 提供方（P0，战略级）**——这是 2026 密码管理器的生死线。建议分两步：① 先在 vault 数据模型加 `CipherData::Passkey` 变体（存 credential_id/public_key/private_key/sign_count），扩展 `types.rs:161-166`；② 实现 OS 级 passkey provider（macOS 用 `LocalAuthentication.framework` + 虚拟 authenticator，Windows 用 WebAuthn API）。可参考 KeePassXC-Browser 的 `passkeys-register`/`passkeys-get` 协议动作。即便不完整做 provider，至少先支持**存储** passkey（导入/备份场景），避免被 Bitwarden/ProtonPass 拉开代差。

2. **浏览器扩展 + Native Messaging（P1）**——Auto-Type 解决不了 SPA 表单和移动端。建议借鉴 KeePassXC-Browser 架构：octopus-desktop 起 native messaging host（stdin/stdout JSON），扩展侧用 TweetNaCl.js 建加密通道。工程量约 1500-2500 行（扩展 + 协议 + host 复用 vault crate）。优先级仅次于 Passkey。

3. **YubiKey 挑战-响应解锁（P1）**——`crates/vault/src/unlock.rs` 加第二因素分支：YubiKey HMAC-SHA1 挑战-响应派生额外 32B 与 master_root_key XOR/concat。Rust 生态有 `yubikey` crate。对开发者用户群价值极高，且与 SSH Agent 改造共享 FIDO2 库。

4. **SSH Agent 集成（P2）**——把 vault 变成系统密钥链。新增 `CipherData::SshKey`（或复用 Field 存 OpenSSH 私钥），desktop crate 加 `sshagent/` 模块实现 RFC 4252 agent 协议。Rust 有 `russh` 生态。对 Octopus 的开发者用户群，这能从「密码箱」升格为「密钥基础设施」。

5. **附件 / 加密文件存储（P2）**——vault 加 `attachments/<uuid>/<file_id>.enc`，复用 git sync 的 256 桶布局。cipher 表加 `has_attachments` 标志。注意 git repo 不适合大二进制（GitHub 100MB 单文件限制），需设计 LFS 或外部存储 fallback。

6. **HIBP 在线泄露检查（P2，低成本高感知）**——`health/` 加 `breach.rs`，调 HIBP range API（k-anonymity，只发 SHA1 前 5 字符）。UI 在健康报告里加「X 个密码已知泄露」红字。工程量 < 200 行。

7. **Card / SecureNote / Identity 补齐（P3）**——扩展 `CipherData` 三变体 + 前端表单。数据模型已预留（`types.rs:13-20`），主要是 UI 工作量。

8. **Windows / Linux Auto-Type（P3）**——port `macos.rs` 到 SendInput（Win）和 XTest（X11）/ wtype（Wayland）。enigo 已是跨平台库，主要是焦点检测和 URL 检测的平台分支。

9. **Keychain 切回真 OS Keychain（P3，依赖签名）**——一旦 octopus-desktop 拿到 Developer ID 签名，`keychain.rs` 的 K_machine 改回 macOS Keychain / Windows Credential Manager / Linux Secret Service，删除 obfuscation 文件路径（接口已抽象，调用方零改动，`keychain.rs:37-39`）。

10. **密码强度 audit 增强（P3）**——`zxcvbn` 已在用，可加自定义字典（用户邮箱、热词、历史密码）做针对性检查。

**战略小结**：Octopus Vault 的工程基础（Argon2id 双向护栏、Zeroize 卫生、git sync 复用）在同类里属于上乘，但**生态位卡在「Auto-Type 单点强，其余全面落后」**。Passkey 和浏览器扩展是必须补的两块，否则在 Bitwarden/ProtonPass 已把 passkey 当主赛道的 2026 会逐渐失去相关性。SSH Agent + YubiKey 是差异化机会——Octopus 的开发者用户群恰好是这两个功能的高价值人群，且与 octopus 主应用（终端/语音/热词工具链）的用户画像高度重合。

Sources:
- [Bitwarden Passkeys blog tag](https://bitwarden.com/blog/tag/passkeys/)
- [Bitwarden Secrets Manager Overview](https://bitwarden.com/help/secrets-manager-overview/)
- [Bitwarden 15M users milestone (Business Wire, 2026-07)](https://www.businesswire.com/news/home/20260728388256/en/)
- [KeePassXC 2.7.11 release notes (2025-11)](https://keepassxc.org/blog/2025-11-23-2.7.11-released/)
- [Proton Pass official](https://proton.me/pass)
- [1Password Travel Mode](https://1password.com/features/travel-mode)
- [KeePassXC-Browser communication protocol](https://github.com/keepassxreboot/keepassxc-browser/blob/develop/keepassxc-protocol.md)


---

## 10. 跨模块总结与战略建议

### 10.1 octopus 最深的护城河：跨模块 pipeline + 统一基础设施

把 9 个模块单独看，每一个相对领域 SOTA 都有差距（ASR 无说话人分离、截图无翻译 UI、剪贴板无加密、翻译无 glossary、Action Bar 仅 macOS、Terminal 无 SSH、录屏无实时转写、OCR 无 VLM、Vault 无 Passkey）。但把 9 个模块合起来看，octopus 有一个**任何单点竞品都无法复制的护城河**：

> **同一套 SQLite schema、同一套 ONNX Runtime 后端、同一套 `octopus-sync` git 同步、同一套 Tauri 命令/事件边界，让 9 个能力彼此之间形成短路径 pipeline。**

具体例子：

| Pipeline | 跨的模块 | 单点工具做不到的原因 |
|---|---|---|
| 语音 → 转写 → 翻译 → CompactEditor 双语对照 | ASR + Translation + ActionBar + CompactEditor | sherpa-onnx + Pot + 翻译扩展三个工具凑，配置三套 |
| 截图 → OCR → FTS5 全文搜索 → 剪贴板历史 | Capx + OCR + Clipboard + Infra | CleanShot + PaddleOCR + Maccy 三个工具凑，搜索不通 |
| Finder 选中文件 → Action Bar → agent CLI（`{{voice}}` 录音填占位符）→ 终端 | ActionBar + ASR + Terminal | PopClip + CapsWriter + iTerm 三个工具凑，无 voice 占位符 |
| 录屏 → 录后 ASR → LLM 润色字幕 → DB 入库 | Record + ASR + LLM + Infra | quickrecorder + Whisper + LLM CLI 三个工具凑，无统一 DB |
| 密码 cipher → Auto-Type 跨应用填充（macOS enigo + AppleScript 检测浏览器 URL） | Vault + Desktop | Bitwarden/1Password 完全没这个能力 |

**这是为什么「补齐单点 SOTA」不是 octopus 的最佳战略**——补齐 9 个 SOTA 等于做 9 个独立产品，维护成本指数爆炸。**octopus 的最佳战略是深化 pipeline 联动**：让现有的跨模块短路径更短、更自动、更鲁棒。

### 10.2 三条横切改进线（性价比高于单模块补齐）

**A. 整库加密基建（横切 Clipboard + Vault + Translation + sync 接入前置）**

- vault 已有成熟基建：`crates/vault/src/keychain.rs` HKDF-SHA256(machine_id || username || 常量) 派生 key + `vault_secret_access.rs` 的 `v1:` 透明加解密 chokepoint
- 翻译已复用：`models.secret_key` 的云端行以 `v1:` 加密（`translate.rs:100-103`）
- 剪贴板接入 sync 前必须加密，否则 git repo 明文外泄——这是触发整库加密的**真正时机**

→ **统一**：把 octopus DB 从明文 SQLite 切到 **sqlite3mc / SQLCipher 整库加密**（`rusqlite` 加 `bundled-sqlite3mc` feature，连接时传 key 即可，应用代码 0 行改动，FTS5 trigram 索引照常工作）。key 管理复用 vault 的 HKDF 派生方案。**一次性、全模块受益**：剪贴板历史 / vault ciphers / 翻译 secret_key / 热词 / action bar 配置全部入库即加密。在接入 sync（B 线）前完成此项，B 线就自动安全。**注意**：本地优先场景靠 FileVault/BitLocker + 0600 文件权限已覆盖威胁模型，此线**仅当 B 线启动时才必要**。

**B. octopus-sync git 同步扩展到 Clipboard（横切 Clipboard + Vault + Hotword）**

- vault + 热词已用 sync，唯独 clipboard 零依赖（grep 确认）
- 统一表好处是同步一张表即同步 text/voice/ocr/image/file 全类型——这是相对 EcoPaste/UniClipboard 的同步优势

→ **统一**：clipboard 接入 sync（必须做防回环），同步一张表全类型，相对竞品是降维打击。一次工程，剪贴板跨设备能力补齐 + 复用现有 sync 基建。

**C. Action Bar / Capx / OCR / Translation 的截图翻译闭环（横切 4 个模块）** ✅ 已实现（2026-08-11）

- ~~截图翻译 UI 缺失（架构文档明确「通路就绪」）~~ → 已实现
- ~~OCR→translate pipeline 缺失~~ → 已实现（`translate_screenshot` 命令）
- Action Bar 的 agent 类型已能启动任意 CLI
- ~~CompactEditor contrast 模式 + 流式翻译 emit 已就绪~~ → 改为 `translate_window` 只读浮窗 + `TranslateEmitTarget::Float` 分支（`emit_to` 定向）

→ **统一**（已落地）：①截图工具栏「翻译」按钮 → `translate_screenshot`；②ActionBar 选中翻译 → `auto_translate` 分支改走浮窗；③两处入口共享 `TranslateEmitTarget::Float` + `do_translate_streaming`。详见 [spec](../superpowers/specs/archived/2026-08-11-screenshot-translate-float-window-design.md)。**仍缺**：③Quick Access Overlay 第三入口 + OS 级划词翻译 + 屏幕翻译（贴图替换）。

### 10.3 单模块 P0 清单（按战略紧迫度）

| 模块 | P0 | 紧迫度原因 |
|---|---|---|
| Vault | Passkey 提供方 | 2026 密码管理器生死线，不补会被 Bitwarden/ProtonPass 拉开代差 |
| ASR | 词级时间戳 + SRT 导出 | 解锁会议/字幕/配音场景；与录屏的 `generate_subtitle` 联动收益翻倍 |
| ASR | 说话人分离 | 相对 sherpa/WhisperX/moss 最显著缺口；会议纪要必备 |
| Record | 录制中实时转写 | 差异化杀手锏；ASR 流式引擎基础已就绪，无竞品（Cap 是云端） |
| Translation | ~~glossary~~ + ~~OCR→translate pipeline~~（后者 2026-08-11 已实现） | 专业翻译痛点（glossary 仍缺）；截图翻译已完成 |
| Screenshot | ~~截图翻译 UI 接线（C 线）~~ ✅ 已完成（2026-08-11） | 已补齐，剩余贴图能力补齐 / Quick Access Overlay |
| OCR | PP-Structure 接入 + VLM OCR | 文档解析能力补齐；PaddleOCR-VL 1.6B 是同源最佳路径 |
| ActionBar | AX 直读选中文本 + 扩展注册中心 | 绕过 Cmd+C 剪贴板污染；启动生态 |
| Terminal | Atuin 集成 + russh SSH | history sync 是最显眼缺口；SSH 扩展运维场景 |

> **修订记录（2026-08-04）**：原 Clipboard 行「secret 检测 + 字段级加密」已删除——字段级加密破坏 FTS5 索引、运行时浮窗可见使加密 ROI 差，与竞品（Maccy/EcoPaste 默认明文）同档位。整库加密（sqlite3mc）降为 P2，**绑定 sync 接入时机**（同步前才必要）。concealed type 检测降为 P3（零成本好公民）。详见 §3.5 修订记录。

### 10.4 octopus 不应该追的方向

- **自研 GPU 渲染终端**（Alacritty/Ghostty 路线）—— xterm.js WebGL 已够用，ROI 极低
- **WASM 插件系统**（Zellij/OxideTerm 路线）—— 与「ASR 工具集」定位不符，维护成本高
- **300+ 主题系统** —— signature 深色 `#0c0c0f` 是有意的，不必追
- **追平 sherpa-onnx / transcribe.cpp 的引擎数** —— 维护 N 个引擎的成本远高于补齐 pipeline 联动
- **追平 PixPin 的贴图功能清单** —— 抄核心 5 项（透明度/锁定/方向键/批量/取色）即可
- **追平 CleanShot Cloud / Cap 分享** —— 与本地优先定位冲突，BYOS（S3/R2）可作为 P3

### 10.5 总体判断

**octopus 的核心价值不在「9 个模块都做到领域 SOTA」，而在「9 个模块彼此之间形成任何单点工具都无法复制的短路径 pipeline」。** 战略上应该深化联动（三条横切改进线 + Action Bar 全栈调度）而非追赶每个领域的功能清单。

工程纪律（spec 驱动、迁移链、bench、Zeroize 卫生、WKWebView 稳定性细节、坐标系 gotcha 等）是 octopus 的隐形资产，tolaria 知识库里的多数竞品在工程深度上都不及 octopus——但这种工程优势不直接转化为用户感知，必须通过**面向用户的 pipeline 联动**显性化（截图翻译、实时转写字幕、跨应用 Auto-Type、agent + voice）才能变成差异化卖点。

---

## 附录 A. 调研方法与可复现性

- **数据来源**：`/Users/wudarui/.tolaria/` 个人知识库（截至 2026-08）+ octopus 主仓 `crates/` 源码（HEAD = `8322cb94`）+ 必要的 web 验证（每节末尾列 Sources）。
- **方法**：主代理建立 module→docs 索引后，9 个并行子代理各负责一个模块，按统一模板（现状 → 对比矩阵 → 独特价值 → 不足 → 改进方向）独立产出。每个子代理同时深读 octopus 源码（带 file:line 锚定）和 tolaria 对应文档。
- **可复现**：worktree `research/tolaria-comparison`（`.worktrees/research-tolaria-comparison`）保留全部产出。如需重跑某个模块的调研，subagent prompt 见主代理对话日志。
- **局限性**：① web 研究限制在每模块 2-4 次搜索，部分 2026 新功能可能漏检；② tolaria 知识库是个人维护，覆盖面偏向作者兴趣（macOS / Rust / 离线 / 本地优先），Windows 生态（STranslate / PixPin）覆盖较弱；③ octopus 是私有仓，社区活跃度无公开数据，对比时只能标「内部项目」。
- **更新策略**：建议每季度回看一次（SOTA 变化最快的是 OCR VLM、Passkey、Whisper Large），重点模块（ASR / Vault / OCR）单独刷新。

## 附录 B. 关键外部参考（按模块）

- ASR: sherpa-onnx (`sherpa-onnx-新一代-kaldi-语音工具集.md`) / transcribe.cpp / transcribe-rs / CapsWriter / FireRedASR2S / YuHuang
- Screenshot: CleanShot X / Snapzy / eSearch / PixPin / HushSnap
- Clipboard: CopyQ / Ditto / Maccy / EcoPaste(+Pro/Sync) / UniClipboard / ortu / VloamClip / Paster
- Translation: kiss-translator / Pot / esearch / Paster + Picovoice/Meta NLLB-200 web 资料
- Action Bar: PopClip / SnipDo / eSearch / Raycast
- Terminal: Alacritty / WezTerm / Ghostty / Zellij / wterm / Codux / OxideTerm / Atuin
- Record: quickrecorder / openscreen / CleanShot / Snapzy / Cap / screenity / RecEasy / esearch
- OCR: ocrs / RapidOCR / Umi-OCR / surya / MonkeyOCR v2 / OvisOCR2 / OpenOCR / deepseek-ocr-rs / tr / unlimited-ocr
- Vault: KeePassXC / KeePassXC-Browser / gopass / Bitwarden / 1Password / vaultwarden / Proton Pass

---

**报告完。** 共 9 个模块 + 总览 + 战略建议 + 附录，约 12000 字。
