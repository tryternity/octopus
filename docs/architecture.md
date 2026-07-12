# 架构概览

octopus 是一个基于 ONNX Runtime 的语音识别（ASR）工具集，支持多种 ASR 引擎和多种使用方式。

## 项目结构

```
octopus/
├── crates/
│   ├── infra/       # 基础设施 (octopus-infra)
│   ├── asr-local/   # 核心推理库 (octopus-asr-local)
│   ├── asr-cloud/   # 云端 ASR 协议层 (octopus-asr-cloud)
│   ├── clipboard/   # 剪贴板历史管理 (octopus-clipboard)
│   ├── ocr/         # OCR 图片识别 (octopus-ocr)
│   ├── capx/        # 屏幕截图 (octopus-capx)
│   ├── llm/         # LLM 润色 (octopus-llm)
│   ├── cli/         # 命令行工具 (octopus-cli)
│   ├── server/      # HTTP/WebSocket 服务 (octopus-server)
│   ├── desktop/     # Tauri 桌面应用 (octopus-desktop)
│   ├── download/    # 模型下载 (octopus-download)
│   └── dlp/         # 视频音频下载 (octopus-dlp)
├── docs/            # 文档
└── usage.md         # 快速使用指南
```

## 模块说明

### octopus-infra（基础设施）

无项目内依赖的最底层 crate，承载跨 crate 共享的基础设施：`consts`（固定路径常量：VAD 模型 / 默认 ASR 模型目录）+ `paths`（`octopus_config_home()` 返回 `~/.octopus`，三端统一）+ `config`（`AppConfig`——应用配置统一 schema）+ `db`（SQLite 嵌入式存储，含 `app_config` 表 / `models` 表 / `prompts` 表 / `clipboard_history` 表（统一存储 text/voice/ocr/image/file，吞并原 `transcriptions` 表）+ FTS5 虚表 / `image_data` 表）+ `net`（网络超时常量：WS/HTTP/gRPC/下载，各 crate 统一引用避免散落不一致）。DB schema 当前 v25（v17→v18：FTS5 backfill；v18→v19：action_bar_items 表；v19→v20：hotwords 表 + paste_input_source_switch；v20→v21：action_bar_items 加 is_async/write_output_to_clipboard 列 + script_runs 表；v21→v22：env 变量 seed；v22→v23：hotword_sets + hotword_hits + db.sql seed 默认「通用」版本；v23→v24：action_bar_items 加 shortcut 列；v24→v25：seed 新增「问豆包」菜单项）。**开发期简化**：`init_schema` 以 db.sql 为唯一 schema 真相——`user_version >= 25` 跳过，v17+ 库跑增量迁移升到 v25（FTS5 backfill + ALTER TABLE 补列 + env seed + v22→v23 热词多版本迁移 + v23→v24 shortcut 列 + v24→v25 问豆包 seed），其他（含全新库）跑 db.sql 建表+seed+yaml 导入一次性到 v25，无 DROP 兜底（开发期无旧库需兼容）。`ensure_db` 打开后设 `PRAGMA journal_mode=WAL` + `busy_timeout=5000`（多任务并发友好，server 多连接不再 SQLITE_BUSY）；`save_app_config` 30 字段写入包 `unchecked_transaction`（原子，中途崩溃全回滚，不再配置半更新）。voice 历史搜索走 FTS5 MATCH（trigram 倒排索引，>=3 字符），<3 字符回退 LIKE（trigram 无法生成 3-gram）。`with_db` 为公开 API 供其他 crate 调用。**并发约束**：`with_db` 内部用 `parking_lot::ReentrantMutex`（**同线程可重入**，无毒化）——闭包内可安全地再调 `with_db`（如间接读 config / 查模型 meta），不再有历史 `Mutex`（非递归）期的同线程重入死锁（arch-fixes ③，2026-07-06）；回归测试 `with_db_reentrant_no_deadlock` 守护，退回 `Mutex` 会挂起。仍为单连接排他（性能上限，非死锁），server 上量再上 r2d2 池。

### onnx-infra（ONNX 推理基础设施）
无项目内依赖的最底层 ONNX 公共设施，从 asr-local 抽取。`paths` 模块提供模型路径查找（`resolve_model_dir`：4 级查找 `~/.octopus/models/` → 绝对路径 → HF cache snapshots）；`session` 模块提供 `apply_session_acceleration`（按平台注册 CoreML/CUDA/DirectML EP，generic 化 `skip_coreml` 参数）。asr-local 和 translation 都依赖此 crate。

### octopus-translation（本地翻译引擎）
本地翻译引擎，双引擎架构：**Opus-MT**（MarianMT，~30M/方向，中英互译，轻量快速）+ **m2m100-418M**（ONNX int8，100+ 语言互译）。`TranslationEngine` trait + 全局 HashMap 缓存（m2m100 按 spec 缓存，opus-mt 按 spec+方向缓存）。`M2M100Engine` / `OpusMTEngine` 分别实现 encoder-decoder greedy 解码（HF `tokenizers` crate）。**Opus-MT greedy 防重复**：repetition_penalty=1.3 + no_repeat_ngram_size=3（MarianMT 训练用 beam search，greedy 易陷入模式重复循环），penalty 逻辑抽为纯函数 `apply_penalties`（6 单测守护边界，防 off-by-one 越界回归）。encoder 输入 `encode(text, true)` 让 post_processor 自动补 `</s>`，超长 `truncate` 兜底。**引擎选择优先级**（自动模式）：opus-mt（轻量优先）→ m2m100 → LLM。`translate_engine` 配置：`""` = 自动，`"local:opus-mt"` / `"local:m2m100-418M"` = 指定本地，`"llm"` = 强制 LLM。**Opus-MT 模型目录** `~/.octopus/models/translate/opus-mt/{zh-en,en-zh}/`（一组模型，两个方向子目录各含 encoder/decoder int8 ONNX + tokenizer.json）。**tokenizer 修复**：Xenova 导出的 tokenizer.json `precompiled_charsmap=null` 致 tokenizers 0.21 panic，加载时删除 `normalizer` 字段规避。**流式翻译**（2026-07-12）：`do_translate_streaming` 按换行切分段落逐段翻译，emit `translate-progress`（增量）/`translate-done`（完成），CompactEditor 前端 listen 实时更新译文区。详见 [spec](superpowers/specs/2026-07-12-translation-bilingual-view-design.md) §9。

### octopus-asr-local（核心推理库）

ASR 推理的核心库，所有上层组件都依赖它。

| 模块 | 说明 |
|------|------|
| `config` | DB 模型配置加载（`AsrConfig`）、模型发现、引擎路由（`resolve_engine_in_config` 按 `{provider}:{category}:{model_name}` 3-part spec 解析）、全局默认引擎兜底（`resolve_active_engine`）、云引擎分类（`EngineCategory::Aliyun` / `EngineCategory::ByteDance`，由 `resolve_category` 按 provider 分支识别） |
| `feature` | 共享特征提取设施：mel filterbank（mel 空间权重，参数化 high_freq）、apply_lfr、hz_to_mel/mel_to_hz、hamming/povey 窗口。抽取自 paraformer/fbank/zipformer 三处重复实现，统一正确性（C1 修复） |
| `audio` | WAV 读取、重采样（`resample_to` 一次性 / `AudioResampler` 流式，支持任意 from→to 速率，含 denoise 48k 桥接）、VAD 语音过滤 |
| `denoise` | 可插拔流式环境降噪后端（`FrameDenoise` trait，由 `denoise_mode` 选择）：`1`=RNNoise（`nnnoiseless`，纯 Rust 移植 Xiph RNNoise，内置默认模型，48kHz/FRAME_SIZE=480→频带特征+VAD/噪声/降噪 GRU→频带增益+OLA，GRU 状态跨帧保持）/ `2`=DeepFilterNet3（`Df3Backend` 包装 libDF v0.5.6 的 `DfTract` + tract 0.19，48kHz 全频带）。`DenoiseProcessor` 为 mode 分发器，采集层前置 |
| `vad` | Silero VAD 语音活动检测 |
| `whisper` | Whisper 离线识别（int8 三件套优先：encoder + dec_init + dec_past；decoder 层数 / D_MODEL 从 session 输出动态获取，支持 tiny/base/small 等不同规模模型（但仅 whisper-small.en 识别质量可用，tiny/base 经实测不可用故不入 seed）；**不支持 Large v3 / Turbo**——这些变体使用 128 mel bins，引擎硬编码 `N_MELS=80` + 静态 80×201 filterbank，`WhisperEngine::new` 加载时校验 encoder 输入维度，遇到 128 mel 会提前 fail 给出明确错误而非踩 ONNX shape mismatch 崩溃；auto-language 两步式检测：先喂 `[sot]` 预测语言 token，再拼完整 `[sot, lang, transcribe, no_ts]` prompt；**特殊 token 强制查询**——各变体 token ID 不同（.en 整体偏移 -1），`token_to_id` 用 `ok_or_else(bail!)` 强制查询，失败立即报错而非静默 fallback 到错误的 multilingual ID；**短音频提早结束**——compute_mel 会把音频 0 填充到 30s，若 VAD 只传入 2s 片段剩余 28s 为静音，原硬编码 `max_tokens=448` 会让模型在静音段幻听（重复最后一句话 / “谢谢观看”），现按实际音频时长 `max_tokens = (seconds × 6 + 10).min(448)` 动态限制解码步数，.en 模型平均 ~6 text tokens/秒；**Mel 频谱 center=True reflect 填充**——与 OpenAI `torch.stft` 默认行为一致，frame t 覆盖 `[t×hop - n_fft/2, t×hop + n_fft/2)`，左右越界样本按 PyTorch `pad_mode="reflect"` 反射填充 200 采样，使 frame 0 中心对齐 sample 0，避免整个时间轴偏移 12.5ms 影响首音节识别） |
| `sensevoice_orig` | 原版 SenseVoice-Small 离线识别（FunASR 4 输入 ONNX：`speech`[560维 LFR m=7/n=6]+`speech_lengths`+`language`(按 config.language 映射 FunASR id:auto=0/zh=3/en=4/yue=7/ja=11/ko=12)+`textnorm`(1=无itn)；**fbank 预处理（2026-07-09 修）**：`compute_fbank`（与 firered 共用）含 DC offset removal + pre-emphasis(0.97)，对齐 kaldi_native_fbank 默认——am.mvn 基于含这两步的特征统计，此前缺它们致真实音频乱码（合成音频落在模型鲁棒区侥幸通过）；**CMVN 必须外部应用**——读 `am.mvn`（kaldi `<AddShift>`+`<Rescale>` 各 560 维）在 LFR 后做 `(feat+addshift)*rescale`，FunASR `WavFrontend` 标准 fbank→LFR→CMVN 不进 ONNX 图、`config.yaml` 的 `cmvn_file:null` 是训练残留字段；vocab=25055、blank=0；tokens.json 为 string 数组；中/英/粤/日/韩。**`skip_corrector=false`**（2026-07-10 改回）：corrector 已重构为「有界热词纠错」——候选仅来自用户热词表 `HotwordIndex`，无热词即 no-op、只纠显式热词，过纠根因消失，故高质量模型也经热词纠错受益。历史过纠教训（旧全词典 n-gram 把"开始语音识别"误纠成"开始于饮食别"）即有界重构的起因；诊断教训仍有效：直调 `engine.transcribe()` 的 e2e 绕过 pipeline 的 corrector 会掩盖纠错效果，须走完整 pipeline。**sherpa nano 简化版（旧 category=`sensevoice`）已移除**，见 [docs/removed-sensevoice-sherpa-nano.md](removed-sensevoice-sherpa-nano.md) |
| `paraformer` | Paraformer 离线识别（fbank: hamming 窗 + DC offset + pre-emphasis） |
| `qwen3_asr` | Qwen3-ASR 离线识别 |
| `zipformer` | Zipformer 离线识别 |
| `moonshine` | Moonshine 离线识别（纯 ONNX 4-session 流水线：preprocess → encode → uncached_decode → cached_decode 循环 + KV cache；英语） |
| `firered` | FireRedASR2-AED CTC 离线识别（小红书 FireRedTeam；单 ONNX `model.int8.onnx`(740M) + `tokens.txt`(vocab=8667)；encoder+CTC branch 导出、弃 attention decoder；80-bin fbank 复用 `fbank::compute_fbank`（无 LFR，含 DC offset + pre-emphasis 0.97 + povey 窗，对齐 FireRedASR 训练 knf 默认——2026-07-09 经 `data/asr_feat.py` 确认）+ CMVN 从 ONNX metadata `cmvn_mean`/`cmvn_inv_stddev` 读、公式 `(fbank-mean)*inv_std`；greedy CTC blank=0；中文+20方言+英） |
| `streaming_paraformer` | Paraformer 流式识别（增量式 fbank: povey 窗 + DC offset + pre-emphasis + 跨帧状态） |
| `streaming_zipformer` | Zipformer 流式识别 |
| `corrector` | 有界热词纠错（候选仅来自 HotwordIndex，命中即替换，消灭全词典过纠）+ 可配方言模糊规则 `FuzzyRules`（f/h、hu/wu、n/l、r/l，存 `app_config.fuzzy_dialect`） |
| `hans` | 简繁体字形转换（单字级，开放词典网 CC-BY 3.0 对照表编译期嵌入）；按 `output_simplified` 归一化 ASR 输出 |
| `pipeline` | 批处理 pipeline 编排（阶段1 新增）：`PipelineConfig`（language/correct/simplify/ngram）+ `transcribe_batch`（VAD 分段 → 逐段转写 → 纠错 → 简繁归一化，收编自 `transcribe_with_vad`，纠错/简繁参数化为 cfg 字段）；`transcribe_with_vad` 退化为从 app_config 构造 cfg 的薄包装（desktop 向后兼容）；cli 经 `AsrEngineManager::transcribe_batch` 复用同一编排 |
| `streaming_engine` | 流式 ASR 引擎抽象：`StreamingSession`（Paraformer / ZipformerCtc / ZipformerTransducer 增量流式，`&self` + 内部 Mutex）；阶段2a impl `StreamingEngine` trait |
| `streaming_runner` | 流式编排 helper（阶段2a 新增）：`StreamingRunner`（持 `Box<dyn StreamingEngine>` + VAD + 静音/标点状态）收编 VAD 静音检测 + 标点触发 + accept/flush/finish；`TranscriptEvent`（Partial/Committed/Final/Error）+ `StreamingEngine` trait（local `StreamingSession` impl，cloud 2c-2）。denoise/resample 不入（留 `audio.rs`，输入为已降噪 16k 样本） |


**数据流（离线）：**
```
音频文件/WAV → read_wav_16k → [VAD 过滤] → 引擎.transcribe → 文本
```

**数据流（流式）：**
```
麦克风 → PCM chunk → resample_to_16k → 引擎.accept_samples → [partial]
                                    └─ 静音≥0.5s → 引擎.flush(insert_comma=true)（padding 冲刷尾音 + 即时逗号）→ [partial]
                                                              → engine.finish → [final]
```

### octopus-asr-cloud（云端 ASR 协议层 + 批引擎）

云端 ASR（Aliyun/ByteDance/Tencent/Baidu 4 provider）WSS 协议层 + 批引擎，cli/server 批处理转译音频文件可选云端 API（不必只靠本地 onnx）。依赖 `octopus-asr-local`（**单向**，asr 不依赖 cloud，避免循环——本地/云端分流在 cli 层）。

| 模块 | 说明 |
|------|------|
| `cloud_types` | `PcmFrame`/`StreamEvent`/`CloudStreamHandle`（`new`/`push_pcm`/`finish`/`close_async`，迁自 desktop；`CLOUD_CLOSE_TIMEOUT_SECS=8`）|
| `aliyun_stream`/`bytedance_stream`/`tencent_stream`/`baidu_stream` | 4 provider WSS 协议层（建连/鉴权/帧编解码/WS 循环），1:1 复刻 desktop `*_stream.rs`，仅改造 `open()`：去 `tauri::async_runtime::RuntimeHandle` 参、`rt.spawn`→`tokio::spawn`；协议字节级零差异，单测随源文件搬入 |
| `config` | `resolve_*_config`（从 `AppConfig.asr.{provider}` 取 `ModelEntry` 校验 `secret_key`）+ `open_cloud_session(spec, lang, pre_roll) -> anyhow::Result<CloudStreamHandle>`（按 `resolve_engine_category` 分发到 4 provider `open`）|
| `batch` | `CloudBatchEngine impl OfflineAsrEngine`：`from_spec`（3-part 云端 spec 校验 + 建 tokio runtime，不查 DB/不连网）；`transcribe` = 单段→单 WSS session→完整文本（`block_on`：open + 分块 push_pcm + close_async，分段由上层 `transcribe_segments` 自动完成）；`skip_corrector()=false`（有界热词纠错安全，2026-07-10 改回）；`is_cloud_spec`（`parse_model_spec` 3-part provider 前缀判云端，不查 DB）|

> desktop 协议层副本已删（2026-06-25 cloud-dedupe，ff-merge main `a16c98f`）：删 `*_stream.rs`×4 + `cloud_types.rs`（1868 行），`CloudPipelineEngine`（`cloud_pipeline.rs`，desktop pipeline 壳保留）+ `pipeline.rs` trait + `engine_aliyun.rs` 改指 `octopus-asr-cloud` 协议层，协议层单源。

### octopus-cli（命令行工具）

通过 clap 提供 6 个子命令：

| 命令 | 说明 |
|------|------|
| `devices` | 列出可用麦克风 |
| `config` | 显示模型发现信息 |
| `transcribe` | WAV 文件离线识别；`--model` 支持 3-part 云端 spec（`provider:category:model_name`，provider=aliyun/bytedance/tencent/baidu）→ `octopus-asr-cloud` 的 `CloudBatchEngine`（WSS，`skip_corrector=false` 有界热词纠错安全），否则本地 onnx；两端都经 `asr::pipeline::transcribe_batch`（VAD 分段 + 纠错 + 简繁）。分流在 cli 层（`pipeline::run` 用 `is_cloud_spec`） |
| `e2e` | 麦克风实时识别（离线/流式） |
| `stream-test` | WAV 文件流式识别测试 |
| `download` | 从 HuggingFace 下载模型到 `~/.octopus/models/<repo>/`（薄封装 `octopus-download`：`--include`/`--exclude` glob 过滤、`--mirror` 镜像；镜像优先级 `--mirror` > config `download_mirror` > 官方源） |

### octopus-server（HTTP 服务）

基于 Axum 的 Web 服务，提供 REST 和 WebSocket 接口。两条 ASR 路径均走 asr helper（阶段3，2026-06-26）：批处理 `/transcribe` → `AsrEngineManager::transcribe_batch` + `PipelineConfig`（VAD 分段 + 纠错 + 简繁）；流式 `/ws/stream` → `pipeline.rs::WsStreamSession`（薄包 `StreamingRunner`，VAD 静音/标点内部收编）→ `event_to_json` 回推 `TranscriptEvent` `{type,text}`。`pipeline.rs`（WS↔runner 桥接 + 序列化，纯逻辑可单测）+ `main.rs`（路由 + WS/HTTP 胶水）。**安全加固（2026-07-05）**：默认绑定 `127.0.0.1`（非 0.0.0.0）、CORS 同源策略、100MB body limit、ASR 推理经 `spawn_blocking` 不阻塞 event loop、`/transcribe` 经 `get_engine` 取引擎 `Arc` 直接转写（不改全局 active，arch-fixes ① 2026-07-06）——同模型并发受引擎内 `Mutex<Session>` 串行化、跨模型天然并行，不再需要全局 `inference_lock`；`AsrEngineManager::new_with_capacity(8)` 放大缓存适配多模型、`serde_json` 安全转义（含控制字符）、SIGTERM/Ctrl-C graceful shutdown。

```
Client ──HTTP POST──→ /transcribe ──→ transcribe_batch（asr::pipeline）──→ JSON 响应
Client ──WebSocket──→ /ws/stream  ──→ WsStreamSession(StreamingRunner) ──→ {type,text} JSON
```

### octopus-clipboard（剪贴板历史管理）

独立的剪贴板历史核心库，仅依赖 `octopus-infra`。基于 `clipboard-rs`（跨平台剪贴板读写 + 监听），替代了原来的 `tauri-plugin-clipboard-manager`。

| 模块 | 说明 |
|------|------|
| `model` | 数据结构：`ItemType`（Text/Image/File）/ `Source`（Clipboard/Asr）/ `ClipboardItem`（含 `ImageMeta`/`FileMeta`/`AsrMeta`）/ `QueryFilter`（6 种过滤 + 分页 + 搜索）|
| `handle` | `ClipboardHandle`：`Mutex<ClipboardContext>` 全局单例（Windows 防锁竞争）+ `AtomicBool` suppress flag（区分 ASR 写入与外部复制，watcher 跳过自身写入）+ `AtomicBool` recording_enabled（`clipboard_enabled` 运行时镜像，false 时 `on_clipboard_change` 不入库；`set_config` 热重载翻转；`main.rs` setup 启动时按 DB 值 `set_recording_enabled` 一次性同步——`new()` 默认 true，否则重启会复活已关闭的监听） |
| `watcher` | `ClipboardWatcher`：后台线程跑 `ClipboardWatcherContext::start_watch()`（阻塞），`on_clipboard_change` 回调依次检查 suppress flag → recording_enabled gate（`clipboard_enabled`=false 则直接 return，不存库不 emit）→ 判断类型（files > image > text 优先级，非三者则静默跳过避免 `read_text` 失败日志污染）→ 去重 → 存 DB → 通知前端 |
| `store` | DB CRUD：`insert_clipboard_item`（通用插入，按 NewClipboardItem 结构体）/ `insert_asr_item(conn, text, engine, model, segments)`（item_type='voice'，meta_info={engine,model,char_count}）/ `insert_ocr_item(conn, text, engine, model)`（item_type='ocr'，meta_info={engine,model,char_count}）/ `query_history`（FTS5 `clipboard_history_fts` JOIN 搜索：查询 ≥3 字符包成 phrase 走 trigram 索引 MATCH，<3 字符 LIKE 子串 fallback；+ 6 种过滤 + 分页 + `ORDER BY created_at DESC, id DESC` 二级排序消除秒级 `iso_now` 同秒不稳）/ `get_item_by_id`（按 id O(1) rowid 读单条）/ `count_history` / `toggle_favorite` / `delete_item`（+ image_data 引用计数回收）/ `delete_items`（批量删除）/ `clear_history`（保留收藏）/ `rebuild_fts_index`（FTS5 索引重建，仅启动时一次性 populate；删除路径由 db.sql 触发器 `clip_fts_ad` 增量同步，无需周期 rebuild）/ `insert_image_data` / `get_image_blob` / `get_image_thumb` / `cleanup_unreferenced_images` + 去重（`find_by_content_hash` 图片 / `find_by_text(text, ItemType)` 文本+文件按类型匹配） |
| `image` | RGBA → PNG → SHA-256 去重（`encode_and_hash`）+ WebP 编码（`webp` crate）+ 缩略图 240×240（Triangle）。`encode_to_webp` 接 `&DynamicImage`，复用调用方已解码的像素。编码降级链由 `consts::IMAGE_SAVE_QUALITY`（`"webp:80;jpeg:80"`，`;` 分割、`:` 解析格式与质量，依次尝试首个成功）驱动：正常尺寸先 lossless WebP，失败后走降级链；超尺寸（>16383px，VP8 上限）跳过 lossless 直接进降级链；每次编码 `catch_unwind` 兜底防超大图 panic（watcher 从剪贴板 RGBA 构造 DynamicImage；screenshot/migration 复用 `load_from_memory` 的结果）。图片 BLOB 存 DB `image_data` 表（不再写文件系统）。`ImageMeta.size` 经 `(SELECT length(blob) FROM image_data WHERE hash = blob_hash)` 子查询算（query_history / get_item_by_id / LIKE / FTS5 四处 SELECT 同步），供列表显示存储大小 |
| `cleanup` | 自动清理：按天数（默认 30）+ 按数量（默认 1000）删除非收藏记录 + 孤立 blob 回收 + FTS5 索引重建（仅在有删除/回收时 rebuild，避免定时清理无删除时无谓全表重建）。**已接入定时调用**：`main.rs` setup 启动时跑一次（image_migration 迁入旧图片后）+ 后台线程每小时从 DB 重读 `clipboard_max_items` / `clipboard_max_age_days` 跑一次（让设置页「最大保留条数 / 自动清理天数」真正生效；用户运行时改限额 1 小时内自动生效）；另有 FTS5 索引维护（启动 rebuild + 删除计数达 10 自动 rebuild，见 store.rs） |

**监听机制（clipboard-rs 内置）：** macOS 轮询 `NSPasteboard.changeCount`（500ms）；Windows 事件驱动 `AddClipboardFormatListener`；Linux X11 XFixes 事件驱动；Linux Wayland 两级轮询（MIME 类型 + text 内容，500ms）。

**ASR 集成：** `coordinator.rs::do_paste` 中先调 `store::touch_created_at`（录音过程 `insert_transcription_at_id` 已在 clipboard_history 创建 voice 条目，paste 时只需 touch 顶到列表顶部，不重复创建），**成功后主动 `emit("clipboard://changed")`**（`paste::paste` 写剪贴板设 suppress flag，watcher 的 `on_clipboard_change` 命中后直接 return、不执行含 emit 的 `on_change` 闭包，故 ASR 记录需主动广播前端才能即时渲染），再调 `paste::paste`（写剪贴板，suppress flag 阻止 watcher 重复记录）。**删除已统一**：transcriptions 表已废弃（v17 DROP），所有 ASR 数据在 clipboard_history（item_type='voice'）；`delete_history` 直接调 `delete_transcriptions`（已写 clipboard_history），删除行数 >0 时主动 `emit("clipboard://changed")` 广播浮窗/设置页双端刷新。

**DB 表（schema v22）：** `clipboard_history`（统一存储 text/voice/ocr/image/file——`item_type` 枚举区分；`content`（voice/ocr/text 全文，image/file 为空串）+ `ref_data`（image=blob_hash，file=JSON 路径数组）+ `meta_info`（JSON，按 item_type 存不同 schema：image `{w,h,size}` / voice `{engine,asr_mode,char_count,polished,polish_model}` / ocr `{engine,model,char_count}` / text `{char_count}` / file `{files:[{size,type}]}`；序列化时 None 字段跳过不写 null）+ `segments`（仅 voice 段 JSON）+ `is_favorite`/`is_rich`/`created_at`/`has_thumbnail`）+ `clipboard_history_fts`（FTS5 虚表，trigram tokenizer，索引 `content` 列——voice/ocr/text 被搜索，image/file content 为空串自动跳过）+ 3 触发器自动同步（`clip_fts_au` 收窄为 `AFTER UPDATE OF content`）+ `image_data`（图片 BLOB 存储：hash/blob/thumb/image_type/width/height/created_at）。v17 废弃 `transcriptions` 表（db.sql 不再含此表）。**FTS5 索引维护**：启动时 rebuild 一次 + 运行中删除计数器达 10 自动 rebuild。**图片存储**：WebP 无损 BLOB 存 `image_data` 表，`clipboard_history.ref_data` 引用 `image_data.hash`；删除条目时引用计数为 0 才删 image_data 行。

### octopus-ocr（OCR 图片识别）

独立的 OCR crate，依赖 `octopus-infra` + `octopus-paddle-ocr`（vendor 自 paddle-ocr-rs，ONNX Runtime 推理后端）。封装 PaddleOCR pipeline（det→cls→rec）。支持 PP-OCRv5 + PP-OCRv6-small（DB config 按 model_name 选择）。

| 模块 | 职责 |
|---|---|
| `engine` | `OcrEngine`：全局单例（`OnceLock`），懒加载模型。`recognize(image_bytes)` 支持任意格式（image crate 自动检测）；`recognize_with_blocks_from_image(&DynamicImage)` 接受已解码图像，`ocr_screenshot` 解码一次传 save + OCR 共用（消除双重解码，2026-07-09）。模型名从 `app_config.ocr_model` 读取（默认 PP-OCRv6-small）。`instance()` 用 double-checked locking（`INIT_LOCK: Mutex<()>`）串行化首次加载、保证模型只加载一次。内部 `Mutex<Option<RapidOcr>>` 提供可变性（`run` 需 `&mut self`；`None` = idle 60s 已释放、下次 `run_ocr` 自动重载并补 `probe(Before/After)`——After 命中 registry `estimated` 首次缓存值恢复 active 条目，避免状态页在重载后永久缺 OCR（旧实现重载不调 probe，2026-07-08 修），详见下方「系统状态页」② OCR idle 释放），OCR 全局互斥保证无竞争。**超长图切分**：`recognize` 对 `height > 1600`px 的长截图按块切分（`CHUNK_HEIGHT=1280` / `CHUNK_OVERLAP=200`）逐块识别 + 按绝对 y 坐标去重（`drop_overlapped_blocks`：记已收录行最大 y 底部 `covered_until_y`（取 chunk 内 `fold max` 而非 `blocks.last().y+h`——det 框按 y 中心排序时末尾矮行底边非最大，极端混排少记致重复行，2026-07-08），下一块中 y 中心 ≤ 该值的行落入 `CHUNK_OVERLAP` 重叠区、已被前块收录而丢弃；2026-07-07 由「文本逐字相等去重」改为坐标去重——OCR 轻微波动如 `hello` vs `hello!` 致文本不等、去重失败，且易误删天然重复行）。**全局并发互斥**：`OcrLockGuard`（`static OCR_BUSY: AtomicBool` + `compare_exchange`）做 RAII 互斥。**后处理**：`merge_same_line_blocks`（det 同行多框合并 + 间隙补空格）+ `segment_english_words`（17.7K 英文词库 `words_common.txt` 贪心最长匹配分词，min_len=1 可匹配 1-2 字母短词；仅 PP-OCRv5 需要——v5 CTC 不输出英文空格；v6 CTC space token 已激活，`use_word_segmentation` 按 model_name 前缀判断跳过）+ `to_markdown`（布局感知，见 `layout` 模块） |
| `layout` | **布局感知 Markdown 输出（2026-07-09）**：`to_markdown(blocks) -> String` 在 `run_ocr`（merge + segment）之后执行，替代原 `join("\n")`。消费 det 框几何信息输出结构化 Markdown：**标题**（框高 / median_h ≥1.6→H1 `#`，≥1.3→H2 `##`）；**列表**（文本前缀匹配 `•`/`-`/`①`/`1.`/`1、`/`1．`/`1）` 等标记，有序统一重编号 `1. 2. 3.`，连续列表项大间距（>median_h×0.8）时回车重编号）；**段落**（连续 Body 行垂直间隙 >median_h×0.8→新段落，段间 `\n\n`）；**多行正文**（同段 ≥2 行用 code fence ` ``` ` 包裹保留原始分行，不 reflow；内容含 ` ``` ` 时加长围栏 ` ```` ` 避免嵌套）；**单行正文**（直接输出）。常量起始值：`MIN_BLOCKS_FOR_LAYOUT=3`、`TITLE_H1_RATIO=1.6`、`TITLE_H2_RATIO=1.3`、`PARAGRAPH_GAP_RATIO=0.8`。块数 <3 不分析布局。`recognize` / `recognize_with_blocks` 返回 String 语义从扁平文本变为 Markdown，消费端（DB content / CompactEditor / AI 输入）零改动受益。前端 ImagePreview 叠加不受影响（blocks 仍是原始 det 框）。 |
| `model` | 模型路径管理（`~/.octopus/models/ocr/<name>/`）+ `is_model_ready`（det.onnx + rec.onnx + keys.txt 三件套检测，cls.onnx 可选） |

**模型**：PP-OCRv5（det 4.5M + rec 16M + cls 572K + keys 18383 行）或 PP-OCRv6-small（det 9.7M + rec 21.5M + keys 18708 行 `ppocrv6_dict.txt`）。ONNX 标准格式，软链到 HF 缓存。

**推理后端迁移（2026-07-06）**：从 ocr-rs（MNN C++ 推理）迁移到 vendored paddle-ocr-rs（ONNX Runtime），消除 MNN cmake + bindgen + libclang 依赖。ort 与 ASR 引擎共用同一推理后端，跨平台零原生编译。`crates/paddle-ocr/` 是从 `paddle-ocr-rs` 按需拷贝的精简版（删 bin/input/model_store/model_registry/output/compat_rapidocr/turbojpeg/clap/opencv/reqwest/serde_yaml，保留 det/rec/cls/pipeline/runtime/vision 核心）。opencv 死代码（~1000 行）分两阶段彻底清理：(1) 删除全部 `#[cfg(feature = "opencv-backend")]` 门控代码；(2) 删除 `VisionBackend` enum 本身——`crates/ocr` 和 `crates/desktop` 零引用确认完全内部类型，移除后所有 `_with_backend` 函数变体合并为单一 pure rust 实现。**关键 bug**：`read_character_file` 原 `trim()` 误删全角空格 U+3000（字典首行）致 CTC 索引偏移 1 位 → 改 `strip_suffix('\r')`。详见 [spec](docs/superpowers/specs/2026-07-06-vendor-paddle-ocr-design.md)。

**vision/numeric.rs（2026-06-12）**：paddle-ocr 内重复的工具函数集中——`l2`（3 处完全相同）、`saturate_cast_i16_from_f32`（2 处）、`cv_round_ties_even_f32`/`saturate_cast_i32_round`/`saturate_cast_i16`/`interpolate_cubic_coeffs`/`clip_i32_exclusive_upper`/`clamp_i32_inclusive` 统一到 `vision/numeric.rs`。同步修 clamp/clip 命名混淆（原 `clip_i32` hi_exclusive vs `clamp_i32` hi_inclusive 语义不可见），改名让 inclusive/exclusive 在函数名上可见。

**det/postprocess/ 拆分（2026-06-12）**：原 2226 行 `mod.rs` 拆为 7 子模块：`threshold.rs`（二值化 + AVX2/SSE4.1）、`contour.rs`（轮廓提取 + 2x2 膨胀 + AVX2）、`box_score.rs`（box/contour 得分计算 + SIMD）、`geometry.rs`（最小外接矩形 + 凸包 + Sklansky）、`unclip.rs`（多边形扩展 + 填充 + 周长/面积）、`filter.rs`（检测框过滤/排序）、`tests.rs`。`mod.rs` 仅保留 `DbPostProcess` struct/impl + `CandidateScratch`/`ScaleTarget` + 模块声明。

**触发方式**：手动——剪贴板浮窗/管理页图片条目点 OCR 按钮（ScanText 图标）。不支持自动 OCR。

**结果处理**：三处入口（截图工具栏 / 图片预览 / 剪贴板图片条目）识别文本后统一走 `insert_ocr_clipboard_item`（desktop 命令：新建 source=ocr 剪贴板条目，engine/model 复用 clipboard_history 列）→ `open_compact_editor_tab(itemId)` 在精简编辑器打开绑定 tab 编辑（Ctrl+S 经 `set_clipboard_item_text` 回写）。截图 OCR 后端闭环（`ocr_screenshot` 内 insert_ocr + 同进程 open tab），其余两入口前端 `ocr_image` 纯识别 → insert → open tab。不再写 search_text / 系统剪贴板 / osascript TextEdit。**并发互斥可见提示**：某入口 OCR 进行中、他入口再点被 `OcrLockGuard` 拒绝时，前端 4 处给出「前一个 OCR 还未完成」反馈——剪贴板列表 / 图片预览 OCR 按钮显琥珀三角（`ocrWarn` 1.8s）、截图屏幕中央黑底 toast、设置页 `showToast`（该错误去掉原 `OCR 失败：` 前缀直接显示）。

### octopus-capx（屏幕截图）

独立的截图 crate，依赖 xcap 0.9.6（crates.io 发布版）。封装截全屏 + 裁剪选区。

| 模块 | 职责 |
|---|---|
| `capture` | `capture_all_monitors()`：截取所有显示器（每个返回 RGBA + 物理像素尺寸 + 显示器坐标）。`crop_region()`：从全屏 RGBA 裁剪矩形 → PNG（一次性截图用）。`crop_region_rgba()`：同但直接返回 `RgbaImage`（零 PNG 编解码，滚动截帧 30ms 热路径专用——原 `crop_region`+`load_from_memory` 往返在 4K/多屏下 CPU 爆表）。黑屏检测日志（权限诊断）。macOS 两处 CGImage 捕获（`capture_region_excluding_window` / `capture_window_region`）共用 `cgimage_to_rgba` helper（BGRA→RGBA 统一转换） |
| `stitch` | 滚动截屏拼接引擎：**Canvas-Anchored NCC + Sobel 梯度匹配**。每帧从画布底部提取 `eff_strip_h`（自适应 `min(strip_h, content_h/3).max(8)`，见下②；上限 `strip_h`=80 为 StitchConfig 字段）px strip → Sobel 梯度特征图（`imageproc`，纯色退化回灰度）→ NCC 模板匹配（`imageproc::template_matching::match_template`，CrossCorrelationNormalized；**大屏（帧宽 > `ncc_downsample_width` 默认 1920）走两阶段 refine**——Triangle 降采样域粗定位 dy → 原分辨率 ±2px 邻域 `ncc_match_range` refine 恢复亚像素，避免降采样锯齿破坏 response 峰值；小屏单阶段。2026-07-06 D）→ 验证（score ≥ `ncc_score_threshold` 默认 0.65 + response 无区分度拒绝 max-min<0.1）→ 抛物线亚像素插值。`strip_h`/`max_scroll`/`ncc_score_threshold`/`ncc_downsample_width` 纳入 `StitchConfig` 字段化（2026-07-06 F，默认值不变行为零变化）。Canvas-Anchored 消除累积漂移。**画布锚点鲁棒性（2026-07-10 六轮迭代）**：canvas-anchored 假设画布底部=真实内容，但底部会退化成常数（暗尾/纯黑尾/首帧整帧空白/1D 假匹配 append 常数块）→ Sobel 失效死锁（常数模板 NCC 必 score≈1.0 假匹配或失配 stuck）。维护：① `content_tail` 每帧基于当前帧检测暗常数尾（行 max-min≤30 且最亮 luma<40 双判）裁掉、finalize 补回；② `eff_strip_h` 按 content_h 自适应（`min(strip_h, content_h/3).max(8)`，矮选区缩小留 2/3 搜索范围）；③ 画布种子用首帧自身缓冲测暗尾（`scan_content_tail_in`，裁剪对象=检测对象同帧）；④ `reseed_canvas_from` 死锚恢复（首帧整帧空白等极端情况从当前内容帧重建）；⑤ **常数尾每帧自愈**（`canvas_bottom_constant` 轻判 → `scan_canvas_constant_tail` 运行 min/max 测尾高 → 非破坏 `truncate` 优先 / reseed 兜底）——死锚不只首帧一次、滚动中可反复出现，锚点维护必须**每帧持续**（非一次性闸门）。主匹配 Sobel+灰度双候选，任一侧退化（常数模板）**不兜底**直接降级（灰度对常数模板必假匹配）。详见 `docs/features/screenshot.md` §4。降级链：**相邻帧参考 fallback**（内容突变失配时，用前一帧有效区匹配当前帧求正确 dy，避免 best-guess 盲 append 污染画布；`prev_gray` + `try_match_prev_frame`，插在 1D 投影前，2026-07-06 方向1）→ 1D 灰度投影 + best-guess（历史 dy 中位数，连续 3 次熔断）。**1D/best-guess 追加画布前经 2D 反向验证**（`verify_alignment_2d`：按候选 dy 算重叠区 `[crop_y-verify_rows, crop_y)` 的 2D 抽样 SAD vs 画布底部 strip，超 `FALLBACK_VERIFY_SAD` 默认 15.0 → 拒绝追加 skip 该帧，靠 Canvas-Anchored 下一帧从画布底部恢复；防 1D 行投影对图文混排假匹配污染画布，2026-07-07。prev_frame 路径 verify=false——其 dy 已过内部 NCC validate，且上一帧 skip 时 prev≠画布底部会误杀）。周期性假匹配锁定（连续相同 dy≥3 次锁定，dy 变化才解锁）。NCC stuck 检测（连续验证失败≥5 次判静止）。画布用 `Vec<u8>` 增量追加 + 惰性缓存。**停止时先关窗口再后台编码**：PNG 快速编码 → 并发两路（线程一写剪贴板~1s，线程二 WebP+DB 入库~2-3s）。 |

**触发方式**：全局快捷键（`screenshot_shortcut`，默认 Alt+S）+ 托盘菜单「截图」。

**多显示器**：每个显示器创建独立 Tauri 窗口（`screenshot_window` / `screenshot_window_N`），用 Tauri `available_monitors()` 获取逻辑坐标 + 尺寸（物理坐标除以 `scale_factor`），定位到对应屏幕。窗口初始 `visible(false)`，前端 Canvas 渲染完截图后调 `show_screenshot_window` 显示（消除白屏闪烁）。确认/取消时关闭所有 `screenshot_*` 窗口。**窗口串行创建**（间隔 150ms）：macOS WKWebView 同时创建多个全屏窗口会 segfault，故 `start_screenshot` 逐个 `sleep(150ms)` 创建，单窗 build 失败则 `log::error!` + `continue` 跳过该屏。

**截图流程**：`start_screenshot` → `capture_all_monitors` 截所有显示器 → 每屏创建不可见窗口 → 前端 `get_screenshot_image`（`ipc::Response` 返回原始 JPEG 字节，前端 `URL.createObjectURL` 加载）按 label 拉取各自截图 → Canvas 渲染（原图 + 暗遮罩 + 选区框 + 8 手柄 + 尺寸标注）→ `show_screenshot_window` 显示 → 选区下方弹出标注工具栏（矩形/箭头/文字/序号/撤销）→ 标注在选区内 Canvas clip 绘制 → Enter 确认：Canvas `toBlob` → `Uint8Array` Raw body 传后端（`ipc::Request`，不经过 base64）→ PNG SHA-256 去重 → WebP BLOB → DB image_data + clipboard_history + 系统剪贴板 → 关所有窗口。

**滚动截屏流程**：用户框选区域 → 按 `screenshot_shortcut`（默认 Alt+S）进入手动滚动模式 → 后台生产 task 30ms 截帧 → `tokio::sync::watch` 通道（丢旧保新）→ 消费 task NCC 实时拼接（preview 编码 fire-and-forget 不阻塞关键路径，2026-07-06 A 队列解耦） → 截图窗口旁显示拼接预览 → 用户点绿色「复制」停止 → **先关截图窗口**（用户感知立即停止）→ 后台并发：线程一 PNG→剪贴板（~1s），线程二 canvas→WebP→DB 入库（~2-3s 后台）→ emit `scroll://done { id }`（不含 base64，前端不再中转数据）。

**IPC 二进制传输**：所有图片传输已从 base64 改为二进制——前端→Rust 用 `ipc::Request` Raw body（`canvas.toBlob → ArrayBuffer → invoke(cmd, arraybuffer)`），Rust→前端用 `ipc::Response`（原始字节 → 前端 `URL.createObjectURL`）。消除 base64 编解码 + JSON 序列化开销。剪贴板历史条目复制（`copy_clipboard_item`）从 DB 读 WebP→PNG→剪贴板，移入 `spawn_blocking` 不阻塞 UI。图片预览（ImagePreview 组件，嵌入 CompactEditor 图片 tab）无标注时「复制」跳过（剪贴板已有数据），有标注时走 Canvas 合成→Raw body。

**macOS 权限**：通过 `cargo run` 运行时，屏幕录制权限需授给终端应用（非二进制）。打包 .app 后绑定 octopus 本身。

详见 [spec](superpowers/specs/2026-06-28-archived-specs.md)。

### octopus-notepad（已移除）

记事本 crate 已于 2026-07-03 彻底移除——剪贴板历史 + 多 tab 精简编辑器已覆盖编辑/查看需求，独立的标题/分类/持久化笔记不再维护。`crates/notepad/` 删除；desktop 的 `notepad_window.rs` / `note_commands.rs` 删除；DB v12→v13 迁移 DROP `notes` 表 + `notes_fts` + 3 触发器；前端 `pages/Notepad/` 删除；托盘「记事本」菜单 + Settings「存入记事本」入口移除。OCR / 文本编辑统一改走 CompactEditor 多 tab（见 [`compact_editor_window`](#compact_editor_window-1)）。详见 [清理 spec](superpowers/specs/2026-07-05-archived-design.md#四记事本移除--多-tab-compacteditor--ocr-统一) 与 [实施计划](superpowers/plans/2026-07-05-archived-plans.md#五记事本移除--多-tab-compacteditor--ocr-统一)（已归档）。

**桌面端集成**（`octopus-desktop`）：
- `compact_editor_commands.rs` + `compact_editor_window.rs`：**统一内容查看器（多 tab）**——tab 切换文本/图片/语音条目，取代独立 ImagePreview 窗口（详见 [统一查看器 spec](superpowers/specs/2026-07-05-archived-design.md#五图片查看器)（已归档 §五 5.6））。窗口单例 + 关窗即销毁，原生标题栏 **880×620 可调 + 记忆**（`WindowState` 存 `app_config`，开窗读记忆无记忆用默认居中；`CloseRequested` 保存——物理像素÷`scale_factor` 存逻辑像素，DPR 缩放修复 `c4eca38`）。Tab 模型 `{ key: '${source}:${itemId}', source: 'clipboard'|'transcription', itemId, itemType?: 'text'|'image', text? }`；图片 tab 嵌入 `ImagePreview` 组件（≤5，超 5 替换最旧；**懒加载**仅活跃 Tab 挂载、非活跃显示占位，2026-07-07——避免隐藏 Tab 仍并发拉全图+建 bitmap 致内存×Tab 数暴涨），语音 tab 只读 textarea。**打开优化（2026-07-07）**：`PendingTabFull`（含 itemType + text，`push_pending_tab` 时一次性读 DB）；建窗时首个 tab 数据拼入 URL query string（`index.html?itemId=...`），前端 `useState` 初始化同步从 `URLSearchParams` 读取——**零 IPC 打开**。**批量双开（2026-07-08）**：`PENDING_TAB: Option` → `PENDING_TABS: Vec<PendingTabFull>`（支持截图 OCR 图片+文本双开）；新增 `open_compact_editor_tabs(items)` 一次 push 全部 + 一次 create/emit——避免连续单开在「窗口刚 build、React 未 mount」中间态丢失第二个 tab（首 tab 经 URL 幸存、第二 tab 被 push 覆盖 + emit 丢）；窗口存在按 `PENDING_TABS.is_empty()` 判 React 是否已 mount take 清空——空=已 mount→emit 即时推送，非空=未 mount→push 进队列让 mount 一并 take（2026-07-08 修：窗口刚 create、React 未 mount 时连续第二次 open 的 emit 会丢第二个 tab）；窗口不存在先 `take_pending_tabs()` 清残留（建窗失败/关窗过早致 stale，防下次首屏污染）再 push；前端 mount `get_pending_compact_tabs` take 全部与 URL 首个按 key 去重。`open_compact_editor_tab` 单开命令保留（转调批量版）。**6 个命令**——`open_compact_editor_tab(item_id, source?)`（单开，转调 `open_compact_editor_tabs`；已开则 emit `compact-editor://open-tab` 推送并聚焦、未开建窗含 URL 参数）/ `get_pending_compact_tabs() -> Vec<PendingTabFull>`（前端 mount take 全部）/ `get_clipboard_item_text(item_id)`（读 content 供文本 tab 载入）/ `get_clipboard_item_type(item_id) -> 'text'|'image'`（前端据此渲染 textarea 或 ImagePreview）/ `get_transcription_text(id) -> String`（读 clipboard_history voice 条目的 content，供语音只读 tab）/ `close_compact_editor`（关窗）。文本 tab：Ctrl+S / Cmd+↵ 经 `set_clipboard_item_text` 回写 DB + 系统剪贴板，关 tab 不删条目（仅关视图）。macOS 开窗切 Regular、关窗切回 Accessory。
- ~~`image_preview_commands.rs` + `image_preview_window.rs`~~ **已删除**（统一查看器 Task 5，`1928e62`）——独立图片预览窗口废弃，功能合并入 CompactEditor 图片 tab；`ImagePreview` 改为**组件**（`pages/ImagePreview/index.tsx`，props `imageId: number`，去掉 `get_pending_image` / `listen("image-preview://load")`，由父 tab 控制 imageId；保留 `listen("ocr-screenshot://result")` 接收截图 OCR blocks），不再经 App.tsx 路由。标注核心 `frontend/src/lib/annotation.ts` + `AnnotationSvg.tsx`（SVG overlay）不变。**性能优化（2026-07-03，归属组件）**：① **canvas 视口固定 + 可见区切片重绘**——canvas `position:sticky` 钉 scrollContainer 视口，物理尺寸 = 视口×dpr（永不超 Chromium 32767 单边硬限，长图不再崩，2026-07-07 实施）；drawBg 滚动/缩放只 drawImage 图片露出视口的 src 切片到视口坐标（不全量重绘），几何换算抽 `viewportMath.ts` 纯函数（17 单测；DOM/sticky 对齐 GUI 核心已验证 2026-07-07：超大图不崩 + 缩放正常）；② 底图 canvas + SVG overlay——标注用 SVG 元素（标注变化零 canvas 操作）；③ zoom 走 `createImageBitmap` 异步预缩放（`zoomVersionRef` 防过时帧）；④ 先 thumb 再 full 渐进加载（`cancelled` 防竞态 + ResizeObserver 自动重算）；⑤ thumb→full 期间 `loadingFullRef` 门控禁止标注。详见 [perf spec](superpowers/specs/2026-07-05-archived-design.md#五图片查看器)（已归档）。
- `clipboard_commands.rs` 图片预览 3 命令：`get_image_full`（取 `image_data.blob` 全分辨率 → `ipc::Response` 原始 WebP 字节）/ `save_image_dialog`（`ipc::Request` Raw body → `blocking_save_file` + `fs::write` 全部 `spawn_blocking`，2026-07-05 第七轮补，避免阻塞 Tokio worker；与 `copy_image_to_clipboard` 同模式）/ `copy_image_to_clipboard`（`ipc::Request` Raw body → `write_image` + `handle_clipboard_change` 全部 `spawn_blocking`）。`copy_clipboard_item` 移入 `spawn_blocking` 避免 UI 冻结。
- `clipboard_commands.rs::insert_ocr_clipboard_item`：OCR 统一入库——识别文本 → `store::insert_ocr_item(conn, text, engine, model)`（item_type='ocr'，meta_info={engine,model,char_count}）→ `emit("clipboard://changed")` → 返回新 id；`current_ocr_meta()` helper 读 `ocr_model` 配置（默认 PP-OCRv6-small，engine 固定 paddle），返回 `(engine, model)` 元组供此命令与 `ocr_screenshot` 复用。三处 OCR 入口（截图 / 图片预览 / 剪贴板图片条目）识别出文本后统一走此命令入库 + `openCompactEditorTab` 打开绑定 tab。
- `clipboard_commands.rs::set_clipboard_item_text`：编辑器回写——同写 `content` + `search_text`（保 FTS 命中，`clip_fts_au AFTER UPDATE OF search_text` 触发器自动同步 FTS5 索引）+ 同步系统剪贴板，**成功后 `emit("clipboard://changed")`**（编辑器是独立窗口，剪贴板列表窗口靠此事件感知条目变化并 `fetchItems()` 重新拉取，否则编辑后列表仍显示旧文本）。供 CompactEditor Ctrl+S / 剪贴板文本条目「编辑」回写。
- `screenshot_commands.rs::ocr_screenshot`：截图 OCR 后端闭环——图片入库（截图历史，`save_screenshot_to_history` helper 三处去重）+ `octopus_paddle_ocr` 识别 → `insert_ocr_item`（入库 + 识别 + OCR 入库均在 `spawn_blocking` 内隔离 CPU 任务，2026-07-06）→ 经主线程调 `open_compact_editor_tabs([(image_id,None),(ocr_id,None)])` 批量开图片+文本 tab（一次调用，避免连续单开中间态丢失文本 tab）+ `emit("clipboard://changed")` + `emit("ocr-screenshot://result", { text, blocks })`（推送 OCR 文本块给图片 tab 的 ImagePreview 叠加）+ 写 `LAST_SCREENSHOT_OCR` 缓存（image_id 关联）；`get_last_screenshot_ocr(image_id)` 命令供 ImagePreview mount 拉取——emit 早于新窗 React mount 会被丢，mount 时按 image_id 校验 take 兜底；不再 write_text / update_search_text（编辑保存时才写剪贴板）。**双重解码消除（2026-07-09）**：PNG 解码一次后 `save_screenshot_to_history` 和 OCR 共用同一 `DynamicImage`（`recognize_with_blocks_from_image`），避免 4K/5K 截图重复解码（省 ~100-300ms）。
- `coordinator.rs`：`static CURRENT_TRANSCRIPTION_ID: AtomicI64`（3 个会话起点记录）+ `current_transcription_id` 命令，供 Result 窗口溯源当前识别记录（无需改文本事件 payload）。**内部结构（2026-06-12 重构）**：`Coordinator::new` 仅做 channel 创建 + 调 `build_coordinator_loop`（状态机循环提取为独立函数）；`begin_recording`（228→~30 行）按引擎分支调 `prepare_streaming_session` / `prepare_cloud_streaming_session`（cfg=cloud）/ `prepare_vad_segmented_session` 三个独立函数。DB 写入队列已提取为 `db_queue.rs`。
- `db_queue.rs`（2026-06-12 新增）：ASR 识别结果的 DB 写入 actor——`DbCommand` enum（Insert/UpdateTextSegments/UpdatePolished/Finalize/UpdateEditedSegments/Delete）+ 后台线程（`get_db_sender` 懒初始化，channel + `recv_timeout` 轮询关机标志）+ `shutdown_db`（排空队列 + join，挂 `RunEvent::ExitRequested`）。调用方非阻塞 `send` 后即返回，落库在后台线程。从 `coordinator.rs` 提取（原 ~180 行）。
- `screenshot_geometry.rs`（2026-06-12 新增）：`start_scroll_recording` 提取出的纯逻辑——坐标换算（窗口原点+CSS 偏移→全局逻辑坐标 `compute_selection_global`）、显示器命中（`find_monitor_for_point`）、物理像素裁剪（`compute_physical_crop`，含 `.max(0.0)` 跨显示器边界防御）、preview 裁剪参数（`compute_preview_crop`，消除两处重复）。所有函数不依赖 Tauri/Quartz 类型，可独立单测。**鼠标穿透轮询**（`start_scroll_recording` 内）：CGEvent 全局鼠标追踪 + `set_ignore_cursor_events` 切穿透 + 激活下方应用，macOS 专属——core_graphics 仅 mac dep，整个轮询 spawn 块 `cfg(target_os = "macos")` gate，非 mac 不启动（2026-07-08 修：原 spawn 无 gate 致非 mac `use core_graphics` 编译失败）。
- 前端：`pages/CompactEditor/`（**统一查看器**：tab 栏文本/图片/语音图标区分 + 工具栏撤销/重做/字号/查找替换/清空/保存 + 内容区 hidden 挂载按 `tab.source`/`tab.itemType` 渲染 textarea（文本可编辑 / 语音只读）或 `<ImagePreview imageId={...}/>`；**键盘/按钮 undo/redo 统一走 `document.execCommand`**——受控 textarea 每次 value 同步清空 WebKit 原生 undo 栈致键盘 Cmd+Z 失灵，详见 [清理 spec §12](superpowers/specs/2026-07-05-archived-design.md#四记事本移除--多-tab-compacteditor--ocr-统一)（已归档））；`lib/compactEditor.ts::openCompactEditorTab(itemId, source?)`。`pages/Notepad/` 已删除；`pages/ImagePreview/` 保留为组件（非路由）。

### octopus-desktop（桌面应用）

基于 Tauri 2 的桌面应用，支持系统托盘、全局快捷键、结果窗口、流式识别。

**识别模式：**

| 模式 | 引擎 | 说明 |
|------|------|------|
| 流式 | Paraformer, Zipformer | 边说边识别，200ms tick 驱动 |
| 离线 | SenseVoice, Whisper, Qwen3-ASR | VAD 分段伪流式，100ms tick 驱动，阈值可配置 |

**窗口管理：**

| 窗口 | 用途 |
|------|------|
| `result_window` | 识别结果展示（可拖拽、多行滚动、透明无边框、置顶）。顶部悬停工具栏：鼠标移入展开，移出收起；工具精简为 7 个——**关闭**（首位，放弃内容保留 DB 记录）/ 系统设置 / 降噪模式 / 润色模式 / 立即润色 / 放大缩小 / **保存**（提交编辑恢复 ASR）。**CM6 改造（2026-07-11）**：文本区从 contentEditable div + 手写光标系统（`caret.ts` 122 行 + `CaretBlink.tsx` 49 行）替换为 CodeMirror 6 纯文本编辑器（`AsrEditor.tsx`），实现**始终可编辑 + 随说随编**——用户输入即暂停 ASR（`enter_edit_mode` → `trim_buffer(5.0)`，麦克风不停、保留最后 5 秒音频恢复后送 ASR 防"嘴比手快"丢字），三种恢复路径：`edit_shortcut`（Cmd+Enter）/ 保存按钮 / 停止输入 2 秒自动 commit。CM6 `updateListener` 区分用户编辑（`isUserEvent`）与程序写入（流式 dispatch）：用户编辑 → 进入编辑态 + `clearDivertedTimer`（防 diverted 覆盖）+ `mapDirtyRanges`（`changes.mapPos` 映射已有 dirty 区间到新坐标）+ `addDirtyRange`（`iterChangedRanges` 累积新插入区间）；流式写入 → `editingRef` 拦截。commit 携带 `{ text, dirtyRanges, hasEdited, caret?, selection? }`（`onCommitRef` 避免闭包陈旧）。后端 `commit_edit` 按 dirty ranges 劈段标 `Edited`，区间外用**字符级 walk**（构建 old_flat 逐字符 kind 映射，clean 区域逐字符匹配跳过被删字符保留原 kind）重建——`rebuild_segments` + `push_or_merge`；dirty ranges clamp 防越界。Idle 态从 DB `restore_segments` 恢复 old_segments（防 clean 区全退化 Raw）。纯删除（`has_edited=false` + 空 dirty）走 `rebuild_segments(&[], ...)` 保留原 kind。**中插 + 选中替换**保留：非编辑态 CM6 `selectionSet` → `notifySelection`（折叠选区=点击即时 `set_caret`，非折叠=拖选 100ms 防抖 `set_selection`）；CM6 mount 后 `view.focus()` 启动即显示光标；用户选中后输入 → 走 dirty range。编辑中点润色 → `polishNow` 先 `commit()`。diverted 延迟 300ms 移入 AsrEditor。**移除的命令**：`CancelEdit` / `UpdateEditBuffer` / `exit_edit_without_commit`。 |
| `settings_window` | 独立设置窗口（原生标题栏、圆角、可调大小）。六页面侧边栏布局：系统设置 / 识别记录 / 剪贴板 / 模型管理 / 提示词 / 系统状态。React 组件化，表单用 react-hook-form。窗口位置记忆。**已存在窗口唤起时 macOS 切 `Regular` + 主线程 `NSApp.activate()` 前台**（`set_focus` 仅设焦点不激活 app，其他 app 在前台时窗口被遮挡；2026-07-12 修复）。`open_settings` 支持初始页面参数（`PENDING_PAGE` 暂存 + `get_initial_page` 拉取 + `settings://navigate` 事件），剪贴板浮窗「管理」按钮直接跳转剪贴板 tab。 |
| `compact_editor_window` | 统一内容查看器窗口（原生标题栏、**1100×680 可调 + 记忆**、居中、min 600×360）。**多 tab**——tab 切换文本（可编辑）/ 图片（嵌入 ImagePreview 组件）/ 语音（只读）条目，tab key = `${source}:${itemId}` 全局唯一；文本 tab 标题 = 文本前 5 字 + `-` + id hex 后 5 位。**Markdown 改造（2026-07-11）**：文本/语音 tab 的 textarea 替换为 CodeMirror 6 编辑器 + markdown-it 实时预览（debounce 150ms），组件 `MarkdownPane`（工具栏左侧：撤销/重做/字号/清空；右侧：视图模式组 + 保存，`flex-1` 隔开）。视图模式：编辑/分屏/预览，可编辑 tab 默认分屏、只读 tab 默认预览；**CM6 + Preview 始终挂载**，用 `display:none` 切换可见性（零 mount/unmount），Splitter 拖拽内联到 MarkdownPane（grid 模板列动态切换）。CM6 原生 `search()` 替代手写查找替换（~180 行），CM6 `history()` 替代 `execCommand` 撤销/重做。仅活跃 tab 挂载 CM6（与图片 tab 懒加载一致）。**滚动条**：`.cm-scroller` 需显式 `overflow: auto`（index.css），Tailwind v4 preflight 可能覆盖 CM6 默认值；tab 内容容器必须 `min-h-0`（flexbox 高度约束链），否则内容撑开容器、滚动条不出现（2026-07-12 修复）。**只读 tab（transcription）保护**：CM6 readOnly + Clear/Save 隐藏（`disableSave = tab.source === 'transcription'`）+ doSave 守卫（`!active`/`transcription`/`itemType !== 'text'` 早返回；temp tab 不早返回、走 insert 分支），keydown 无条件调 doSaveRef.current() 由 doSave 内部决定。**tab 管理同步计算模式**：await 后基于 `tabsRef.current` 算 next → 同步写 ref + setTabs(next) + setActiveIdx(literal)，不使用 setTabs updater（避免 React 异步队列致 ref 陈旧 + setActiveIdx 失败）。代码块无高亮（highlight 回调预留 Shiki/Mermaid/PlantUML 埋点），**声明式复制按钮**（markdown-it `code_block`+`fence` 渲染规则输出 `.md-codeblock`+`[data-copy]` 按钮，mermaid 跳过），事件委托统一处理（useEffect deps `[]`）。预览链接拦截：`#anchor` 滚动 / `http(s)` 走 `openUrl` / 其余 `preventDefault`。i18n 基础设施（`lib/i18n.ts`，轻量自建 `t()` + `useT()` + flatten + `zh-CN.yaml`/`en.yaml` 嵌套结构），后端 config `ui_language` 字段（默认 `zh-CN`），设置面板 GeneralPanel 切换；**i18n 全面覆盖（2026-07-12）**：前端全部 7 个 page 模块 + Settings 全面板 + Rust `tray.rs` 已提取约 450 个 i18n key，locale YAML 为前后端单一真相源（前端 `vite-plugin-yaml` import + flatten，Rust `include_str!` 编译期嵌入 + `serde_yaml` 解析 + flatten），语言切换时前端 `emit("locale-changed")` → 各窗口 `initI18n` 中的 `listen("locale-changed")` 自动同步本地 locale + Rust `i18n::reload` + `tray::rebuild_tray_labels` 同步更新托盘菜单文案。详见 [spec](superpowers/specs/2026-07-12-i18n-full-coverage-design.md)。**窗口记忆**：`CloseRequested` 存位置/大小到 `app_config`（物理像素÷`scale_factor` 存逻辑像素），开窗读记忆。**URL 不注入 text**（长文本致 WebView 白屏），首个 tab 元数据（itemId/source/itemType/imgWidth/imgHeight）注入 URL，text 由 mount 后 `get_pending_compact_tabs` 批量拉取，`mergePendingTabs` 把 pending 合并进 URL 占位 tab（pending 同 key 覆盖占位——旧 dedup `continue` 跳过 pending 致首个文本 tab 永久 text="" 空白，已修）。命令：`open_compact_editor_tab(item_id, source?)`（转调批量 `open_compact_editor_tabs`）/ `get_pending_compact_tabs() -> Vec` / `get_clipboard_item_text(item_id)` / `get_clipboard_item_type(item_id)` / `get_transcription_text(id)` / `insert_clipboard_text_item(text) -> i64`（temp tab 保存：入库新文本条目 + 同步系统剪贴板）/ `close_compact_editor`；辅助 `open_temp_compact_editor(app, &TempTabPayload)` 打开 temp tab（窗口存在 emit / 不存在 store+建窗，托盘「图文编辑」与 action_bar 结果共用；`TempTabPayload { text, mode?, original_text?, translated_text? }`——mode="contrast" 为翻译对照模式）。单例：open 时已存在则 show+focus，否则创建；macOS 开窗切 Regular、关窗 `Destroyed` 经 `on_compact_editor_closed` 切回 Accessory（与 settings 对称）。入口：剪贴板文本「编辑」/ 图片「预览」、OCR 识别后（统一 insert_ocr_clipboard_item → openCompactEditorTab）、截图 OCR（图片 + 文本双 tab）、语音识别记录「查看」（source=transcription 只读 tab）、**托盘菜单「图文编辑」**（source=temp 空白 tab，保存经 `insert_clipboard_text_item` 入库 + `promoteTempTab` 升级为 clipboard tab；action_bar 翻译/润色结果同走 temp tab）。**翻译对照模式（2026-07-12）**：`Tab.mode='contrast'` 时渲染 `TranslationContrastPane`（替代 `MarkdownPane`），左原文/右译文双栏，各列独立 CM6 编辑器 + Markdown 预览（无 split，已外层分栏），新增视图布局切换（只原文/对照/只译文）。入口三条：(1) action bar 翻译（流式：立即开 contrast tab 译文 loading，后台逐段翻译 emit `translate-progress`/`translate-done`）；(2) 普通文本 tab 工具栏「翻译」按钮（fire-and-forget `translate_text`，立即切 mode='contrast' + 后台 emit 更新）；(3) 截图翻译（数据通路已支持，UI 后续）。对照视图含可拖拽 splitter（grid 布局调比例 + localStorage 持久化，splitter 颜色 `bg-muted-foreground/30` 与行号线区分）+ 每列单 toggle 按钮（编辑/预览两态）。保存只写译文（`translatedText`），原文是脚手架不持久化；temp→clipboard 升级时 mode 回退 single。后端 `translate_text` 命令 fire-and-forget + `do_translate_streaming` 逐段翻译 emit。引擎优先 **Opus-MT**（30M 轻量），其次 m2m100。详见 [spec](superpowers/specs/2026-07-12-translation-bilingual-view-design.md)。语音结果窗**不用**独立编辑器——改为原地尺寸双模式，见 `result_window` 行。 |
| `image_migration` | 一次性迁移：`~/.octopus/clipboard_images/` → DB `image_data` BLOB。幂等（已存在的 hash 跳过），迁移成功后删除目录。启动时 `main.rs` setup 阶段调用。 |
| `clipboard_window` | 剪贴板历史浮窗（300×600，无边框圆角透明置顶，`clipboard_shortcut` 默认 CmdOrCtrl+Shift+D 唤起——toggle 按焦点判断：失焦状态按快捷键直接 `show`+`set_focus` 激活，仅「可见且有焦点」才收起，避免 always-on-top 窗口失焦后仍 visible 导致需按两次；窗口位置记忆）。顶部标题栏（X + 「剪贴板」 + 右侧三 toggle 成组：监听开关 + **预览开关（Eye/EyeOff，默认关闭，localStorage 记住选择）** + Pin）。**hover 预览 overlay（2026-07-12）**：预览开关开启时，hover / 键盘 ↑↓ 选中条目后列表右侧弹出 200×200 absolute overlay（`z-30`），文本→等宽可滚动 `text-[11px]`、图片→缩略图、文件→路径。智能定位：选中在上半→预览在下方（底边与条目底边重叠 2px）；下半→预览在上方（顶边与条目顶边重叠 2px），最小化遮挡。边框 `border-foreground/15` + `shadow-2xl`。选中变化（hover `onMouseEnter` / 键盘 `selectedIndex`）实时更新预览内容（长文本截断 500 字防卡顿，完整内容走 CompactEditor）。**键盘/hover 导航防冲突**：键盘 ↑↓ 时设 `keyboardNavRef=true`，300ms 内忽略 scrollIntoView 滚动误触的 `onMouseEnter`，防选中位置打转（`onHover` prop 独立于 `onSelect`，click 不受影响）。预览面板用标题栏 Eye 开关控制，不设 X 关闭。**浮窗失焦时隐藏预览**（onFocusChanged 监听）。**预览定位坐标系**：abs 子元素的 top 是内容坐标（随滚动移动），clamp 上下界须含 listEl.scrollTop 与内容坐标对齐（曾因视口常量 clamp 致长列表滚动后预览消失）。**缩略图竞态**：cancelled 守卫防快速切换时旧 IPC 结果覆盖。详见 [spec](superpowers/specs/2026-07-12-clipboard-preview-pane-design.md)。 |

**窗口背景色策略（2026-07-07 最终定型）：** Rust 建窗时从主题配置读 background hex，拼入 URL `?bg=2e3440`（`theme::window_bg_hex(label)` 白名单判断——只 settings/compact_editor 返回 hex）。`index.html` `<head>` 脚本读 URL bg 参数直接设裸 `#hex`——**零 CSS 依赖、零 JS bundle 依赖、HTML 解析首帧即有色**。透明窗口（result/clipboard/screenshot）无 bg 参数，不设背景色。`applyThemeById` 加脏检查（`data-theme` 值相同直接 return）避免重复 style recalc。`main.tsx` 不再设背景色（已被 index.html URL hex 替代）。教训：`transparent:true` 不覆盖 html 背景色（html backgroundColor 仍渲染为不透明层）；`var(--color-background)` 依赖 CSS 加载有延迟；截图遮罩由 React 组件画在选区外（选区内全透明看桌面），body/html 背景会盖住选区。

**窗口创建注意事项（2026-07-07，主题系统开发中踩坑总结）：**
1. **窗口背景色只对非透明窗口通过 URL `?bg=hex` 注入**——Rust 建窗时从主题配置读 background hex 拼入 URL，`index.html` `<head>` 脚本同步设裸 `#hex`（零 CSS 依赖）。透明窗口（`transparent:true`）不注入——`transparent:true` 只让窗口**支持**透明，html `backgroundColor` 仍渲染为不透明层，设了会"显形"。result/clipboard 靠 transparent + body transparent 实现穿透；screenshot 靠 React 组件画选区**外**遮罩（选区**内**全透明看桌面）。
2. **窗口位置保存/恢复必须 inner 对称**——`inner_position()` + `inner_size()`（都基于内容区，不含标题栏）。混用 `outer_position` + `inner_size` 会在不同 DPI / 标题栏高度下产生坐标偏差。物理像素÷`scale_factor` 存逻辑像素。
3. **多显示器位置越界检测**——保存的位置是绝对逻辑坐标，副屏关闭/拔掉后坐标失效。恢复前用 `available_monitors()` 检测坐标是否在任一显示器范围内（50px 容差），不可见则 fallback 到居中。**关键：`Monitor::position()` 和 `Monitor::size()` 返回物理像素，必须除以 `scale_factor` 统一到逻辑像素再比较**——否则 Retina（scale=2）下物理 0-3840 把逻辑 460 也包含进去，副屏坐标永远匹配到主屏。CompactEditor（`compact_editor_window.rs`）和 result/clipboard（`window_position.rs`）均已修复。完整位置/最大化/多显示器设计与不变量详见 [spec](superpowers/specs/2026-07-08-window-position-maximize-design.md)。
4. **最大化窗口创建（十二次反复后定型）**：
   - `builder.maximized(true)` 在 WRY 底层**不生效**（build 后 `is_maximized=false`）
   - build 后 `win.maximize()` 在 `show()` 前调用——macOS 隐藏窗口 maximize 无 zoom 动画，但 `show()` 后可能有细微过渡
   - **最终方案**：最大化时用保存坐标匹配 `available_monitors()` 找到对应显示器（副屏最大化不挪到主屏）→ 用该显示器尺寸减四边 80px 余量创建大窗体 → `show()` → `maximize()`（视觉差异极小）→ 确保 `is_maximized=true`（用户 un-maximize 恢复正常尺寸）
   - **保存真实位置**：最大化关闭时先 `unmaximize()` → 读 `inner_position()`（真实位置，反映窗口在哪个屏幕）→ `re-maximize()` → 保存。不能直接读最大化时的 `inner_position()`（返回全屏位置，可能跨屏到主屏原点）。DB key `compact_editor_last_normal_pos` 存 `x,y,w,h`
   - **三层 fallback**：坐标匹配到已连接显示器 → 该屏大窗体 + maximize；匹配失败 → `primary_monitor` 大窗体 + maximize；连主屏都拿不到 → 默认 880×620 居中
   - **不能**用主屏尺寸直接创建不走 `maximize()` API——`is_maximized=false` 会导致保存错误的非最大化状态
   - **物理/逻辑像素混用**是反复出现的 bug 源——所有 `Monitor::position()` / `Monitor::size()` 必须除以 `scale_factor` 再与逻辑坐标比较
5. **前端 `listen` import 来源**——项目有 `lib/tauri.ts` 封装（自动 `e.payload` 解包），但部分文件（如 `Settings/index.tsx`）直接从 `@tauri-apps/api/event` 导入原生 `listen`（回调收到 `Event<T>` 对象而非 payload）。两种 import 混用导致 `typeof page === "string"` 在原生 import 的回调里永远 false。新代码应统一用 `lib/tauri.ts` 封装版 `listen`。
6. **剪贴板浮窗列表上限 200 条**（2026-07-07 从 50 放宽）——`useClipboardHistory` 一次查询 `size: 200`，无无限滚动/分页。
7. **Tauri 命令 async vs 同步**（2026-07-08）——Tauri 2 中同步 `fn` 命令在 UI 主线程执行，阻塞事件循环。含 DB 查询 / 硬件枚举（cpal `list_microphones`）/ CPU 密集推理（ONNX OCR / SHA-256 校验大文件）的命令必须声明为 `async fn` + `spawn_blocking`。已改：`get_config`（5 次 DB + cpal）、`ocr_image`（ONNX 推理）、`verify_model`（SHA-256 校验 230-740MB 模型文件）、`save_image_item`（WebP 解码 + PNG/JPEG 编码 + 文件写入）、`download_model`（`bootstrap_manifest` SHA-256 校验大文件，两处）、滚动截图保存（`blocking_save_file` + `fs::write`）。轻量 DB 查询命令（`set_config`/`get_history`/`check_shortcut`）暂保持同步——开销极小，改 async 收益有限。`ocr_screenshot` 已是正确的 `spawn_blocking` 模式。**剪贴板图片去重 hash 优化**（2026-07-08）：watcher 不再 PNG 编码只为算 hash——直接 hash RGBA 像素（`hash_rgba`），省去大图 PNG 编码 CPU 开销（4K 截图可省 ~1s）。**DF3 降噪 fallback**（2026-07-08）：`Df3Backend::process_frame` 失败时必须 `out.copy_from_slice(pcm)` 直通原始 PCM——否则输出残留历史数据导致爆音。
8. **物理/逻辑坐标转换**（⚠️ 关键，反复踩坑 6+ 次）——macOS 有两套坐标 API，**必须区分**：
   - `CGEvent::location()` → **逻辑坐标（points）**，原点主屏左上角 y 向下。**不除 scale**。与 Tauri `LogicalPosition` 一致。
   - `Monitor::position()` / `Monitor::size()` → **物理像素**（Retina 下如 3840×2160）。**必须除 `scale_factor()`**（Retina=2.0）。
   - 曾误把 CGEvent 当物理坐标除 scale → 浮窗位置偏到完全无关的地方、副屏选中浮窗出现在主屏。`crates/desktop/src/action_bar_commands.rs::get_mouse_position`、`compact_editor_window.rs`、`window_position.rs` 均已修复。**碰撞检测**：浮窗定位后检查鼠标所在显示器边缘（Monitor position/size ÷ scale_factor 转逻辑坐标），右溢出贴右边缘、左溢出贴左边缘，防止浮窗被屏幕截断。
**AI 命令面板（action_bar_window + 菜单 DB + 脚本执行 + Extension Package）：**
`action_bar_window` 迷你浮窗（2026-07-08，菜单 DB 化 2026-07-09），全局热键触发。选中文本→模拟 Cmd+C→弹出两级菜单→AI/搜索/翻译/网页/脚本→CompactEditor 展示结果。仅 macOS 支持，capabilities 白名单必须包含 `action_bar_window`。

- **菜单 DB（`action_bar_items` 表）**：自引用 `parent_id` 两级菜单，5 种 `action_type`：`submenu`/`ai`/`url`/`script`/`copy`。`PRAGMA foreign_keys = ON` 级联删除。`action_data` 存动作参数（AI prompt / URL / 脚本内容或绝对路径 / copy 文本）。图标支持文件名或内联 SVG（⚠️ 浮窗和设置页已改数字徽章，`icon` 字段保留兼容但不再 UI 展示）。同级菜单项上限 35 个（后端 create 校验）。`shortcut` 列（`0-9 a-z` 单字符，全局唯一）存储 `Alt+字符` 组合快捷键——浮窗打开时按 `Alt+字符` 直接执行对应命令，跨主菜单和子菜单层级。仅非 `submenu` 类型可设快捷键。⚠️ macOS Option 键改变 `e.key` 输出（Alt+H → "˙"），浮窗用 `e.code`（物理键代码）+ `codeToChar` 提取字符匹配。设置页保存后 `emit("action-bar://items-changed")` 通知浮窗即时刷新菜单。**系统内置 seed**（db.sql）：AI 子菜单（润色/摘要/解释）、搜索子菜单（Google/百度/Bing）、网页、翻译（auto_translate）。**v25 新增「问豆包」**（`script` 类型，`#osascript` 内联脚本：`open -a Doubao` 启动 Electron app → `delay 2` → `tell process "Doubao"` 中 `keystroke "v" using command down` 粘贴 + `key code 36` 回车；⚠️ Electron app 用 `activate`/`launch` 不可靠，必须用 `do shell script "open -a"` 启动；seed 用 title 去重 + `WHERE NOT EXISTS` 而非固定 id，避免与用户自建项冲突）。

- **脚本执行（`spawn_script`，9 种 magic comment 运行时）**：script 按第一行 magic comment 分发运行时——`#shell`/`#osascript`/`#powershell`/`#python`/`#node`/`#deno`/`#bun`/`#javascript`/`#typescript`，后三种为自动探测（JS: node→bun→deno，TS: npx tsx→bun→deno，预探测 `--version` + `OnceLock` 缓存探测结果）。选中文本通过环境变量 `$OCTOPUS_TEXT` 传递（不拼字符串，防注入）；超 200KB 写临时文件 + marker `_____ULTRA_LONG_TEXT_____`（脚本后清理）。`run_script` 重构为 `spawn_script` + `wait_with_timeout`（同步：try_wait 轮询 60 秒强杀 + pipe 线程并发读取防死锁）/ `wait_forever`（异步：`child.wait()` 阻塞等待，不超时不轮询，CPU 0%）+ `run_script_async` / `run_script_sync_blocking`。`script_error_msg()` 覆盖超时/异常/非零退出码。script 类型支持 `is_async`（异步 fire-and-forget / 同步 spawn_blocking 等待）+ `write_output_to_clipboard`（仅同步+成功+有 stdout 时写剪贴板）。所有 script 执行结果落库 `script_runs` 表（stdout/stderr/exit_code/超时/耗时，截断 64KB），设置页「执行记录」子页可查看+清理。

- **Extension Package**（2026-07-10）：`~/.octopus/extensions/` 下含 `config.yaml`（YAML 元数据：name/description/version/author + action + rules 预留 + skill 预留）的文件夹 = 一个 Package。导入集成进菜单编辑（非独立子页）：新增子项选类型「扩展包」→ EditForm 拖拽区（zip/文件夹）或「选择文件/文件夹」→ `import_extension` 仅校验+重复检测（不复制）→ 保存时 `install_extension` 复制到 extensions + 创建 DB `action_bar_items` 记录（`action_data` 存脚本**绝对路径**，`action_type=script`）。`spawn_script` 检测 `action_data` 前缀：绝对路径→读文件内容 + 设 `OCTOPUS_PACKAGE_DIR` 环境变量；否则→内联。`config.yaml` 的 `skill` 块纯声明性（`ref` 关联 `~/.agents/skills` 或 `skill.file` 自带 SKILL.md），一期仅 config 预留 agent 接口。详见 [spec](superpowers/specs/2026-07-10-extension-package-design.md)。

- **键盘导航**：上下键切焦点层（main↔sub），左右键当前行移动（主菜单 submenu 项自动展开子菜单预览但不抢焦点，非 submenu 项收起），**1-9 + a-z 快捷定位第 1-35 项**（只移动高亮不执行，超出范围无效），菜单溢出时 scrollIntoView 自动滚动 + voice 色 <</> 箭头指示器（ScrollRow 组件），定位到 submenu 项同步展开预览，执行用 Enter），Esc 直接关闭浮窗（一次）。`focusLayer` 状态独立于 `view` 状态。

- **窗口焦点协调（FLOAT_DEPTH 引用计数）**：全局快捷键不得将 settings/compact_editor 带到前台。macOS 上 WKWebView 要求 app 进程 active 才能获得键盘焦点，而 `set_focus` 触发 `NSApp.activate` 会把所有可见 Regular 窗口带到前台。解法（视觉焦点协调方案）：(1) show 前记录前台 app（`NSWorkspace.frontmostApplication`）+ app 非活跃时临时隐藏其他可见窗口（`WINDOWS_TO_HIDE_ON_FLOAT`：settings/compact_editor/clipboard_window）；(2) `set_focus` 激活浮窗获得键盘焦点；(3) hide 时先 `activate` 原前台 app 交还焦点，再 `show` 恢复被隐藏的 Regular 窗口。实现：`activation::before_floating_window_show` / `after_floating_window_hide` 公共函数（`FLOAT_DEPTH` 引用计数支持多浮窗嵌套——只有最外层 depth==1 记录状态/交还焦点），action bar + 剪贴板浮窗共用。`action_bar_show_result` 调 `after_floating_window_hide_keep_active`（递减 depth + 清状态 + 恢复隐藏窗口，跳过 deactivate 避免 CompactEditor 被压后台）。剪贴板浮窗另加 `Focused(false)` 事件回调 `restore_hidden_windows_only`（失焦 = 虚拟关闭：`float_depth_decrement_and_is_zero` 扣减——**depth>0（仍有浮窗存活如 action_bar）时直接 return 不清状态**，只有 depth==0 才清状态 + deactivate。纯逻辑提取为 `float_depth_increment` / `float_depth_decrement_and_is_zero` / `float_clear_state`，单测覆盖 5 场景）。`TRIGGER_IN_PROGRESS: AtomicBool` 重入 guard。

- **执行收口（`execute_action_bar`）**：async command 收口重构为 inner fn（返回 `Result<bool>`）+ 三路 match：Ok(true)=ai 已自收口直通、Ok(false)=url/script/copy 成功 hide+finalize、Err=异常仅 finalize（不 hide，前端显示红色气泡提示）——确保 `?`/Err 不会泄漏重入锁和 depth。`hide_action_bar_window` 经 `run_on_main_thread` 投递主线程（`after_floating_window_hide` 的 `NSApplication::deactivate` 需 `MainThreadMarker`）；`finalize_action_bar` 仅 AtomicBool 线程安全即时执行。trigger 阶段 suppress watcher → Cmd+C → 读选中 → 立即恢复剪贴板（选中文本不入库）；**所有路径（含成功路径）显式 `clear_suppress`** 撤销。URL 参数编码复用 `percent-encoding` 库（`url_encode_param`）。

- **设置页（命令面板 tab）**：CRUD + 排序（递归 TreeNode 树形控件：chevron 展开/收起 + 注册表式等宽序号 `01`/`1.1` + 全部展开/收缩按钮；新增用内存草稿模式——不写 DB，保存时才 create，取消零脏数据；EditForm 单页导航（非弹窗），菜单项单击=展开/收起 submenu，编辑=独立 Pencil 按钮，删除二次确认）。详见 [spec §6.3](superpowers/specs/2026-07-09-action-bar-menu-db-design.md) + [脚本增强 plan](superpowers/plans/2026-07-10-action-bar-script-enhancement.md) + [Extension Package plan](superpowers/plans/2026-07-10-extension-package.md)。


**macOS 动态激活策略（Dock 图标显隐）：** 应用启动即 `Accessory` 模式（无 Dock 图标，纯托盘应用）。两个**常规窗口**（`settings_window` / `compact_editor_window`）任一打开时切 `Regular`（设置窗口另经 `set_dock_icon()` 用 `objc2` 手动 `setApplicationIconImage`——release 裸二进制无 .app bundle，Tauri 仅 debug 自动设图标）。关窗 `Destroyed` 经 `activation.rs::restore_accessory_if_no_regular_window` 协调——**仅当常规窗口全无存活才切回 `Accessory`**，否则保持 `Regular`（避免 app 降级时 macOS 连带收掉其余常规窗口——`image_preview_window` 已随统一查看器移除，现 `REGULAR_WINDOWS` 仅两窗；协调逻辑对 settings↔compact_editor 仍生效，曾致「关 CompactEditor 连带关 image_preview」即此 bug 的修复保留）。`#[cfg(target_os = "macos")]` 条件编译，Windows / Linux 无此逻辑。

**跨平台贴图窗口（pin_window）：** 截图后「钉住」功能——创建原生浮动窗口置顶显示截图，支持拖拽（左键）、缩放（滚轮，以鼠标位置为锚点）、关闭（hover 右上角红色关闭按钮）。绕过 WebView 直接用原生窗口，单窗内存 < 5MB。三平台各自实现 `PinWindow` trait（`create(png_data, x, y, w, h)`）：**macOS** 自定义 `PinNSWindow`（`define_class!`）+ `PinNSImageView`（拖拽经 `performWindowDragWithEvent`、缩放改 frame）+ `PinCloseBtnView`（NSImageView 子类，预渲染 PNG 图标规避 `drawRect:` 崩溃）；`NSTrackingArea`（`MouseEnteredAndExited | ActiveAlways | InVisibleRect`）检测 hover 显示/隐藏关闭按钮；静态 `PIN_WINDOWS: Mutex<Vec<SendWindow>>` 跟踪窗口 + `setReleasedWhenClosed(false)` 防悬空 + 关闭时延迟 `cleanup`。**Windows** Win32 `WS_EX_TOPMOST|LAYERED|TOOLWINDOW` + `UpdateLayeredWindow`（预乘 BGRA + GDI `StretchBlt` HALFTONE 缩放）；`WM_MOUSEMOVE`+`TrackMouseEvent` 检测 hover，GDI 绘制关闭按钮到 layered DC；每窗独立线程跑 `GetMessageW` 循环（显式判 0/-1）；GDI 资源经 `run_gdi_calls` 闭包 + defer 清理防泄漏。**Linux** GTK3 Toplevel + Cairo 自绘（`scroll-event` 缩放锚定 `event.coords()` + `win.move_()`；`motion-notify`/`leave-notify` 检测 hover，Cairo 绘制关闭按钮）。右键关闭菜单已三平台移除（hover 按钮体验更优）。`screenshot_commands::pin_screenshot` 双路径：前端 `composeAndCropBytes` 合成带标注/马赛克的 Canvas PNG → `FileReader.readAsDataURL` 转 base64 → 后端解码（`img_base64: Option<String>`）；None 时 fallback 到后端 `ALL_CAPTURES` 裁剪（不含标注）。前端 `isPinningRef` 防重复点击锁。详见 [spec](docs/superpowers/specs/2026-07-06-cross-platform-pin-window-design.md)。

**透明窗口点击穿透（统一方案，3 处使用点）：** octopus 有三处透明窗口需要「部分区域可交互、其余区域鼠标穿透到下层 app」——它们的核心矛盾相同（`setIgnoresMouseEvents(true)` 是全窗口开关，设了之后可交互区域也收不到事件），解法统一为 **Rust 后台轮询读全局鼠标位置，按区域切换穿透**：

| 使用点 | 可交互区域 | 穿透区域 | 实现 | 坐标方案 |
|--------|-----------|---------|------|---------|
| **result_window**（精简态） | 顶部 720×116 小条（与窗口同宽，`BAR_W=720`） | 小条下方透明区 | `start_click_through_poller`（`result_window.rs`） | `cursor_position()` 物理坐标 |
| **clipboard_window**（dock 收缩态） | 边缘 8px 细条 | 其余透明区 | `start_edge_poll`（`clipboard_dock.rs`） | `cursor_position()` 物理坐标 |
| **screenshot**（滚动录制） | 工具栏 + 预览窗（`interactive_rects`） | 其余遮罩区 | `start_scroll_recording` 内嵌轮询（`screenshot_commands.rs`） | `CGEvent` Quartz 逻辑坐标 ⚠️ |

**统一模式**（result_window + clipboard_dock）：`tokio::interval(33ms)` 轮询 Tauri 跨平台 `cursor_position()`（物理坐标）+ `outer_position()`（物理坐标）直接比较——无需 scale 换算，多显示器不同 DPI 安全。macOS 在 NSWindow 直调 `setIgnoresMouseEvents`（via `run_on_main_thread`），Windows/Linux 用 Tauri `set_ignore_cursor_events`。**Wayland 限制**：`cursor_position()` 恒返回 (0,0)，穿透失效（协议层限制，改用 XWayland 可恢复）。**为什么不用前端 `setIgnoreCursorEvents` + mousemove**：一旦 `setIgnoreCursorEvents(true)`，窗口完全不收鼠标事件（NSWindow 连 tracking area 都禁），前端 mousemove 不再触发 → 无法检测光标重入 → 重入失效。必须 Rust 后端读全局位置。

**截图不可统一**：截图穿透的核心价值不只是穿透鼠标——它还**激活光标下方的 app**（`CGWindowListCopyWindowInfo` 查鼠标下的 PID → `activateWithOptions`），让用户能直接滚动下层窗口。这依赖 Quartz 坐标系 + macOS 窗口列表 API，且整段 `#[cfg(target_os = "macos")]` gate（macOS 专属功能）。强行统一会增加坐标转换复杂度且无跨平台收益。

**窗口加载就绪（ready）机制：** 结果窗 webview 首次加载有延迟，若后端在页面就绪前 `emit('show-result')`，事件丢失导致「文本不显示 / 不弹窗」。`result_window.rs` 以 `WINDOW_READY`（AtomicBool）+ `PENDING_TEXT`（Mutex<Option<String>>）兜底——未 ready 时暂存文本，前端 `index.html` 加载完成后发起 `result_window_ready` Tauri command → 后端置 ready 并冲刷积压文本。`show_result` / `update_result` 把「判 ready + 写 pending」收进同一把 `PENDING_TEXT` 锁，与 `result_window_ready` 的 store(true)+take 互斥，消除启动首帧 TOCTOU 文本滞留。**`show_result` 的物理 `window.show()` 无条件执行**（不受 ready 门控，仅 `emit('show-result')` 受门控）——冷启动首启 webview 未 ready（走 pending 分支）时按快捷键也能立即弹窗，可见窗口的 webview 优先首绘亦加速 ready；`#container` 默认 `opacity:0`，提前 show 不产生空窗闪烁。**前端 `result_window_ready` 时还主动调一次 `refreshActive()`** 拉取首帧工具栏配置（`edit_shortcut` / `polish_mode` / `denoise_mode` 等），避免冷启动到首次 `show-result`（录音）/ `config-changed`（设置改动）之间，窗口内 keydown 监听器读到 `edit_shortcut` 初始默认值（2026-07-05 第八轮补；防御性——当前窗口可见即聚焦的路径多已间接触发 refreshActive，主动拉取消除对该隐式依赖）。

**核心状态机（Coordinator）：**
- 单线程 mpsc channel 串行化所有事件
- 流式模式：Streaming → (StoppingPolish) → (Polishing) → Pasting
- 离线模式（VadSegmented 伪流式）：VadSegmented → WaitingCompletion → (StoppingPolish) → (Polishing) → Pasting
- 云端流式模式（cloud feature，VAD-gated per-utterance streaming）：Streaming（cloud，`CloudPipelineEngine`，独立 100ms tick 线程）→ (StoppingPolish) → (Polishing) → Pasting；stop 时 close 在飞 → `CloudClosing` 中间态
- **StoppingPolish（Toggle 停止时立即润色仍在途）**：若用户点了「立即润色」后 LLM 未返回就 Toggle 结束录音，进入 `StoppingPolish { transcript }` 等待 `Command::PolishDone`，完成后按 `polish_mode` 走 final 路径。修复原 bug：原实现 `clear_polish_pending` 后走 final 路径，导致立即润色结果被 stage 切换丢弃 + `polish_mode=0` 时最终润色被跳过 → 只粘贴原文。**优化**：若 polished 非空且无新增 ASR（`!has_raw()`，段模型无 Raw 段即无新增），跳过最终润色直接 paste（mode=1/2 也跳过），避免平白多一次 LLM 调用
- **音频处理流水线（drain_samples → VAD → ASR，三种 stage 共用同一前处理）**：从 cpal 回调到引擎输入只走一条路径，所有降噪 / 重采样都在 `SharedAudioState::drain_samples` 内部完成，coordinator 层从不直接调 DenoiseProcessor。详见 `crates/desktop/src/audio.rs::process_pipeline`。

  ```
  cpal Stream 回调（设备原生 SR）
    │  → SharedAudioState.samples（Mutex<Vec<f32>>）
    ▼
  drain_samples()                    ← coordinator 每 tick 调用
    │  1. take(samples)
    │  2. process_pipeline(raw, SR, flush=false)
    │     │
    │     ├─ 直通路径（denoise_mode=0 / 后端加载失败 / 单帧推理失败 降级）：
    │     │     原生 SR ───────────resampler──────────▶ 16k
    │     │
    │     └─ 降噪路径（denoise_mode=1 RNNoise / 2 DeepFilterNet3，详见「环境降噪」）：
    │           原生 SR ──down_sampler──▶ 48k
    │             ──DenoiseProcessor.process_samples──▶ 48k 已降噪
    │             ──resampler────────────────────────▶ 16k 已降噪
    │
    │  GRU 隐状态跨 tick / 跨段连续保持（flush=false 保滤波器+GRU 续接，
    │  噪声估计是连续物理过程）；仅会话级 start() 调 reset()（DF3 = 重载模型）
    ▼
  samples: Vec<f32>（16k 单声道，已降噪 或 直通）—— 三种 stage 看到的是同一份
    │
    ├─ Streaming（本地流式 [`LocalPipelineEngine`]→[`asr::StreamingRunner`] / 云端流式 [`CloudPipelineEngine`]，
    │   coordinator 经 [`desktop::StreamingPipeline`]→`Box<dyn StreamingPipelineEngine>`，阶段2c-1/2c-2）：
    │     pipeline.tick(&samples, &mut transcript) → Vec<PipelineEvent>（2d，原 changed:bool）
    │       pipeline 内（承载层，local/cloud 共享）：engine.tick → TranscriptEvent（Partial/Committed/Final/Error）
    │         → Partial/Committed 幂等 apply_engine_full（changed=true）；Final 无条件覆盖；Error warn+stash last_error（下 tick 注入 PipelineEvent::Error，2d）
    │       · local engine：runner.push_samples → VAD 静音检测 → accept_samples(→Partial)
    │           → 累积静音 ≥0.5s → flush(true) 插逗号(→Committed)；stop 用 finish_with_tail(→Final)
    │     coordinator 端：dispatch_tick 调 pipeline.tick → apply_pipeline_events 路由（PersistRaw→DB / Emit→update_result /
    │       Polish→check_and_trigger_polish / Error→update_result），2d 统一事件循环（删原 handle_streaming_tick）
    │
    ├─ VadSegmented（本地离线引擎，[`desktop::VadSegmentedPipeline`]（pipeline.rs，2c-3 收编原 coordinator 逻辑））：
    │     pipeline.tick(&samples, &mut transcript) → Vec<PipelineEvent>（2d）
    │       run_tick：audio_buffer.extend + compute_speech_chunks(vad)（检测 VAD 跨 tick 累积，**切段后 reset+preroll 归零防漂移**）
    │       → 静音 ≥ segment_silence / 持续 ≥ SEGMENT_DURATION_S（20s，**force_cut 不门控 has_speech**——漂移失灵时强制清空、filter_vad 兜底）：
    │           filter_speech_from_buffer(filter_vad, send_buffer)  // 过滤 VAD（每段 reset+preroll）
    │           → spawn_blocking(engine.transcribe) → mpsc rx 按 seq 有序回填 completed_results（2c-3 删 TranscriptionDone）
    │       changed → [PersistRaw{vad_segmented}, Emit]；segment_cut → [Polish{INFINITY}]（段边界润色）
    │     coordinator 端：dispatch_tick → apply_pipeline_events 路由（2d，删原 handle_vad_segmented_tick）
    │
    └─ Streaming · cloud engine（[`CloudPipelineEngine`](../crates/desktop/src/cloud_pipeline.rs)，cfg cloud，阶段2c-2；
          coordinator 经统一 `dispatch_tick` 驱动（2d，原 handle_streaming_tick 已删），cloud 走独立 100ms `CloudStreamingTick` 线程）：
          engine 内 pre_roll_buffer 滚动追加 samples（保留后 200ms = CLOUD_PREROLL_BUFFER_SAMPLES）
          compute_speech_chunks(vad, &samples) → onset 检测（≥2 speech chunks）
          ├─ 无活跃 WSS + onset（连续 2 tick 确认，消除噪声脉冲）：
          │     cloud_pipeline::open_cloud_session(asr_engine, language, pre_roll) → CloudStreamHandle
          │       ├─ Aliyun：resolve_aliyun_config → aliyun_stream::open
          │       ├─ ByteDance：resolve_bytedance_config → bytedance_stream::open
          │       ├─ Tencent：resolve_tencent_config → tencent_stream::open
          │       └─ Baidu：resolve_baidu_config → baidu_stream::open
          │     session.push_pcm(&samples)
          ├─ 有活跃 WSS + 持续语音：
          │     push_pcm(&samples)（→ s16le / base64 → WS frame）
          │     drain_cloud_session → try_recv_text：
          │       ├─ StreamEvent::Text(partial) → current_partial = partial（**预览层，不进 transcript/DB**，仅 display）
          │       └─ StreamEvent::Finished → committed_text 逗号拼接 partial → 产 Committed 事件
          │         （承载层 apply_engine_full + coordinator DB + check_and_trigger_polish；!closing && !speaking → session.take drop）
          └─ 有活跃 WSS + 静音 ≥ pause_polish_threshold_ms：
                session.finish()（**非阻塞**，发 finish-task / 末帧负包）
                → is_closing = true（后续 tick drain 最终结果，不阻塞 coordinator）
          coordinator 端（cloud 分支，2d 事件流）：dispatch_tick → apply_pipeline_events 路由——changed(=Committed) →
            PersistRaw(DB) + Polish(停顿润色)；**每 tick Emit**（display + current_partial 预览，预览不进 DB）；
            WSS 开启失败 / StreamEvent::Failed → 承载层 last_error → 下 tick PipelineEvent::Error → update_result（删原 take_error）。
            stop → take_close_handle → spawn close_async → `Stage::CloudClosing`（close 留 coordinator，不可消除）
  ```

  **关键不变量**：
  - **降噪在 drain_samples 内部完成**——三种 stage 拿到的 `samples` 都是 16k 已降噪（或降级直通）样本；VAD 与 ASR 用同一份降噪后信号，避免参数 / 状态不一致致 VAD 误判而 ASR 准的解耦 bug。云端引擎（cloud）的 pre-roll 同样从 drain_samples 取，云端收到的是干净音频。
  - **降噪 GRU 与 VAD LSTM 状态语义相反**：降噪 GRU **跨 tick / 跨段连续保持**（`flush=false`，噪声估计是连续物理过程，仅会话 `start()` 才 reset）；检测 VAD **跨 tick 有状态累积**（看完整流，稳语音/静音边界），**切段后 `reset()` + `vad_preroll()` 归零预热**（2026-07-07 修：会话内从不 reset 会致 LSTM 跨段漂移 → 真实语音持续 prob<0.5 → `has_speech` 卡 false → "几段后不吐字"；切段点是安全重置点，preroll 与构造时对称防段首丢字）；过滤 VAD **每段 reset+preroll**（2026-07-09 审查补 preroll，与检测流对称——冷启动段首几帧 prob 偏低可能丢音头，preroll 喂静音让 LSTM 进入静音稳态；独立冷启动+预热，等价每段新 VAD 但复用 ONNX Session）。详见「VAD 分段切分策略」。
  - **降级不 panic**：`denoise_mode=0` / 后端模型缺失 / 单帧推理失败 → `process_pipeline` 走直通分支（原生→16k），仅 warn 日志，识别继续不阻断录音。
  - **cloud engine 的 VAD 用法与 VadSegmented 一致**：同一个 `compute_speech_chunks`（迁自 coordinator，现 `pipeline::compute_speech_chunks` pub(crate)）+ `SileroVad` 检测 onset，但**不切分过滤**（不调 `filter_speech_from_buffer`）——云端服务端自己有切句逻辑（DashScope server-side `max_sentence_silence` / 豆包 `show_utterances`），客户端 VAD 只负责「何时开 / 何时关 WSS」的生命周期门控。**onset 抗噪**：连续 2 个 tick（~200ms）检测到语音才开 WSS（`speech_confirm_count`），消除单次噪声脉冲导致的空 session 误触发。
- **云端流式（cloud feature，`CloudPipelineEngine`，阶段2c-2）**：当 `is_cloud_engine(cfg)`（`asr_engine` 解析 category=Aliyun / ByteDance / Tencent / Baidu）时启用——coordinator 在 `handle_toggle` 建 `CloudPipelineEngine`→`Stage::Streaming`（与本地流式同 Stage，cloud 走独立 100ms `CloudStreamingTick` 线程）。与本地 Streaming / VadSegmented 不同——**不调用 `TranscriptionEngine::transcribe`**，而是 `CloudPipelineEngine`（[`cloud_pipeline.rs`](../crates/desktop/src/cloud_pipeline.rs)）直接管理一条云端 WebSocket 长连接，由 VAD 决定连接生命周期，`tick` 产 `Vec<TranscriptEvent>` 由 `StreamingPipeline` 承载层 apply_engine_full。**四个云端 provider** 统一返回 [`CloudStreamHandle`](../crates/desktop/src/cloud_types.rs)（含 `push_pcm`/`finish`/`try_recv_text`/`close_async` 共用方法）：
  - **Aliyun**（[`aliyun_stream.rs`](../crates/desktop/src/aliyun_stream.rs)）：阿里云百炼 DashScope。**三套协议自动分发**（[`is_qwen_realtime_endpoint`] 按 endpoint 路径分流）：
    - **Fun-ASR / Paraformer**（`/api-ws/v1/inference`）：任务型协议（`run-task` → 二进制 PCM → `finish-task` → `result-generated`（按 `sentence_id` + `sentence_end` 跨句累积）→ `task-finished`）
    - **Qwen-ASR Realtime**（`/api-ws/v1/realtime`）：OpenAI Realtime 风格会话协议（`session.update` → base64 PCM via `input_audio_buffer.append` → `session.finish` → `conversation.item.input_audio_transcription.text`/`completed`）
  - **ByteDance**（[`bytedance_stream.rs`](../crates/desktop/src/bytedance_stream.rs)）：字节跳动豆包大模型 ASR 双向流式（`bigmodel_async` 优化版）。二进制帧协议（4B header + payload），gzip 压缩，固定 endpoint `wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async`，鉴权经 `X-Api-Key` + `X-Api-Resource-Id` 握手 headers。`source`=Resource ID（如 `volc.bigasr.sauc.duration`），`secret_key`=API Key。详见 [spec](superpowers/specs/2026-06-21-archived-spec.md#bytedance-asr-design)。

  **生命周期**（`CloudPipelineEngine::tick` 内编排，产 `TranscriptEvent` 不直接写 transcript/emit）：① 语音 onset（连续 2 tick 确认）→ 根据 `EngineCategory` 调 `cloud_pipeline::open_cloud_session`（内部分派到对应 provider 的 `xxx_stream::open`，建连 + 初始化 + 推 100ms pre-roll）；② 持续语音 → `push_pcm` 推帧 + `drain_cloud_session` 把 partial 写到 engine 自持的 `current_partial`（**预览层不碰 transcript/DB**，coordinator display 拼 transcript + current_partial）；③ 静音 ≥ `pause_polish_threshold_ms` → `finish()`（**非阻塞**）→ `is_closing=true` → 后续 tick drain 最终结果；④ `StreamEvent::Finished` → `committed_text` 逗号拼接 `current_partial` → 产 `Committed` 事件（承载层 apply_engine_full + coordinator DB + `check_and_trigger_polish`）→ drop session。**四个 provider 共享 `PcmFrame` / `StreamEvent` / `CloudStreamHandle` 类型**（定义在 [`cloud_types.rs`](../crates/desktop/src/cloud_types.rs)），`CloudStreamHandle` 的 `push_pcm` / `finish` / `try_recv_text` / `close_async` 为共用实现。**partial 与 transcript 分离**（消除 partial 覆盖历史文本的 bug）、**非阻塞 finish**（消除 `close()` 的 `block_on` 冻结 coordinator 线程的 bug）。**DB INSERT 时机**：cloud 只在 commit（Finished→`Committed`，承载层 `changed=true`）时 `update_transcription_raw`（INSERT/UPDATE text+segments）——与本地 Streaming 路径每次 accept_samples 都 INSERT 不同。如果整个录音过程中从未触发 Finished（用户没停顿够就 Toggle stop / 点立即润色），记录从未创建 → 后续 `Finalize` / `UpdatePolished`（均为 UPDATE WHERE id=?）静默 0 行，数据丢失。**修复**：`finalize_cloud` 在 append partial 后、`start_final_polish_or_paste` 之前先调 `update_transcription_raw` 确保 INSERT；`handle_polish_now` 在 `take_polish_input` 之前也调 `update_transcription_raw`（本地路径已 INSERT 为 no-op，cloud 路径补 INSERT）。**tick 间隔 100ms**，**pre-roll 滚动缓冲 200ms**。Toggle 停止时若 WSS 仍活跃 → spawn `close_async`（**非阻塞**，审查三1）+ 进 `Stage::CloudClosing`（持 transcript/current_partial），close 完成回 `Command::CloudStreamingDone { text, session_id }` → `handle_cloud_streaming_done` 校验 `transcript.id == session_id`（跨会话护栏，见下）后 `apply_engine_full` + finalize → 走润色/粘贴。详见 [spec](superpowers/specs/2026-06-19-archived-design.md)（§ dashscope-streaming-design，已归档）。
- **最终润色异步化**：停止后若启用润色（mode=1/2），`start_final_polish_or_paste` 进入 `Stage::Polishing`（spawn 独立线程跑 LLM 网络请求，托盘显「处理中」、结果窗显「最终润色中」），LLM 完成回调 `Command::FinalPolishDone` 后 `do_paste` 落地；未启用润色则直接 `do_paste`。**润色期间协调器线程不阻塞**，`Cancel`（Esc）可即时回滚 Idle、丢弃在途结果，`Toggle` 被互斥忽略（防并发缓存污染）。**跨会话护栏**：`Command::FinalPolishDone` 携带 `session_id`（= 发起润色时的 transcript.id），`handle_final_polish_done` 校验当前 Polishing id 匹配才落地——Cancel+重开+再润色时旧结果匹配新 Polishing 的污染被拦（与 `PolishDone` 同理）。Polishing 仅持 `id` + `raw_text`（不需 Transcript 其余字段）
- **粘贴异步化（`do_paste`）**：`do_paste` 先同步 `show_result` + 置 `Stage::Pasting`（状态机线程），再把真正的落库粘贴（`paste::paste`——含 enigo 键盘模拟 + 焦点切换 `sleep`）投递到 `tauri::async_runtime::spawn` + `tokio::task::spawn_blocking`，完成后回 `Command::PasteDone`——粘贴期间不占用 Tauri UI 主线程、不阻塞协调器线程。**macOS 键盘模拟线程安全**：`paste_via_clipboard` 的 V 键用固定虚拟键码 `Key::Other(9)`（`kVK_ANSI_V`）而非 `Key::Unicode('v')`——enigo 0.6.1 的 `Key::Unicode` 在 macOS 走 `get_layoutdependent_keycode`（循环调用非线程安全的 Carbon `TIS*`/`UCKeyTranslate` API），在 `spawn_blocking` 非主线程执行会触发 SIGTRAP（`Trace/BPT trap: 5`）；`Key::Other` 直接当 keycode 用绕过 layout 查找。详见 [spec](superpowers/specs/2026-06-17-archived-design.md)。**粘贴前输入源切换（2026-07-10）**：`paste_via_clipboard`（ASR 粘贴）和 `simulate_paste_platform`（剪贴板浮窗双击粘贴）执行 Cmd+V 前，经 `crate::input_source::switch_to_ascii_for_paste()` 临时切到 ABC 输入源（RAII guard drop 时恢复原输入源），避免 CJK IME composing 状态下粘贴出乱码。受 `switch_input_source_on_paste`（默认 `true`）控制。TIS API 线程安全分析见 [spec §3](superpowers/specs/2026-07-10-input-source-switch-design.md#3-线程安全分析)。
- **取消录音（Cancel）**：结果窗按 Esc → 前端 `invoke('cancel_recording')` → `coordinator::cancel_recording` Tauri command → `Coordinator::cancel` 发 `Command::Cancel`。`handle_cancel` 跨阶段生效——Streaming 停采集 + reset 引擎，VadSegmented 停 tick + 停采集，WaitingCompletion / Polishing 丢弃在途结果，统一回 `Idle` + 隐藏 result 窗 + 托盘置 Idle（Idle 下为 no-op）。Esc 同时 `currentWindow.hide()` 提供即时反馈（区别于运行时配置子系统的 4 个命令，`cancel_recording` 定义在 `coordinator` 模块）。**取消时清理 DB 脏数据（2026-06-21 审查修复）**：原实现仅回 `Idle` 不删除已 `INSERT` 的过程记录，导致垃圾数据遗留。修复后 `handle_cancel` 检查当前 `transcript.db_inserted()` ——`true` 则经 `DbCommand::Delete { id }` 后台删除该条未完成记录；`Polishing` / `Pasting` 阶段（仅有 `id` 无 transcript）直接删除（这两个阶段意味着已识别但被用户取消，不应保留）。**`StoppingPolish` 阶段**（Toggle 停止时立即润色仍在途）：Cancel 丢弃在途润色结果 + 删除 DB 脏数据（同其他阶段的 Cancel 语义）。与 Discard 的「保留识别历史」行为对称
- **放弃识别（Discard）**：工具栏「关闭」按钮（首位，close.svg 图标）→ 前端 `invoke('discard_recording')` → `Coordinator::discard` 发 `Command::Discard`。`handle_discard` 与 Cancel 共享停止逻辑（停采集 + reset 引擎 / 断 WSS），但**额外 finalize DB 记录**（`DbCommand::Finalize`：`raw_text` + `duration_ms` + `polish_status="off"` 入库，保留识别历史），**跳过 `do_paste`**（不粘贴、不入剪贴板）。与 Cancel 的本质区别：**Cancel 丢弃一切并删除 DB 过程记录（`DbCommand::Delete`），Discard 保留识别历史**。`Pasting` 阶段 no-op（粘贴进行中无法撤回），`Polishing` 阶段丢弃润色结果（`FinalPolishDone` 到达时若 stage 已回 Idle 或属另一会话的 Polishing，由 `session_id` 护栏丢弃）。`discard_recording` 同样定义在 `coordinator` 模块
- **音频采集按需启停（替代常驻，修复菜单栏麦克风指示灯常亮）**：`cpal::Stream` 所有权收归 `SharedAudioState`（`Mutex<Option<cpal::Stream>>`），不再 `std::mem::forget` 泄漏保活——**每次录音 `start()` 现场建流（`build_stream`）+ play，`stop()` pause + drop（take 出 Option 在本线程析构）**：空闲期无流、菜单栏麦克风指示灯灭、不触发麦克风权限；录音期间流持续播放、回调内 `is_recording` 作冗余守卫。**Send-safety（已根治）**：`cpal::Stream` 为 `!Send + !Sync`，但 SharedAudioState 的 Arc 被 `move` 进 Coordinator 的 `std::thread::spawn` 循环闭包、仅该线程独占持有（`audio` 不在 Coordinator 结构体字段），故 Stream 的建（start）/ 播（play）/ 停（stop）/ 析构（stop take-drop 或循环线程退出）全程同线程、无跨线程访问；cpal 回调线程只持有独立 clone 的 `Arc<Mutex<Vec>>`/`Arc<AtomicBool>`。`unsafe impl Send/Sync` 在此前提下 sound（注释记录该不变量）。建流失败由 `start()` 返回 `Err`、上层降级。**多采样格式支持**：`build_stream` 按 `config.sample_format()` 分派 F32 / I16 / U16 三类——F32 直接取均值；I16 → `s as f32 / i16::MAX`；U16（部分 Linux 驱动 / 老旧设备）→ `(s - 32768) / 32768`（center-zero 还原）。cpal 错误回调（如设备中途断开）从 `debug!` 提升至 `error!` 日志级别，便于故障排查
- **音频初始化防闪退**：`AudioRecorder::open()` 仅校验麦克风存在（失败 `log::error` + 仍持有静音占位 `SharedAudioState`，应用进托盘不 `expect` panic）；真正的 `build_stream` 推迟到首次 `start()`，建流失败（无设备 / 权限拒绝 / 占用）由 `start()` 返回 `Err`、上层降级（采样恒空 → 识别静默 → 空文本回 `Idle`），改配置后重启恢复
- **流式重采样器缓存**：非 16kHz 麦克风源的流式重采样经 `crates/asr-local/src/audio.rs` 的 `AudioResampler`（有状态 `rubato::FftFixedIn` + 跨帧 leftover 缓冲）——`desktop::SharedAudioState` 持 `Mutex<Option<AudioResampler>>`，源速率不变时**复用同一规划器**（避免每 tick 的 FFT planner 重规划开销，并保留滤波器跨帧状态保边界 glitch-free），仅 `stop` 时 `flush` 补零吐尾 + 置 `None`；`drain_samples` 不 flush。`AudioResampler` 经编译期断言 `Send+Sync`（固化 `SharedAudioState` 的 `unsafe impl` 前提，防 rubato 升级引入非 Send 字段静默退化为 UB）
- **环境降噪（可插拔后端，采集层前置）**：麦克风音频送入 VAD/ASR 前，经 `crates/asr-local/src/denoise.rs` 的 `DenoiseProcessor`（mode 分发器，对外接口与旧 RNNoise-only 一致）降低背景噪声。降噪为**可插拔后端**（`FrameDenoise` trait，`process_frame(&[f32;480], &mut [f32;480])` 用 `[-1,1]` 单声道契约），由 `app_config.denoise_mode` 选择：
  - `0` = 关闭（直通，零开销）。
  - `1` = RNNoise（`RnnoiseBackend`，`nnnoiseless` 纯 Rust 移植 Xiph RNNoise，内置默认模型，48kHz FRAME_SIZE=480(10ms)→频带特征 + VAD/噪声/降噪 GRU → 频带增益 → iSTFT+OLA）。**默认**。
  - `2` = DeepFilterNet3（`Df3Backend`，libDF v0.5.6 的 `DfTract` + tract 0.19，48kHz 全频带，编译期内嵌 ~7.9MB `DeepFilterNet3_onnx.tar.gz` 模型）。质量最佳（干净语音 gain≈0.96、带噪 gain≈0.60、RTF≈0.015–0.036）。DF3 **懒加载**：`new(mode=Df3)` 仅占位，首次 `process_samples` 才加载模型（避免构造热路径阻塞）。**DF3 加载失败降级 RNNoise**（非直通）——tract 为纯 Rust 后端、内部做运行时 SIMD 检测，SIGILL 风险极低；`catch_unwind` 兜底 panic；失败转 `RnnoiseBackend`（用户仍得基础降噪），仅 RNNoise 也 OOM 才最终直通。
  - 缺省 `1`（`default_denoise_mode()`）。`denoise_mode: u8` 亦可由工具栏运行时切换（`set_denoise_mode` 命令）并持久化回 DB `app_config` 表。

  **帧边界隔离 ndarray 版本**：libDF（deep_filter）依赖 ndarray 0.15，asr 现有 ndarray 0.17（ort/whisper 等）。Cargo 允许同 workspace 共存（不同 major）。`FrameDenoise` trait 只用原生 `&[f32]`/`&mut [f32]`，绝不暴露 ndarray 类型；`Df3Backend` 内部用与 libDF 同实例的 `ndarray_015`（package rename）构造 `ArrayView2 [1,480]` 喂 `DfTract::process`，asr 的 0.17 类型完全不触及。

  **DF3 依赖（git，非 crates.io）**：`df = { git = "https://github.com/Rikorose/DeepFilterNet.git", tag = "v0.5.6", package = "deep_filter", features = ["tract", "default-model", "transforms"] }`（libDF 不在 crates.io，只能 git）。tag v0.5.6 对应 commit `978576aa`，tract `^0.19.4`（解析到 0.19.16，**不可用 0.21.x**——0.21.4 在 native 有 codegen bug 致权重 NaN，连官方 `deep-filter` bin 也崩）。

  **Send/Sync**：`DfTract` 含 `Arc<dyn RealToComplex<f32>>`（无 `+ Send`）→ `!Send`，故 `Df3Backend` 经 `unsafe impl Send/Sync`（照 VST3 plugin/src/lib.rs:9-11）。安全性：`DenoiseProcessor` 在 `Mutex` 内、coordinator 单线程串行 lock+process（audio.rs:94 注释），实际无跨线程并发，unsafe 仅满足类型约束不引入数据竞争。`RnnoiseBackend`（`Box<DenoiseState<'static>>`）天然 Send，无需 unsafe。

  **状态保持与降级**：GRU 隐状态 + 特征缓冲 **跨 `drain_samples` 周期、跨 VAD 分段连续保持**（噪声估计是连续物理过程，与 `filter_vad` 每段 reset 故意相反）；新会话 `start()` 调 `reset()`（DF3 reset = 重载 7.9MB 模型，仅会话边界可接受）。链路 `process_pipeline`：原生SR→(`down_sampler`)→48k→DenoiseProcessor→(`resampler`)→16k（`flush` 语义同重采样器：`drain_samples` 不 flush 保连续、`stop` flush 取尾）。**三级降级**：`mode=0`→直通；后端加载/单帧推理失败→warn + backend 置 `None`→直通；**不 panic**、不阻断录音。无外部模型文件依赖（RNNoise 内置模型 / DF3 编译期内嵌），不进 DB、不参与引擎选择。

  **DF3 加载日志**：tract 加载 DF3 模型时刷大量 DEBUG（`tract_core::optim` 的 `applying patch`、`tract_hir::infer` 的 shape 推断），`crates/desktop/src/main.rs` 的 `tauri_plugin_log::Builder` 对 `tract_core`/`tract_hir`/`tract_onnx`/`tract_linalg` 四子模块 `level_for(Warn)` 压制；**保留** `df::tract` 自身 `Info`（`Loading model ...` / `Init encoder` / `Running with model type ...`）作加载进度信号。RNNoise 无 tract 依赖，不受影响。

  **历史**：第一版曾用第三方 `dfn3.onnx` + ort（模型缺陷压语音至 ~10%，已弃用换 RNNoise，见 [`2026-06-16-denoise-deepfilternet-design.md`](superpowers/specs/2026-06-16-archived-design.md)）；本版改用官方原生 libDF + tract（spike 验证 gain=0.958 不压语音），DF3 与 RNNoise 并存。详见 [spec](superpowers/specs/2026-06-17-archived-design.md)
- **VAD 分段切分策略**（`VadSegmentedPipeline::run_tick`，pipeline.rs，2c-3 收编自原 coordinator；完整双 VAD 架构与不变量详见 [spec](superpowers/specs/2026-07-08-vad-segmented-pipeline-design.md)）：静音边界切分（主）+ 连续超时强制切断（兜底）
  - 静音切分：检测到语音后静音 ≥ `segment_silence`（默认 400ms）→ 切分，**无 overlap**（静音是自然语句边界，下一段从干净开始）
  - 强制切断：音频缓冲达 `SEGMENT_DURATION_S`（20s 常量）→ 强制切断，**保留末尾 200ms（常量 `SEGMENT_OVERLAP_MS`）作下一段 overlap**（语句被硬切，需重叠保连贯）。**`force_cut` 不门控 `has_speech`**（2026-07-07 修）：detect_vad LSTM 漂移致 `has_speech` 卡 false 时，原 `&& has_speech` 会令 force_cut 永不触发 → buffer 无限堆积不吐字；解绑后达上限必切，由 `filter_vad`（每段 reset+preroll、不受漂移污染）独立兜底判定有无语音（无语音则不 spawn 但 buffer 已清，防堆积）。`segment_duration` / `segment_overlap` 原为 config 字段，因属实现细节（用户不可感知）已改为常量
  - **双 VAD 实例（检测流 vs 过滤，修 LSTM 状态污染）**：SileroVad 是有状态 LSTM（`compute()` 更新 `h`/`c`，`reset()` 归零）。`VadSegmented` stage 持**两个独立实例**：① 检测用 `vad`——逐 tick 喂入顺序音频、跨 tick 有状态累积（续接上下文使语音/静音边界判定更稳），喂 `compute_speech_chunks`；**切段后 `reset()` + `vad_preroll()` 归零预热**（2026-07-07 修：会话内从不 reset → LSTM 跨段漂移 → 真实语音持续 prob<0.5 → `has_speech` 卡 false → 不吐字；切段点安全：段已切完、下段从干净状态开始，preroll 与构造时对称防段首丢字）；② 过滤用 `filter_vad`——仅 `filter_speech_from_buffer` 用，**每次过滤前 `reset()` + `vad_preroll()` 归零预热**（2026-07-09 审查补 preroll，与检测流①对称：冷启动段首几帧 prob 偏低，`filter_speech` 的 `first_active` 可能偏后丢音头；preroll 喂静音让 LSTM 进入静音稳态、改善首帧响应），恢复「每段独立冷启动」语义（等价旧代码每 buffer 新建 VAD，但 ONNX Session 全局缓存（启动 preheat 加载、同 path 复用，`SileroVad::new` 仅 clone Arc + zeros h/c），过滤只 reset 不重建，兼顾正确性与性能）。分离原因：检测流已按顺序见过 `samples`，而 `send_buffer`（`overlap_tail` + `audio_buffer`）与之重叠，若共用一个有状态 VAD 会双重喂入 + 跨段污染 LSTM → 段首 gating 失真（裁掉语音起音或混入前导噪声）
  - **`filter_speech` 两端 trim（修首尾字丢失）**：检测流切出的单段经 `filter_speech_from_buffer` → `octopus_asr_local::audio::filter_speech` 过滤，**只 trim 首尾静音、保留中间全部音频**（含句内 ~50ms 停顿 / 轻声帧），**不逐帧删除**低于阈值帧——逐帧删会破坏句子连续时间结构 → 声学特征错乱 → 漏字 / 乱码 / 粘连。扫描首个 / 末个高于阈值的帧，各外扩 `SPEECH_PAD_MS`（120ms，@480 样本/30ms 帧 = 4 帧）作为起止点，补回 VAD 响应延迟切掉的首字音头、与衰减残尾被判静音的尾字尾音（参考 silero-vad `speech_pad_ms` 默认 30ms）；该 padding 远低于段间静音阈值（仅借回纯静音、不触及相邻段语音）。`transcribe_with_vad` 的 `segment_audio_vad`（>30s 长音频走此路径）共用同一 `SPEECH_PAD_MS`，段首预借 / 段尾后补同模式
  - 每段经 `filter_speech_from_buffer` 过滤静音后，由 `VadSegmentedPipeline` 内部 `spawn_offline` 派发到 **Tauri 全局异步运行时**（`tauri::async_runtime::spawn`）执行 `engine.transcribe`（底层 CPU 密集推理已 `spawn_blocking` 包裹、不阻塞 runtime worker）；闭包持 **`SendOnDrop` guard**——正常 send 后置 `done=true`，task panic（unwind）/ future cancel 时 Rust 保证局部变量 drop → guard Drop 发 Err sentinel 递减 `active_count`，保其归零（2026-07-09 审查防御：防 coordinator `WaitingCompletion` 因 `active_count>0` 永挂、吞后续 Toggle；profile=panic=unwind），完成经 **mpsc rx** 回填 `completed_results: HashMap<seq,String>` + `completed_seq` 游标连续消费（**2c-3 删 `Command::TranscriptionDone`**，改 pipeline 内部 mpsc，coordinator 不再参与段完成回调）；段间不做 overlap 去重——force_cut 段虽带 200ms overlap_tail，但仅 ≈1 字、与正常重字不可区分，曾因子串匹配误删真词（如「识别」），已移除去重逻辑改为逗号直接拼接。**识别失败 / 空结果仍占位该 `seq`（写空串）以保证游标连续推进**——否则缺失序号会让消费卡死、该次录音此后所有有效段积压丢失；**跨会话保护（2c-3）**：pipeline drop → mpsc rx disconnect，旧会话迟到段不污染新会话（原 `TranscriptionDone` 携带 `session_id` 比对 `transcript.id` 的机制随命令删除，改由 pipeline 生命周期兜底——快速双击 Toggle / 录音中重启时旧 pipeline drop 即切断其 rx，残留异步转写回调无处回填）
- **末段收尾（finish，2026-07-09 修）**：stop 时若末段 `audio_buffer` 未达切段条件（`silence_cut`/`force_cut` 都不触发——末尾静音不足 `segment_silence` 或 buffer 未满 `SEGMENT_DURATION_S`），`VadSegmentedPipeline::finish`（pipeline.rs L530）主动合并 `overlap_tail`+`audio_buffer` → `filter_speech_from_buffer` 判语音 → 非空则 `spawn_offline`+`active_count+1`。否则末段滞留丢失（`active_count==0` 时 coordinator 直接 finalize 连整句丢 → 用户报告「停录音后半句识别不到/卡住」）。末段 spawn 异步，finish 内 drain 拿不到——靠 `active_count>0` 进 `Stage::WaitingCompletion` 轮询收尾。详见 [spec §4.1](superpowers/specs/2026-07-08-vad-segmented-pipeline-design.md)
- **Transcript 段模型文本状态机**：识别文本状态由 `Transcript`（`crates/desktop/src/transcript.rs`）统一管理——内部用 `segments: Vec<Segment>`（每段 `{kind: Raw/Polished/Edited, text}`）作结构化真相源 + `caret_gap`（新语音生长缝隙，0..=segments.len()，==len 即末尾追加，默认零回归）+ `pending_delete: Option<(usize,usize)>`（选中替换待删范围，延迟消费）+ `selection_insert_offset: Option<usize>`（选中替换的插入点 = selection start，跨润色持久——润色后 caret 须恢复到此位置而非末尾）。`finish_text()` 段扁平化为唯一展示/落库/复制文本（派生，不另存）。引擎累积全量经 `apply_engine_full` 取尾部 delta 在 `caret_gap` 生长（VadSegmented 走 `append_segment`）；`set_caret`/`set_selection` 经 `split_at` 劈段定位。`Stage::Streaming`/`VadSegmented`/`WaitingCompletion` 各持 `transcript`。停止后 `Stage::Polishing`（最终润色中，持 `id`+`raw_text`）→ `Stage::Pasting`（持 `id`+`raw_text`+`polished_text`+`polish_status`）；入库的 `engine`/`engine_mode` 在过程入库的 raw 阶段已写，`Pasting` 不再持有。详见 [spec §1/§4/§11](superpowers/specs/2026-07-05-archived-design.md#三asr-光标定位与中间插入选中替换)（已归档）
- **停顿驱动润色**：流式 / 伪流式统一——静音 ≥ `pause_polish_threshold_ms`（默认 600ms，可配置）/ 伪流式段边界完成时，经 `take_polish_input()` 取 segments 快照送 LLM **全篇一次润色**（mode=2 only；润色语义见下「结果窗光标定位 + 中间插入 + 选中替换」条），**不重置流式引擎**（只读送 LLM，引擎状态原样保留）。默认 600ms > Active Flush 500ms（GUI 约束 `>= 600`，须大于句间停顿最大值，否则润色先于尾音冲刷、快照缺尾音），润色在 tick 流程最末执行，pending 期间新 delta 缓存到 `pending_delta`、PolishDone 后 flush，快照可靠
- **立即润色（PolishNow）**：工具栏「立即润色」按钮（`tool-polish-now`）点击 → `invoke('polish_now')` → `Command::PolishNow` → `handle_polish_now`：**忽略 `polish_mode`**（不受 mode=0/1/2 限制，区别于停顿润色的 mode=2 限制），经 `llm_config_ignore_mode()` 取 LLM 配置，复用 `take_polish_input` → `spawn_polish_thread(ignore_mode=true)` → `Command::PolishDone` 路径。`spawn_polish_thread` 新增 `ignore_mode` 参数控制是否绕过 mode 检查。`handle_polish_done` 接受 `Streaming`/`VadSegmented`/`WaitingCompletion`/`CloudClosing` 四阶段（防用户点按钮后停录音致 stage 切换、润色结果被丢弃；**cloud 流式走 `Stage::Streaming`、`CloudClosing` 同样是活跃会话，必须支持**，否则用户用云端引擎时点「立即润色」会被忽略），写回后 `emit("polish-done")` 通知前端恢复按钮（成功/失败/stage 不匹配均通知）。**`handle_polish_now` / `handle_enter_edit_mode` / `commit_edit_apply` 三个 transcript 操作函数都支持全部活跃会话 stage**（`Streaming`/`VadSegmented`/`WaitingCompletion`/`CloudClosing`），否则云端引擎路径下编辑/立即润色功能全部失效。**`handle_polish_now` 所有早退路径（stage 不匹配 / transcript 空 / 已 pending / LLM 配置缺失）都 emit `polish-done`**——否则前端 `btnPolishNow.disabled=true` 永久卡死。`Transcript::display_text()` 同步变更：段模型下 `finish_text()` 段扁平即展示（= display_text），润色后 raw→polished 自然反映到展示区，使 PolishNow 在任意 mode 下都能让润色文本覆盖 raw 回显
- **结果窗光标定位 + 中间插入 + 选中替换**（[spec](superpowers/specs/2026-07-05-archived-design.md#三asr-光标定位与中间插入选中替换) §1–§11，已归档）：
  - 非编辑态 Result 窗显示自定义闪烁光标（`CaretBlink` 组件，纯定位指示器，非 contentEditable）。前端点击算 char offset（code-point 计数，与 Rust `char` 对齐）→ `invoke("set_caret", {offset})` → `set_caret` 劈段定位 `caret_gap`；后续 delta 从该处生长，光标后文本右推、光标经 caret 透传链跟随（`Emit{caret}`→`update_result(..,caret)`→前端 `setCaretPos`）。
  - **选中替换（产品核心特色：随时覆盖说错的内容）**：用户选中已识别文本的任意一段，继续说话——新语音自动替换选区，而非追加在末尾。这是本产品区别于其他语音输入工具的关键能力（传统方案只能追加，无法修正中间错误）。实现机制：
    - **延迟删除**：拖选 → `invoke("set_selection", {start,end,text})` → `set_selection` 记录 `pending_delete` + `selection_insert_offset`（**不立即删字**，保留浏览器原生高亮，用户可重新选择）。`pending_delete` 在**下次引擎 tick**（`apply_engine_full` / `append_segment`）被消费——在**所有 early return 之前**无条件消费（旧代码在 delta 空 / diverted 分支 `return false` 跳过消费 → 选区永远不删的致命 bug，经历 4 轮修复才收敛到当前位置）。
    - **消费即返回 true**：即使 delta 为空（引擎静音期产出 same-as-before 的 full），只要 `pending_delete` 被消费，`apply_engine_full` 返回 `true` → pipeline 产 `Emit` 事件 → 前端即时刷新（展示删后文本 + caret 在选中起点）。否则选区虽删但前端不刷新（用户看不到变化，以为没生效）。
    - **`selection_insert_offset` 跨润色持久**：停顿触发润色时 `take_polish_input` 检查此字段——有值 → `polish_caret_at_tail=false`（强制精确恢复 caret 到 selection start），`polish_apply` 恢复 caret 后回写此字段（多次润色仍有效）。**无此字段时** caret 落末尾 → 后续新词追加末尾（选中开头的 bug 根因）。`set_caret` / `clear_pending_delete` / `commit_edit` 清零（取消选中 / 编辑提交时）。
    - **跨会话选中（Idle → Toggle 两阶段，2026-07-05 方案 C 重构）**：用户在录音结束后（Idle）选中文字 → 前端 `currentSelectionRef` 缓存 `{start,end,text}`（**不存后端**——旧 `idle_selection: Option<(text,start,end)>` 长期缓存有失焦残留 / 编辑后 stale text 指错位 / 拖选后编辑残留三类 bug，已移除）。Toggle 开新会话（全局热键，纯后端触发）**不直接开录音**：`emit("prepare-record", prepare_id)` + spawn 200ms 看门狗 + 进 `pending_prepare` 等待态；前端 listen prepare-record → `invoke("start_recording", {prepareId, selection: [text,start,end]|null})`；后端 `StartRecording` 校验 `prepare_id` 匹配 `pending_prepare` 后调 `begin_recording(selection)`（**cloud/streaming/vad 三分支对称**：`Some` → `commit_edit(text)`+`set_selection(start,end)` 种子 transcript；`None` → 普通开。Bug C 修复前 cloud 分支漏植入选区，致云端用户跨会话选中替换退化为末尾追加，已修）。看门狗 200ms 超时发 `FallbackStart` 兜底普通开（冷启动前端未 mount / 未响应）。`prepare_id` 仿 `PolishDone session_id` 跨会话/超时护栏防重复开。延续态 `show_result` 发占位符 + `update_result` 发旧文本（不走 show-result else 分支，否则前端把非占位符当最终文本清空 caret）。**代价**：普通录音也走 emit→前端回推往返（~10-30ms，`audio.start` 本身更慢，占比小）。等待态中断：再按 Toggle/Cancel/Discard 取消等待；SetCaret/SetSelection no-op。
    - **`coordinator` 的 `SetSelection` 命令**（仅 `start`/`end`，跨会话选区的全文改由 `start_recording.selection` 携带，`text` 字段已移除），活跃 stage → `set_selection`（延迟消费）；Idle/等待态 → no-op（跨会话选中改由前端 `currentSelectionRef` 在 prepare-record 时推回）。前端 `onMouseDown` 时在 `document` 上注册一次性 `mouseup` listener（鼠标移出 textRef/浮窗时 React `onMouseUp` 不触发），handler 内按 `isCollapsed` 分流（折叠→set_caret 中插；非折叠→clampRangeToContainer 裁剪后 set_selection + 写 `currentSelectionRef`）；`blur`/选区折叠/`enterEdit`/`commitEdit`/`cancelEdit`/`show-result`/`clear-result`/`hide-result` 时清 `currentSelectionRef` 防 stale。
    - **前端拖选三重陷阱（2026-07-05 修复，耗时极长，务必避免重踩）**：从右往左拖选到文本开头时选中替换失效，根因是三个独立的 WKWebView/浏览器行为叠加：
      1. **`Range.startContainer` 飘移到父容器**——从右往左选到开头时鼠标划出 textRef 左边界，浏览器把 Range 的 startContainer 设为父容器节点 → `el.contains(range.commonAncestorContainer)` 返回 `false` → 选中逻辑被跳过。修复：`clampRangeToContainer` 用 `compareBoundaryPoints` 把 Range 强制裁剪到容器内（`setStart`/`setEnd` 到 `selectNodeContents(el)` 的边界）。
      2. **React `onMouseUp` 不在 textRef 外触发**——鼠标移出浮窗到其他应用区域释放时，React 的 `onMouseUp={handleTextMouseUp}` 不执行（事件不经过 textRef）。修复：`onMouseDown` 时在 `document` 上注册一次性 `mouseup` listener（`addEventListener` + handler 内 `removeEventListener`），任何位置的 mouseup 都能捕获。
      3. **mouseup 时鼠标在容器外**——`caretRangeFromPoint` 返回的节点不在容器内。修复：用 `el.getBoundingClientRect()` 判断鼠标 X 坐标——`< rect.left` → offset=0（开头），`> rect.right` → 末尾，在容器内 → `caretRangeFromPoint` 精确定位。
      - **通用教训**：WKWebView 中拖选到容器边界是一个高频踩坑区——浏览器选区 API（`Selection`/`Range`）在边界处行为不稳定（容器飘移、选区折叠、节点不在容器内），不能直接信任 `window.getSelection()` 的原始值。三重防御（Range clamping + document mouseup + 坐标边界判断）缺一不可。`mousedown` 时缓存起点 offset 是兜底方案，让选区重建完全不依赖 mouseup 瞬间的 DOM 状态。
  - **编辑态**：coordinator 主循环 `editing` 标志置位时 tick 跳过喂引擎（硬暂停）。`commit_edit(flat)` 整篇压成单 `Edited` 段（raw/polished 清零）+ UPDATE DB（v14 `segments` JSON + `text` 列）。
  - **润色全篇一次**：edited 段冻结 preserve、raw/polished 重润（best-effort 串匹配回填）；pending 期间新 delta 缓存到 `pending_delta`，PolishDone 后 flush（段模型无 `raw_len`/`increase`，pending 期间段不变，比旧三文本分层模型更不易 flicker）。
  - **CaretBlink 踩坑**：组件须接收 `RefObject`（effect 内读 `.current`），不可传 `container={textRef.current}` 作 prop——editing 切换致 textRef 重挂载时 render 阶段读到 detached 旧 div，光标错落首位（f32f1a9 修，详见 spec §11.6）。
  - **前端渲染健壮性（spec §12，2026-07-04 4 bug 修复）**：① **textRef 是 contentEditable div，React 19 对其 children 的 commit 不写 DOM**（保护用户编辑，`flushSync` 也无效）→ `renderResultNow` 须 imperative `textRef.textContent = newText`（非编辑态）同步 DOM、`measureCaretPx` 长度读 DOM `firstText.nodeValue`（非 state text，否则 state 新 text 算 target、DOM 旧文本 clamp 到旧末尾 → 光标错位 + 新文字空白）、`flushSync(setText)` 驱动 state 让 `CaretBlink` effect 触发重测——这是「流式追加文字不渲染」的真根因；② `CaretBlink` 须监听 scroll（passive + rAF 节流）重测 px（视口相对，随 `scrollTop` 变）+ 视口外（`px.top` 超 `[0, clientHeight]`）隐藏，否则上滚后光标停容器底旧位闪烁错位；③ `onScroll` 恢复 `stickToBottom=true` 时立即 `scrollTop = scrollHeight`（不等下个 tick 100-200ms），消除「滚回底部最新文字滞留视口下方空白」间隙；④ textRef div 加 `whitespace-pre-wrap`，否则编辑态 `innerText` 的 `\n` 在 `white-space:normal` 下折叠成空格（后端 `commit_edit`/`finish_text`/DB `text` 列全保留 `\n`，纯前端 CSS 问题）。后续代码审查又修 §13/§14 四 bug（diverted 计时器漏清覆盖最终文本 / `CaretBlink` 初始 measure 改 rAF 消 layout thrashing / `measureCaretPx` 多文本节点定位 / `enterEdit` 按进入前点击位恢复光标）。
  - **前端单测基建（2026-07-04，commit e797e0f）**：引入 vitest 4 + jsdom 29；`measureCaretPx`/`codePointOffsetTo`/`codePointOffsetBefore`/`placeCaretAtCodePoint` 抽到 `Result/caret.ts`（纯函数可测，`locateCpOffset` 为 measure/place 共享 helper），`caret.test.ts`（14 测）锁 code-point → UTF-16 offset 对齐 + null/空容器/多文本节点分支；jsdom 无 `Range.getBoundingClientRect`，defineProperty 补零矩形；`renderResultNow` 组件级测试留后续。
  - **前端审查追加修复（spec §13/§14，2026-07-04）**：第三/四/五轮代码审查补 4 个 Result 窗前端 bug——① show-result else 分支（最终/插入态立即渲染）须 `clearTimeout(divertedTimer)` + 清 `pendingDiverted`，否则误判 diverted 启动的 300ms 计时器会覆盖刚落地的最终文本（§13.1）；② CaretBlink 初始 `measure()` 改 rAF（同帧 `flushSync`+`textContent` 写后同步读 `getBoundingClientRect` = 强制回流，高频 ASR 每帧叠加；代价 1 帧光标滞后）（§13.2）；③ `measureCaretPx`/`placeCaretAtCodePoint` 经 `locateCpOffset` 多文本节点遍历定位（`whitespace-pre-wrap` 多行/编辑残留 `<br>` 下 pos 越首节点不再 clamp 错位，防御性）（§14.1）；④ `enterEdit` 进编辑前用 `caretPosRef` 捕获点击位、`placeCaretAtCodePoint` 恢复，否则光标无条件落末尾（§14.2，仅纯点击可恢复，拖选 caretPos=null 仍落末尾）。**第七轮（2026-07-05）补同源漏清**：clear-result / hide-result 分支也须 `clearTimeout(divertedTimer)` + 清 `pendingDiverted`（与 §13.1 show-result else 分支同源）——录音取消 / 隐藏窗时若有 pending diverted 300ms 计时器在跑，到期回调 `renderResultNow` 会把废弃纠正文本写回 `text` / `displayedRef`（state 污染 + 违背 clear 语义）；show-result placeholder 分支早已清，取消/隐藏两路径此前漏清。
- VAD 标点：基于 SileroVad 静音检测，>0.5s 静音插入逗号。**段间拼接标点去重**：`consume_completed_results` 在段间补逗号前同时检查「新段不以标点开头」和「已有文本不以标点结尾」，避免 ASR 引擎返回的自带句尾标点与补的逗号连续出现（`。，` `？，`）
- 流式尾音冲刷（Active Flush）：流式模式累积静音 ≥0.5s 时把憋住的尾音即时吐出——Zipformer 用 edge-replicate lookahead padding（3 chunks，与 `finish` 共享 `run_padding_flush`）对齐右上下文，Paraformer 用 CIF force-fire（`run_cif_final`）；**同时追加逗号**（`flush(insert_comma=true)`），提供即时分句反馈——此前逗号只在下一句话到来时插入，停顿期间无标点。每个静音段仅触发一次（`flushed` 标志，恢复说话时重置）。详见 [spec](superpowers/specs/2026-06-14-archived-design.md)
- **Paraformer 流式尾部 CIF force-fire**：CIF 机制 alpha 累积达阈值 1.0 才 fire 产出 token，`finish()` 时残留 alpha >0 但 <1.0 的声学特征卡在 `encoder_out_cache` 不触发 → 最后一个字被吞。`run_cif_final()` 在 CIF 循环结束后检查残留，alpha >0.5 则 force-fire 为最后一个 token 送 decoder（<0.5 视为噪声不 fire）；sherpa-onnx 官方也丢弃此残留（已知 trade-off），我们做了改善
- **Paraformer 流式 3 个严重 bug 修复**（sherpa-onnx 源码对照）：①离线 CMVN 重复 `* scale`——`extract_cmvn_from_metadata` 已在 inv_stddev 乘 sqrt(512)，`transcribe` 又乘一次 → 特征放大 22.6 倍，移除重复；②流式位置编码缺负号——`k_scale` 应为 `-ln(10000)/(half_dim-1)`，缺负号导致高维频率爆炸、随音频变长退化；③`process_chunk_final` 仍 mask 右侧 alpha——尾部 3 帧无下个 chunk 处理，mask 掉永久丢失 ~180ms 语音，新增 `mask_alphas_left_only`。另：`flush()` 最后一个 chunk 也走 force-fire（`run_cif_final`）；`accept_samples` 恢复逗号但加 `ends_with_punct` 防重复标点
- **Paraformer fbank 特征提取修复（5 个根因）**（详见 [spec](superpowers/specs/2026-06-21-archived-spec.md#paraformer-fbank-feature-extraction-fix)）：流式识别质量严重退化（token 重复 `thedayday`/`tomtomor`、英文粘连无空格）的根因全部在 fbank 层。①**缺 DC offset removal**——每帧 FFT 前未减帧均值（sherpa-onnx 默认 `remove_dc_offset=true`）；②**缺 pre-emphasis**——未做 `y[i]=x[i]-0.97*x[i-1]` 预加重（sherpa-onnx 默认 `preemph_coeff=0.97`）；③**窗口函数错误**——流式应用 povey 窗 `(0.5-0.5cos)^0.85` 而非 hamming 窗；④**mel 滤波器 high_freq 错误**——应用 7600 Hz（`high_freq=-400`）而非 8000 Hz；⑤**流式架构缺陷**——重叠 chunk 重复提取 fbank 致帧边界断裂，重写为**增量式 fbank**（`raw_samples` 线性追加 + `fbank_cache` 增量计算，对齐 sherpa-onnx `OnlineFbank`）。另：`decode_tokens` 重写为 sherpa-onnx `Convert()` 兼容的空格逻辑（英文词间加空格、`@@` BPE 合并）；新增 `smart_append()` 在 chunk 边界拼接时检测 ASCII↔非 ASCII 过渡插入空格
- **Fbank 特征提取参数化**（`paraformer.rs::compute_fbank`）：统一签名 `compute_fbank(samples, window, preemph_coeff)`——流式 Paraformer 用 povey 窗，离线 Paraformer 用 hamming 窗。**Pre-emphasis 无跨帧状态**：帧重叠（shift=160 < len=400）时上一帧末尾 ≠ 本帧 start-1，故直接从连续缓冲回溯 `samples[start-1]`（减本帧 mean 近似去直流），消除 `preemph_prev` 状态字段。离线 `ParaformerEngine` 与流式 `StreamingParaformer` 共享同一 `compute_fbank` 但窗口参数不同。Mel 滤波器（`MEL_FILTERBANK` static）high_freq=7600 Hz，mel 空间三角权重
- **Paraformer BPE 跨 chunk 整体解码**：模型输出的是 BPE 子词 token（如 `val@@` + `ue`），非完整单词。`StreamingParaformer` 累积 `all_token_ids: Vec<i64>` 跨所有 chunk，`accept_samples`/`flush` 整体 `decode_tokens(all_token_ids)` 返回完整 ASR 文本——避免 chunk 边界各自解码导致 BPE 续接断裂（`val`/`ue` 分开）。`StreamingSession` Paraformer 路径用 `punct_prefix`（已提交 ASR + 逗号）+ `committed_chars`（已提交字符数）管理逗号：静音点冻结快照 + 插逗号，新 delta = `full_asr.skip(committed_chars)` 拼在逗号后
- **Paraformer 流式热路径性能优化**（零拷贝，每 chunk 节省 ~420KB 堆分配 + 消除 FFT 重复规划）：① decoder_caches 更新用 `copy_from_slice` 复用预分配 Array3（省 16×320KB）；② encoder 输入用 `into_shape` 零拷贝 reshape（省 45KB clone）；③ `run_cif`/`run_cif_final` 用 `as_slice().ok_or_else(|| anyhow!(...))?` 直接拿 `&[f32]`（省 20-40KB to_vec，非连续内存时返回错误而非 panic）；④ decoder input 键名 `in_cache_0..15` 预分配为 `cache_keys: Vec<String>`（省 16× format!）；⑤ **FFT 规划提升为全局静态** `FBANK_FFT: Lazy<Arc<dyn rustfft::Fft<f32>>>`（`paraformer.rs`，与 `POVEY_WINDOW` 同位置），`compute_fbank` 与 `StreamingParaformer::compute_new_fbank_frames` 共用——消除每 chunk `FftPlanner::new()` + `plan_fft_forward(512)` 的堆分配 + twiddle 规划计算；⑥ `apply_feat_overlap` 用 `ArrayView2::from_shape` 包装 `&self.feat_cache`（省 8×560×4B ≈ 17.5KB clone）；⑦ `run_decoder` 用 `ArrayView3::from_shape` 包装 `acoustic: &[f32]`（省 `to_vec()` 拷贝，与 `qwen3_asr.rs` 的 `ArrayView3+TensorRef` 模式一致）；⑧ `run_decoder` 的 `enc_len` / `acoustic_len` 单元素张量用栈数组 `[x]` + `ArrayView1::from(&[x])` 替代 `Array1::from_vec(vec![x])`（省 2 次微小堆分配）；⑨ `reset()` 的 decoder_caches 清零按形状分治：形状仍为初始 `(1, encoder_output_size, cache_time)` 时 `fill(0.0)` 复用内存，被 run_decoder 慢路径改过维度时才重分配恢复初始形状；⑩ 离线 `transcribe`（`paraformer.rs`）CIF 循环改用 `enc_tensor.slice(s![0, ..enc_len_scalar, ..]).as_slice().ok_or_else(|| anyhow!(...))?` 直接借用，消除 `enc_tensor.clone().into_raw_vec_and_offset()` 的整段 encoder 输出拷贝，`enc_tensor` 保留供 decoder `view()` 使用；附带将离线 transcribe 的 `speech_lengths` / `acoustic_len` / `enc_len_for_dec` 单元素张量统一为栈数组 + `ArrayView1`
- **mask_alphas 越界防护**（`streaming_paraformer.rs`）：`mask_alphas` / `mask_alphas_left_only` 取 `n = alphas.len().min(enc_len)` 再循环，消除 ONNX 返回异常尺寸时 `alphas[i]` panic 风险
- **Paraformer 边界鲁棒性防御**：① `smart_append`（`paraformer.rs`）边界空格判定追加 `last_byte != 0x20 && first_byte != 0x20`，避免 `existing` 末尾或 `new` 首字符已是空格时再 push 空格导致双空格（空格 `0x20` 本身满足 `< 0x80` ascii 判定）；② `run_cif` / `run_cif_final`（`streaming_paraformer.rs`）的 encoder slice 改为 `..enc_len.min(enc_tensor.shape()[1])`，防御 ONNX `enc_len_data[0]` 与实际张量维度不一致（padding/截断异常）时的 slice panic，与 `mask_alphas` 同模式
- **Paraformer accept_samples 清 input_finished（会话内状态污染修复）**：`input_finished` 标记在 `flush()` 静音冲刷时置 `true`，让 `compute_new_fbank_frames` 末帧越界零 padding 多算帧、配合 CIF force-fire 吐尾音；仅 `reset()`（会话边界：录音停止 / 取消）清除。**Paraformer 流式会话内不 reset**（累积上下文跨 chunk），故用户停顿冲刷尾音后继续说话时 `accept_samples` 若不清，`input_finished` 持续 `true` → 每次多算越界零 padding 帧 → 特征错乱 → **首次停顿后会话级乱码 / 丢字 / 大量重复字**（本「首字 / 尾字」专项最严重的一个）。修复：`accept_samples` 入口置 `false`（语义 = 继续说话，回正常帧计算模式）。详见 [spec](superpowers/specs/2026-06-21-archived-spec.md#paraformer-fbank-feature-extraction-fix) §10
- **流式 partial 渲染单调性（防闪烁）**：`StreamingZipformer::process_chunks` 三个返回点（sample_buffer 空 / 样本不足凑 chunk / 末尾）统一经 `decoded_current()` 返回当前段文本——避免「样本不足凑 chunk 时早退返回 None、`StreamingSession` 丢 current_segment 只回 accumulated」导致长短态逐帧交替闪烁（coordinator 每 tick drain ~3200 样本，凑不够 chunk 时走早退）。`StreamingPipeline::tick` 承载层加幂等门（`text != transcript.full()` 才 apply_engine_full，changed 才产 PersistRaw+Emit，2d），消除静音期 flush 同文本反复重绘。前端 `update-result` listener 单调渲染：新文本是已显示内容的前缀（`startsWith`）则立即渲染并清待处理跳变；跳变 / 段切换延迟合并（`DIVERTED_DELAY_MS=300`）只渲染最新，连续跳变不闪烁。
- **设置窗口子系统（settings_commands + settings_window）**：独立 Tauri 窗口 `settings_window`（`settings_window.rs`），原生标题栏、800×600 可调。`set_config(key, value)`（通用字段写入器，`apply_config_value` 做字段类型/范围校验；**快捷键先注册后持久化**：`asr_shortcut` / `clipboard_shortcut` / `screenshot_shortcut` / `edit_global_shortcut` / `polish_global_shortcut` 先 `unregister` 旧的 + `register` 新的（`clipboard_shortcut` 经 `clipboard_window::register_clipboard_shortcut`、`screenshot_shortcut` 经 `screenshot_commands::register_screenshot_shortcut`、`edit_global_shortcut` 经 `result_window::register_edit_global_shortcut`（handler 调 `trigger_global_edit`：show+set_focus 结果窗——CM6 改造后仅唤起窗口，不再 emit toggle 事件）、`polish_global_shortcut` 经 `result_window::register_polish_global_shortcut`（handler 调 `trigger_global_polish`：show 结果窗**不聚焦** + emit `global-polish-trigger` → 前端 `polishNow`）），注册成功才写共享 `AppConfig` + `save_app_config`（30 字段），**任一失败则恢复旧快捷键并返回 Err**（前端 toast 报冲突；`clipboard_shortcut` / `screenshot_shortcut` 此前用 `let _ = on_shortcut(...)` 吞错——冲突时静默存入无效配置、重启后仍失败，已统一为回滚策略）。`edit_shortcut` / `hide_toolbar` / `clipboard_enabled` 改动发 `config-changed` 事件让结果窗 `refreshActive` 刷新 + 设置页/浮窗 `clipboard_enabled` toggle 双向同步）。**系统设置页**（GeneralPanel.tsx）：6 张卡片——交互（麦克风/降噪/识别工具栏自动隐藏/剪贴板监听 clipboard_enabled）、模型选择（语音识别模型 asr_engine / 润色模型 polish_llm / OCR 模型 ocr_model——asr 下次录音、polish 立即、ocr 因 `OcrEngine` OnceLock 单例需重启生效）、快捷键（语音识别/立即润色/语音编辑/剪贴板浮窗；窗口内编辑默认 CmdOrCtrl+Enter（跨平台）不在此管理）、语音识别（识别语言/硬件加速/拼音纠错/简繁输出/句间停顿）、语音识别润色（润色模式/润色提示词/润色间隔/润色停顿阈值）、剪贴板（最大保留条数/自动清理天数）。OCR 模型选项来自 `list_ocr_models`（models 表 `domain='ocr' AND is_enabled=1`）经 `build_ocr_options_public` 组装进 `ConfigResponse.ocr_models`；asr/polish 行从原「语音识别」「语音识别润色」Card 移入此 Card 集中管理。快捷键按钮用 `ShortcutButton` 组件（kbd 标签风格，⌘/⌥/⇧ 符号），捕获时过滤纯修饰键。「引擎接入」section 已移除（embedded 模式不需要用户配置）。润色模型 select 用 `llm_models.find(m => m.current)?.name` 匹配当前选中（避免 3-part spec 与裸名不匹配的问题）。
- **运行时配置子系统（SharedRuntimeConfig）**：工具栏可运行时切换 `asr_engine` / `polish_mode` / `polish_llm` / `denoise_mode`，无需重启。`runtime_config.rs` 提供 `SharedRuntimeConfig`（`type = Arc<RwLock<AppConfig>>`，挂 `tauri::State`）——**完整 `AppConfig` 的唯一真相源**，取代旧 `RuntimeConfig` 部分镜像（消除字段同步遗漏，新增运行时生效字段零同步代码）。8 个 Tauri 命令（`toolbar_state` / `list_asr_engines` / `switch_asr_engine` / `set_polish_mode` / `list_llm_models` / `switch_polish_llm` / `set_denoise_mode` / `polish_now`）读写共享 `AppConfig`（即时生效）+ `persist_*` best-effort 持久化回 `~/.octopus/app_config 表`（写盘失败仅 `warn`，本次仍生效、重启回退；`polish_now` 不写盘，只触发润色流程）。**`switch_asr_engine` / `switch_polish_llm` 前端传裸 `model_name`，后端查 DB 取 `provider` / `category` 构造 3-part spec（`"{provider}:{category}:{model_name}"`）写入共享 `AppConfig` + app_config 表**——保证持久化值与 `parse_model_spec` 解析一致。`list_*` 的 current 判定经 `parse_model_spec(current).model_name()` 提取裸名比较，兼容 3-part 和裸名两种历史格式。`switch_asr_engine` 同时经 `tray::update_tray_engine_label` 实时刷新系统托盘菜单的「引擎: <model_name> (<mode>)」项（`TRAY_ITEMS` 缓存 `engine_info` MenuItem handle，`set_text` 更新而非重建，规避 `MenuItem::with_id` 重复 ID panic）。Coordinator 闭包持共享 `AppConfig` 句柄，**在 Toggle 进入 `Idle` 时重读 `asr_engine` / `polish_mode` / `polish_llm` 并经 `resolve_active_engine` 校验有效性——保留完整 3-part spec（`rc.asr_engine.clone()`）写回 `config.asr_engine`，失效则兜底 `local:zipformer:zipformer-small-ctc`**，保证 `is_streaming_engine` 判定 / `use_streaming` 重算 / `StreamingSession::new` / 离线 `transcribe` / transcriptions.engine 记录全用完整有效 spec；`main.rs` 启动 preheat 同样解析（preheat 与实际工作模型一致）。**外部修改共享 `AppConfig` 后立即同步到 coordinator（2026-06-18 改进）**：`set_config`（设置窗口）和 `switch_polish_llm`（工具栏浮层）写完共享 `AppConfig` 后调 `coordinator.update_runtime()` → `Command::UpdateRuntime` → `sync_runtime_fields` 把 `polish_llm` / `polish_mode` / `asr_correct` / `output_simplified` / `hide_toolbar` 同步到 config 快照，**无需 Toggle 即可生效**（用户在录音中改 polish_llm 下次润色就用新模型）。`asr_engine` 不走此路径（需重建引擎实例）。`polish_mode` 仍保留每 tick 读 `set_mode`（双保险立即生效）。详见 [spec](superpowers/specs/2026-06-16-archived-design.md)

**文本持久化（嵌入式 SQLite）：**
- 存储：`~/.octopus/octopus.db`（`crates/infra/src/db.rs`，全局 `OnceLock<parking_lot::Mutex<Connection>>`（无毒化）；asr crate 经 `pub use octopus_infra::db` 以 `crate::db` 暴露；cli/server/desktop 共用）
- `clipboard_history` 表（schema v22，统一存储）：识别历史 = item_type='voice' 条目。结构化真相源为 `segments`（段 JSON `[{kind:raw|polished|edited, text}]`）+ `content`（= `finish_text()` 扁平）。meta_info JSON 存 `{engine, asr_mode, char_count, polished, polish_model, duration_ms}`。v17 废弃原 `transcriptions` 表（db.sql 不再含此表）。
- **过程增量入库（schema v18）**：voice 条目 id = 识别开始毫秒时间戳（`INTEGER PRIMARY KEY`，应用写入）。入库时机分散到识别过程各事件，每次同步写 `segments`（真相源）+ `content`（扁平）+ `meta_info`（json_set 更新 char_count/polished/duration_ms）：首次有 ASR 文本 → `insert_transcription_at_id`（INSERT voice 条目）；分段 / 流式 partial → `update_text_segments`；停顿润色完成 → `update_polished`；停止 → `finalize_transcription`（含 `duration_ms`）。DB 失败仅 `warn` log 不阻塞识别（best-effort）。详见 [DB 合并 spec](superpowers/specs/2026-07-05-archived-design.md#八db-表合并--fts5-搜索)（已归档）。
- **非阻塞 DB 写入（actor 模式）**：上述 `INSERT`/`UPDATE`/`finalize` 不在协调器线程同步执行——`update_transcription_raw` / `PasteDone` 等调用方仅 `db_queue::get_db_sender().send(DbCommand)` 入队后立即返回，真实落库由**后台 DB 写线程**（`static DB_SENDER: OnceLock<Sender<DbCommand>>` 懒加载 spawn，位于 `db_queue.rs`）单线程消费。mpsc 的 FIFO 保证同 id 的 `Insert` 必在 `UpdateRaw` 之前被消费（故 `mark_db_inserted()` 在 send 后即置位仍安全——真实顺序由 channel 保，不由标志位保）。识别主循环不再被 SQLite I/O 阻塞。
- **关机优雅 drain**：后台写线程 `&'static Sender` 永不 drop，进程 kill 时队列里未处理命令会丢失（典型路径：录音结束 → `Finalize` 入队 → 用户立即退出 → 该条记录停留未 finalize 态）。`db_queue::shutdown_db()` 置 `DB_SHUTDOWN`（AtomicBool）→ 后台线程排空 `try_iter()` 剩余命令后退出，主线程 `JoinHandle::join` 等待落库完成；`main.rs` 挂到 `tauri::RunEvent::ExitRequested`（macOS Cmd+Q / 关闭最后一个窗口触发），保证退出前队列清空。
- `models` 表：模型目录（**唯一来源**，schema 见 `crates/infra/src/db.sql`，首次建库 `user_version=0` 时整体执行一次 seed；本地 ASR（is_local=1，12 行）初始**全 `is_enabled=0`**（待下载就绪），默认兜底引擎 `zipformer-small-ctc` 代码写死（`FALLBACK_ASR_ENGINE_NAME`）不占 seed 行，`app_config.asr_engine` 空/不匹配时 `fallback_engine` 硬构造（见下「全局默认引擎」）；列 `domain` / `provider` / `category` / `model_name` / `source` / `secret_key` / `language` / `is_local` / `is_thinking` / `is_streaming` / `is_enabled` / `description`，唯一键 `UNIQUE(domain, provider, category, model_name)`；`load_models_at` 仅读 `domain='asr' AND is_enabled=1`，`domain='llm'` 经 `load_llm_model(spec)` 按 `{provider}:{category}:{model_name}` 3-part spec 读；引擎激活由 `app_config.asr_engine` 决定，无 `is_active` 列，见「模型管理」）
- **`app_config` 表（v3+，替代旧 `config.yaml`）**：应用行为配置的统一存储（29 字段 key-value TEXT，含 `category` 分组列默认 `'default'` + `description` 描述列），由 `db.sql` seed 默认值 + `load_app_config()` 按字段类型解析。写入用 `ON CONFLICT DO UPDATE SET config_value`（仅改值，保留 description + category）。旧 `config.yaml` 首次启动时一次性导入 DB 后重命名为 `.bak`（迁移逻辑在 `init_schema` 中）。新增 `active_polish_prompt` key（存 prompts 表 id 字符串，默认 `'1'`）。
- **`prompts` 表（v4+，润色提示词管理）**：多 prompt 管理（替代旧单文件 `VOICE_POLISH.md`）。列：`id`（PK AUTOINCREMENT，用户不可编辑）/ `title`（用户可读别名，允许重复）/ `category`（固定 `voice_text_polish`）/ `content`（风格规则，不含增量逻辑）/ `description` / `is_system` / 时间戳。seed 2 条系统内置：`id=1` 默认润色 + `id=2` 进阶润色（断续纠正），均 `is_system=1`（不可编辑/删除）。`app_config.active_polish_prompt` 存激活 id（默认 `'1'`）。`llm::prompt::build_system_prompt(content) = content + INCREMENTAL_RULE`（第 7 条增量规则代码常量强制拼接，用户不可见）。启动时 `main.rs` 从 DB 读 active prompt → `set_system_prompt`；设置窗口 6 个 Tauri 命令（`list_prompts` / `get_active_prompt` / `set_active_prompt` / `create_prompt` / `update_prompt` / `delete_prompt`），切换即时生效（`set_system_prompt` 写 `RwLock<String>`，下次润色用新 prompt）。
- `model.json` / `history.txt` / `record.txt` 已从代码彻底删除——DB 是唯一配置/存储源
- `polish_status` 基于润色调用结果：未启用→`off`；启用且返回非空→`done`；启用但返回空或失败→`failed`
- 润色三档（`polish_mode`：0 关闭 / 1 仅最终 / 2 中间+最终）：中间润色由 `check_and_trigger_polish` 在停顿点触发（流式静音 ≥ `pause_polish_threshold_ms`（默认 600ms）/ 伪流式段边界），把 `Transcript.take_polish_input()`（完整 ASR；已编辑时分块 `edited + 新增`）送 LLM 润色，节流 `polish_interval`（下限 `MIN_POLISH_INTERVAL_SEC=1.0s`）；最终润色在 `start_final_polish_or_paste`（停止后）：启用润色→`Stage::Polishing` 异步线程跑 LLM，回调 `Command::FinalPolishDone` 后 `do_paste`；未启用→直接 `do_paste`。详见 [设计](superpowers/specs/2026-06-14-archived-design.md)。
- 停止空文本边界：Toggle 停止录音时若 `transcript.full()` 为空（麦克风静音 / VAD 未检出语音），`start_final_polish_or_paste` 空文本分支直接回 `Idle`，必须对称清理 `result_window::hide_result` + `tray → Idle` 两类 UI 反馈（缺一则"正在聆听…"框残留）；详见 [设计 §4.5](superpowers/specs/2026-06-14-archived-design.md)。**云端 WSS 连接失败（2026-06-21 审查修复）**：`CloudPipelineEngine::tick` 中 `cloud_pipeline::open_cloud_session` 返回 `Err` 时，除 `error!` 日志 + 复位 `is_speaking=false` 外，**产 `TranscriptEvent::Error("⚠️ 云端连接失败：<msg>")`**（承载层 last_error → 下 tick `PipelineEvent::Error` → `apply_pipeline_events` → `update_result`，2d 删原 `take_error`），让用户即时感知错误而非卡在"正在聆听…"假死状态。session 由 `!is_closing && !is_speaking` 分支自动 take，下次语音 onset 重开 WS（瞬时抖动自动重试；持续失败如 Key 无效每次 onset 报错，用户可见可排查）

支持三种引擎接入模式：
- **embedded**（默认）：内嵌 octopus-asr-local，本地推理
- **remote-ws**：通过 WebSocket 连接远程 octopus-server
- **remote-grpc**：通过 gRPC 连接远程推理服务
- **云引擎（cloud feature）**：`app_config.asr_engine` 解析为 `provider='aliyun'`（`EngineCategory::Aliyun`）时，路由 `AliyunEngine`（desktop crate，`cloud` feature 后）；`provider='bytedance'`（`EngineCategory::ByteDance`）时直接走 `CloudPipelineEngine`（`Stage::Streaming` cloud 分支，无独立 engine）。均不走 `engine_mode` 分支。详见下方「云端 ASR 引擎」
- **远程超时保护**：`WsRemoteEngine` / `GrpcRemoteEngine` / `AliyunEngine` 的 `transcribe` 均以 `tokio::time::timeout(8s)` 包裹（连接 + 收发全程），`health_check` 同样 `timeout(3s)`——规避网络断开 / 后端无响应致 ASR 队列无限期卡死。超时返回 `Err`，经序列空洞修复的空串占位分支保证 `completed_seq` 连续推进、不拖死后续分段

**系统状态页（system_status 模块，2026-07-08）：** 设置窗「系统状态」tab 的后端资源监控，辅助排查内存类问题（如截图窗口 Object URL 泄漏）。后端持续采样 + 推送，前端订阅展示。

- `crates/desktop/src/system_status_commands.rs`：
  - `SystemStatusSampler`（Tauri State 单例，`main.rs` setup 创建 + `manage` + `start`）：常驻 tokio 后台循环每 2s（`SAMPLE_INTERVAL_SECS`）用 sysinfo 采样 octopus 自身进程（`pid = std::process::id()`）的 RSS + CPU、系统级 used/total + global CPU，写入容量 60 的 `RingBuffer`（2 分钟窗口，满则丢最旧），组装 `SystemStatusSnapshot` 更新 `current` + `app.emit("system-status", snap)`。
  - **持久化 `sys: Mutex<System>`**：CPU 使用率基于「两次刷新差分」，必须跨 tick 保留基线（每 tick 新建 System 会恒为 0）。`new()` 预热一次 `refresh_cpu_usage()` 建基线，`sample_and_emit` 依次 `refresh_processes`(默认带 `.with_cpu()`，进程级 cpu）→ `refresh_memory`（系统级 used/total）→ `refresh_cpu_usage`（global CPU，`refresh_memory` 不刷 CPU）。锁序：sys 锁先 `drop`，再取 ring→current（不嵌套），emit 在所有锁外。
  - `get_system_status` 命令：返回 `current` 完整快照（前端首屏 `invoke`）。
  - `ModelMemoryRegistry`（`Arc<ModelMemoryRegistry>`，sampler 持有）：模型内存估算表，`inner`（active 列表，状态页展示当前加载中模型）+ `estimated`（首次估算值持久缓存，跨 unload/reload 保留）；`upsert_active(id, bytes)` 写 active（覆盖）并把首次值 `or_insert` 进 estimated（防偏低污染），`estimated(id)` 取首次缓存值，`remove(id)` 仅清 active 保留 estimated 供 reload 复用，`mark_active_unmeasured(id)` 仅写 active 占位 0、**不写 estimated**（After `now<=before` 测不到差值时用，下次 reload 走 estimated miss 重算可自愈）——修复 OCR idle 释放后重载状态页永久缺条目 + ASR 淘汰后重载 ort arena 复用致估算偏低（2026-07-08）+ `now<=b` 条目永久缺失（2026-07-09 审查 Q2）。
- **模型加载埋点（依赖反转）**：`crates/infra/src/model_probe.rs` 提供全局 `set_probe(ProbeFn)` / `probe(LoadPhase::{Before,After,Unload}, id)`，未注入时 no-op（infra 不依赖 sysinfo/desktop）。**`probe` 实现 clone 闭包（Arc 引用计数 +1）后释放锁再调用**（2026-07-08，避免持锁执行用户闭包——fallback 路径 sysinfo 扫全部进程慢、会阻塞其他线程的 probe）。asr-local（`load_engine_into_cache` cache miss 段、`SileroVad::new` cache miss 段、`StreamingSessionManager::switch_model` 流式引擎加载段）与 ocr（`OcrEngine::instance` 首次加载 + `run_ocr` idle 释放后重载段）在加载前后调 `probe`，id 形如 `"asr:<name>"` / `"vad:silero"` / `"ocr:<name>"`（流式与离线 ASR 同 `asr:` 前缀，同一模型算一条）。`StreamingSessionManager` 设 `max_cache=2`，`set_active` 入缓存前淘汰非 active（保护正用）+ `probe(Unload)`，对齐离线 `AsrEngineManager`（2026-07-09 审查 Q3/Q4——原「流式不设上限」决策被用户多引擎切换场景推翻）。desktop `start()` 注入闭包：Before 读进程内存（`read_self_probe_memory`：macOS phys_footprint、其他平台 RSS）存 `before_map`（key `(ThreadId, id)`——多线程并发加载同一未缓存模型时按线程×模型配对，防 before/after 错拿致估算失真）、After 优先 `registry.estimated(id)` 命中复用首次值（reload 场景，不算偏低差），未命中再读算差：`now > before` → `registry.upsert_active`；`now <= before`（ort arena 复用 / 并发释放，差值测不到）→ `mark_active_unmeasured`（登记 active 占位、不写 estimated，下次 reload 重算）；`Unload` 分支 → `registry.remove(id)`（仅清 active 列表，estimated 保留供下次 reload；OCR idle 释放、ASR 缓存淘汰 `cache.remove` 后补 `probe(Unload, "asr:{k}")` 与 OCR idle 释放对称）。
- **边界**：sysinfo 读取失败（`process(pid)=None`）→ `log::warn` + 跳过本次、保留上次快照；采样循环 `catch_unwind(AssertUnwindSafe)` 包裹单次采样，panic 不影响主进程。
- 前端：`Settings/index.tsx` `NAV_ITEMS` 加「系统状态」tab（`Activity` 图标，自包含面板放 `!configResp` 守卫前）→ `SystemPanel.tsx`：mount `invoke('get_system_status')` 首屏 + `listen('system-status')` 增量（`newerSnapshot` 按 `sampled_at` 严格大于去重）+ unmount unlisten（cancelled 标志防 race）。布局 B：顶部汇总（进程总内存 + 系统 CPU）+ 内存/CPU 并排 Card（各带手画 SVG sparkline）+ 模型估算列表（标注「约」+ RSS 差值口径说明）。纯逻辑（`fmtBytes`/`sparklinePoints`/`newerSnapshot` + 双指标 `fmtBytesOrDash`/`sparklineDataFromNullable`）抽 `systemStatusMath.ts` + colocated 单测（仓库惯例，先例 `viewportMath.ts`）。
- **双指标 + OCR idle 释放（2026-07-08 精炼）**：① 进程内存双指标——sysinfo 的 `resident_size`（RSS）含 mmap 模型权重（偏高 ~450M），与 macOS 活动监视器 `phys_footprint` 口径不一致；新增 macOS `proc_pid_rusage` FFI（`read_self_phys_footprint`，flavor `RUSAGE_INFO_V0=0`——非 16，16 是另一套 `proc_info` API 的 `PROC_PIDRUSAGE`；读 `RusageInfoV0.ri_phys_footprint` 字节偏移 72；非 macOS 返回 None）→ `ProcessStats.real_bytes: Option<u64>`、`TimeSeries`/`SamplePoint` 加 `real`；前端 macOS 主显实际占用辅 RSS、非 macOS 显 RSS（`hasReal` 切换 Card 标题/主数/sparkline）。② OCR idle 60s 释放——`OcrEngine.inner` 改 `Mutex<Option<RapidOcr>>`（+`last_used`+`model_name`），首次 `instance()` spawn **std::thread 守护线程**（ocr crate 共享 cli/server，无 tokio runtime 假设）每 30s 检查 idle>60s → `*inner=None`（drop RapidOcr）+ `probe(Unload)`；`run_ocr` 重载（不调 probe，避免刷新 registry 首次估算）与 `run` 合并到同一 inner lock 作用域（消除守护线程在「重载后、run 前」无锁窗口竞态释放致 `expect` panic）。ASR/VAD 常驻不动。**OCR 释放后进程内存数值不立即下降**（macOS allocator 行为，非 bug）：RapidOcr drop 后 ort session 内存走 `malloc/free`，libmalloc free 不主动 `munmap` 归还物理页；真实收益是「重载复用 free list 不重新涨」+「压力可回收」，决定接受现状 + 文档/状态页说明（未做 ort 禁 arena / `malloc_zone_pressure_relief`，效果未验证），状态页 OCR 条目消失即为释放成功标志。

### octopus-download（通用下载器）

通用文件下载 crate（分块并发 + 断点续传 sidecar + SHA256 校验 + 镜像 fallback），解终端用户下载大模型的三痛点：需装 Python + huggingface-cli、国内需切镜像、无参数 hf-cli 拉整个仓库（实际只需 int8 量化文件）。两模块：

- **`core`（通用，零 HF 知识）**：`Downloader` 走 probe（GET Range bytes=0-0 取 total/accept-ranges/etag）→ 规划分段 → 并发 Range+seek-write → 进度聚合（mpsc）→ 校验 → 原子 rename。`DownloadTask { url, mirrors, dest, expected_hash }`。sidecar `<dest>.part.resume.json` 记录各段进度（`url_hash` 基于 dest、镜像无关，故镜像源可复用进度），支持崩溃续传；最终整文件 SHA256 校验兜底（不注入 If-Range，避免不支持它的镜像回退 200 全文重传）。
- **`hf`（HuggingFace 适配层）**：`fetch_siblings`（GET /api/models/{repo} 解析 rfilename/etag/lfs.oid）、`should_download`（手写 fnmatch，`*` 跨 `/`，对齐 hf-cli，已 Python golden 验证）、`resolve_tasks`（构造每文件 DownloadTask：镜像 URL 在前 + 官方 fallback + LFS→Sha256(lfs.oid) / 非 LFS→Etag）。

下载到 `target_dir/{repo}/{path}`（由调用方决定）。**已接入模型管理（阶段1）**：cli `download` 子命令薄封装此 crate（构 `HfRequest` → `resolve_tasks` 解析 siblings + glob 过滤 → 逐文件 `Downloader::download`，进度经 mpsc 推送打印），`target_dir = ~/.octopus/models`，落 `~/.octopus/models/<repo>/<path>/`；`resolve_model_dir` 已加该路径为查找级（见下节）。镜像优先级 `--mirror` > config `download_mirror` > 官方源。详见 spec `superpowers/specs/2026-06-21-archived-spec.md#download-model-integration-design`。

### octopus-dlp（视频/音频下载转码 sidecar）

独立子进程二进制（`octopus-dlp`），由 cli `transcribe-url` 子命令经 `tokio::process` spawn 调用（先找 `~/.octopus/bin/octopus-dlp`，缺则 `cargo run -p octopus-dlp` 兜底），把在线音视频 URL 转成 ASR 可消费的 16kHz mono PCM。流程：`prepare_dependencies`（确保 `yt-dlp` + `ffmpeg` 在 `~/.octopus/bin` 或 PATH）→ `yt-dlp --dump-json` 取元数据 → `yt-dlp -f ba/b` 下载最佳音频流 → `ffmpeg` 转码分离。

- **输出协议**：stdout 纯 f32le PCM 采样流（默认，主进程逐 chunk 读取送 ASR）；stderr **首行**输出元数据 JSON（`{title, duration, author}`，`VideoMetadataOutput`）——分离 stdout/stderr 避免元数据混入 PCM 字节流。`-o/--output <FILE>` 改输出 WAV 文件（16kHz mono，44B 头）而非流式 PCM。
- **缓存复用**：下载文件名 = URL 的 MD5（`~/.octopus/tmp/{md5}.{ext}`）；`--unclear` 且文件已存在则跳过下载，跨次复用同一缓存。
- **临时文件清理（RAII）**：`DownloadedFileGuard` 在 drop 时删除下载的临时文件（`--unclear` 保留），覆盖所有退出路径——ffmpeg `spawn()?`/`wait()?` 的 `?` 提前返回、正常完成、`exit(1)` 均触发清理，避免转码失败时下载文件泄漏磁盘（2026-07-09 审查修复）。

详见 spec `superpowers/specs/2026-07-09-dlp-sidecar-design.md`。

## 模型管理

模型配置**唯一来源**是 `~/.octopus/octopus.db` 的 `models` 表。小模型（VAD + 默认 ASR）随应用打包到固定路径，开箱即用；大模型按需下载——`octopus-cli download <repo>`（命令行）或设置窗口「模型管理」页（GUI）下到 `~/.octopus/models/<repo>/`（阶段1 接 `octopus-download`），兼容旧 hf-cli 下到 `~/.cache/huggingface/hub/` 的模型。

**GUI 模型管理（设置窗口页面 3，5 tab 化 2026-07-10）**：`ModelsPanel` 重构为 5 tab——常量（环境变量编辑器）/语音识别/文本模型/扫描识别/翻译模型。每个模型 tab 分**本地**和**云端**两个 section（`is_local` 字段区分），顶部显示「当前使用」模型标记（左 voice 色条横幅）。section 用 `CollapsibleSection` 组件可折叠（ChevronDown 旋转，默认展开）。视觉：胶囊式 pill tabs + 左色条编码模型状态（voice=就绪、border=禁用）+ `max-w-[560px]`。`crates/desktop/src/model_commands.rs` 命令——
- `list_downloadable_models`：**v2 直读 DB** `list_all_local_asr_models`（`domain='asr' AND is_local=1`，**不过滤 is_enabled**），按 `is_enabled` 显示就绪/下载。
- `download_model(repo)`：**v2 先探查** → 未命中下载。**变量模板替换**：repo 中 `{huggingface}` 等占位符替换为 DB `category='env'` 环境变量实际值（替代旧 `download_mirror` 前缀拼接）。
- `verify_model`：完整性 sha256 复核。
- `get_env_vars` / `set_env_var` / `delete_env_var_cmd`：环境变量 CRUD（category='env'，内置 huggingface/modelscope/github 不可删）。DB v22 迁移补 seed。

**is_enabled 语义 = 文件就绪（v2）**：`true`=文件完备可被引擎加载，`false`=未就绪/未下载。写 DB 后调 `asr::config::reload_models_config()` 刷新 AsrConfig 缓存（`RUNTIME_CONFIG` v2 改 `RwLock<Option<Arc<AsrConfig>>>`，对齐 `APP_CONFIG` 模式），让「系统设置」引擎下拉即时更新——未就绪的模型不进下拉。local 模型 `secret_key` 重载为「文件清单 + sha256」JSON（api 模型仍是 key，按 `is_local` 分支，不冲突）。前端 `dist/settings/models.js`（IIFE 隔离；卡片按 is_enabled 显示「✓ 已就绪（+重新校验）/ 下载」；`index.html` 仅两处局部改动——`#page-models` 容器 + `<script src="models.js">`）。manifest（文件清单 + sha256，map 格式存 secret_key）下沉 `asr::manifest`，desktop/cli 共用；**cli `octopus-cli sync-models`** 批量扫描就绪本地模型、自举写 secret_key + 同步 is_enabled（首次填充/批量复核）。spec `superpowers/specs/2026-06-21-archived-spec.md#model-management-gui-design` §9。

```
~/.octopus/
├── octopus.db          # 嵌入式 SQLite（models + clipboard_history + app_config + prompts + image_data + action_bar_items + script_runs 表，唯一存储）
├── config.yaml.bak     # 旧 config.yaml 迁移后的备份（首次启动自动生成，可安全删除）
└── models/
    ├── silero_vad_v4.onnx   # VAD（1.8M，find_silero_vad 固定加载，随包）
    ├── zipformer/           # 默认 ASR（27M，随包）
    └── <HF repo>/           # ★ cli download 下的大模型（如 Systran/faster-whisper-large-v3/）

~/.cache/huggingface/hub/   # 旧 hf-cli 大模型缓存（兼容：resolve 第 4 级仍查此处）
```

**模型目录解析（`config::resolve_model_dir`）** —— source 字段四级查找（纯本地 IO，不联网 / 不下载）：
1. `~/.octopus/<source>`（随包小模型，如 `models/zipformer`）
2. 绝对路径（`source` 是绝对路径且存在）
3. `~/.octopus/models/<source>`（★ cli download 下的大模型，优先于旧 hf-cli 缓存）
4. `find_hf_cache`（`~/.cache/huggingface/hub/models--<repo>/snapshots/<hash>/`，兼容旧 hf-cli）

模型缺失时报错，提示运行 `octopus-cli download <source>`（不自动下载，保持 resolve 纯查找语义）。

**统一 DB 存储（v3+）：**
所有配置统一存储在 `~/.octopus/octopus.db`（SQLite），不再使用独立 config.yaml 文件：

| 表 | 用途 | 初始化方式 |
|----|------|------------|
| `models` | 引擎/LLM 模型配置 | db.sql seed |
| `clipboard_history` | 剪贴板 + 识别历史（统一存储 text/voice/ocr/image/file） | 运行时写入 |
| `app_config` | 应用行为配置（key-value TEXT，字段随 AppConfig struct 增长） | db.sql seed + yaml 迁移 |
| `action_bar_items` | AI 命令面板菜单项（自引用 parent_id 两级菜单，5 种 action_type，shortcut 组合快捷键） | db.sql seed + 运行时 CRUD |
| `script_runs` | script 执行记录（stdout/stderr/exit_code/耗时，截断 64KB） | 运行时写入 |

- **应用行为配置** `app_config` 表 → `infra::config::AppConfig`（`octopus_infra::config::load_config()` → `db::load_app_config()`）。schema 统一定义在 infra，asr/desktop/cli 共享。值统一 TEXT 存储。
- **load/save 机制（serde 自动，2026-07-07 重构）**：`load_app_config_at` / `save_app_config_at` 不再手动逐字段枚举，而是以 `AppConfig::default()` 的 JSON 形态作为类型模板，把 DB TEXT 按模板类型还原（Bool→"true"/"false"、Number→i64 先 f64 后、String→原样），再 `serde_json::from_value` 反序列化；save 用 `serde_json::to_value` 遍历所有字段 upsert。**字段增删自动反映，无需手动维护字段列表**。parse 失败保留 default（同旧行为）。历史手动枚举曾 4 次踩坑（新增字段漏注册 load/save → 内存改了不写库 / 重启回退默认值），见 archived specs 2026-06-28。
- **回归守卫**：`app_config_roundtrip_all_fields` 测试——为每个字段设哨兵值，save→load 后 Debug 格式全比较，任何字段未往返都会失败。
- **config-changed 事件（无条件 emit，2026-07-07 修复）**：`set_config` 写 DB 后**无条件** `emit("config-changed")`，前端收到后幂等重读 `get_config` 刷新。旧代码只对 `hide_toolbar`/`edit_shortcut`/`clipboard_enabled` 三个 key emit（手动白名单，与 load/save 手动枚举同反模式，踩坑 5 次——`clipboard_tab_modifier` 漏白名单导致改了不立即生效）。**注意**：无条件 emit 只影响前端值同步，不改变"需重启"字段的语义——`ocr_model` 等用 `OnceLock` 单例的字段仍需重启，它们不监听 config-changed 事件。
- **新增配置字段清单**（serde 重构后只需 3 处，非旧的 7 处）：
  1. `crates/infra/src/config.rs`：`AppConfig` struct 加字段 + `#[serde(default = "default_xxx")]` + `default_xxx()` fn + `Default` impl 初始化
  2. `crates/desktop/src/settings_commands.rs`：`apply_config_value` match 加校验+赋值分支（如需类型/范围校验）
  3. `crates/infra/src/db.sql`：`app_config` seed INSERT 加新行（新安装用户；老库 serde default 兜底）
  - **load/save 已自动跟随**，无需改 `db.rs`。round-trip 测试自动覆盖新字段。
- **DB 模型目录** `models` 表 → `asr::config::AsrConfig`（`octopus_asr_local::config::load_config()`，首次 `db::ensure_db()` 自动建表 + seed，读后缓存到 `RwLock<Option<Arc<AsrConfig>>>`——v2 可刷新：模型管理页 `set_model_enabled`/`set_model_secret_key` 后调 `reload_models_config()` 从 DB 重读替换，引擎下拉即时更新；对齐 `APP_CONFIG` 模式）。
- **配置持久化**：`persist_*`（单键 `save_config_key`，ON CONFLICT 仅改 config_value）、`set_config`（全量 `save_app_config`，serde 遍历所有字段 ON CONFLICT），均写 DB。旧 `write_config_yaml` 已移除。
- **yaml 迁移**：全新库（`user_version < 17`）建库时检测旧 `~/.octopus/config.yaml` → 解析导入 DB 覆盖 seed → 重命名为 `config.yaml.bak`。迁移逻辑在 `init_schema` 中一次性执行（导入后库即 v22，后续启动跳过）。
- **`write_to_clipboard`**（默认 `true`）：粘贴后是否把识别结果留在剪贴板，方便他处再粘贴；与 `paste_method`（`clipboard` / `direct` / `none`）构成三模式矩阵——`clipboard` 模式 true 时不恢复原剪贴板内容、false 时恢复（`paste_via_clipboard` 按 `files > image > text` 优先级用 `ClipboardBackup` 备份原内容——图片 `read_image`/`set_image`、文件 `read_files`/`write_files`、文本 `read_text`/`write_text`——ASR 文本粘贴后还原，旧实现只 `read_text` 导致图片/文件被空串吞掉丢失）；`direct` 模式 true 时 enigo 输入后末尾写剪贴板、false 时不碰剪贴板；`none` 模式忽略此配置（其唯一目的就是写剪贴板）。`false` 时三种粘贴行为等同重构前现状（不破坏现有用户习惯）。详见 [spec §6](superpowers/specs/2026-06-14-archived-design.md)。
- **`switch_input_source_on_paste`**（默认 `true`，仅 macOS）：粘贴前临时切换到 ASCII 输入源（ABC）→ 模拟 Cmd+V → 恢复原输入源（三段式文本注入，参考 VoxFlow `VoxFlowTextInsertion`）。CJK 输入法 composing 状态下模拟 Cmd+V 可能乱码/丢字符，此配置根治。实现 `crates/desktop/src/input_source.rs`——用 `osascript -l JavaScript`（JXA）在独立进程调 Carbon TIS API（v3 终版：v1 直接 FFI → SIGTRAP；v2 GCD `dispatch_sync_f` → 仍 SIGTRAP；v3 独立进程 main thread 天然满足 TIS 要求）。RAII guard `InputSourceGuard` 构造时切到 ABC（当前已是 ABC 则跳过，省 fork）、drop 时恢复。`paste_via_clipboard`（ASR 粘贴）和 `focus_tracker::simulate_paste_platform`（剪贴板浮窗双击粘贴）两条路径均接入。详见 [spec §3](superpowers/specs/2026-07-10-input-source-switch-design.md#3-线程安全分析与实现演进)。

**UI 主题系统（2026-07-07，借鉴 Wox）：**
- **配置**：`clipboard_theme` 字段（AppConfig），存主题 id（`light` / `glass-dark` / `nord` / 用户自定义）。serde 自动 load/save，`config-changed` 无条件 emit 触发全窗口热切换。
- **3 套内置主题**（`crates/desktop/src/theme.rs`）：Warm Paper（纸质感暖灰浅色）、Obsidian Glass（黑曜石深色 `#121216`）、Nord Aurora（北极极光冷蓝 `#2e3440`）。暗色主题用**纯不透明实色**——CSS `backdrop-filter` 在 Tauri WebView 下无法实现 Wox 的原生 NSVisualEffectView 均匀模糊。
- **token 体系**：标准 Tailwind 语义色（`background`/`foreground`/`muted`/`accent`/`border`/`voice`）+ 3 个扩展 token：`surface`（不透明背景，result_window/截图工具栏用）、`tool-icon`（result_window 工具栏图标色）、`icon-filter`（截图工具栏图标 CSS filter，暗色=`brightness(0) invert(1)` 反色黑色 SVG）。
- **图标适配两类**：SVG `<img>`（截图/剪贴板/图片查看器工具栏）用 `var(--icon-filter)` 在暗色主题反色；Lucide React 图标（编辑器/图片查看器的缩放/自适应按钮）靠父容器设 `color: var(--color-foreground)` 让 `currentColor` 继承。
- **应用机制**（经四次性能优化，最终架构）：
  1. `index.html` 阻断脚本：body 解析前同步从 localStorage 恢复 `data-theme` + 自定义主题 CSS 注入（零 IPC，不依赖 `__TAURI_INTERNALS__`——该对象在 `<head>` 解析时尚未注入）
  2. `main.tsx`：JS bundle 加载后执行，此时 `__TAURI_INTERNALS__` 已就绪——对非透明窗口设 `html.style.backgroundColor` 防白屏（截图窗口设半透明黑底）
  3. `index.css`：3 套 `[data-theme="xxx"]` 预编译规则块，属性选择器命中（消除运行时 var() 覆盖开销）
  4. `App.tsx`：mount 时异步 `applyThemeFromConfig` 校正（首次运行/清缓存/多窗口不同步时必需）+ 监听 `config-changed` 事件驱动
  5. 后端 `list_themes` OnceLock 缓存 + `get_theme_id` 轻量单键读
- **用户扩展**：`~/.octopus/themes/*.json` 可新增自定义主题（同 id 覆盖内置），`list_themes` 命令合并返回。
- **剪贴板浮窗键盘导航**（2026-07-07）：搜索框持焦模型，`↑↓` 移动选中（边界夹紧不循环）、`Enter` 粘贴（复用 `paste_clipboard_item` 双保险）、空内容 `←→` 切 tab / 有内容让位光标、`Tab` 恒定切 tab、`<修饰键>+1..7` 跳 tab（修饰键可配置 `clipboard_tab_modifier`，macOS Option 用 `e.code` 而非 `e.key`）。详见 [spec](superpowers/specs/2026-07-07-clipboard-keyboard-nav-design.md)。

**引擎选择（单一真相 = `app_config.asr_engine`）：**
- `models` 表无 `is_active` 列（开发期 schema 变更采用删库重初始化——见 `crates/infra/src/db.sql` 注释；`init_schema` 仅 `user_version < 22` 时执行 db.sql 建表+seed+yaml 导入（v22 跳过），v17+ 库跑增量迁移升到 v22，无 DROP 兜底）。
- **provider × category taxonomy**（`provider`=vendor/运行位置，与 `category`=引擎族/模型系列 正交）：

  | `provider` | ASR（`category`） | LLM（`category`） |
  |---|---|---|
  | `local` | `zipformer`/`whisper`/`sensevoice-orig`/`paraformer`/`qwen3-asr`/`moonshine`/`firered` | —（暂无本地 LLM） |
  | `aliyun` | `Fun-ASR` / `Paraformer-Realtime`（run-task 协议，`/api-ws/v1/inference`）/ `Qwen-ASR`（OpenAI Realtime 协议，`/api-ws/v1/realtime`） | `qwen` / `deepseek`（经 DashScope 代管） |
  | `bytedance` | `Doubao-ASR`（豆包大模型 ASR 双向流式，`bigmodel_async`，二进制帧协议） | — |
  | `tencent` | `Tencent-ASR`（腾讯云实时语音识别，WebSocket HMAC-SHA1 签名鉴权） | — |
  | `baidu` | `Baidu-ASR`（百度智能云实时语音识别，WebSocket START 帧鉴权） | — |
  | `deepseek` | — | `deepseek`（直连） |
  | `bigmodel` | — | `glm`（智谱） |

- **模型选择 spec（`asr_engine` / `polish_llm` 统一 3-part 格式）**：配置字符串支持 `"{provider}:{category}:{model_name}"` 三段格式从 DB `models` 表唯一定位模型（见 [spec](superpowers/specs/2026-06-17-archived-design.md)）：
  - `"local:zipformer:zipformer-small-ctc"` → 4 字段精确匹配本地 zipformer
  - `"aliyun:Fun-ASR:fun-asr-realtime"` → 云端 DashScope FunASR（run-task 协议）
  - `"aliyun:Qwen-ASR:qwen3-asr-flash-realtime"` → 云端 DashScope Qwen-ASR Realtime（OpenAI Realtime 协议）
  - `"aliyun:Paraformer-Realtime:paraformer-realtime-v2"` → 云端 DashScope Paraformer 实时（run-task 协议）
  - `"bytedance:Doubao-ASR:doubao-asr-1.0-streaming"` → 云端豆包大模型 ASR 1.0（bigmodel_async 二进制帧协议）
  - `"tencent:Tencent-ASR:16k_zh"` → 云端腾讯实时语音识别（16k 中文通用，HMAC-SHA1 签名鉴权）
  - `"baidu:Baidu-ASR:15372"` → 云端百度实时语音识别（中文加强标点 dev_pid=15372，START 帧鉴权）
  - `"aliyun:qwen:qwen-plus"` / `"deepseek:deepseek:deepseek-v4-flash"` / `"bigmodel:glm:glm-4-flashx"` → LLM
  - 裸名 `"{model_name}"`（无冒号）→ 仅全局 fallback 路径用（跨 provider/category 搜，优先 local）
  - 旧 2-part（1 冒号）→ warn + 裸名兜底（迁移期）
  - 统一解析在 `infra::db::parse_model_spec`（返回 `ModelSpec::Full` / `NameOnly`），ASR 经 `asr::config::resolve_engine_in_config` 查找，LLM 经 `infra::db::load_llm_model` 查找。区分三段是因为 DB 唯一键是 `UNIQUE(domain, provider, category, model_name)`，不同 provider 或 category 下可有同名模型（如 `deepseek-v4-flash` 在 deepseek 直连与 aliyun 代管下各一行）。
- 全局默认引擎由 `resolve_active_engine(asr_engine)` 解析：**兜底引擎短路**（裸名为 `zipformer-small-ctc` 时跳过 DB 查找，直接返回硬构造兜底 entry，不触发 warning）→ 其余 spec 匹配命中则用；空/不匹配回退兜底 `zipformer-small-ctc`（`DEFAULT_ASR_MODEL_DIR` 本地打包路径，开箱可用）。返回 `ResolvedEngine.model_name` 始终是**裸名**（去掉前缀），下游缓存和加载按裸名工作。
- **云引擎路由（`provider='aliyun'` → `AliyunEngine`，`provider='bytedance'` → 豆包流式，`provider='tencent'` → 腾讯流式，`provider='baidu'` → 百度流式）**：`resolve_active_engine` 解析时若 `provider='aliyun'` → 返回 `EngineCategory::Aliyun`；`bytedance` → `ByteDance`；`tencent` → `Tencent`；`baidu` → `Baidu`（均由 `resolve_category(provider, category)` 按 provider 分支识别——`engine_category_from_str` 对云 provider 返回 `None`，靠 provider 而非 category 映射）。`desktop/src/main.rs` 启动时 `resolve_active_engine` → `Aliyun` 建 `AliyunEngine`（需开 `aliyun` feature）；`ByteDance` / `Tencent` / `Baidu` 不建独立 TranscriptionEngine（只支持流式），直接经 `is_cloud_engine` 路由到 `CloudPipelineEngine`（`Stage::Streaming` cloud 分支）。云 ↔ 本地切换改 `app_config.asr_engine` 后**重启**生效。
- **流式判定数据驱动**：是否走流式识别由 `models.is_streaming` 列决定——`is_streaming_engine(cfg)` = `resolve_active_engine(cfg.asr_engine).entry.is_streaming && category != Aliyun && category != ByteDance && category != Tencent && category != Baidu`（seed：zipformer×2 + paraformer×4 + Qwen-ASR Realtime = 流式；whisper / sensevoice-orig / firered / qwen3-asr×2 / moonshine×2 / aliyun Fun-ASR / Paraformer-Realtime / bytedance Doubao-ASR / tencent Tencent-ASR / baidu Baidu-ASR = 非流式），不再按 category 硬编码匹配。**云端引擎（Aliyun / ByteDance / Tencent / Baidu）被显式排除**——其 `is_streaming=1` 表示支持云端 WS 流式（aliyun feature），而非本地 `StreamingSession`；aliyun feature 未启用时也不会错误进本地 streaming 路径。**流式引擎内部分流**：`StreamingSession::new` 检测 `decoder.onnx` 存在性——CTC 走 `StreamingZipformer`（单 session log_probs argmax），Transducer 走 `StreamingZipformerTransducer`（三 session RNN-T greedy decoding，跨 chunk 维持 `token_buf`）。**云端引擎走 `Stage::Streaming` cloud 分支（`CloudPipelineEngine`，cloud feature gated）**——Toggle 进 Idle 时 `is_cloud_engine`（检测 Aliyun / ByteDance / Tencent / Baidu）分支先于 `use_streaming` 判断并 `return`。**StreamingSession::new 失败降级**：引擎不可用时（如模型文件缺失 / category 不支持）自动降级到默认引擎 `local:zipformer:zipformer-small-ctc` 重试（warn 日志），再失败才放弃录音——避免用户选了不可用引擎后录音白白启动即失败。`run-octopus.sh` 默认启用 `--features "embedded aliyun"`，否则云端引擎不可用。Coordinator 的 `use_streaming` 据此在 Toggle 进入 `Idle`（切引擎 / 切模式）时重算——流式引擎走本地流式 partial，非流式引擎自动回退 VAD 分段伪流式。`StreamingSession::new` 同样走 `resolve_active_engine`（带兜底），与 `is_streaming_engine` 对称——避免 DB 未命中时 `is_streaming_engine` 兜底成功（→ 进 streaming 路径）但 `StreamingSession::new` 创建失败（→ session 错误）。
- 显式参数（cli `--model`、server 请求 `engine`、`AsrEngineManager.switch_model`）优先级更高，支持 spec 格式、**不走兜底**（匹配不到直接报错）。
- VAD 模型固定路径（`find_silero_vad` 直接返回 `~/.octopus/models/silero_vad_v4.onnx`），不进 DB、不读配置。
- **手编 `models` 表 / `app_config` 表需重启进程生效**（`OnceLock` 缓存，运行中不可热更新；运行时修改走 `RuntimeConfig` + `persist_*`）。DB schema `user_version` 当前为 22（开发期简化：以 db.sql 为唯一真相，增量迁移仅 v17→v22 链——FTS5 backfill + ALTER TABLE 补列 + env seed；schema 变更直接改 db.sql + 升 user_version，删库重初始化）。

### 云端 ASR 引擎（AliyunEngine + ByteDance 流式）

#### AliyunEngine（阿里云 DashScope，分块式）

`crates/desktop/src/engine_aliyun.rs`（`cloud` cargo feature 后，默认不开）impl `TranscriptionEngine`，接入阿里云百炼 DashScope 实时语音识别 WebSocket。与本地引擎不同：**不在 ASR crate 内**，而在 desktop crate——因为它是分块式 `TranscriptionEngine`（每段 VAD 开一条 WS 跑完整协议），与本地离线引擎共享 coordinator 的 chunk 路径接口（`is_streaming=0` → 不进本地 `StreamingSession`）。

**三套协议自动分发**（`is_qwen_realtime_endpoint` 按 endpoint 路径分流）：

| 接口 | endpoint | 协议 | model_name seed |
|---|---|---|---|
| Fun-ASR | `/api-ws/v1/inference` | 任务型（run-task） | `fun-asr-realtime` |
| Paraformer | `/api-ws/v1/inference` | 任务型（run-task） | `paraformer-realtime-v2` |
| Qwen-ASR | `/api-ws/v1/realtime` | OpenAI Realtime 风格 | `qwen3-asr-flash-realtime` |

- **Fun-ASR / Paraformer 协议流程**（`run_session`）：① parse_model_spec → 取 model_name → 查 `cfg.asr.aliyun[model_name]` 拿 endpoint + secret_key（空则 bail 明确报错含 sqlite3 命令）；② WS 握手 + `Authorization: Bearer <key>`；③ 发 `run-task`（text frame，streaming=duplex，format=pcm，sample_rate=16000，language_hints，**`input:{}` 必须在 `payload` 内部**）；④ 流式发二进制 PCM 帧（f32[-1,1]→s16le，200ms 分块）；⑤ 发 `finish-task`；⑥ 收 `result-generated` 按 `sentence_id` + `sentence_end` 跨句累积（heartbeat=true 跳过）；⑦ `task-finished` 收尾。段级超时 8s。
- **Qwen-ASR Realtime 协议流程**（`run_qwen_realtime_transcribe`）：① URL 追加 `?model=<model_name>`；② WS 握手 + `Authorization: Bearer <key>`；③ 发 `session.update`（Manual 模式 turn_detection=null，pcm/16k）；④ 发 base64 PCM via `input_audio_buffer.append`（200ms 分块）；⑤ `input_audio_buffer.commit` + `session.finish`；⑥ 收 `conversation.item.input_audio_transcription.completed`（transcript 字段）；⑦ `session.finished` 收尾。
- **鉴权**：WS 握手请求经 `IntoClientRequest` + 追加 `Authorization: Bearer <secret_key>` header。
- **无运行时状态**：每次 `transcribe` 从 DB 重新解析 → 取最新 endpoint/key（运行时切引擎可即时生效）。
- **健康检查**：`health_check()` 保守返回 `true`，避免每次启动探活消耗 API 额度；真实健康度在首次 transcribe 时由错误路径暴露。

#### ByteDance（字节跳动豆包大模型 ASR，双向流式）

`crates/desktop/src/bytedance_stream.rs`（`cloud` cargo feature 后）接入火山引擎豆包大模型 ASR 1.0/2.0 的双向流式优化版（`bigmodel_async`）。**不实现 `TranscriptionEngine`**（豆包只支持流式，无 chunk 离线接口），直接经 `CloudPipelineEngine`（`Stage::Streaming` cloud 分支），由 `bytedance_stream::open` 返回 `CloudStreamHandle` 管理一条 WSS 长连接。与其他 provider 共享 `PcmFrame` / `StreamEvent` 类型（`cloud_types.rs`）。

**协议**（二进制帧，与 Aliyun 的 JSON 文本帧完全不同）：

| 维度 | 说明 |
|---|---|
| Endpoint | 固定 `wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async`（不来自 DB） |
| 鉴权 | WS 握手 headers：`X-Api-Key`（= `secret_key`）、`X-Api-Resource-Id`（= `source`，如 `volc.bigasr.sauc.duration`）、`X-Api-Request-Id`（UUID）、`X-Api-Sequence: -1` |
| 帧格式 | 4B header（ver/hdr + msg_type/flags + ser/comp + reserved）+ 4B payload_size + payload（全大端序） |
| 消息类型 | `0x1` FULL_CLIENT_REQUEST（初始 JSON config）、`0x2` AUDIO_ONLY_REQUEST（音频帧）、`0x9` FULL_SERVER_RESPONSE（JSON 结果）、`0xF` ERROR_RESPONSE |
| Flags | `0x0` NO_SEQUENCE、`0x2` NEG_SEQUENCE（末帧/负包，告诉服务端音频结束） |
| 压缩 | `0x1` GZIP（音频 PCM + config JSON 均 gzip；响应 payload 也 gzip） |
| 结果 JSON | `{"result":{"text":"全文","utterances":[{"definite":true,"text":"..."}]}}` |
| 结束信号 | 客户端发 flags=0x2 末帧 → 服务端回 flags=0x3（NEG_WITH_SEQUENCE） |

**DB 映射**：`source` = Resource ID、`secret_key` = API Key、`model_name` = `doubao-asr-1.0-streaming` / `doubao-asr-2.0-streaming`。详见 [spec](superpowers/specs/2026-06-21-archived-spec.md#bytedance-asr-design)。

#### Tencent（腾讯云实时语音识别，双向流式）

`crates/desktop/src/tencent_stream.rs`（`cloud` cargo feature 后）接入腾讯云实时语音识别 WebSocket API。**不实现 `TranscriptionEngine`**（只支持流式），直接经 `CloudPipelineEngine`（`Stage::Streaming` cloud 分支），由 `tencent_stream::open` 返回 `CloudStreamHandle` 管理一条 WSS 长连接。与其他 provider 共享 `PcmFrame` / `StreamEvent` 类型（`cloud_types.rs`）。

**协议**（URL 签名鉴权 + Raw PCM binary + JSON text 响应）：

| 维度 | 说明 |
|---|---|
| Endpoint | 固定 `wss://asr.cloud.tencent.com/asr/v2/<appid>?{params}`（appid 在路径段，参数在 query） |
| 鉴权 | **URL 签名（HMAC-SHA1）**：参数按字典序排序→拼签名原文→`Base64(HMAC-SHA1(str, SecretKey))`→URL-encode→追加到 URL |
| 必填参数 | `secretid`、`timestamp`、`expired`、`nonce`、`engine_model_type`（如 `16k_zh`）、`voice_id`（UUID）、`signature` |
| 可选参数 | `voice_format=1`（PCM）、`needvad=1`、`filter_punc=1`、`vad_silence_time=1000` |
| 音频帧 | WebSocket Binary 帧，**原始 PCM s16le 字节**（无压缩/无额外头），200ms = 6400 字节 |
| 响应 | Text 帧 JSON：`code`（0=正常）、`result.slice_type`（0=开始/1=非稳态/2=稳态终态）、`result.voice_text_str`、`final=1`（全部结束） |
| 结束信号 | Text 帧 `{"type":"end"}` → 服务端返回 `final=1` → 断开 |
| 文本累积 | 客户端按 `index` 存 `slice_type=2` 稳态句（`BTreeMap<index, text>`），partial（0/1）覆盖当前句 |

**DB 映射**：`source` = `{appid}:{secretid}` 复合（冒号分隔）、`secret_key` = SecretKey（HMAC 签名密钥）、`model_name` = `engine_model_type`（如 `16k_zh` / `16k_zh_en`）。详见 [spec](superpowers/specs/2026-06-21-archived-spec.md#tencent-asr-design)。

#### Baidu（百度智能云实时语音识别，双向流式）

`crates/desktop/src/baidu_stream.rs`（`cloud` cargo feature 后）接入百度智能云实时语音识别 WebSocket API。**不实现 `TranscriptionEngine`**（只支持流式），直接经 `CloudPipelineEngine`（`Stage::Streaming` cloud 分支），由 `baidu_stream::open` 返回 `CloudStreamHandle` 管理一条 WSS 长连接。与其他 provider 共享 `PcmFrame` / `StreamEvent` 类型（`cloud_types.rs`）。协议最简洁——无 HMAC 签名、无 gzip 压缩、无二进制帧头。

**协议**（START 帧鉴权 + Raw PCM binary + JSON text 响应）：

| 维度 | 说明 |
|---|---|
| Endpoint | 固定 `wss://vop.baidu.com/realtime_asr?sn=<UUID>` |
| 鉴权 | **START 帧 JSON `data` 内直接传 appid + appkey**（无 HMAC、无 token、无 header） |
| 初始化 | Text 帧 `{"type":"START","data":{appid,appkey,dev_pid,cuid,format:"pcm",sample:16000}}` |
| 音频帧 | WebSocket Binary 帧，**原始 PCM s16le 字节**（无压缩/无头），160ms = 5120 字节 |
| 响应 | Text 帧 JSON：`type`（`MID_TEXT` 临时/`FIN_TEXT` 稳态/`HEARTBEAT` 心跳）、`result`（文本）、`err_no`（0=正常） |
| 结束信号 | Text 帧 `{"type":"FINISH"}` → 服务端完成识别后关闭连接 |
| dev_pid | 15372（中文加强标点，推荐）、15376（多方言，需 user 参数）、17372（英文加强标点） |

**DB 映射**：`source` = AppID（纯数字字符串）、`secret_key` = API Key（appkey）、`model_name` = dev_pid 字符串（如 `15372`）。详见 [spec](superpowers/specs/2026-06-21-archived-spec.md#baidu-asr-design)。

## 支持的 ASR 引擎

| 引擎 | 类型 | 特点 |
|------|------|------|
| Whisper | 离线 | 多语言；传 `auto` 且 DB `models.language` 配了具体语种时优先用后者（`entry_language` 覆盖），否则自动检测 |
| SenseVoice | 离线 | 快速；按 config.language 映射 FunASR id（默认 zh=3 强制中文，抑制跨语误判→片假名），非自动检测；输出兜底过滤日韩字符 |
| Paraformer | 离线/流式 | 中文优化 |
| Qwen3-ASR | 离线 | 大模型能力；`auto`/空时不注入 language 让模型自检（支持中英混合），显式语种时注入 `language X`；模型自检的 `language <词> <|asr_text|>` 前缀由 `decode_tokens` 剥离（按 token ID 定位 + `trim_start` 容忍 BPE 引导空格） |
| Zipformer | 离线/流式 | CTC + Transducer（RNN-T）；路由层检测 `decoder.onnx` 分流 |
| Moonshine | 离线 | 英文优化，轻量（24M/58M）；`optimize_for_inference` 可能引发 layout 计算错误，`MoonshineEngine` 跳过 optimize 直接用原始 session |
| Aliyun（云端） | 流式（cloud engine） | 阿里云 DashScope 三协议：Fun-ASR/Paraformer（run-task）/ Qwen-ASR Realtime（OpenAI 风格）；详见上方「云端 ASR 引擎」 |
| ByteDance（云端） | 流式（cloud engine） | 字节跳动豆包大模型 ASR（bigmodel_async，二进制帧 + gzip）；详见上方「云端 ASR 引擎」 |
| Tencent（云端） | 流式（cloud engine） | 腾讯云实时语音识别（WebSocket，HMAC-SHA1 URL 签名）；详见上方「云端 ASR 引擎」 |
| Baidu（云端） | 流式（cloud engine） | 百度智能云实时语音识别（WebSocket，START 帧鉴权）；详见上方「云端 ASR 引擎」 |

### Zipformer 引擎族（CTC vs Transducer）

`EngineCategory::Zipformer` 下有两个引擎 struct，由路由层（`engine.rs` `switch_model`）在实例化时检测模型目录下有无 `decoder.onnx` 自动分流：

| Struct | 模型目录特征 | 解码方式 | 典型模型 |
|---|---|---|---|
| `ZipformerCtcEngine` | 仅 `model.int8.onnx`（单 session） | CTC argmax + blank/repeat skip | `zipformer-ctc` / `zipformer-small-ctc` / `zipformer-multi` |
| `ZipformerTransducerEngine` | `encoder.int8.onnx` + `decoder.onnx` + `joiner.int8.onnx`（三 session） | RNN-T greedy decoding | `sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30`（154M）/ `zh-xlarge-int8-2025-06-30`（726M） |

**流式路径同样分流**——`StreamingSession` 在创建时检测 `decoder.onnx`，分流到 `StreamingZipformer`（CTC）或 `StreamingZipformerTransducer`（RNN-T）。两者实现 `ZipformerStreamOps` trait，`StreamingSession` 通过 trait 统一分发 `accept_samples` / `flush` / `finish` / `reset`。Transducer 流式引擎跨 chunk 维持 `token_buf`（decoder 上下文窗口）+ encoder 缓存状态。

**CTC**：单 session，encoder 同时输出 log_probs，逐帧 argmax，跳过 blank(0) 和重复 token。

**Transducer（RNN-T）**：三 session 协作，遵循 sherpa-onnx 流式 greedy search 约定——
- **encoder**：与 CTC 版状态管理完全相同（cached_key/N、cached_val/N、cached_conv/N、embed_states、processed_lens），输出 encoder_out `[T', enc_dim]` 而非 log_probs
- **decoder**：无状态，输入最近 `context_size`（默认 2）个 token `[N, 2]` → decoder_out `[enc_dim]`
- **joiner**：融合 encoder frame + decoder_out → logit `[vocab_size]`，argmax 决定发射 / blank
- **解码循环**：每个 encoder frame 跑 joiner → argmax；非 blank(0) 则发射 token、滑动窗口更新、重跑 decoder；blank 则移到下一 frame。内循环安全上限 20 次/frame（防理论无限循环）
- **encoder_dim 动态读**：从 encoder 输出 shape 最后一维读（zh=512, xlarge=768），不硬编码
- **token_buf 初始化**：`[-1, ..., -1, 0]`（长度 = context_size，末位 blank），结束后剥离 context padding

两个引擎共享 `load_vocab`（tokens.txt 解析）、`initial_encoder_states`（encoder 缓存初始化）、`decode_token_ids`（token ID → 文本，支持 BBPE + SentencePiece byte-fallback）三个自由函数。

**Whisper 特征归一化（per-chunk，与 sherpa-onnx 一致）**：使用 whisper 特征（`is_whisper=true`，即 Transducer 系列 + `zipformer-ctc`）的模型，其 `normalize_whisper_features` 公式为 `mel = (max(log10(clamp(x, 1e-10)), max_v - 8.0) + 4.0) / 4.0`（与 sherpa-onnx `NormalizeWhisperFeatures` 完全一致）。**关键约束**：① 归一化公式最后一步是 `(x + 4) / 4`（范围~0-2），不是简单的 shift（曾错误用 `x - clamp_min`，范围 0-8，尺度差 4 倍）；② 流式引擎做 **per-chunk 归一化**（每个 chunk 独立 normalize 后送 encoder），与 sherpa-onnx 行为一致——此前误改为 pseudo-global（每次重算 history+buffer 全局归一化）反而导致 max_v 跨 tick 不稳定；③ Transducer 流式引擎的 `history_samples` 仅保留最后 1 帧（160 samples），与 CTC 一致——此前保留全部未消费样本导致 history 无限膨胀。

## 拼音纠错与热词校正 (ASR Corrector)

为了在不引入重型深度学习模型（如 MacBERT 等动辄几百 MB 的模型）的前提下，实现极致轻量的纠错与专有名词（热词）校正，项目实现了一套基于 **“拼音映射 + 长度归一化 Bigram 转移概率”** 的轻量级后处理纠错引擎。

### 核心特性
- **纯静态与轻量化**：纠错所需的 unigram 词表与 bigram 共现表（各精简至高频的前 40,000 条，压缩后约 450KB）直接通过 `include_bytes!` 静态嵌入二进制中，无需额外网络下载，运行时解压，额外内存占用约 30MB。数据源自 jieba `dict.txt.big`（unigram）与 gotokenizer `bigram.txt`（bigram），由 `crates/asr-local/scripts/generate_corrector_data.py` 离线生成到 `src/corrector_data/*.txt.gz`（已提交；更新语料时手动重跑该脚本）。
- **配置开关控制**：由 `app_config` 表中的 `asr_correct` 字段控制（默认 `false`）。
- **作用范围**：corrector 现为「有界热词纠错」，所有中文引擎都经此层（sensevoice-orig / qwen3 / 云端四家 `skip_corrector()` 2026-07-10 全改 `false`——有界版无热词即 no-op，过纠不可能发生，高质量引擎也受益）。跳过仅两类：①「language=en」——corrector 是中文拼音纠错器，对英文无意义且可能扰动，`transcribe_with_vad` 在注入点基于 `language=en`（desktop=`config.language`、server=请求、CLI=`--language`）自动跳过，覆盖 moonshine 等 en-only 模型；② `app_config.asr_correct=false`（主开关，默认关）。诊断教训：直调 `engine.transcribe()` 的 e2e 绕过 pipeline 的 corrector 会掩盖纠错效果，须走完整 pipeline。

- **方言模糊规则可配**（2026-07-10）：四组方言混淆做成用户可勾选开关（设置页「热词管理」面板，复用 Settings 的 Card/Row/Toggle 设计语言），存 `app_config.fuzzy_dialect`（逗号分隔 token）：`f/h`（声母 f→h）、`hu/wu`（单字 hu→wu + 其余 huX→wX 如 huang→wang）、`n/l`（声母 n→l）、`r/l`（声母 r→l，n/r/l 在 n/l+r/l 同开时都归一到 l，首字母不同互不冲突）。基础规则（平翘舌 + 前后鼻音）始终开。归一化单向、索引与查询共用 `normalize_fuzzy_pinyin` → 双向对称命中；规则变更经 `corrector::reload_fuzzy_dialect` 重建索引（key 由 normalize 生成，规则变 key 必变）。**r/l 已知局限**：仅救首字（如「热词→乐视」，r/l 把首字「热 re→le」与「乐」归一一致，但第二字「词 ci」≠「视 shi→si」，sh/c 刻意不归一避免级联误命中）；对纯 r/l 混淆（热↔乐、肉↔漏、人↔林）完整有效。**注**：当前为「有界热词纠错」——候选源从「全词典倒排索引」改为「用户热词 `HotwordIndex`」（命中即替换，过纠根因消失）；下方滑窗 / Bigram 打分 / jieba 惩罚 / 贪心扫描算法**保留**，仅在热词候选间排序。
- **拼音首字母 + 写入规范化**（2026-07-10，2026-07-11 搬至 `infra::hotword_text` 供 db.rs 迁移复用、消除 asr-local 循环依赖）：`pinyin_initials(word)`（汉字→大写首字母串，非汉字跳过：八爪鱼→BZY、浮窗→FC、热词→RC）+ `normalize_words_text(words)`（任意空白切词→去重→按 `(pinyin_initials, 词文本)` 升序→空格拼接）。后者用于热词版本 `words_text` 写入规范化（始终保持有序去重形态）；前端搜索改为汉字包含 + 字母/命中度排序。与纠错共用同一 `pinyin` crate 保证一致。

### 纠错算法逻辑
1. **滑窗候选召回 (Sliding Window)**：使用 2 字和 3 字的字符滑窗扫描识别出的文本，通过拼音库计算滑窗文本的拼音，并在此拼音的 $O(1)$ 模糊拼音倒排索引（支持南方口音混淆，如 `zh/ch/sh` <-> `z/c/s`、`in/en` <-> `ing/eng`、`n` <-> `l`、`r` <-> `l` 等）中召回**相同字符长度**的同音/近音候选词。
2. **局部上下文打分 (Local Context Scoring)**：每个候选词的评分取**窗口前后各 15 字**（共 ≤33 字）做 `jieba.cut` 分词 + Bigram 打分，而非全句分词。利用未登录词（typo）容易被 `jieba` 拆碎分词的特性，使用 **「句子总 log 概率 / 分词后 Token 数量」** 归一化消除长度偏置。候选词打分用**增量 gain**（候选局部分 − 原词局部分 + 惩罚），比绝对分更准确（消除无关上下文噪声）。
3. **基于 Jieba 字典的自适应惩罚**：
   - 如果原滑窗词是 Jieba 字典中的已登录词（即 `jieba.cut().len() == 1`，说明它是合法的词，如 `"坐上"`），系统施加极高的修改惩罚（`-1.5`）以保护正确表述不被误改；
   - 如果原滑窗词是未登录词（typo，如 `"以经"` 被 Jieba 拆分为 `"以"` 和 `"经"`），则修改惩罚降低（`-0.2`）以积极纠错。
4. **单次贪心扫描**：`correct_greedy` 从左到右单次 `while` 扫描，每处取最优候选词**原地替换**后步进整个窗口宽度（`i += sz`，跳过已纠正字防重叠二次纠错），未替换才 `i += 1`，替代旧 `correct_depth` 的递归回头（最多 5 轮全句扫描）。性能从 $O(N^3 \cdot K)$（全句 clone + 全句分词 × 候选数 × 递归轮数）降到 $O(N \cdot K \cdot 30^2)$（局部窗口分词 × 候选数 × 单轮）。

## 热词多版本管理（hotword-sets）

v1 扁平单表 `hotwords`（word/status/hit_count）于 2026-07-11 升级为「多版本词表 + 多选叠加 + 全局命中」（spec：[2026-07-11-hotword-sets-design.md](superpowers/specs/2026-07-11-hotword-sets-design.md)）。不同工作/场景用不同热词集合，像「主题」一样可切换，多个版本同时勾选叠加生效。

- **数据层**（`infra/db.rs` + `db.sql`，schema v22→v23）：
  - `hotword_sets`（版本）：`id / name(UNIQUE) / enabled / words_text / created_at / updated_at`。`words_text` 是空格分隔的规范词文本——版本 = 一坨纯文本，**非逐词 DB 行**。
  - `hotword_hits`（全局命中）：`word(PK) / hit_count`。命中按词全局记一份，**不绑版本**（同词跨版本命中累加到同一行）。
  - 全新库由 db.sql seed 默认空「通用」版本（`INSERT OR IGNORE`，开箱即用）；升级库（v22）v23 一次性迁移：现有 active 热词 → 「通用」版本 `words_text`（normalize 排序去重；db.sql 已 seed 的空「通用」经 `ON CONFLICT` upsert 并入 active 词、不丢词），hit_count → `hotword_hits`，pending 词丢弃。旧 `hotwords` 表保留停用（不 DROP，留待后续清理）；Rust 侧旧 hotword 函数（`list_hotwords`/`insert_hotword`/`confirm_pending_hotword` 等）已删。
- **生效词 = enabled 版本并集**（`list_active_hotword_words`）：`SELECT words_text FROM hotword_sets WHERE enabled=1` → 切词去重并集 → `HotwordIndex`。多选叠加；全关 = 空集 = corrector no-op（零过纠铁证保留）。签名 `() -> Vec<String>` 不变，main.rs setup / reload 调用点零改。
- **命中全局**（`bump_hotword_hit_by_word`）：corrector 命中替换某词 → pipeline 经 `drain_hits()` 取走命中词 → `hotword_hits` 该词 `INSERT ... ON CONFLICT(word) DO UPDATE SET hit_count=hit_count+1` upsert +1。命中分层保留（corrector 只收集、pipeline 写库），与版本无关。
- **UI**（设置页「热词」面板 `HotwordPanel.tsx`）：方言模糊 Card（f/h、hu/wu、n/l、r/l，存 `app_config.fuzzy_dialect`）保留 + 版本管理 Card（enabled toggle / 行内重命名 / 导出 / 删除 / 新建 / 导入新版本，inline input 输入名）+ 选中版本卡片网格（单词添加 / 卡片✕删 / 命中数 inline 且 >0 高亮 / 汉字搜索 / 默认·字母·命中度排序 / 导入追加·覆盖 / 挖掘）。**挖掘确认面板**（点「挖掘」展开）：候选词 chip 默认全选可逐个取消/手动补词，确认才 `add_words_to_set` 落库。新增词一次性高亮定位（`recentlyAdded` 替换语义，组件重挂自然清空）。用户体感 = 逐词管理，底层系统透明维护 `words_text`。
- **导入/导出**（`import_hotwords` / `export_hotwords`，`spawn_blocking` + `tauri-plugin-dialog`）：txt 纯文本（词任意空白分隔）；导入三模式——`new`（新建版本）/ `append`（追加并集）/ `overwrite`（覆盖），均经 normalize。导出某版本 `words_text` → 用户选路径存 txt。
- **挖掘**（`miner::collect_candidate_words`）：扫历史 transcript + jieba 分词 + 词频过滤（≥MIN_USER_COUNT），返回候选词 `Vec<String>`（**不写 DB**，废弃 v1 的 pending→逐词确认流）。两步确认：命令层 `list_hotword_candidates` 仅返回候选 → 前端确认面板（默认全选、可取消勾选、可手动补词）→ 用户确认才调 `add_words_to_set(id, words)` 批量追加到当前选中版本（不再直接落库、无弹窗选版本）。
- **desktop 命令**（`hotword_commands.rs`，12 个 Tauri 命令）：`list_hotword_sets` / `create_hotword_set` / `rename_hotword_set` / `delete_hotword_set` / `toggle_hotword_set` / `add_word_to_set` / `remove_word_from_set` / `add_words_to_set`（批量，挖掘确认用） / `list_hotword_hits` / `list_hotword_candidates`（候选不写库） / `import_hotwords` / `export_hotwords`。写操作后统一 `reload_after_write` 刷新 corrector 热词索引。

## ASR 输出简繁归一化 (Hans Variant Normalization)

ASR（尤其 Qwen3-ASR 在 `language=auto` 下）输出会混入繁体字；sherpa-onnx [#3509](https://github.com/k2-fsa/sherpa-onnx/issues/3509) 显示 `language` 参数不可靠。故在 ASR 输出边界做**单字级字形归一化**（保持 auto 多语言优势，不依赖 language 参数）：

- **实现**：`crates/asr-local/src/hans.rs`，基于「开放词典网」(kaifangcidian.com) CC-BY 3.0 单字对照表（`data/t2s.txt` 繁→简、`data/s2t.txt` 简→繁，`include_str!` 编译期嵌入，零运行时文件依赖）。仅转字形、不转地域用词（"愚能"转换）；简→繁一对多取数据首选（已消歧，如「发→發」）。
- **开关**：`app_config.output_simplified`（默认 `true`=简体）；`true`→繁转简，`false`→简转繁。
- **注入点**：`engine.rs::transcribe_with_vad` 返回前（offline 统一出口）+ `streaming_engine.rs::finish` 返回前（streaming 统一出口），在 corrector 之后、paste/入库之前。增量中间显示段不转换（短暂过程，最终输出归一化）。

## ASR 硬件加速与自动降级机制 (ASR Hardware Acceleration & Fallback)

为了最大化利用用户本机的 GPU 资源加速语音识别，同时避免因显卡驱动或算子不支持导致应用程序崩溃，系统在 `octopus-asr-local` 核心引擎中实现了一套手自动一体的硬件加速及平滑降级机制。

- **开关**：`app_config.asr_hardware_accelerated`（`bool`，默认 `false`）。`false` 直接走 CPU。
- **按平台注册 EP**（关键修正：曾跨平台全注册 CUDA+DirectML+CoreML，macOS 上 init Linux/Windows 专用 EP 的失败路径直接 segfault——SIGSEGV 绕过 Rust 的 `match Err`、进程被 OS 杀无法 catch，故必须按平台预防）：macOS 仅 CoreML、Linux CUDA、Windows 仅 DirectML（2026-06-20 起删 CUDA——DirectML 通吃 DX12 GPU，实时转写够用，YAGNI）。
- **feature-level 二道防线**（2026-06-20）：除上述代码层 `#[cfg]` 按平台注册，`crates/asr-local/Cargo.toml` 的 ort feature 也按平台条件化（target-specific dependency：mac=coreml / linux=cuda / win=directml，base 仅 `download-binaries`）。cuda/directml feature 在 mac 关闭 → 即便代码层 cfg gate 被退化、误在 mac 注册 CUDA EP，ort `register()` 也会因 feature off 直接返回 `MissingFeature`、不走 FFI dlopen-libcuda（segfault 那条路径），从而不崩。详见 [spec](superpowers/specs/2026-06-21-archived-spec.md)（📄 `2026-06-21-archived-spec.md#ort-cross-platform-feature-design` §7.3，已归档）。
- **两层降级**：① EP 注册失败（驱动/库缺失）→ 捕获 `Err` 回退纯 CPU session，进程不崩；② **qwen3-asr 显式跳过 CoreML**——其动态算子 CoreML **不报错而是把图分区**跑（CoreML 跑支持的算子、CPU 跑剩下的，CPU↔CoreML 张量拷贝开销 dominate，比纯 CPU 还慢），故检测 active 引擎 `category=qwen3-asr` 时主动走 CPU。zipformer 等静态图照常吃满 CoreML。
- **VAD 免加速**：Silero VAD 极小（1.8MB）+ 实时性要求极高，上 GPU 的上下文切换开销远超收益，固定 CPU，不受 `asr_hardware_accelerated` 影响。

## 技术栈

- **推理引擎**: ONNX Runtime（通过 ort crate）；可选硬件加速——按平台注册 CoreML/CUDA/DirectML execution provider（`app_config.asr_hardware_accelerated` 控制，默认 `false`，两层降级见上节），VAD 固定 CPU。config 经 `APP_CONFIG` OnceLock 缓存避免每次 session 构建重复读 DB。
- **音频处理**: cpal（录音）、rubato（重采样，含 denoise 48k 桥接）、nnnoiseless（RNNoise 降噪）、rustfft（各引擎 fbank STFT）、hound（WAV 读取）
- **Web 框架**: Axum + Tokio
- **桌面框架**: Tauri 2
- **模型加载**: HuggingFace Hub 本地缓存
- **嵌入式存储**: rusqlite（`bundled` feature，自带 SQLite C 库）— desktop 用，存识别历史 + 模型配置
