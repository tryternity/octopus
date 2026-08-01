# ASR 引擎

> octopus 的 ASR 引擎层——7 个本地 ONNX 引擎 + 4 个云端 provider、流式/离线双模、VAD 分段、拼音纠错、简繁归一化、硬件加速。

## 本地引擎

所有本地引擎在 `crates/asr-local/src/` 下，实现 `OfflineAsrEngine` trait（离线）或经 `StreamingSession` 包装为流式。

| 引擎 | 模块 | 类型 | 特点 | 关键约束 |
|------|------|------|------|----------|
| Whisper | `whisper` | 离线 | 多语言；int8 三件套（encoder + dec_init + dec_past）；auto-language 两步式检测 | 仅 80 mel bins，**不支持 Large v3 / Turbo**（128 mel 遇到即 fail）；仅 whisper-small.en 质量可用；短音频按实际时长 `max_tokens=(seconds×6+10).min(448)` 防静音段幻听；Mel 频谱 center=True reflect 填充 |
| SenseVoice-orig | `sensevoice_orig` | 离线 | FunASR 4 输入 ONNX（speech[560 维 LFR]+speech_lengths+language+textnorm）；中/英/粤/日/韩 | CMVN 必须外部应用（读 `am.mvn` 做 `(feat+addshift)*rescale`）；vocab=25055、blank=0；`skip_corrector=true`；**language 映射**（2026-07-09）：`transcribe` 按 config.language 映射 FunASR id（auto=0/zh=3/en=4/yue=7/ja=11/ko=12，默认 config.language 强制 zh 抑制跨语误判）+ 输出兜底过滤日文假名/韩文（语言 token 是 soft prompt 非硬约束）——原硬编码 LANG_AUTO（0=多语自动检测）忽略 config.language，中文音频偶发误判日韩→片假名 |
| Paraformer | `paraformer` | 离线/流式 | 中文优化；fbank: hamming 窗 + DC offset + pre-emphasis | 离线与流式共享 `compute_fbank` 但窗口参数不同；流式版用增量式 fbank + CIF 机制 |
| Qwen3-ASR | `qwen3_asr` | 离线 | 大模型能力；auto/空时不注入 language 让模型自检（支持中英混合） | `skip_corrector=true`；**显式跳过 CoreML**（动态算子致图分区比纯 CPU 还慢）；decode_tokens 剥离模型自检的 `language <词> <|asr_text|>` 前缀 |
| Zipformer | `zipformer` | 离线/流式 | CTC + Transducer（RNN-T）；路由层检测 `decoder.onnx` 分流 | CTC 单 session argmax；Transducer 三 session RNN-T greedy；encoder_dim 动态读（zh=512, xlarge=768） |
| Moonshine | `moonshine` | 离线 | 英文优化，轻量（24M/58M）；纯 ONNX 4-session 流水线（preprocess→encode→uncached_decode→cached_decode 循环 + KV cache） | `optimize_for_inference` 引发 layout 计算错误，`MoonshineEngine` 跳过 optimize 直接用原始 session |
| FireRed | `firered` | 离线 | FireRedASR2-AED CTC（小红书）；单 ONNX `model.int8.onnx`(740M)；中文+20方言+英 | 80-bin fbank 复用 `fbank::compute_fbank`（无 LFR，含 DC offset + pre-emphasis 0.97 + povey 窗，对齐 FireRedASR 训练 knf 默认）+ CMVN 从 ONNX metadata 读 `cmvn_mean`/`cmvn_inv_stddev`；greedy CTC blank=0；vocab=8667 |

## 云端引擎

4 个 provider 的 WSS 协议层在 `crates/asr-cloud/src/`（`aliyun_stream`/`bytedance_stream`/`tencent_stream`/`baidu_stream`），desktop 协议层副本已删，协议层单源。统一返回 `CloudStreamHandle`（`push_pcm`/`finish`/`try_recv_text`/`close_async`）。

| Provider | 协议 | Endpoint | 鉴权 | DB 映射 |
|----------|------|----------|------|---------|
| Aliyun | 3 协议自动分发（`is_qwen_realtime_endpoint` 按 endpoint 路径分流） | Fun-ASR/Paraformer: `/api-ws/v1/inference`（run-task 任务型）；Qwen-ASR: `/api-ws/v1/realtime`（OpenAI Realtime 风格） | WS 握手 header `Authorization: Bearer <secret_key>` | `secret_key`=API Key |
| ByteDance | 二进制帧 + gzip 压缩，`bigmodel_async` 双向流式 | 固定 `wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async` | WS 握手 headers: `X-Api-Key`+`X-Api-Resource-Id`+`X-Api-Request-Id`(UUID)+`X-Api-Sequence: -1` | `source`=Resource ID、`secret_key`=API Key |
| Tencent | URL 签名 + Raw PCM binary + JSON text 响应 | 固定 `wss://asr.cloud.tencent.com/asr/v2/<appid>?{params}` | **HMAC-SHA1 URL 签名**：参数字典序排序→拼签名原文→`Base64(HMAC-SHA1)`→URL-encode→追加到 URL | `source`=`{appid}:{secretid}`、`secret_key`=SecretKey、`model_name`=engine_model_type（如 `16k_zh`） |
| Baidu | START 帧鉴权 + Raw PCM binary + JSON text 响应（最简洁，无 HMAC/无 gzip/无二进制头） | 固定 `wss://vop.baidu.com/realtime_asr?sn=<UUID>` | **START 帧 JSON `data` 内直接传 appid + appkey**（无 HMAC、无 token、无 header） | `source`=AppID、`secret_key`=API Key(appkey)、`model_name`=dev_pid（如 `15372`） |

### Aliyun 三协议细节

| 接口 | endpoint | 协议流程 | model_name seed |
|------|----------|----------|-----------------|
| Fun-ASR | `/api-ws/v1/inference` | `run-task`（streaming=duplex, format=pcm, 16k）→ 二进制 PCM 帧 → `finish-task` → `result-generated`（按 `sentence_id`+`sentence_end` 跨句累积）→ `task-finished` | `fun-asr-realtime` |
| Paraformer | `/api-ws/v1/inference` | 同 Fun-ASR（run-task 任务型） | `paraformer-realtime-v2` |
| Qwen-ASR | `/api-ws/v1/realtime` | `session.update`（Manual 模式 turn_detection=null）→ base64 PCM via `input_audio_buffer.append` → `input_audio_buffer.commit`+`session.finish` → `conversation.item.input_audio_transcription.completed` | `qwen3-asr-flash-realtime` |

### ByteDance 帧格式

4B header（ver/hdr + msg_type/flags + ser/comp + reserved）+ 4B payload_size + payload（全大端序）。消息类型：`0x1` FULL_CLIENT_REQUEST、`0x2` AUDIO_ONLY_REQUEST、`0x9` FULL_SERVER_RESPONSE、`0xF` ERROR_RESPONSE。Flags：`0x0` NO_SEQUENCE、`0x2` NEG_SEQUENCE（末帧）。压缩 `0x1` GZIP。

## Zipformer CTC vs Transducer

`EngineCategory::Zipformer` 下两个引擎 struct，由路由层在实例化时检测模型目录下有无 `decoder.onnx` 自动分流。流式路径同样分流（`StreamingSession` 检测 `decoder.onnx`）。

| 维度 | CTC | Transducer (RNN-T) |
|------|-----|---------------------|
| Struct | `ZipformerCtcEngine` | `ZipformerTransducerEngine` |
| 模型文件 | 仅 `model.int8.onnx`（单 session） | `encoder.int8.onnx` + `decoder.onnx` + `joiner.int8.onnx`（三 session） |
| 解码方式 | CTC argmax + blank(0)/repeat skip | RNN-T greedy decoding（encoder→decoder→joiner） |
| encoder 输出 | log_probs | encoder_out `[T', enc_dim]` |
| decoder | 无 | 无状态，输入最近 `context_size`（默认 2）个 token → decoder_out |
| joiner | 无 | 融合 encoder frame + decoder_out → logit `[vocab_size]`，argmax 决定发射/blank |
| 解码循环 | 逐帧 argmax | 每 encoder frame 跑 joiner→argmax；非 blank 发射+滑窗更新+重跑 decoder；blank 移下一 frame；内循环安全上限 20 次/frame |
| token_buf | 无 | 初始化 `[-1,...,-1,0]`（长度=context_size），结束后剥离 context padding |
| encoder_dim | — | 动态读（zh=512, xlarge=768） |
| 典型模型 | `zipformer-ctc` / `zipformer-small` / `zipformer-multi` | `sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30`（154M）/ `zh-xlarge-int8-2025-06-30`（726M） |

两者实现 `ZipformerStreamOps` trait，`StreamingSession` 通过 trait 统一分发 `accept_samples`/`flush`/`finish`/`reset`。共享 `load_vocab`、`initial_encoder_states`、`decode_token_ids`（支持 BBPE + SentencePiece byte-fallback）。

## VAD 分段切分策略

`VadSegmentedPipeline::run_tick`（`crates/desktop/src/pipeline.rs`）——静音边界切分（主）+ 连续超时强制切断（兜底）。

**双 VAD 实例（检测流 vs 过滤）**：SileroVad 是有状态 LSTM（`compute()` 更新 `h`/`c`，`reset()` 归零）。持两个独立实例：
- **检测 VAD**（`vad`）：逐 tick 喂入顺序音频、跨 tick 有状态累积，**切段后 `reset()`+preroll 归零**（防 LSTM 跨段漂移致真实语音持续判静音 → "几段后不吐字"），喂 `compute_speech_chunks`
- **过滤 VAD**（`filter_vad`）：仅 `filter_speech_from_buffer` 用，**每次过滤前 `reset()` 归零**（等价每段独立冷启动，但 ONNX Session 全局缓存复用）

| 策略 | 触发条件 | overlap 处理 |
|------|----------|-------------|
| 静音切分（主） | 检测到语音后静音 ≥ `segment_silence`（默认 400ms） | **无 overlap**（静音是自然语句边界） |
| 强制切断（兜底） | 缓冲达 `SEGMENT_DURATION_S`（20s 常量）；**不门控 `has_speech`**（detect_vad 漂移致 has_speech 卡 false 时强制清空防堆积，由 filter_vad 独立兜底判定） | **保留末尾 `SEGMENT_OVERLAP_MS`（200ms）作下一段 overlap** |

**`filter_speech` 两端 trim**：只 trim 首尾静音、保留中间全部音频（含句内 ~50ms 停顿），不逐帧删除。扫描首个/末个高于阈值的帧，各外扩 `SPEECH_PAD_MS`（120ms，@480 样本/30ms 帧 = 4 帧）作为起止点。

**段间拼接**：force_cut 段虽带 200ms overlap_tail（≈1 字），但段间不做 overlap 去重（曾因子串匹配误删真词），改为逗号直接拼接。段间补逗号前同时检查「新段不以标点开头」和「已有文本不以标点结尾」（避免 `。，` `？，`）。

**段完成消费**：每段经 `filter_speech_from_buffer` 过滤后 `spawn_blocking(engine.transcribe)`，完成经 mpsc rx 回填 `completed_results: HashMap<seq,String>` + `completed_seq` 游标连续消费。识别失败/空结果仍占位该 `seq`（写空串）保证游标连续推进。pipeline drop → mpsc rx disconnect，旧会话迟到段不污染新会话。

## 流式 vs 离线模式

数据流（离线）：
```
音频文件/WAV → read_wav_16k → [VAD 过滤] → 引擎.transcribe → 文本
```

数据流（流式）：
```
麦克风 → PCM chunk → resample_to_16k → 引擎.accept_samples → [partial]
                                    └─ 静音≥0.5s → 引擎.flush(insert_comma=true) → [partial]
                                                              → engine.finish → [final]
```

| 模式 | 引擎 | tick 驱动 | 驱动方式 |
|------|------|-----------|----------|
| 流式 | Paraformer, Zipformer（CTC/Transducer） | 200ms tick | `StreamingRunner.tick` → engine.tick → `TranscriptEvent` |
| 离线（伪流式） | Whisper, SenseVoice-orig, Qwen3-ASR, FireRed, Moonshine | 100ms tick | `VadSegmentedPipeline.tick` → VAD 分段 → `spawn_blocking(transcribe)` |
| 云端流式 | Aliyun, ByteDance, Tencent, Baidu | 100ms 独立线程 | `CloudPipelineEngine.tick` → WSS session 生命周期 |

**`TranscriptEvent`**（`streaming_runner.rs`）：
- **Partial**：增量预览，幂等 apply（`text != transcript.full()` 才更新）
- **Committed**：静音点冻结，逗号拼接，写入 transcript + DB
- **Final**：`finish_with_tail` 产出的最终文本，无条件覆盖
- **Error**：warn + stash `last_error`，下 tick 注入 `PipelineEvent::Error`

**partial 与 transcript 分离**（云端）：partial 写到 engine 自持的 `current_partial`（**预览层不碰 transcript/DB**），coordinator display 拼 transcript + current_partial。

**流式 partial 渲染单调性**：`StreamingZipformer::process_chunks` 三个返回点统一经 `decoded_current()` 返回当前段文本，避免长短态逐帧交替闪烁。承载层幂等门（`text != transcript.full()` 才 apply）。前端跳变延迟合并（`DIVERTED_DELAY_MS=300`）。

## 离线引擎管理（AsrEngineManager）

`crates/asr-local/src/engine.rs::AsrEngineManager`——集中管理离线引擎生命周期，与流式 `StreamingSessionManager` 对称：`cached_engines: RwLock<HashMap<String, Arc<dyn OfflineAsrEngine>>>` 按模型缓存，`active_engine` + `active_engine_name` 记当前，`max_cache` 缓存上限。

- **`Mutex<Session>` 内部可变性**：ort `Session::run` 要求 `&mut self` 独占借用，而 `OfflineAsrEngine::transcribe(&self)` 需 `Send + Sync`，故各引擎 struct 持 `Mutex<Session>`（推理时短时 `lock()` 取锁）——多线程共享 `Arc<dyn OfflineAsrEngine>`，避免每次重载数百 MB 模型的内存/CPU 浪费
- **两种获取路径**：`switch_model`（设全局 active，cli/desktop 单路场景，active 单例语义）/ `get_engine`（只读返回 `Arc`、不改 active，server 多并发；同模型并发受引擎内 `Mutex<Session>` 串行化，跨模型天然并行，无需 server 级全局锁）
- **缓存上限可配**：`new()` 默认 2（桌面控内存）；`new_with_capacity(max)` 供 server 放大（注入 `new_with_capacity(8)` 适配多模型并发）。切回已缓存模型耗时 0ms
- **宿主集成**：desktop 在 Tauri Setup 建管理器 + 独立线程后台加载（不卡 GUI）；server 注入 `AppState` + 启动 `switch_model` 预热，`/transcribe` 用 `get_engine` 取 Arc 直接转写（不再持全局 `inference_lock`，跨模型请求天然并行）

## 流式引擎复用（StreamingSessionManager）

`crates/asr-local/src/streaming_engine.rs::StreamingSessionManager`——对齐离线 `AsrEngineManager`：按模型缓存 `Arc<dyn StreamingEngine>`，desktop 录音 `active_session(spec, lang)` 懒加载取用 + `reset()` 复用，消除每次录音秒级重载 encoder+decoder 两个 ONNX Session。

- **靠 reset 复用、非并发共享**：ort `Session::run` 是 `&mut`，Session 本就不能跨连接并发；流式 `StreamingSession` 又有连接级状态（punct_prefix/decoder_caches…），故 reset 复用。
- **`StreamingRunner.engine` 由 `Box` 改 `Arc`**：让 pipeline drop 时仅释放 Arc clone、manager 原 Arc 仍持有 → 引擎不销毁、下次复用。连带 server `WsStreamSession` 同步改 Arc。
- **仅 desktop 接入**；server（每连接独立状态、连接结束即 drop，非大并发）与 cloud（独立路径）不接入——共享 ort Session 需拆动静字段而 ort 推理持 `&mut` 串行无并发收益（YAGNI）。
- **max_cache=2 驱逐**：`set_active` 入缓存前淘汰非 active（保护正用）+ `probe(Unload)`，防用户配置多流式引擎反复切换致 OOM。
- **模型变更懒加载覆盖**：`active_session` 检测 spec≠active 自动 switch，`switch_active_model` 无需主动联动本 manager（2026-07-17 后统一激活命令）。
- **状态页联动**：`switch_model` 加 `probe(Before/After)`（id=`asr:<bare>`，与离线 `load_engine_into_cache` 同前缀），驱逐时 `probe(Unload)`——见 [desktop-app.md](./desktop-app.md) §13。

## 流式尾音冲刷（Active Flush）

流式模式累积静音 ≥0.5s 时把憋住的尾音即时吐出，每个静音段仅触发一次（`flushed` 标志，恢复说话时重置）。**同时追加逗号**（`flush(insert_comma=true)`）。

| 引擎 | 冲刷机制 |
|------|----------|
| Zipformer | **edge-replicate lookahead padding**（3 chunks，与 `finish` 共享 `run_padding_flush`）对齐右上下文 |
| Paraformer | **CIF force-fire**（`run_cif_final`）：CIF 机制 alpha 累积达阈值 1.0 才 fire，`finish()` 时残留 alpha >0.5 则 force-fire 为最后一个 token 送 decoder（<0.5 视为噪声不 fire） |

**Paraformer `accept_samples` 清 `input_finished`**：`flush()` 静音冲刷时置 `true`（末帧越界零 padding 多算帧 + CIF force-fire），`accept_samples` 入口置 `false`（继续说话回正常帧计算）。仅 `reset()`（会话边界）彻底清除。

## ASR 纠错器（Corrector）

`crates/asr-local/src/corrector.rs`——基于拼音映射 + Bigram 转移概率的轻量级中文拼音纠错。

**数据**：unigram 词表 + bigram 共现表（各 40,000 条，压缩后 ~450KB）`include_bytes!` 静态嵌入，运行时解压 ~30MB。源自 jieba `dict.txt.big` + gotokenizer `bigram.txt`，由 `scripts/generate_corrector_data.py` 离线生成。

**开关**：`app_config.asr_correct`（2026-08-01 默认改 `true`，加了热词即生效）。

**跳过规则**（两类）：
1. **引擎级**：`OfflineAsrEngine::skip_corrector()` 返回 true → 跳过（Qwen3-ASR、SenseVoice-orig、云端 `CloudBatchEngine`）
2. **语言级**：`language=en` → 跳过（corrector 是中文拼音纠错器，对英文无意义）

实际作用于：Whisper、Paraformer、Zipformer、FireRed 等中文引擎。

**算法**：
1. **滑窗候选召回**：2/3 字滑窗 → 拼音 → $O(1)$ 模糊拼音倒排索引（`zh/ch/sh`↔`z/c/s`、`in/en`↔`ing/eng`、`n`↔`l`）召回相同字符长度的同音/近音候选
2. **局部上下文打分**：窗口前后各 15 字（≤33 字）jieba 分词 + Bigram 打分，「句子总 log 概率 / Token 数量」归一化；候选用增量 gain（候选局部 − 原词局部 + 惩罚）
3. **Jieba 字典自适应惩罚**：原词已登录（`jieba.cut().len()==1`）→ 惩罚 `-1.5`（保护正确词）；未登录（typo）→ 惩罚 `-0.2`（积极纠错）
4. **单次贪心扫描**：`correct_greedy` 从左到右单次扫描，取最优候选原地替换后步进整个窗口宽度（`i += sz`），未替换才 `i += 1`。$O(N \cdot K \cdot 30^2)$

## 简繁归一化（Hans）

`crates/asr-local/src/hans.rs`——单字级字形归一化。

- **数据**：「开放词典网」CC-BY 3.0 单字对照表（`data/t2s.txt` 繁→简、`data/s2t.txt` 简→繁），`include_str!` 编译期嵌入。仅转字形不转地域用词；简→繁一对多取数据首选（已消歧）
- **开关**：`app_config.output_simplified`（默认 `true`=简体）；`true`→繁转简，`false`→简转繁
- **注入点**：`engine.rs::transcribe_with_vad` 返回前（离线统一出口）+ `streaming_runner.rs::finish` 返回前（流式统一出口），在 corrector 之后、paste/入库之前。增量中间显示段不转换

### 流式热词纠错（2026-08-01 激活）

流式听写路径此前 `correct` 硬编码 `false`，热词索引建好但永不执行。2026-08-01 修复：

- **激活 correct 开关**：`coordinator` 传 `correct = config.asr_correct && language != "en"` 给 `StreamingRunner`（`session.rs` + `lifecycle.rs` 两处 `from_session`）。`StreamingRunner.maybe_correct` 对 Partial/Committed 过 corrector，`finish` 对 Final 过 corrector（对称批量 `postprocess_text`）。
- **命中计数入库**：`finish` 后 `drain_hits()` + `bump_hotword_hit_by_word`（`lifecycle.rs`），与批量路径对称——corrector 收集命中、coordinator 持久化。
- **门控位置**：`is_english` 在 coordinator 算（`StreamingRunner` 不持 language）；`skip_corrector` 流式引擎 trait 无此方法（zipformer/paraformer 都不 skip），暂不考虑。
- **`asr_correct` 默认改 `true`**：用户加了热词就期望生效。corrector 无热词即 no-op（零过纠铁证保留）。存量库 DB 已有值不受影响（`INSERT OR IGNORE`），老用户可在设置页手动开。

详见 [spec](../superpowers/specs/2026-08-01-hotword-streaming-effective.md)。

## 硬件加速

- **开关**：`app_config.asr_hardware_accelerated`（默认 `false`）。`false` 直接走 CPU
- **按平台注册 EP**：macOS 仅 CoreML、Linux CUDA、Windows 仅 DirectML（代码层 `#[cfg]` 按平台注册）
- **feature-level 二道防线**：`crates/asr-local/Cargo.toml` 的 ort feature 按平台条件化（target-specific dependency：mac=coreml / linux=cuda / win=directml）。cuda/directml feature 在 mac 关闭 → 即便误注册，ort `register()` 返回 `MissingFeature` 不走 FFI dlopen
- **两层降级**：
  1. EP 注册失败（驱动/库缺失）→ 捕获 `Err` 回退纯 CPU session，进程不崩
  2. **qwen3-asr 显式跳过 CoreML**——动态算子 CoreML 不报错而是把图分区跑（CPU↔CoreML 张量拷贝开销 dominate，比纯 CPU 还慢），检测 `category=qwen3-asr` 时主动走 CPU
- **VAD 免加速**：Silero VAD（1.8MB）固定 CPU，不受开关影响

## 音频重采样（AudioResampler）

`crates/asr-local/src/audio.rs::AudioResampler`——状态化重采样器，解决流式录音中每 tick 重建重采样器（曾每 625ms 新建 `rubato::FftFixedIn`）的 CPU 抖动与边界爆音。

- **缓存 FFT 规划**：生命周期内仅初始化时规划一次 Rubato FFT，后续复用
- **边界零碎样点缓冲**：内部 `buffer: Vec<f32>` 暂存不满一帧的样本，下次输入拼接——消除边界点击爆音（clicks）与音频截断
- **流尾冲刷**：录音结束 `flush()` 零填充输出最后一帧，确保 ASR 还原末尾音频

## 特征提取

> 设计决策、引擎配置矩阵、勿改清单详见 [fbank 特征提取 spec](../superpowers/specs/2026-07-09-asr-fbank-feature-extraction-design.md)（SenseVoice 真实音频乱码根因 + 对齐 kaldi_native_fbank）。本节为实现细节。

`crates/asr-local/src/feature.rs`——共享特征提取设施（mel filterbank 参数化 high_freq、apply_lfr、hz_to_mel/mel_to_hz、hamming/povey 窗口），抽取自 paraformer/fbank/zipformer 三处重复实现。

### Whisper 特征归一化（per-chunk）

使用 whisper 特征（`is_whisper=true`，即 Transducer 系列 + `zipformer-ctc`）的模型：

```
mel = (max(log10(clamp(x, 1e-10)), max_v - 8.0) + 4.0) / 4.0
```

关键约束：
1. 最后一步 `(x + 4) / 4`（范围~0-2），不是简单 shift
2. 流式引擎做 **per-chunk 归一化**（每个 chunk 独立 normalize 后送 encoder）
3. Transducer 流式引擎的 `history_samples` 仅保留最后 1 帧（160 samples）

### Paraformer fbank 5 步

`paraformer.rs::compute_fbank(samples, window, preemph_coeff)`——统一签名，流式用 povey 窗，离线用 hamming 窗。Mel 滤波器（`MEL_FILTERBANK` static）high_freq=7600 Hz。Pre-emphasis 无跨帧状态（从连续缓冲回溯 `samples[start-1]`）。

流式特征提取 5 个关键点：
1. **DC offset removal**——每帧 FFT 前减帧均值
2. **Pre-emphasis**——`y[i]=x[i]-0.97*x[i-1]`
3. **窗口函数**——povey 窗 `(0.5-0.5cos)^0.85`
4. **mel high_freq**——7600 Hz（`high_freq=-400`）
5. **增量式 fbank**——`raw_samples` 线性追加 + `fbank_cache` 增量计算（对齐 sherpa-onnx `OnlineFbank`），消除重叠 chunk 重复提取

### SenseVoice-orig / FireRed 共用 fbank（`fbank.rs::compute_fbank`）

`paraformer.rs::compute_fbank` 是 Paraformer 私有实现（上述 5 步），而 SenseVoice-orig（`compute_fbank_features` 经 LFR→560 维）与 FireRed（纯 80-bin）共用 `fbank.rs::compute_fbank`——纯 80-bin log-fbank（**窗函数参数化**：SenseVoice hamming / FireRed povey，无 LFR）。2026-07-09 审查修复补齐对齐 kaldi_native_fbank 默认的两步预处理（此前缺它们致 SenseVoice 真实音频乱码，合成音频落在模型鲁棒区侥幸通过）：

1. **DC offset removal**——每帧 FFT 前减帧均值（**始终执行**，knf 默认 `remove_dc_offset=true`）
2. **Pre-emphasis**——`y[i]=x[i]-preemph_coeff*x[i-1]`（**参数化**：SenseVoice / FireRed 均传 0.97 对齐 knf 默认。SenseVoice 的 am.mvn 基于含此步的特征统计；FireRed 配置 2026-07-09 经 `FireRedTeam/FireRedASR` `data/asr_feat.py` 确认——用 kaldi_native_fbank、仅覆盖 dither/num_bins/snip_edges → knf 默认 preemph=0.97 + povey 窗）

帧重叠（shift=160 < len=400）下取准确前序样本：从连续缓冲回溯 `samples[start-1]`（减本帧 mean 近似，对齐 paraformer.rs:503），无需跨帧状态。mel filterbank 用 mel 空间权重（`fd47f86` 修复方向，勿改回 Hz 空间）。

## 模型目录解析

`config::resolve_model_dir`——source 字段四级查找（纯本地 IO，不联网/不下载）：

| 级 | 路径 | 用途 |
|----|------|------|
| 1 | `~/.octopus/<source>` | builtin 模型（如 `asr/zipformer-small`，首次启动下载到此） |
| 2 | 绝对路径 | `source` 是绝对路径且存在 |
| 3 | `~/.octopus/models/<source>` | cli download 下的本地模型（优先于旧 hf-cli 缓存） |
| 4 | `find_hf_cache`（`~/.cache/huggingface/hub/models--<repo>/snapshots/<hash>/`） | 兼容旧 hf-cli |

模型缺失时：builtin 模型（source_type=0）首次启动由下载页自动下载；其余模型报错提示运行 `octopus-cli download <source>`。

## 引擎选择

### 3-part spec

CLI `--model` / server 请求 `engine` / `AsrEngineManager.switch_model(spec)` 支持 3-part spec `"{provider}:{category}:{model_name}"` 格式从 DB `models` 表唯一定位（唯一键 `UNIQUE(domain, provider, category, model_name)`）。**激活态**（desktop/server 启动默认）由 DB `is_enabled=1` 决定，经 `resolve_active_engine(domain)` 读取，不再经 config 字符串（2026-07-17 重构后）。

| spec | 含义 |
|------|------|
| `"local:zipformer:zipformer-small"` | 本地 zipformer |
| `"aliyun:Fun-ASR:fun-asr-realtime"` | 云端 DashScope FunASR（run-task） |
| `"aliyun:Qwen-ASR:qwen3-asr-flash-realtime"` | 云端 Qwen-ASR Realtime（OpenAI Realtime） |
| `"aliyun:Paraformer-Realtime:paraformer-realtime-v2"` | 云端 Paraformer 实时（run-task） |
| `"bytedance:Doubao-ASR:doubao-asr-1.0-streaming"` | 云端豆包大模型 ASR |
| `"tencent:Tencent-ASR:16k_zh"` | 云端腾讯实时语音识别 |
| `"baidu:Baidu-ASR:15372"` | 云端百度实时语音识别 |
| `"{model_name}"`（裸名，无冒号） | 仅全局 fallback 路径用（跨 provider/category 搜，优先 local） |
| 旧 2-part（1 冒号） | warn + 裸名兜底（迁移期） |

统一解析在 `infra::db::parse_model_spec`（返回 `ModelSpec::Full`/`NameOnly`），ASR 经 `asr::config::resolve_engine_in_config` 查找，LLM 经 `infra::db::load_llm_model` 查找。

### resolve_active_engine 兜底

仅服务「全局默认」（server 启动 preheat、请求未带 engine 时）。显式 spec 路径（cli `--model`、`AsrEngineManager.switch_model`、server 请求带 engine）直接走 `resolve_engine_in_config + pick_entry`，**不走兜底**（匹配不到直接报错）。

解析规则：
1. **兜底引擎优先查 DB**：裸名为 `zipformer-small`（`FALLBACK_ASR_ENGINE_NAME`）时先 `resolve_engine_any` 查 DB（source_type=0 builtin 行），命中则用 DB entry（含真实 is_available 状态）
2. DB 无此行（极端情况，如 DB 损坏）→ 硬构造兜底 entry（source_type=0, source=`DEFAULT_ASR_MODEL_DIR`）
3. 非空且命中 → 返回裸名（去掉前缀）+ category + entry
4. 空/不匹配 → 回退兜底 `zipformer-small`

返回的 `ResolvedEngine.name` 始终是**裸名**（不含 `local:`/`category:` 前缀）。

### is_streaming 数据驱动

是否走流式识别由 `models.is_streaming` 列决定：

```
is_streaming_engine() = resolve_active_engine("asr").entry.is_streaming
                       && as_engine_category() != Aliyun
                          && category != ByteDance
                          && category != Tencent
                          && category != Baidu
```

**seed**：zipformer×2 + paraformer×4 + Qwen-ASR Realtime = 流式（`is_streaming=1`）；whisper / sensevoice-orig / firered / qwen3-asr×2 / moonshine×2 / 云端全部 = 非流式。

**云端引擎显式排除**——其 `is_streaming=1` 表示支持云端 WS 流式，而非本地 `StreamingSession`。`StreamingSession::new` 失败时降级到默认引擎 `local:zipformer:zipformer-small` 重试。

**流式引擎内部分流**：`StreamingSession::new` 检测 `decoder.onnx` 存在性——CTC 走 `StreamingZipformer`，Transducer 走 `StreamingZipformerTransducer`。

**云引擎路由**：`resolve_active_engine("asr").as_engine_category()` 按 provider 分支识别 → `EngineCategory::Aliyun`/`ByteDance`/`Tencent`/`Baidu`。Aliyun 建 `AliyunEngine`（需 `aliyun` feature，`is_streaming=0` → chunk 路径）；ByteDance/Tencent/Baidu 不建独立 engine，直接经 `is_cloud_engine` 路由到 `CloudPipelineEngine`（`Stage::Streaming` cloud 分支）。云↔本地切换经设置页 `switch_active_model("asr", id)` 后**下次录音生效**（reload_active_engine 刷新 ACTIVE_ENGINES 缓存）。

---

## 热词纠错（Corrector + HotwordIndex）

> 源文件：`crates/asr-local/src/corrector.rs`、`crates/infra/src/pinyin.rs`、`crates/infra/src/hotword_index.rs`。

**核心架构决策**：候选词来源从「全词典模糊拼音」改为「仅 HotwordIndex」——根除了过度纠错问题。HotwordIndex 是有界集合（仅含用户配置的热词），空热词时 no-op（零过纠）。

**方言模糊规则**（可配置，6 组，存 `app_config.fuzzy_dialect`）：
- f/h（浮/护）—— 声母 f→h
- hu/wu（胡/吴）—— 声母 hu→wu
- n/l（刘/牛）—— 声母 n→l
- r/l（热/乐）—— 声母 r→l
- yun/yong（孕/用）—— 整音节归一 yun→yong，解决「孕妇」→「用户」误识
- fei/hui（飞/回）—— 整音节归一 fei→hui

前 4 组是声母规则（影响所有同声母字），后 2 组是整音节归一（含声母+韵母，匹配精确，不影响其它字）。

**命中统计分层**：
- Corrector 只收集命中（`pending_hits` + `drain_hits()`），不写 DB。
- Pipeline 负责 bump DB（corrector 从不触碰 DB，避免测试污染）。
- 挖掘两步走：`list_hotword_candidates`（不写 DB）→ 用户确认面板 → `add_words_to_set`（批量写）。

**热词多版本管理**：
- `hotword_sets` 表（v23 新增）：`id, name(UNIQUE), enabled, words_text, created_at, updated_at, sync_md5`（v46 加，git 同步增量 diff 用）。勾选叠加生效（生效词 = 所有 `enabled=1` 版本的全局并集去重）。
- **单版本词数上限 `HOTWORD_SET_MAX_WORDS = 3000`**（2026-08-01）：写入（`set_hotword_set_words` / `add_word_to_set` / `add_words_to_set`）前 `ensure_within_capacity` 校验 normalize 后词数，超限 bail「词典容量已满，建议另建新词典」。限制理由：`HotwordIndex::from_words` 构建 O(N) + fuzzy `match_score` 逐词 O(N)，词数过大影响启动 + 搜索性能。
- `hotword_hits` 表：`word PK, hit_count`。全局统计（按 word，不按版本）；删版本不删命中。
- **`words_text` 不变量**：始终规范化——按任意空白分割 → 去重 → 按 `(pinyin_initials, localeCompare)` 排序 → 空格拼接。所有写入路径（增/删/导入/挖掘）都经 `normalize_words_text`。
- `pinyin_initials` 放在 infra（asr-local 依赖 infra，反向不可），asr-local re-export。
- 导入 3 模式：新建版本 / 追加到当前版本 / 覆盖当前版本（覆盖需确认）。
- **fuzzy 搜索**（2026-08-01）：`filter_hotwords_fuzzy(query, words)` Tauri 命令复用 `matcher::match_score`（exact > prefix > word-prefix > pinyin > fuzzy 五级 scoring，与 ActionBar 同款），汉字 + 拼音首字母匹配（如「by」命中「八爪鱼」bzy），按匹配度降序。前端 debounce 120ms 调用。
- WKWebView 不支持 `window.prompt/confirm` → 用 inline input + `@tauri-apps/plugin-dialog` 原生确认。

**全引擎一致**：11 个引擎均 `skip_corrector=false`，确保热词纠错对所有引擎一致生效。
