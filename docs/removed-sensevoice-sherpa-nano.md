# 已移除：sherpa nano 简化版 SenseVoice（category = `sensevoice`）

> 移除时间：2026-06-30。**此文档防止以后重新接入 sherpa 简化版。**

## 移除了什么

- DB seed 行 `category='sensevoice'`（`sense-voice-nano`，`csukuangfj/sherpa-onnx-sense-voice-funasr-nano-int8-2025-12-17`）+ `~/.octopus/octopus.db` 中对应记录
- 引擎 `SenseVoiceEngine`（`crates/asr-local/src/sensevoice.rs` **整文件删除**）
- `EngineCategory::SenseVoice` 枚举 + `config.rs` / `db.rs` 路由 + `AsrSection.sensevoice` 字段
- `cli` 中的显示名与 section 遍历

## 为什么移除

1. **原版更好且已接入**：`sensevoice-orig`（`WisemeAI/sensevoice-small-quant`，FunASR 原生 4 输入 ONNX）识别质量更高、实测可用。留一个高质量引擎即可，无需维护两套。
2. **sherpa 简化版是生态产物**：单输入 `x` + base64 tokens + vocab 60515 是 sherpa-onnx 的导出格式，octopus 不必承担这份维护成本。
3. **corrector 上下文**：通用中文 corrector 对高质量模型过纠有害（已证实，sensevoice-orig 设 `skip_corrector=true`）。保留单一高质量 SenseVoice 路径，corrector 策略更清晰。

## 保留了什么（勿误删）

原 `sensevoice.rs` 里的 **fbank 特征设施**被原版 SenseVoice 和 FireRed 复用，已迁入 `crates/asr-local/src/fbank.rs`：

- `compute_fbank_features`（fbank + LFR → 560 维）← `sensevoice_orig`
- `compute_fbank`（纯 80-bin fbank，无 LFR）← `firered`

若看到 `fbank.rs` 注释提到「原服务于 SenseVoice sherpa 简化版」，那是历史溯源，**不是该引擎还在**。

## 以后若要 SenseVoice

**直接用 `sensevoice-orig` 路径**（category = `sensevoice-orig`，见 `crates/asr-local/src/sensevoice_orig.rs`），**勿再加 sherpa 简化版**。原版质量更高、CMVN 走标准 `am.mvn`、`tokens.json` 直读（非 base64）。

## 模型文件（可选清理）

sherpa 模型未随代码删除（属用户数据，不主动删）。如需释放空间：

```bash
rm -rf ~/.cache/huggingface/hub/models--csukuangfj--sherpa-onnx-sense-voice-*
```
