# SCK 单流输出 spike 报告（2026-07-27）

## 背景

Phase 0 探索任务——验证 SCK 私有 KVC 能否让 `system audio` + `microphone` 合并到单条 `.audio` 流输出，从而避开整个混音难题。详见 `docs/superpowers/plans/2026-07-27-screen-record-audio-mix-redesign.md` Task 0.1。

## 执行

**命令**：
```bash
.build/release/octopus-sck-helper --spike-single-stream
```

**spike 代码**：`crates/record/native/macos/Sources/OctopusSckHelper/main.swift` 末尾的 `SpikeSingleStream` enum（commit `73e68ab2`，spike 完成后删除）。

## 验证项

### P1: 当前 `captureMicrophone` 私有 KVC，mic 走哪条 output type？

**结果**：mic 只出现在 `rawValue=2`（492 次），不出现在 `.audio`（`.audio` 只有 267 次，全是 system audio）。

**结论**：mic 走私有 output type `rawValue=2`，与 system audio 完全分离。**与当前 helper 已知行为一致**。

### P2: 有没有别的私有 KVC 能让 mic 合并到 `.audio`？

探测了 7 个候选 selector（用 `responds(to:)` 做能力检测，避开 `setValue:forKey:` 对未定义 key 抛 ObjC NSException 的 crash 风险）：

| selector | 结果 |
|---|---|
| `setAudioMix:` | not supported |
| `setCombinedMicrophoneAudio:` | not supported |
| `setMergeMicrophone:` | not supported |
| `setMicrophoneIntoAudioStream:` | not supported |
| `setMixMicrophoneIntoAudio:` | not supported |
| `setIncludeMicrophoneInAudioCapture:` | not supported |
| `setMicrophoneMix:` | not supported |

**结论**：**没有任何已知私有 KVC 能让 mic 合并到 `.audio` 流**。A2 路径不可行。

### P3: 未做（P1/P2 已足够决策）

## 决策

**A2 失败 → 转 Phase 1-2 A1（AVAudioEngine 实时混音）**。符合 spec §1.2 的预期（「A2 大概率失败，spike 主要是确认排除」）。

## 🔑 意外发现（对 Phase 1-2 设计的关键影响）

spike 抓到的实际音频格式与 spec/plan 的假设不符。这些是**上轮可能踩的坑**，必须修正 spec/plan：

### 发现 1：system `.audio` 是 **planar** stereo（不是 interleaved）

| 字段 | 值 | 含义 |
|---|---|---|
| `sampleRate` | 48000.0 | ✅ 与配置一致 |
| `channelsPerFrame` | 2 | stereo |
| `formatFlags` | `0x29` | `IsFloat(0x01) \| IsPacked(0x08) \| IsNonInterleaved(0x20)` |
| `bytesPerFrame` | 4 | **每 plane 的帧字节数**（1 ch × Float32），不是 2ch × Float32 |

**含义**：SCK `.audio` 输出的 `CMSampleBuffer` 里的 `AudioBufferList` 是 **planar**——L 和 R 分别存在 `mBuffers[0]` 和 `mBuffers[1]`，而不是 interleaved（L0,R0,L1,R1,...）存在单个 buffer。

**影响 spec §2.1/§2.3**：`AudioMath.targetFormat` 仍可定义 interleaved（mixer 内部用），但 PCMConverter 的 `extractFloats` **必须处理 planar → interleaved 转换**。否则 L/R 混乱，输出爆音或单边丢失。

### 发现 2：mic 是 **mono SInt16**（不是 Float32，不是 stereo）

| 字段 | 值 | 含义 |
|---|---|---|
| `sampleRate` | 48000.0 | ✅ 与配置一致 |
| `channelsPerFrame` | 1 | **mono** |
| `formatFlags` | `0xc` | `IsSignedInteger(0x04) \| IsPacked(0x08)` |
| `bytesPerFrame` | 2 | **1 ch × SInt16** |

**含义**：mic 输出是 **SInt16（16-bit signed integer）PCM**，mono。与 system audio 的 Float32 + stereo 完全不同。

**影响 spec §2.3**：PCMConverter 必须处理：
- **位深归一化**：SInt16 → Float32（除以 32768.0）
- **声道归一化**：mono → stereo（duplicate 或 planar→interleaved）

### 发现 3：两路采样率都是 48k（不需重采样）

| 路 | sampleRate |
|---|---|
| system `.audio` | 48000.0 |
| mic `=2` | 48000.0 |

**影响 spec §2.3**：原 spec 假设要 AVAudioConverter 做重采样。**实际不需要**——只需处理位深 + 声道。AVAudioConverter 仍可用（它会处理位深/声道），但不涉及采样率转换的复杂度。

## 🎯 S1 止损信号未触发

spec §3.1 S1：「SCK `.audio` 回调给的是压缩格式（AAC 等）」。实测两路都是 **PCM**（Float32 + SInt16），**不是压缩格式**。S1 不触发。

## 对 Phase 1-2 的具体修正

| 原 spec/plan 假设 | 实测 | 修正动作 |
|---|---|---|
| system `.audio` interleaved | planar | PCMConverter `extractFloats` 加 planar→interleaved 分支 |
| mic 格式假设 Float32/stereo | SInt16/mono | PCMConverter `extractFloats` 加 SInt16→Float32 + mono→stereo |
| 需要 AVAudioConverter 重采样 | 不需要 | AVAudioConverter 仍用，但仅做格式（位深+声道）转换，不做采样率转换 |

**修正后的 Phase 1-2 设计更简单了**——不用处理采样率差异，只需位深 + 声道 + planar/interleaved 归一化。

## 代码

spike 代码已 commit `73e68ab2`，报告产出后删除（下一步）。
