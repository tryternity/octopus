# 屏幕录制音频混音重做 — 设计规格（spec）

> **Status: 📝 设计阶段**（2026-07-27，分支 `feat/record-followup`，brainstorming 已完成）。
>
> **本 spec 范围**：把当前「双轨写入」改为「单轨混音」，解决「系统音频 + 麦克风都开时默认播放器只能听到一边」的痛点。
>
> **不在本 spec 范围**：视频轨逻辑、helper 子进程协议、Rust session 层、DB schema、前端 UI、配置项新增、录后混音 / 重导出路径。
>
> **关联文档**：
> - 原始录屏设计：`docs/superpowers/specs/2026-07-25-screen-record-design.md`
> - 原始 plan（含上一轮失败诊断）：`docs/superpowers/plans/2026-07-25-screen-record.md` §「Task 8 后续」
> - 调研报告（openscreen 三套混音方案对比）：本 spec §0.4

---

## 实现注记（Implementation Notes）

实施过程中与原 spec 描述的偏差回写至此处（实施时填充）。

<!-- TODO（实施时填）: -->

---

## 0. 决策回顾

### 0.1 问题陈述

录屏开启「系统音频 + 麦克风」时，输出 mp4 在 QuickTime Player 等默认播放器**只能听到一边**：

- 当前方案（commit `f8bbe8ed`）：`setupWriter` 里**麦克风先 add（track 1）、系统音频后 add（track 2）**。播放器默认播 track 1 = 麦克风，**系统音频丢失**。
- 上一轮尝试（commit `6cb6fe90` ~ `f9741968`）5 轮实时混音全部失败，已 revert。核心失败信号：`AVAssetWriterInput.append(sb)` 调用了，但 ffprobe 显示 audio track 数仍为 0。

### 0.2 brainstorming 决策清单

| 维度 | 决策 | 理由 |
|---|---|---|
| **技术路径** | C → A1：先探 A2（SCK 私有 API 单流），探不通上 A1（AVAudioEngine 实时混音） | A2 若可行是最优解（零混音零依赖）；失败也只是损失半天，立即转 A1 |
| **止损机制** | 技术止损线（非时间盒），以 **e2e 客观产出**为准判定 | 避免上轮「盯中间信号反复调」的泥潭；每轮迭代后跑完整 e2e，以 e2e 结果为准 |
| **降级路径** | 触发止损 → revert 回当前双轨方案 + 记录教训 | 与上轮一致，最简洁，YAGNI 不加 config 开关 |
| **验收标准** | 三重 + 波形对比（ffprobe 单轨 + 人工听音 + 3 次稳定 + 波形分析） | 最严，避开上轮「差不多就行」的模糊状态 |
| **麦克风增益** | 写死 1.4（学 openscreen） | 先让它能响，调优下轮再说；YAGNI 不加 config + UI 滑块 |
| **格式归一化** | 显式强制 48k/stereo/float32 | 确定性优先，不依赖 AVAudioMixerNode 默认行为 |
| **PTS 策略** | 自产 PTS（`emittedFrames * 1e9 / sampleRate`），丢弃源 timestamp | 学 openscreen，避开上轮 5 次失败的核心坑（PTS 配对/对齐） |
| **改动面** | 音频字节处理只动 Swift helper 一层（Rust 端仅 2 处极小改承载调试日志） | Rust 永远不碰音频字节，所有音频逻辑集中在 helper |

### 0.3 排除的方案（含理由）

| 方案 | 排除理由 |
|---|---|
| **A3. 录后 ffmpeg amerge** | 用户等待 10~30s + ffmpeg 分发成本；完全回避实时混音所有坑，但用户明确不接受等待时间 + ffmpeg 依赖 |
| **AVAudioPlayerNode + scheduleBuffer** | 为文件播放设计，不适合连续流；调度复杂度高于 AVAudioSourceNode |
| **保留双轨 + config 开关** | YAGNI；触发止损直接 revert 即可，不增加表面积 |
| **保持双轨，调 add 顺序（system-first）** | openscreen macOS 路径同款，看似「system-first 至少能听到系统音频」，但导出/重导出时麦克风丢；治标不治本 |

### 0.4 openscreen 调研结论（背景）

openscreen 三套路径给了正反两面的参考：

| 路径 | 方案 | 借鉴价值 |
|---|---|---|
| macOS helper | 双轨写入，不混音 | ❌ 跟我们同病（导出器只读 track 0，麦克风轨丢失） |
| **Windows helper** | 439 行手写实时 PCM 混音 → 单轨 | ✅ **正解**，设计干净 |
| Web 回退 | WebAudio 图混音（10 行） | ✅ 思路好（让图引擎管重采样/时间戳），Rust/Swift 等价物是 AVAudioEngine |

**关键启示**：上一轮失败的**不是「实时混音」方向，而是「手动 vDSP + CMSampleBuffer 构造」这条具体技术路径**。openscreen Windows 用「独立队列 + 固定 chunk + 自产 PTS + 静音垫底」回避了我们踩的所有坑。在 macOS 上，相同设计原则落到 SCStream→AVAssetWriter 的组合，需要 AVAudioEngine 做图引擎。

---

## 1. 总体架构

### 1.1 问题边界

**要解决**：录屏开启「系统音频 + 麦克风」时，输出 mp4 只能在默认播放器听到一边。

**不在范围内**（YAGNI）：
- 不动视频轨逻辑
- 不动 helper 的子进程协议 / Rust session 层
- 不引入 ffmpeg 依赖
- 不加新的 config 项（沿用现有 `record_system_audio` / `record_microphone` / `record_microphone_device`）
- 不做「录后混音 / 重导出」路径

### 1.2 两阶段策略：C → A1

```
┌─────────────────────────────────────────────────────────────┐
│ Phase 0: A2 探索（半天硬上限，无结论即转 A1）                │
│   探：SCStream 私有 API 能否让 system+mic 合一条 .audio 流   │
│   方法：写一个 ~50 行 spike，只起 SCStream 不写文件，看回调  │
│   判定：能否在单条 .audio 输出里同时拿到两路音频            │
│   ↓                                                          │
│   成功 → Phase 1a：直接单流单 input（零混音，最优解）       │
│   失败 → Phase 1b：A1 AVAudioEngine 实时混音                │
└─────────────────────────────────────────────────────────────┘
```

**为什么先花半天探 A2**：
- 若 SCK 私有 API 能单流输出，是**零混音、零依赖、零 PTS 痛点**的最优解——直接 `AVAssetWriterInput` 单 input 接住，连 AVAudioEngine 都不用起。
- 成本极低（半天 + 一个 spike），收益极大（绕开整个混音难题）。
- 失败也只是损失半天，立即转 A1，不影响主路径。

**预期**：A2 大概率失败（P1 已知 mic 走 rawValue=2、P2 私有 API 文档稀缺），spike 主要是**确认排除**，让我们心里有底地走 A1。

### 1.3 主路径 A1：AVAudioEngine 实时混音（如果 A2 失败）

```
┌─ Swift helper (main.swift) ─────────────────────────────────┐
│                                                              │
│  SCStream ──.audio callback──┐                              │
│  SCStream ──.microphone(=2)─┐│                              │
│                            ↓↓                               │
│                   CMSampleBuffer                            │
│                            ↓                                │
│              toInterleavedFloat32 (转 PCM)                  │
│                            ↓                                │
│              AVAudioSourceNode × 2 (推→拉桥)               │
│                            ↓                                │
│              AVAudioMixerNode (mix + 自动重采样)            │
│                            ↓                                │
│              installTap(onBus:0, format:48k/stereo/f32)     │
│                            ↓                                │
│              AVAudioPCMBuffer                              │
│                            ↓                                │
│              自产 PTS + 转回 CMSampleBuffer                 │
│                            ↓                                │
│              AVAssetWriterInput.append (单轨)               │
│                            ↓                                │
│                      mp4 (单 audio track)                   │
└─────────────────────────────────────────────────────────────┘
```

**关键设计点**：
1. **格式归一化**：source 节点输出端 + mixer + tap 全部显式设 `AVAudioFormat(48k, 2ch, float32)`，不依赖 mixer 默认。
2. **PTS**：丢弃 AVAudioEngine timestamp，用 `emittedFrames * 1e9 / 48000` 自产严格单调 PTS。
3. **麦克风增益**：mixer 之前给 mic 节点接 `AVAudioMixerNode` bus gain 设 1.4。
4. **静音垫底**：某路暂无数据时，mixer 自动补零（AVAudioMixerNode 原生行为）。

### 1.4 改动面（只动一层）

| 层 | 改动 |
|---|---|
| **Swift helper** | `setupWriter`（建单 input）+ `stream(_:didOutputSampleBuffer:)`（送 mixer 而非直接 append）+ 新增 mixer/tap/PTS 逻辑 |
| Rust protocol | **极小改**（仅新增 `HelperEvent::Debug` 变体承载调试日志，§3.4） |
| Rust session | **极小改**（reader task 把 Debug 事件透传到 log，§3.4） |
| Rust desktop 命令 | **不动** |
| DB config | **不动**（沿用现有 `record_*` key） |
| 前端 | **不动** |

音频字节处理逻辑**只在 Swift helper 一层**。Rust 端的两处极小改仅为承载调试日志（§3.4），不触及音频数据。

---

## 2. A1 详细组件设计

### 2.1 AVAudioEngine 图拓扑

```
                    ┌─────────────────────────────────────┐
                    │      AVAudioEngine                  │
                    │                                     │
   system PCM ─────►│  srcSystem (AVAudioSourceNode) ──┐  │
   (from SCK .audio)│                                  │  │
                    │                                  ↓  │
                    │              mixer (AVAudioMixerNode) ──installTap──► PCM buffer
                    │                                  ↑  │                  │
   mic PCM ────────►│  srcMic (AVAudioSourceNode) ──┬──┘  │                  │
   (from SCK =2)    │                               │gain │                  │
                    │                          (bus gain=1.4)                  │
                    │                                                               │
                    └───────────────────────────────────────────────────────────────┘
                                                                                    │
                                                                                    ▼
                                                                    自产 PTS + 转 CMSampleBuffer
                                                                                    │
                                                                                    ▼
                                                                    AVAssetWriterInput.append (单轨)
                                                                                    │
                                                                                    ▼
                                                                                mp4
```

**节点定义**：

| 节点 | 类型 | 输出格式 | 职责 |
|---|---|---|---|
| `srcSystem` | `AVAudioSourceNode` | 显式 `commonFormat=.float32, sampleRate=48000, channels=2` | 把 SCK `.audio` 回调的 `CMSampleBuffer` 转 PCM 后按 pull 模型喂给 mixer |
| `srcMic` | `AVAudioSourceNode` | 同上 | 同上，对 SCK rawValue=2 的 mic 回调 |
| `mixer` | `AVAudioMixerNode` | 48k/2ch/float32 | 合并两路 + 自动重采样 + 自动补零（某路空时） |
| tap | mixer.outputNode bus 0 | 显式 48k/2ch/float32 | 拉取混合 PCM |

**为什么用 `AVAudioSourceNode` 而非 `AVAudioPlayerNode`**：
- SCK 是推模型（回调给 buffer），AVAudioEngine 是拉模型（节点通过 `render` 回调要数据）。`AVAudioSourceNode` 的 `renderBlock` 为「程序提供 PCM」设计——正好做推→拉桥。
- `AVAudioPlayerNode` 需要 `scheduleBuffer` + 时间点，调度复杂，且为文件播放设计，不适合连续流。
- `AVAudioSourceNode` 是 macOS 10.15+ API（我们最低支持版本内）。

### 2.2 推→拉桥：环形缓冲（核心组件）

整个方案的工程难点——SCK 推、SourceNode 拉，速率不同步，必须有缓冲。

```
SCK 回调（推）                环形缓冲                SourceNode render（拉）
─────────────────         ───────────────         ──────────────────
didOutputSampleBuffer  →  写入尾部  →   render(_,frames)  →  从头部读 N frames
（不固定 chunk）           [|||||||||]                              （固定 chunk）
                                                                     ↓
                                                                拉不够 → 补零
```

**关键参数**：

| 参数 | 值 | 理由 |
|---|---|---|
| 缓冲容量 | 48_000 × 2 channels × 2 秒 = ~380KB | openscreen ring 是 2 秒，足够吸收抖动 |
| 缓冲元素类型 | `Float` | 与 mixer 输入对齐 |
| 缺数据时 | 补零，**不阻塞 render** | openscreen 同款策略；阻塞 render 会导致 AVAudioEngine 整个图卡死 |
| 满时 | 丢弃最旧数据（环形覆盖） | 避免内存涨；丢几百 ms 旧数据好过卡死整个图 |

**线程模型**：
- **写**（SCK 回调，AVAudioEngine 内部线程）：加锁写入
- **读**（SourceNode render，realtime 线程）：**无锁或极短临界区**——realtime 线程不能阻塞
- **必须避免上轮的坑**：v3 crash（exclusivity violation 多线程并发 ring buffer）、v4 死锁（reentrant sync）。用 `os_unfair_lock` 或 `SeqLock`，**不用 `DispatchQueue.sync`**。

### 2.3 PCM 转换：`CMSampleBuffer` → `[Float]`

```swift
func toInterleavedFloat32(_ sb: CMSampleBuffer, targetFormat: AVAudioFormat) -> [Float] {
    // 1. 从 CMSampleBuffer 拿 CMAudioSampleBufferGetAudioBufferListWithBytes → AudioBufferList
    // 2. 用 AVAudioConverter（如格式不是目标格式）重采样到 48k/2ch/float32
    // 3. 拷贝到 [Float]
}
```

**关键不确定性**：SCK `.audio` 输出的 `CMSampleBuffer` 实际 PCM 格式。我们已配 `sampleRate=48_000, channelCount=2`，理论上 SCK 按配置给，但实际可能是：
- Float32 interleaved（最可能）
- Float32 planar
- SInt16 interleaved
- 或 AAC（压缩）—— **技术止损信号 S1**：若 SCK 给压缩格式，转 PCM 需额外解码器，复杂度暴涨。

**A1 实现第一步就要验证这点**——mixer 启动前先打印一次 `CMSampleBuffer` 的 `formatDescription`。

### 2.4 PTS 自产（学 openscreen，避开上轮核心坑）

```swift
var emittedFrames: AVAudioFramePosition = 0
let sampleRate: Double = 48_000

func onMixerTap(_ buffer: AVAudioPCMBuffer, when: AVAudioTime) {
    // 1. 自产 PTS（学 openscreen，丢弃 when）
    let pts = emittedFrames * 1_000_000_000 / AVAudioFramePosition(sampleRate)

    // 2. 构造 CMSampleBuffer（用 pts + buffer.audioBufferList）
    guard let sb = makeCMSampleBuffer(from: buffer, pts: pts, sampleRate: sampleRate) else {
        return  // 技术止损信号 S2 相关：makeCMSampleBuffer 持续失败
    }

    // 3. append 到单条 AVAssetWriterInput
    if writerInput.isReadyForMoreMediaData {
        writerInput.append(sb)
        emittedFrames += AVAudioFramePosition(buffer.frameLength)
    }
}
```

**对照上轮失败**：上轮 v1/v2 都栽在 PTS 配对（试图对齐两路源 PTS），这次完全丢弃源 PTS，按 `emittedFrames` 自产严格单调。

### 2.5 AVAssetWriter 单 input

`setupWriter()` 改动（伪代码，与现有 `addAudioInput` 共用 settings）：

```swift
// 旧（双轨）：
if nativeMicrophoneEnabled { microphoneAudioInput = try addAudioInput(to: writer, bitRate: 128_000) }
if request.audio.system.enabled { systemAudioInput = try addAudioInput(to: writer, bitRate: 192_000) }

// 新（单轨）：
let anyAudio = nativeMicrophoneEnabled || request.audio.system.enabled
if anyAudio {
    mixedAudioInput = try addAudioInput(to: writer, bitRate: 192_000)  // 单轨，bitRate 用 192k
    setupAudioMixer()  // 建 AVAudioEngine 图 + installTap
}
```

**bitRate 取舍**：用 192k（系统音频的值）。单轨要承载两路内容，192k 余量更足。

**关掉音频时**：`anyAudio=false` → 不建 input、不起 mixer、SCK 仍可录但 `.audio` 回调被忽略 → 仅 video mp4。

**只开一边**：`anyAudio=true` → 走单轨路径，mixer 一路有数据一路补零。**与「都开」走同一代码路径**，简化逻辑。

### 2.6 生命周期：start / pause / resume / stop

| 事件 | 动作 |
|---|---|
| start | 建 writer + 单 input → 起 AVAudioEngine（先 prepare，再 start）→ 起 SCStream |
| pause | AVAudioEngine.pause()（停 tap 推送，但 SCK 仍在录）→ writer 暂停 append |
| resume | AVAudioEngine.resume() → writer 恢复 append |
| stop | AVAudioEngine.stop() + remove tap → writer.finishWriting → 发 `recording-stopped` |

**关键**：上轮 v5 失败信号是「drain 在 didStartWriting=false 时 removeFirst 但 append 静默 return」。这次用 AVAudioEngine.pause/resume 替代手动 drain，让 engine 管流控。

---

## 3. 技术止损线 + 验收标准 + 测试策略

### 3.1 技术止损线

**判定原则**：不在中间过程过早喊停，但**每完成一轮实现迭代就跑一次完整 e2e**，以 e2e 客观产出为准。

#### e2e 验证流程（每轮迭代后必跑）

```
1. ./scripts/build-macos-helper.sh        # 重 build helper
2. ./run-octopus.sh --no-lto             # 启动 app
3. 录屏 ≥30s（开系统音频 + 开麦克风）
4. 停止录制，等 recording-stopped 事件
5. ffprobe <mp4>                          # 看 track 数 + 格式
6. 听音 + 波形分析                        # 看是否同时听到两路
```

#### 止损信号（任一命中 → 立刻 revert 回双轨）

| # | 信号 | 来源 | 对应上轮坑 |
|---|---|---|---|
| **S1** | SCK `.audio` 回调给的是压缩格式（AAC 等），转 PCM 需额外解码器 | `CMSampleBuffer.formatDescription` 检查 | 新坑，但会导致复杂度暴涨 |
| **S2** | AVAudioEngine.start() 报错，或 installTap 报错（权限/连接失败） | 启动日志 | 「路不通」硬信号 |
| **S3** | installTap 回调进了，但连续 3s 拿到 `frameLength=0` 或 nil buffer | tap 回调计数 | SCK→SourceNode 桥没接通 |
| **S4** | **`writerInput.append(sb)` 调用了 N 次（N≥10），但 ffprobe 显示 audio track 数=0**（或 duration=0） | ffprobe | **上轮 v4/v5 核心坑，最关键** |
| **S5** | 连续 3 次完整 e2e 都拿不到可解码的 audio track，即便不报错 | e2e | 上轮「不报错但静默失败」陷阱 |

#### 止损后动作

1. **立即 revert** 到当前双轨方案（commit `f8bbe8ed` 状态），不留半残代码
2. 把失败信号 + 诊断 + 已尝试的修复写进本 spec「实现注记」章节
3. 通知用户，由用户决定下一轮（换 ffmpeg amerge / 换 AVAudioSourceNode + AVAudioSinkNode 组合 / 放弃单轨）

**关键纪律**：S4 是上一轮的核心失败信号（v4 加 guard 后 append 日志出现但 track 仍为 0），**这次一旦在 e2e 复现，第 1 轮就喊停**，不再「调一调试试」。

### 3.2 验收标准（四项全过才算「重做成功」）

| # | 检查项 | 命令/方法 | 通过标准 |
|---|---|---|---|
| **A1** | ffprobe 单轨 | `ffprobe -show_streams <mp4>` | `streams[codec_type=audio]` 恰好 1 条 |
| **A2** | 人工听音 | QuickTime 播放 | 同时听到系统音频 + 麦克风，无单边丢失 |
| **A3** | 连续 3 次稳定 | 重复 3 次完整 e2e | 每次都满足 A1+A2，无 crash、无 audio track=0 |
| **A4** | 波形对比 | `ffmpeg -i <mp4> -c:a pcm_s16le out.wav` + Audacity/ffprobe 查波形 | 波形连续、无爆音（峰值 < 0dBFS）、chunk 边界无突变、有语音段 RMS 合理 |

**A4 波形细节**（参考 openscreen Windows AudioMixer 特性验证）：
- **连续性**：波形无静默间隙（除非用户真静默）
- **无爆音**：峰值不超过 0dBFS（clamp -1.0~1.0 生效）
- **chunk 边界**：10ms 或 AVAudioEngine render chunk 边界无爆音尖峰
- **RMS 合理**：有语音段 RMS 在 -30 ~ -10 dBFS 区间

**降级路径**（触发止损）：revert 回双轨，验收标准不适用。

### 3.3 测试策略

按 AGENTS.md「TDD 优先 + 无法先写测试的场景事后补录」准则。AVAudioEngine + SCK + AVAssetWriter 链路**无法纯单测**（需硬件/系统 API），但可分层测试。

#### 可单测的部分（TDD 先行）

| 层 | 可测内容 | 测试方式 |
|---|---|---|
| **PCM 转换**（§2.3） | `toInterleavedFloat32(CMSampleBuffer)` 正确性 | 构造已知 PCM 的 `CMSampleBuffer`，验证转出的 `[Float]` 值 |
| **PTS 自产**（§2.4） | `emittedFrames * 1e9 / 48000` 单调递增 + 正确 | 纯函数，输入 `emittedFrames`，验证 PTS 序列 |
| **环形缓冲**（§2.2） | 推/拉/补零/丢弃最旧 | 单线程模拟推+拉，验证边界（空时补零、满时丢最旧） |
| **增益**（§2.4） | mic `*1.4` 正确 | 输入 `[0.5]`，验证输出 `[0.7]` |

用 Swift XCTest，在 helper 的 Swift Package 里跑（`swift test`）。**先写测试再实现**（TDD）。

#### 只能 e2e 的部分（事后补录）

| 层 | 为何不能单测 | 验证方式 |
|---|---|---|
| AVAudioEngine 图整体 | 需真实音频硬件 + SCK 权限 | e2e（§3.1 流程） |
| SCK `.audio` PCM 格式 | 系统行为，无法 mock | e2e 第一轮首要验证（S1 信号） |
| AVAssetWriter.append 是否生效 | 系统行为，无法 mock | e2e（S4 信号） |

**事后补录**：e2e 验证通过后，把 A1/A2/A3/A4 四项检查固化成 `scripts/verify-audio-mix.sh` 脚本（ffprobe + ffmpeg 波形抽取 + 简单阈值检查），方便回归。

### 3.4 实现期调试支撑

为了快速定位止损信号，实现期在 helper 加 **结构化日志**（stderr → Rust session 的 reader task 已就绪）：

| 日志点 | 内容 | 用途 |
|---|---|---|
| `audio-format-detected` | SCK `.audio` 的 `formatDescription`（PCM/采样率/声道/位深） | 验证 S1 |
| `engine-started` / `engine-start-failed` | AVAudioEngine 启动结果 | 验证 S2 |
| `tap-buffer-received` | 首次 + 每秒一次：tap 收到的 frameLength | 验证 S3 |
| `audio-appended` | 累计 append 次数 + 总 frames | 验证 S4（对比 ffprobe duration） |
| `audio-input-status` | writerInput.isReadyForMoreMediaData 翻转 | 排查背压 |

日志通过现有 `HelperEvent::Warning` 扩展或新增 `HelperEvent::Debug` 类型携带（protocol.rs 改动极小）。

---

## 4. A2 探索 spike + 实施顺序 + 文档同步

### 4.1 A2 探索 spike 设计

A2 是「先花半天探 SCK 私有 API 能否单流输出」。设计成**独立 spike，不污染主代码路径**——目的是快速拿一个 yes/no 答案。

#### Spike 形态

**独立 Swift 文件**，放在 `crates/record/native/macos/Sources/OctopusSckHelper/` 下，但**不接入 helper 主流程**。手动 `swift run` 或加一个隐藏 subcommand 跑。

#### Spike 要验证的 3 件事

| # | 验证内容 | 方法 | yes/no 信号 |
|---|---|---|---|
| **P1** | SCK 当前私有 KVC（`captureMicrophone=true` + `microphoneCaptureDeviceID`）的 mic 音频，**走的是哪条 output type？** | spike 里只起 `SCStream` 配上 mic KVC，注册 `.audio` + `rawValue=2` 两个 output，打印每个回调收到的 type + buffer 量 | mic **只**出现在 rawValue=2 不出现在 `.audio` → A2 失败（当前已知行为，spike 确认） |
| **P2** | 有没有**别的私有 KVC**能让 mic 合并进 `.audio`？ | 文献检索 + spike 试探（如 `audioMix`、`combinedMicrophoneAudio`、`mergeMicrophone` 等 selector probe） | 没找到任何能合并的 KVC → A2 失败 |
| **P3** | 如果 P1/P2 都失败，**禁用 mic KVC，改用独立的 AVCaptureDevice 录麦克风，能不能和 SCK `.audio` 在 AVAssetWriter 单 input 里交错 append？** | spike 里 SCK 录系统音频 + AVCaptureDevice 录 mic，两条都 append 到同一条 `AVAssetWriterInput` | PTS 不单调 / 时间戳错乱 / append 拒绝交错 → A2 失败 |

#### Spike 退出条件（任一）

- **P1 确认 mic 走 rawValue=2，且 P2 找不到合并 KVC** → A2 失败，转 A1（**最可能结果**）
- P3 成功 → 意外收获，但工程复杂度接近 A1（要管两路 PTS），**仍建议转 A1**（A1 用 AVAudioEngine 更干净）
- 半天（4 小时）用尽 → 转 A1

#### Spike 产出物

无论成功失败，spike 结束后产出：
1. **一份 spike 报告**（markdown，放进 `docs/superpowers/research/2026-07-27-sck-single-stream-spike.md`）记录 P1/P2/P3 的实测结果
2. **spike 代码删掉**（不进 main 流程）
3. **决策记录**写进本 spec「实现注记」章节

### 4.2 实施顺序（spec 和 plan 的骨架）

```
Phase 0: A2 探索（半天硬上限）
  └─ spike P1/P2/P3 → 大概率失败 → 转 A1
  └─ 产出: spike 报告 + 决策

Phase 1: A1 基础设施（TDD 先行）
  ├─ Task 1.1: PCM 转换函数 toInterleavedFloat32 + 单测
  ├─ Task 1.2: PTS 自产函数 + 单测
  ├─ Task 1.3: 环形缓冲 RingBuffer<Float> + 单测（os_unfair_lock 或 SeqLock）
  └─ Task 1.4: 麦克风增益 + 单测

Phase 2: AVAudioEngine 图集成
  ├─ Task 2.1: 建 AVAudioEngine + 2× AVAudioSourceNode + AVAudioMixerNode
  ├─ Task 2.2: installTap + 推→拉桥接（写环形缓冲 / render 读环形缓冲）
  ├─ Task 2.3: setupWriter 改单 input
  └─ Task 2.4: 调试日志（§3.4 的 5 个日志点）

Phase 3: 首轮 e2e + 止损判定
  ├─ Task 3.1: build helper + run-octopus
  ├─ Task 3.2: 录屏 ≥30s，跑 §3.1 e2e 流程
  ├─ Task 3.3: 检查 S1-S5 止损信号
  └─ 决策点: 命中任一 → revert + 记录；全过 → 进 Phase 4

Phase 4: 验收 + 固化
  ├─ Task 4.1: 跑 4 项验收（A1/A2/A3/A4）
  ├─ Task 4.2: 写 scripts/verify-audio-mix.sh 回归脚本
  └─ Task 4.3: 清理调试日志（保留必要的）

Phase 5: 文档同步
  ├─ Task 5.1: 更新 spec（A2 结果 + 实际实现偏差）
  ├─ Task 5.2: 更新 plan（每 task 实际状态）
  ├─ Task 5.3: 更新 docs/architecture.md（音轨章节）
  └─ Task 5.4: z-sync-superpowers skill 跑一遍
```

**关键阶段门**：
- **Phase 0 → Phase 1**：A2 决策点（半天）
- **Phase 3 → Phase 4**：止损判定点（e2e 通过才继续）

### 4.3 改动文件清单

| 文件 | 改动类型 | 说明 |
|---|---|---|
| `crates/record/native/macos/Sources/OctopusSckHelper/main.swift` | **大改** | setupWriter 单 input + AVAudioEngine 图 + 推拉桥 + PTS 自产 |
| `crates/record/native/macos/Sources/OctopusSckHelper/RingBuffer.swift` | **新增** | 环形缓冲（Phase 1.3） |
| `crates/record/native/macos/Sources/OctopusSckHelper/Tests/` | **新增** | XCTest 单测（PCM/PTS/RingBuffer/gain） |
| `crates/record/native/macos/Package.swift` | **小改** | 加 XCTest target（如还没有） |
| `crates/record/src/protocol.rs` | **小改** | 加 `HelperEvent::Debug`（携带结构化日志） |
| `crates/record/src/session.rs` | **极小改** | reader task 把 Debug 事件透传到 log |
| `scripts/verify-audio-mix.sh` | **新增** | e2e 回归脚本（Phase 4.2） |
| `docs/superpowers/specs/2026-07-27-screen-record-audio-mix-redesign.md` | **新增** | 本 spec |
| `docs/superpowers/plans/2026-07-27-screen-record-audio-mix-redesign.md` | **新增** | 实施计划 |
| `docs/superpowers/research/2026-07-27-sck-single-stream-spike.md` | **新增** | A2 spike 报告 |
| `docs/architecture.md` | **小改** | 音轨章节更新为「单轨混音」 |
| `docs/superpowers/plans/2026-07-25-screen-record.md` | **小改** | Task 8 后续章节更新 |

**不动**：Rust session 主体逻辑、desktop 命令、DB schema、前端、配置项。

### 4.4 文档同步

按 AGENTS.md「文档先行 + 文档同步」：

| 时机 | 动作 |
|---|---|
| **现在** | 写 spec → 用户 review → 写 plan |
| Phase 0 结束 | 把 A2 spike 结果回填到 spec「实现注记」章节 |
| Phase 3 结束 | 如果止损，把失败诊断回填到 spec「实现注记」章节 |
| Phase 4 结束 | plan 每 task 状态更新（review plan 强制）+ architecture.md 音轨章节 |
| 全部完成 | z-sync-superpowers skill 跑一遍 |

### 4.5 关键风险清单

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| AVAudioSourceNode render 拉不到数据（S3） | 中 | 致命 | 环形缓冲补零 + e2e 首轮验证 |
| append 仍不生效（S4，上轮核心坑） | 中 | 致命 | e2e 首轮验证 + 第 1 轮就喊停 |
| SCK `.audio` 是压缩格式（S1） | 低 | 致命 | 首个日志点验证，半天可判 |
| 麦克风权限未授权导致 engine 启动失败 | 低 | 可恢复 | 已有权限流程 + S2 日志 |
| AVAudioEngine 与 SCK 线程冲突 | 中 | 致命 | 环形缓冲无锁设计 + e2e 验证 |
| 2 秒环形缓冲内存压力（380KB） | 低 | 可忽略 | 已极小，无需优化 |

---

## 附录 A：上一轮失败诊断（来自 plan `2026-07-25-screen-record.md` §「Task 8 后续」）

5 轮实时混音迭代（commit `6cb6fe90` ~ `f9741968`）均失败，已 revert（`f8bbe8ed`）。失败模式：

| 版本 | 策略 | 失败信号 |
|---|---|---|
| v1 (`6cb6fe90`) | PTS 配对 | crash（exclusivity violation） |
| v2 (`8c2de529`) | 帧计数 ring buffer | crash（reentrant sync 死锁） |
| v3 (`bb348b73`) | 独立 mixQueue | 0 audio track（drain 在 didStartWriting=false 时 removeFirst 丢数据） |
| v4 (`0a0ea142`) | 加 guard didStartWriting | append ENTERED 日志出现但 `input.append(sb)` 仍不生效 |
| v5 (`f9741968`) | flushPendingMixSamples | 同 v4，audio track 数仍为 0 |

**核心症状**：`input.append(sb)` 始终不生效，audio track 数仍为 0。诊断穷尽仍无法定位（可能是 AAC 编码器格式不匹配 / CMSampleBuffer 构造问题 / isReadyForMoreMediaData 背压）。

**本 spec 的应对**：S4 止损信号专门针对此——首轮 e2e 一旦复现「append 调用 N≥10 但 track=0」就立刻停，不再「调一调试试」。

## 附录 B：openscreen Windows AudioMixer 关键设计（移植参考）

来源：`/Users/wudarui/workspace/agent/openscreen/electron/native/wgc-capture/src/audio_sample_utils.cpp`（439 行）

| openscreen 设计决策 | 本 spec 对应 |
|---|---|
| 两路源各自 PCM → 独立 `Mutex<VecDeque<f32>>` 队列 | §2.2 环形缓冲（独立两个） |
| `push` 时立刻重采样+声道归一到输出格式（最近邻） | §2.3 PCM 转换（用 AVAudioConverter） |
| 固定 10ms chunk 拉，缺数据填零 | §2.2 环形缓冲补零策略 |
| **丢弃源时间戳，自产 PTS**（`emittedFrames / sampleRate`） | §2.4 PTS 自产 |
| `sleep_until(nextDeadline)` 墙钟节流 | macOS 不需要（AVAudioEngine 自带 render 节拍） |
| mic 单独 `*1.4` gain 后再 mix | §2.1 mic bus gain = 1.4 |
| 相加后 `clamp(-1, 1)` | AVAudioMixerNode 自动 clamp（待验证 A4） |
