# qwen3-asr 推理实现审查复核与修复

> Date: 2026-06-20
> 状态：审查的 6 条结论已逐条复核（对照 sherpa-onnx C++ 权威实现）；#1/#2/#5/#6 已修（commit 926550d）；#3 经核实为幻象不改；#4 已实现正确 sizing（分支 `perf/qwen3-asr-kv-cache-sizing`，读 decoder past_key dim1 替代硬编码 2048）。
> Worktree/分支：`fix/qwen3-asr-review`
> 关联文件：`crates/asr/src/qwen3_asr.rs`、参考实现 `sherpa-onnx/csrc/offline-recognizer-qwen3-asr-impl.{h,cc}` + `offline-qwen3-asr-model.cc`

## 1. 背景

对 `crates/asr/src/qwen3_asr.rs`（Qwen3-ASR offline 推理：conv_frontend → encoder → 自回归 decoder）收到一份 6 条结论的代码审查。审查本身可能基于幻觉/过时行号——**复核是关键**：每条都对照 sherpa-onnx 的 C++ 官方实现定性，区分「真实问题」与「幻象」，避免改错或引入回归。

模型：`csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25`。

## 2. 复核结论总表

| # | 审查结论 | 判定 | 处置 |
|---|---|---|---|
| 1 | 空输入死锁 | ✅ 真实（高危） | 已修 |
| 2 | auto 语言检测失效 | ✅ 真实（**审查对、原代码注释错**） | 已修 |
| 3 | 缺音频特征裁剪 | ❌ 幻象 | 不改 |
| 4 | KV cache 内存抖动 | ⚠️ 真实代价，但非 Rust 缺陷（C++ 同样） | 专项跟进（§6） |
| 5 | `Box::leak` 内存泄漏 | ✅ 真实（小） | 已修 |
| 6 | mel filterbank 稀疏密集相乘 | ✅ 真实（perf） | 已修 |

## 3. 关键证据（对照 C++）

### 3.1 #2 auto 语言 —— 审查正确，原代码注释错误

C++ `BuildSourceIds`：
```cpp
const std::vector<int64_t> *ids_after = &prompt_ids_after_;  // = Encode("<|audio_end|><|im_end|>\n<|im_start|>assistant\n")
if (!language.empty()) {
    auto language_ids = tokenizer_->Encode("language " + language);
    prompt_ids_after_with_language =
        prompt_ids_after_ + language_ids + {asr_text_token_id_};  // 仅此处追加 <asr_text>
    ids_after = &prompt_ids_after_with_language;
}
// source_ids = before + audio_pad×audio_token_len + ids_after
```

即：
- **language 非空** → prompt 以 `language <lang> <asr_text>` 结尾。
- **language 空（auto）** → prompt 以 `assistant\n` 结尾，**不含 `<asr_text>`**；由模型自行预测 `language <检测> <asr_text>` 再出文本。

Rust 原实现 `ids.push(ASR_TEXT)` 无条件追加（`qwen3_asr.rs` 修复前），在 auto 模式下：
- 跳过模型的语言自检（auto 失效）；
- 使 `decode_tokens` 里剥离 `language…<asr_text>` 前缀的清理逻辑成为**死代码**（C++ `GenerateText` 末尾有完全相同的清理，正为 auto 路径服务）。

原代码注释「`<asr_text>` 是生成起始标记，始终注入」论断有误。

### 3.2 #3 缺音频裁剪 —— 幻象

审查担心「`audio_pad` 占位符数（`audio_token_len`）少于 `audio_features` 帧数（`trimmed_len`）→ 跨注意力失配」。经核实**不会发生**：

- C++ `GenerateText` 同样 `audio_token_len = min(audio_token_len, trimmed_len)`，且**直接传完整 `trimmed_audio_features`**（仅当 `context_len > max_seq_len` 时才 `TruncateAudioFeatures`）。
- 结构不变式：`trimmed_len ≤ encoder 输出长 = conv_num_frames = valid_frames` 恒成立 ⇒ `audio_token_len = min(valid_frames, trimmed_len) = trimmed_len`。故 pad 数与特征帧天然对齐，无需额外裁剪。
- 溢出路径（`context > model max`）只对 >~150s 长音频触发；本 app 经 VAD 分段，每段远短于此，N/A。

### 3.3 #4 KV cache 内存抖动 —— 参考设计的共享代价

C++ `CreateEmptyKVCache`：
```cpp
std::vector key_shape = {batch, max_total_len_, kv_h, hd};  // max_total_len_ 取自 ONNX past_key dim1
// 每层 alloc + std::memset(..., 0, numel * sizeof(float))
```

即 C++ **同样每调用分配 `[1, max_total_len]×28` 层 KV cache 并 memset 0**——Rust 忠实镜像该模式，非 Rust 独有缺陷。

**零填充是承重的**：未写入位置 K/V=0 ⇒ 注意力贡献为 0，这正是「不显式 mask 未写位置也能正确工作」的原因。因此**裸 buffer 复用不清零会 corrupt 输出**（stale V≠0 会被 attend）。

唯一差异（也是真正的可优化点）：`max_total_len` 来源——C++ 读模型 `past_key` shape dim1（固定、可能 ≠ 2048）；Rust 硬编码 `2048.max(s0 + MAX_NEW_TOKENS)`。正确 sizing 见 §6 跟进。

## 4. 已实施修复（#1/#2/#5/#6）

### #1 空输入死锁防御
`compute_mel_features` 入口对空 samples 早返回 `(0, MEL_NUM_BINS)`；`transcribe` 对 0 帧短路返回空文本。
- 根因：`samples.len()==0` 时反射条件 `s < 0 || s >= samples.len() as isize` 退化为 `s < 0 || s >= 0`（恒真），反射在 `-120 ↔ 119` 振荡，死循环卡死进程（在 Mutex 内 → 整个引擎死锁）。
- 对齐 C++ `Decode` 头部 `f.empty()` / `num_frames < 2` 返回空。

### #2 auto 语言 prompt 对齐 C++
`<asr_text>` 移入 `if !language.is_empty() && language != "auto"` 块内（与 `language <lang>` 一起注入）。
- 见 §3.1。修复后 auto 路径自洽：prompt 以 `assistant\n` 结尾 → 模型吐 `language <检测> <asr_text> <文本>` → `decode_tokens` 剥离前缀。
- **行为变更**：auto 模式现在真正走模型语言自检（原为强行带 `<asr_text>` 直出文本）。需本地 e2e 验证中英混合场景。

### #5 `cache_names` 去 `Box::leak`
56 个 KV cache 输入名（`cache_key_i` / `cache_value_i`）提升为进程级 `static CACHE_NAMES: Lazy<Vec<(&'static str, &'static str)>>`，`Box::leak` 仅发生一次。
- 原实现每实例化 leak 56 个串；模块级 `transcribe`（CLI 路径）每次调用都 `Qwen3AsrEngine::new` → 每次泄漏。LRU 淘汰/频繁切换模型时累积。

### #6 mel filterbank 稀疏化
新增 `static MEL_FILTERBANK_RANGE: Lazy<Vec<(usize, usize)>>`，预计算每个 mel bin 的非零频率区间 `[start, end)`；内层循环 `for k in start..end` 只扫非零段。
- filterbank 是三角滤波、高度稀疏（201 个频率里大部分权重为 0），跳过 ~90% 的 `× 0.0` 无效乘加。
- 区间内全非零（三角滤波在 `[left_hz, right_hz]` 内单调升再降，无内部空洞）→ 数值结果不变。

## 5. 验证

- `cargo check -p octopus-asr --all-targets`：零 warning。
- `cargo test -p octopus-asr`：48 passed / 0 failed（含新增 3 个回归测试）：
  - `compute_mel_features_empty_samples_does_not_hang`（#1 死锁回归）
  - `compute_mel_features_single_sample_no_panic`（反射边界 len==1）
  - `mel_filterbank_range_is_contiguous_nonzero`（#6 正确性：区间内全非零、区间外全零）
- **#2 行为变更**：环境无模型/GUI，未做 e2e；待用户本地跑中英混合音频确认。
- **`cargo check --workspace` 失败**：main 既有、与本次无关——`crates/desktop` 报 `octopus_llm::test_connection` 未找到（llm 单独 check 通过且 `lib.rs` 已 `pub use`，疑似 desktop↔llm 特征/解析层问题，属 setting-ui 工作范畴）。本改动未触及，不阻塞 asr。

## 6. #4 跟进（已实现，分支 `perf/qwen3-asr-kv-cache-sizing`）

**目标**：KV cache 正确 sizing，消除硬编码 `2048` floor 的潜在失配 + 动态维度下省内存。

**已实施方案**（对齐 C++ `InitDecoderSession`）：
- 新增 `fn decoder_kv_max_len(decoder: &Session) -> Option<usize>`：按名查找 decoder 的 `cache_key_0` 输入，读其 shape dim1；`>0` 返回 `Some`，动态（-1）返回 `None`。
- `Qwen3AsrEngine` 新增字段 `kv_max_len: Option<usize>`，`new()` 中从 decoder session 读取并存储（`log::debug` 打印实际值/动态）。
- `transcribe` 中 `let max_total_len = self.kv_max_len.unwrap_or(s0 + MAX_NEW_TOKENS);` 替代原 `2048.max(s0 + MAX_NEW_TOKENS)`。
  - dim1 具体 → 用模型声明值（正确 sizing，对齐 C++）。
  - 动态 → 仅装 prompt+生成（`s0 + MAX_NEW_TOKENS`），短音频下比 2048 floor 显著省内存。loop 的 `cur_len + s <= max_total_len` 写入守卫与 `cur_len < max_total_len` 终止条件保证不越界。

**验证**：`cargo check -p octopus-asr` 零 warning；`cargo test -p octopus-asr` 48 passed/0 failed。`decoder_kv_max_len` 依赖 ONNX session，无法离线单测。

**待本地 e2e 验证**（环境无模型）：打印的 dim1 实际值（确认动态/具体）、对应内存与延迟对比。

**未做（YAGNI/需实测）**：buffer 复用消除 per-call alloc。须**保留清零**（零填充承重，见 §3.3）；仅当确认模型按 `cache_position`/`attention_mask` mask 未写位置时才可免清零——需实测，暂不做。

## 7. 不做（YAGNI 边界）

- #3 的 `TruncateAudioFeatures` 防御性裁剪（正常路径天然对齐，加了反而偏离参考）。
- KV cache 免清零复用（需模型 masking 行为实测，未验证前不动）。
- 长音频（>150s）的 `context > max_seq_len` 溢出裁剪（VAD 分段场景 N/A）。
