# ASR fbank 特征提取设计（对齐 kaldi_native_fbank）

> 适用：`crates/asr-local/src/fbank.rs` / `paraformer.rs` / `feature.rs`
> SenseVoice-orig / FireRed / Paraformer 三引擎的特征提取管线。
> 实现细节（5 步 / 增量式 / CMVN 公式）详见 [`docs/features/asr-engine.md`](../../../features/asr-engine.md) §特征提取，本 spec 聚焦**设计决策、引擎配置矩阵、勿改清单**。

## 背景

2026-07-09 审查发现 **SenseVoice-orig 真实语音识别乱码**——合成音频能出对的结果，真实录音系统性近音错乱。根因不在 ONNX 推理，而在 **fbank 特征提取与训练时 kaldi_native_fbank（knf）默认不一致**：模型的 `am.mvn` / CMVN 统计量基于「含 DC offset removal + pre-emphasis」的特征，推理时缺这两步 → 特征分布偏移 → 乱码。

> **合成音频掩盖效应**：合成音频频谱干净、落在模型鲁棒区，缺预处理也能侥幸通过；真实音频分布偏移才暴露。故 fbank / CMVN 修复的 e2e 必须**真实录音 + 断言文本**（详见 memory `funasr-onnx-cmvn-external`）。

## 根因（修复前 `fbank.rs::compute_fbank`）

- **缺 DC offset removal**（knf 默认 `remove_dc_offset=true`）
- **缺 pre-emphasis**（knf 默认 `preemph_coeff=0.97`）
- **窗函数硬编码 hamming**（FireRed 训练用 povey，且 FireRed 此前 preemph 传 0.0）

## 设计

### 接口

| 函数 | 位置 | 用途 |
|------|------|------|
| `compute_fbank(samples, window: &[f32], preemph_coeff: f32)` | `fbank.rs:53` | SenseVoice / FireRed **共用**，纯 80-bin log-fbank |
| `compute_fbank_features(samples)` | `fbank.rs:33` | SenseVoice 的 LFR 包装（`compute_fbank` + `apply_lfr` → 560 维） |
| `compute_fbank(samples, window, preemph_coeff)` | `paraformer.rs:462` | Paraformer **私有**（增量式：流式 povey / 离线 hamming，详见 archived spec） |

`feature.rs` 共享设施：`hamming_window` / `povey_window` / `mel_filterbank`（mel 空间，参数化 high_freq）/ `apply_lfr`。

### 常量

`FBANK_FFT_SIZE=512` / `FBANK_FRAME_LEN=400`（25ms）/ `FBANK_FRAME_SHIFT=160`（10ms）/ `FBANK_NUM_BINS=80` / `FBANK_SAMPLE_RATE=16000` / `high_freq=8000`（Nyquist，`fbank.rs`）/ Paraformer 私有 `high_freq=7600`（`-400`）。

### 预处理管线（`compute_fbank` 内逐帧）

1. **DC offset removal**（始终执行，不可关闭）：每帧 FFT 前减帧均值 `mean(frame_buf)`。
2. **Pre-emphasis**（参数化 `preemph_coeff`）：`y[i] = x[i] - preemph_coeff * x[i-1]`。帧重叠（shift=160 < len=400）下取准确前序样本——从连续缓冲回溯 `samples[start-1]`（减本帧 mean 近似，对齐 `paraformer.rs:503`），**无跨帧状态**。
3. **窗函数**（参数化 `window: &[f32]`）：SenseVoice `HAMMING_WINDOW` / FireRed `POVEY_WINDOW`（均 `Lazy<Vec<f32>>` static）。
4. FFT → power spectrum → mel filterbank → `ln(sum + 1e-10)`。

### 引擎配置矩阵

| 引擎 | 窗 | preemph | LFR | CMVN 源 | 输入预处理 |
|------|----|---------|-----|---------|-----------|
| sensevoice_orig | hamming | 0.97 | ✓（7 窗口 → 560 维） | `am.mvn` 外部 `(feat+addshift)*rescale` | — |
| firered | povey | 0.97 | ✗（纯 80-bin） | ONNX metadata `(fbank-mean)*inv_std` | `×32768` |
| paraformer（离线） | hamming | 0.97 | 私有 | 私有 | — |
| paraformer（流式） | povey | 0.97 | 增量式 | 私有 | — |

## 关键不变量（勿改）

- **DC offset 始终执行**——不可参数化关闭。SenseVoice 的 `am.mvn`、FireRed 的 cmvn 统计都基于含此步的特征。
- **preemph 0.97 对齐 knf 默认**——SenseVoice / FireRed 均传此值；改值会致特征分布与训练统计不符 → 乱码。
- **mel 空间 filterbank**——`fd47f86` 已从 Hz 改 mel（对齐 paraformer / knf），勿轻信「mel→Hz 提升 Kaldi 兼容」（详见 memory `fbank-mel-space-not-hz`）。
- **FireRed 窗 = povey + preemph = 0.97**——经 `FireRedTeam/FireRedASR` `data/asr_feat.py` 确认（用 knf、仅覆盖 `dither`/`num_bins`/`snip_edges` → knf 默认 preemph=0.97 + povey 窗）。旧 `preemph=0.0 + hamming` 是「配置未确认时的保守旧行为」，2026-07-09 改正（commit `d73e41b`）。

## 验证

- **e2e 须真实录音 + 断言文本**：合成音频落在模型鲁棒区掩盖预处理缺失。
- SenseVoice（hamming + 0.97）：e2e 通过。
- FireRed（povey + 0.97）：e2e 确认无乱码。

## 关联

- [Paraformer fbank 5 步修复](2026-06-21-archived-spec.md#paraformer-fbank-feature-extraction-fix)（archived）——Paraformer 私有 `compute_fbank` 的 5 个根因（DC offset / pre-emphasis / povey 窗 / high_freq / 增量式）。
- memory：`fbank-mel-space-not-hz` / `fbank-preemphasis-dc-knf-align` / `funasr-onnx-cmvn-external` / `sensevoice-gguf-rust-infeasible`。
