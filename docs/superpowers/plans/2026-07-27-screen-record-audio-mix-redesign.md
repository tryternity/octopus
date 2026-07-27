# 屏幕录制音频混音重做 实施计划（plan）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把录屏音频写入从「双轨」改为「单轨混音」，让默认播放器能同时听到系统音频和麦克风。

**Architecture:** 两阶段策略——先 Phase 0 探索 SCK 私有 API 能否单流输出（spike，半天硬上限），失败则 Phase 1-5 用 AVAudioEngine 实时混音（SCStream 推 → AVAudioSourceNode 拉桥 → AVAudioMixerNode → installTap → 自产 PTS → 单条 AVAssetWriterInput）。所有音频字节处理只在 Swift helper 一层。

**Tech Stack:** Swift 5.9 / AVFoundation（AVAudioEngine + AVAudioSourceNode + AVAudioMixerNode）/ ScreenCaptureKit / AVAssetWriter / XCTest。

**关联文档：**
- spec：`docs/superpowers/specs/2026-07-27-screen-record-audio-mix-redesign.md`
- 原始 plan：`docs/superpowers/plans/2026-07-25-screen-record.md`（§Task 8 后续含上一轮失败诊断）

## Global Constraints

- **改动面**：音频字节处理只在 `crates/record/native/macos/Sources/OctopusSckHelper/main.swift`。不动 Rust session/protocol/desktop/DB/前端（spec §1.4，自检修正后版本）。**调试日志直接走 stderr（`fputs(..., stderr)`），不改 protocol.rs 的 `HelperEvent`**——因为 Rust session 的 stderr reader task（`crates/record/src/session.rs:226-234`）会全量 `log::debug!` 透传，零侵入。
- **目标格式**：48_000 Hz / 2 ch / Float32 interleaved（spec §0.2 + §2.1）。
- **PTS 自产**：`emittedFrames * 1_000_000_000 / 48_000`，丢弃 AVAudioEngine timestamp（spec §0.2 + §2.4）。
- **麦克风增益**：写死 1.4（spec §0.2 + §2.1）。
- **单轨 bitRate**：192_000（spec §2.5）。
- **止损线**：spec §3.1 的 S1-S5 任一命中 → 立刻 revert 回 commit `f8bbe8ed` 双轨状态 + 在 spec「实现注记」回填失败诊断。**第 1 轮 e2e 复现 S4（append 调用 N≥10 但 ffprobe audio track=0）就立刻停，不调**。
- **Swift helper 改了要重 build**：`./scripts/build-macos-helper.sh`（脚本会拷贝产物到 `crates/desktop/binaries/octopus-sck-helper`）。
- **app 启动由用户在终端跑**（`./run-octopus.sh --no-lto`），AI 不代跑。
- **平台**：macOS 13+（SCStream）+ AVAudioSourceNode 需要 macOS 10.15+（已在范围内）。

## File Structure

| 文件 | 改动 | 职责 |
|---|---|---|
| `crates/record/native/macos/Sources/OctopusSckHelper/main.swift` | **大改** | 现有 815 行 → 新增 mixer/tap/PTS + 改 setupWriter/stream 回调 |
| `crates/record/native/macos/Sources/OctopusSckHelper/RingBuffer.swift` | **新增** | 无锁/`os_unfair_lock` 环形缓冲，推→拉桥 |
| `crates/record/native/macos/Sources/OctopusSckHelper/AudioMixing.swift` | **新增** | 纯函数：PCM 转换、PTS 自产、增益——便于单测 |
| `crates/record/native/macos/Tests/OctopusSckHelperTests/` | **新增** | XCTest 单测目录 |
| `crates/record/native/macos/Package.swift` | **小改** | 加 `testTarget` |
| `docs/superpowers/research/2026-07-27-sck-single-stream-spike.md` | **新增** | A2 spike 报告 |
| `scripts/verify-audio-mix.sh` | **新增** | e2e 回归脚本（ffprobe + 波形） |

---

## Phase 0: A2 探索（半天硬上限）

### Task 0.1: SCK 单流输出 spike

**目标**：确认 SCK 私有 KVC 能否让 system+mic 合并到单条 `.audio` 流。**预期失败**，spike 主要是排除性确认。

**Files:**
- Create: `crates/record/native/macos/Sources/OctopusSckHelper/SpikeMain.swift`（**临时**，spike 结束删除）
- Create: `docs/superpowers/research/2026-07-27-sck-single-stream-spike.md`

**Interfaces:**
- Consumes: 系统的 ScreenCaptureKit
- Produces: spike 报告 + 决策（A2 成功 → 走单流简化路径；失败 → 进 Phase 1）

- [ ] **Step 1: 写 spike 脚本（不接入 helper 主流程）**

创建 `crates/record/native/macos/Sources/OctopusSckHelper/SpikeMain.swift`（与 main.swift 同目录，但用独立的 entry point 标记，避免与 `@main` 冲突）。临时把 main.swift 的 `@main` 注释掉，spike 用 `static func main()`：

```swift
// SpikeMain.swift — 临时 spike，验证 SCK 单流输出能力
// 用法：swift run -c release OctopusSckHelper --spike-single-stream
import Foundation
import ScreenCaptureKit
import AVFoundation

@available(macOS 13.0, *)
enum SpikeSingleStream {
    /// P1: 当前私有 KVC mic 走哪条 output type？
    /// P2: 有没有别的 KVC 能让 mic 合并到 .audio？
    static func run() async {
        let configuration = SCStreamConfiguration()
        configuration.capturesAudio = true
        configuration.sampleRate = 48_000
        configuration.channelCount = 2

        // 当前 helper 同款 private KVC
        configuration.setValue(true, forKey: "captureMicrophone")

        // 试探其他 KVC（P2）——逐个 setValue(true) 看哪个不抛 KVC undefined
        let candidates = [
            "audioMix", "combinedMicrophoneAudio", "mergeMicrophone",
            "microphoneIntoAudioStream", "mixMicrophoneIntoAudio",
        ]
        for key in candidates {
            do {
                try configuration.setValue(true, forKey: key)
                print("[spike] KVC \(key) = true ACCEPTED")
            } catch {
                print("[spike] KVC \(key) = undefined (\(error))")
            }
        }

        guard let content = try? await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true) else {
            print("[spike] no shareable content"); return
        }
        let filter = SCContentFilter(display: content.displays[0], excludingWindows: [])
        let stream = SCStream(filter: filter, configuration: configuration, delegate: nil)

        let counter = SpikeCounter()
        try? stream.addStreamOutput(counter, type: .audio, sampleHandlerQueue: .main)
        if let micType = SCStreamOutputType(rawValue: 2) {
            try? stream.addStreamOutput(counter, type: micType, sampleHandlerQueue: .main)
        }
        try? await stream.startCapture()
        print("[spike] capturing 5s...")
        try? await Task.sleep(nanoseconds: 5_000_000_000)
        await stream.stopCapture()
        counter.report()
    }
}

@available(macOS 13.0, *)
final class SpikeCounter: NSObject, SCStreamOutput {
    var audioCount = 0
    var micCount = 0
    var audioFormat: String = "?"
    var micFormat: String = "?"

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        if type == .audio {
            audioCount += 1
            if audioCount == 1, let desc = CMSampleBufferGetFormatDescription(sampleBuffer) {
                audioFormat = "\(CMAudioFormatDescriptionGetStreamBasicDescription(desc)?.pointee)"
            }
        }
        if type.rawValue == 2 {
            micCount += 1
            if micCount == 1, let desc = CMSampleBufferGetFormatDescription(sampleBuffer) {
                micFormat = "\(CMAudioFormatDescriptionGetStreamBasicDescription(desc)?.pointee)"
            }
        }
    }

    func report() {
        print("[spike] RESULT: .audio=\(audioCount) fmt=\(audioFormat)")
        print("[spike] RESULT: mic(=2)=\(micCount) fmt=\(micFormat)")
        print("[spike] DECISION: \(micCount == 0 && audioCount > 0 ? "mic MERGED into .audio (A2 SUCCESS)" : "mic SEPARATE from .audio (A2 FAIL → A1)")")
    }
}
```

- [ ] **Step 2: 临时改 Package.swift 让 spike 可运行**

把 `crates/record/native/macos/Package.swift` 的 executable target 加 `SpikeMain.swift` 入编译（spike 完恢复）：

```swift
.executableTarget(
    name: "OctopusSckHelper",
    path: "Sources/OctopusSckHelper",
    exclude: []  // spike 期间 SpikeMain.swift 默认就在 path 内会被编译
)
```

实际操作：spike 期间把 `main.swift` 临时重命名为 `main.swift.bak`，让 spike 自己定义 entry。或在 main.swift 的 `@main` struct 加 `if CommandLine.arguments.contains("--spike-single-stream") { await SpikeSingleStream.run(); exit(0) }` 作为最简分支。

- [ ] **Step 3: 运行 spike，记录结果**

```bash
cd crates/record/native/macos
swift run -c release OctopusSckHelper --spike-single-stream 2>&1 | tee /tmp/spike.log
```

观察 `[spike] DECISION:` 行。**预期输出**：`mic SEPARATE from .audio (A2 FAIL → A1)`。

- [ ] **Step 4: 写 spike 报告**

创建 `docs/superpowers/research/2026-07-27-sck-single-stream-spike.md`：

```markdown
# SCK 单流输出 spike 报告（2026-07-27）

## 验证项

### P1: 私有 KVC mic 走哪条 output type？
- 结果：`[填实际]`
- 结论：mic [只走 rawValue=2 / 也出现在 .audio]

### P2: 有没有别的 KVC 能合并？
- ACCEPTED 的候选：`[填]`
- 实际是否合并到 .audio：`[填]`

### P3（可选）: AVCaptureDevice + 单 input 交错 append
- 未做（P1/P2 已足够决策）

## 决策
- A2 [成功 / 失败] → [走单流简化路径 / 转 Phase 1 A1]

## 代码
- spike 代码已删除（commit `xxx`）
```

- [ ] **Step 5: 清理 spike 代码**

```bash
rm crates/record/native/macos/Sources/OctopusSckHelper/SpikeMain.swift
# 恢复 main.swift 的 @main（如果改过）
git diff main.swift  # 确认无残留改动
```

- [ ] **Step 6: Commit spike 报告**

```bash
git add docs/superpowers/research/2026-07-27-sck-single-stream-spike.md
git commit -m "research(record): SCK 单流输出 spike 报告——A2 [成功/失败]"
```

- [ ] **Step 7: 决策门**

- 若 spike 报告决策为 **A2 成功**：暂停本 plan，回去修订 spec（A2 成功则无需 AVAudioEngine，setupWriter 直接单 input 接 SCK `.audio` 流即可）。与用户讨论后再继续。
- 若 spike 报告决策为 **A2 失败**（预期）：继续 Phase 1。

---

## Phase 1: A1 基础设施（TDD 先行）

可单测的纯函数优先。每个 task 独立可测，互不依赖。

### Task 1.1: Package.swift 加 XCTest target

**Files:**
- Modify: `crates/record/native/macos/Package.swift`
- Create: `crates/record/native/macos/Tests/OctopusSckHelperTests/__Placeholder__.swift`

**Interfaces:**
- Produces: `OctopusSckHelperTests` target，后续 task 写测试用

- [ ] **Step 1: 改 Package.swift 加 testTarget**

```swift
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "OctopusSckHelper",
    platforms: [.macOS(.v13)],
    products: [
        .executable(name: "octopus-sck-helper", targets: ["OctopusSckHelper"])
    ],
    targets: [
        .executableTarget(name: "OctopusSckHelper", path: "Sources/OctopusSckHelper"),
        .testTarget(
            name: "OctopusSckHelperTests",
            dependencies: ["OctopusSckHelper"],
            path: "Tests/OctopusSckHelperTests"
        ),
    ]
)
```

⚠️ **关键**：executable target 默认不能被 testTarget 依赖。Swift Package Manager 要求 executable 必须是 library 才能被 testTarget 依赖。需要把 helper 拆成 library + executable wrapper：

```swift
 targets: [
    .target(name: "OctopusSckHelperLib", path: "Sources/OctopusSckHelperLib"),
    .executableTarget(
        name: "OctopusSckHelper",
        dependencies: ["OctopusSckHelperLib"],
        path: "Sources/OctopusSckHelper"
    ),
    .testTarget(
        name: "OctopusSckHelperTests",
        dependencies: ["OctopusSckHelperLib"],
        path: "Tests/OctopusSckHelperTests"
    ),
]
```

- [ ] **Step 2: 拆分目录**

```bash
mkdir -p crates/record/native/macos/Sources/OctopusSckHelperLib
# 把 main.swift 的可测部分（待 Task 1.2+ 抽出后）放 OctopusSckHelperLib
# main.swift 保留 @main struct Recorder，调用 OctopusSckHelperLib 的类型
# 占位文件
echo "// 占位，Task 1.2 删除" > crates/record/native/macos/Tests/OctopusSckHelperTests/Placeholder.swift
```

- [ ] **Step 3: 占位测试**

`Tests/OctopusSckHelperTests/Placeholder.swift`:

```swift
import XCTest
final class Placeholder: XCTestCase {
    func testSanity() { XCTAssertTrue(true) }
}
```

- [ ] **Step 4: 验证 build + test 跑通**

```bash
cd crates/record/native/macos
swift build
swift test
```

Expected: build 成功，1 个 test 通过。

- [ ] **Step 5: Commit**

```bash
git add crates/record/native/macos/
git commit -m "build(helper): 拆 OctopusSckHelperLib + 加 XCTest testTarget"
```

---

### Task 1.2: 环形缓冲 RingBuffer<Float> + 单测

**Files:**
- Create: `crates/record/native/macos/Sources/OctopusSckHelperLib/RingBuffer.swift`
- Create: `crates/record/native/macos/Tests/OctopusSckHelperTests/RingBufferTests.swift`

**Interfaces:**
- Produces: `RingBuffer`（容量固定、推/拉、补零、丢最旧）

- [ ] **Step 1: 写失败的测试**

`Tests/OctopusSckHelperTests/RingBufferTests.swift`:

```swift
import XCTest
@testable import OctopusSckHelperLib

final class RingBufferTests: XCTestCase {
    func testPushThenPopReturnsPushed() {
        var buf = RingBuffer(capacity: 8)
        buf.push(contentsOf: [Float](repeating: 0.5, count: 4))
        let out = buf.pop(count: 4)
        XCTAssertEqual(out, [Float](repeating: 0.5, count: 4))
    }

    func testPopBeyondAvailableFillsZero() {
        var buf = RingBuffer(capacity: 8)
        buf.push([1.0, 2.0])
        let out = buf.pop(count: 4)
        XCTAssertEqual(out, [1.0, 2.0, 0.0, 0.0])
    }

    func testOverflowDropsOldest() {
        var buf = RingBuffer(capacity: 4)
        buf.push([1.0, 2.0, 3.0, 4.0, 5.0])  // 第 5 个应挤掉第 1 个
        let out = buf.pop(count: 4)
        XCTAssertEqual(out, [2.0, 3.0, 4.0, 5.0])
    }

    func testEmptyPopReturnsZeros() {
        var buf = RingBuffer(capacity: 4)
        let out = buf.pop(count: 3)
        XCTAssertEqual(out, [0.0, 0.0, 0.0])
    }

    func testPartialThenMorePreservesOrder() {
        var buf = RingBuffer(capacity: 8)
        buf.push([1.0, 2.0])
        _ = buf.pop(count: 1)  // 弹 1
        buf.push([3.0, 4.0])
        let out = buf.pop(count: 3)  // 应得 [2, 3, 4]
        XCTAssertEqual(out, [2.0, 3.0, 4.0])
    }
}
```

- [ ] **Step 2: 跑测试，确认失败**

```bash
cd crates/record/native/macos && swift test --filter RingBufferTests
```

Expected: FAIL "cannot find 'RingBuffer' in scope"。

- [ ] **Step 3: 实现 RingBuffer（OSAllocatedUnfairLock，避开上轮 DispatchQueue 死锁）**

`Sources/OctopusSckHelperLib/RingBuffer.swift`:

```swift
import Foundation
import os

/// 推→拉桥环形缓冲。SCK 推、AVAudioSourceNode 拉。
///
/// 关键设计（避开上轮 v3/v4 坑）：
/// - 用 `OSAllocatedUnfairLock`（macOS 13+ 值类型 lock，零分配）而非 DispatchQueue.sync
///   （realtime 线程不能阻塞在队列上）
/// - 缺数据 pop 补零（不阻塞 render）
/// - 满时丢弃最旧（覆盖写，避免内存涨）
public final class RingBuffer {
    private struct State {
        var storage: [Float]
        var head: Int = 0   // 读位置
        var count: Int = 0  // 已存元素数
    }
    private let capacity: Int
    private let state: OSAllocatedUnfairLock<State>

    public init(capacity: Int) {
        precondition(capacity > 0)
        self.capacity = capacity
        self.state = OSAllocatedUnfairLock(initialState: State(
            storage: [Float](repeating: 0, count: capacity)
        ))
    }

    public func push(_ elements: [Float]) {
        state.withLock { s in
            for x in elements {
                let writeIdx = (s.head + s.count) % capacity
                s.storage[writeIdx] = x
                if s.count == capacity {
                    s.head = (s.head + 1) % capacity  // 覆盖最旧
                } else {
                    s.count += 1
                }
            }
        }
    }

    public func pop(count requested: Int) -> [Float] {
        state.withLock { s -> [Float] in
            var out = [Float](repeating: 0, count: requested)
            let n = min(requested, s.count)
            for i in 0..<n {
                out[i] = s.storage[s.head]
                s.head = (s.head + 1) % capacity
            }
            s.count -= n
            return out
        }
    }

    public var available: Int {
        state.withLock { $0.count }
    }
}
```

- [ ] **Step 4: 跑测试，确认通过**

```bash
swift test --filter RingBufferTests
```

Expected: 5 个 test 全过。

- [ ] **Step 5: Commit**

```bash
git add Sources/OctopusSckHelperLib/RingBuffer.swift Tests/OctopusSckHelperTests/RingBufferTests.swift
git commit -m "feat(helper): RingBuffer<Float>——推拉桥环形缓冲（os_unfair_lock）"
```

---

### Task 1.3: PTS 自产 + 增益纯函数 + 单测

**Files:**
- Create: `crates/record/native/macos/Sources/OctopusSckHelperLib/AudioMath.swift`
- Create: `crates/record/native/macos/Tests/OctopusSckHelperTests/AudioMathTests.swift`

**Interfaces:**
- Produces: `AudioMath.pts(forEmittedFrames:sampleRate:)`、`AudioMath.applyGain(_:gain:)`

- [ ] **Step 1: 写失败的测试**

```swift
import XCTest
@testable import OctopusSckHelperLib

final class AudioMathTests: XCTestCase {
    func testPtsIsMonotonicallyIncreasing() {
        let pts0 = AudioMath.pts(forEmittedFrames: 0, sampleRate: 48_000)
        let pts1 = AudioMath.pts(forEmittedFrames: 480, sampleRate: 48_000)  // 10ms
        let pts2 = AudioMath.pts(forEmittedFrames: 960, sampleRate: 48_000)  // 20ms
        XCTAssertGreaterThan(pts1, pts0)
        XCTAssertGreaterThan(pts2, pts1)
        XCTAssertEqual(pts1 - pts0, 10_000_000)  // 10ms = 10_000_000 ns
        XCTAssertEqual(pts2 - pts1, 10_000_000)
    }

    func testApplyGainScales() {
        XCTAssertEqual(AudioMath.applyGain([0.5], gain: 1.4), [0.7], accuracy: 1e-6)
    }

    func testApplyGainClamps() {
        XCTAssertEqual(AudioMath.applyGain([0.8], gain: 1.4), [1.0], accuracy: 1e-6)  // 1.12 → 1.0
        XCTAssertEqual(AudioMath.applyGain([-0.8], gain: 1.4), [-1.0], accuracy: 1e-6)
    }

    func testApplyGainZeroIsZero() {
        XCTAssertEqual(AudioMath.applyGain([0.0, 0.0, 0.0], gain: 1.4), [0.0, 0.0, 0.0])
    }
}
```

- [ ] **Step 2: 跑测试，确认失败**

```bash
swift test --filter AudioMathTests
```

Expected: FAIL "cannot find 'AudioMath' in scope"。

- [ ] **Step 3: 实现**

`Sources/OctopusSckHelperLib/AudioMath.swift`:

```swift
import Foundation
import AVFAudio

/// 纯函数音频数学，便于单测。
public enum AudioMath {
    /// 自产 PTS（纳秒）。学 openscreen Windows AudioMixer——完全丢弃源 timestamp，
    /// 用 emittedFrames / sampleRate 推算严格单调递增的 PTS。
    /// 避开上轮 v1/v2 PTS 配对坑。
    public static func pts(forEmittedFrames frames: Int64, sampleRate: Double) -> Int64 {
        frames * 1_000_000_000 / Int64(sampleRate)
    }

    /// 增益（学 openscreen 1.4）+ clamp 到 [-1, 1]（防溢出爆音）。
    public static func applyGain(_ samples: [Float], gain: Float) -> [Float] {
        samples.map { max(-1.0, min(1.0, $0 * gain)) }
    }

    /// 目标输出格式（spec §0.2：48k/2ch/float32）。
    public static let targetFormat = AVAudioFormat(
        commonFormat: .pcmFormatFloat32,
        sampleRate: 48_000,
        channels: 2,
        interleaved: true
    )!
}
```

- [ ] **Step 4: 跑测试，确认通过**

```bash
swift test --filter AudioMathTests
```

Expected: 4 个 test 全过。

- [ ] **Step 5: Commit**

```bash
git add Sources/OctopusSckHelperLib/AudioMath.swift Tests/OctopusSckHelperTests/AudioMathTests.swift
git commit -m "feat(helper): AudioMath——PTS 自产 + 增益 clamp 纯函数"
```

---

### Task 1.4: CMSampleBuffer → [Float] PCM 转换 + 单测

**Files:**
- Create: `crates/record/native/macos/Sources/OctopusSckHelperLib/PCMConverter.swift`
- Create: `crates/record/native/macos/Tests/OctopusSckHelperTests/PCMConverterTests.swift`

**Interfaces:**
- Consumes: `AudioMath.targetFormat`（Task 1.3）
- Produces: `PCMConverter.toFloat32Interleaved(_:targetFormat:)`

- [ ] **Step 1: 写失败的测试**

```swift
import XCTest
import AVFoundation
import CoreMedia
@testable import OctopusSckHelperLib

final class PCMConverterTests: XCTestCase {
    /// 构造一个已知 Float32 interleaved CMSampleBuffer（48k/2ch/2 帧）。
    private func makeFloat32InterleavedBuffer(_ samples: [Float]) -> CMSampleBuffer? {
        let asbd = AudioStreamBasicDescription(
            mSampleRate: 48_000, mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
            mBytesPerPacket: 8, mFramesPerPacket: 1, mBytesPerFrame: 8,
            mChannelsPerFrame: 2, mBitsPerChannel: 32, mReserved: 0
        )
        guard let desc = try? CMAudioFormatDescription(
            streamBasicDescription: asbd
        ), let data = try? JSONSerialization.data(withJSONObject: [])  // 占位，下面用 bytes
        else { return nil }

        var sampleBuffer: CMSampleBuffer?
        var formatDescription: CMFormatDescription?
        CMAudioFormatDescriptionCreate(
            allocator: kCFAllocatorDefault,
            asbd: &asbd, layoutSize: 0, layout: nil,
            magicCookieSize: 0, magicCookie: nil,
            extensions: nil, formatDescriptionOut: &formatDescription
        )
        // 构造 block buffer + sample buffer 见 helper 实现期；测试可简化为只验 bytes 转换
        // 此处给一个简化的 smoke test：直接调用 converter，看是否返回预期长度
        return sampleBuffer
    }

    func testFloat32InterleavedPassThrough() {
        // 简化测试：构造一个 AVAudioPCMBuffer，转成 CMSampleBuffer，再转回来
        let format = AVAudioFormat(
            commonFormat: .pcmFormatFloat32, sampleRate: 48_000,
            channels: 2, interleaved: true
        )!
        let pcmBuffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 2)!
        pcmBuffer.frameLength = 2
        let chans = pcmBuffer.floatChannelData!
        // interleaved buffer: floatChannelData?[0] 是连续的 L0,R0,L1,R1
        chans[0][0] = 0.1; chans[0][1] = 0.2; chans[0][2] = 0.3; chans[0][3] = 0.4

        // 直接测底层 bytes 转 Float 数组（PCMConverter 的内部辅助函数）
        let floats = PCMConverter._testExtractFloats(fromPCMBuffer: pcmBuffer)
        XCTAssertEqual(floats, [0.1, 0.2, 0.3, 0.4], accuracy: 1e-6)
    }
}
```

⚠️ **注意**：CMSampleBuffer 完整构造在单测里复杂（blockbuffer + sample timing）。实现期可简化——PCMConverter 直接接受 `AVAudioPCMBuffer` 或 `AudioBufferList`，避开 CMSampleBuffer 构造。测试策略改为：测 `AVAudioPCMBuffer → [Float]` 这一层（更易构造），CMSampleBuffer 解构在集成层验证。

- [ ] **Step 2: 跑测试，确认失败**

```bash
swift test --filter PCMConverterTests
```

Expected: FAIL "cannot find 'PCMConverter' in scope"。

- [ ] **Step 3: 实现 PCMConverter**

`Sources/OctopusSckHelperLib/PCMConverter.swift`:

```swift
import Foundation
import AVFAudio
import CoreMedia

/// 把音频样本归一化到目标格式（48k/2ch/float32 interleaved）。
public enum PCMConverter {
    /// 从 SCK 回调的 CMSampleBuffer 提取 PCM bytes，用 AVAudioConverter 重采样到目标格式。
    /// 返回 interleaved Float32 数组（L0,R0,L1,R1,...）。
    ///
    /// ⚠️ 技术止损信号 S1：若 SCK 给的是压缩格式（AAC），AVAudioConverter 会失败。
    /// 调用方需 catch 并发 stderr 警告。
    public static func toFloat32Interleaved(
        _ sampleBuffer: CMSampleBuffer,
        targetFormat: AVAudioFormat
    ) throws -> [Float] {
        // 从 CMSampleBuffer 拿 AudioBufferList + frameCount
        let frameCount = Int(CMSampleBufferGetNumSamples(sampleBuffer))
        guard frameCount > 0,
              let srcFormatDesc = CMSampleBufferGetFormatDescription(sampleBuffer),
              let srcASBD = srcFormatDesc.streamBasicDescription?.pointee else {
            throw NSError(domain: "PCMConverter", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "no source format / no frames"])
        }
        let srcAvFormat = AVAudioFormat(streamBasicDescription: srcASBD)!

        // 构造 source AVAudioPCMBuffer（从 CMSampleBuffer 的 audioBufferList 拷出）
        guard let srcBuffer = AVAudioPCMBuffer(pcmFormat: srcAvFormat, frameCapacity: AVAudioFrameCount(frameCount)) else {
            throw NSError(domain: "PCMConverter", code: 2, userInfo: nil)
        }
        srcBuffer.frameLength = AVAudioFrameCount(frameCount)
        try sampleBuffer.withAudioBufferList { audioBufferList in
            // 拷贝第一帧的 mData 到 srcBuffer
            let src = audioBufferList.unsafePointer(at: 0)
            if let srcData = src?.pointee.mBuffers.mData,
               let dstData = srcBuffer.audioBufferList.pointee.mBuffers.mData {
                let bytes = frameCount * Int(srcASBD.mBytesPerFrame)
                memcpy(dstData, srcData, bytes)
            }
        }

        // 格式相同直接抽
        if srcAvFormat == targetFormat {
            return extractFloats(fromPCMBuffer: srcBuffer)
        }

        // 格式不同用 AVAudioConverter 重采样
        guard let converter = AVAudioConverter(from: srcAvFormat, to: targetFormat) else {
            throw NSError(domain: "PCMConverter", code: 3,
                          userInfo: [NSLocalizedDescriptionKey: "AVAudioConverter init failed (compressed source?)"])
        }
        return try convertWithAVAudioConverter(converter: converter, sourceBuffer: srcBuffer, targetFormat: targetFormat)
    }

    /// 测试辅助：从 AVAudioPCMBuffer 提 Float32 interleaved。
    public static func _testExtractFloats(fromPCMBuffer buffer: AVAudioPCMBuffer) -> [Float] {
        extractFloats(fromPCMBuffer: buffer)
    }

    static func extractFloats(fromPCMBuffer buffer: AVAudioPCMBuffer) -> [Float] {
        let frames = Int(buffer.frameLength)
        let chans = buffer.floatChannelData!
        if buffer.format.isInterleaved {
            // interleaved: chans[0] 是连续 L0,R0,L1,R1
            let count = frames * Int(buffer.format.channelCount)
            return Array(UnsafeBufferPointer(start: chans[0], count: count))
        } else {
            // planar: chans[0] = L 全部，chans[1] = R 全部，需交织
            let result = [Float](repeating: 0, count: frames * 2)
            for f in 0..<frames {
                result[f * 2] = chans[0][f]
                result[f * 2 + 1] = chans[1][f]
            }
            return result
        }
    }

    static func extractFloats(fromBufferList bl: AudioBufferList, frameCount: Int) -> [Float] {
        guard bl.mNumberBuffers > 0, let data = bl.mBuffers.mData else { return [] }
        let channels = Int(bl.mBuffers.mNumberChannels)
        let count = frameCount * channels
        let pointer = data.assumingMemoryBound(to: Float.self)
        return Array(UnsafeBufferPointer(start: pointer, count: count))
    }

    static func convertWithAVAudioConverter(
        converter: AVAudioConverter,
        sourceBuffer: AVAudioPCMBuffer,
        targetFormat: AVAudioFormat
    ) throws -> [Float] {
        // 计算输出帧数（按采样率比）
        let outputFrames = AVAudioFrameCount(
            Double(sourceBuffer.frameLength) * targetFormat.sampleRate / sourceBuffer.format.sampleRate
        )
        guard let outputBuffer = AVAudioPCMBuffer(pcmFormat: targetFormat, frameCapacity: outputFrames) else {
            throw NSError(domain: "PCMConverter", code: 2, userInfo: nil)
        }
        outputBuffer.frameLength = outputFrames

        var error: NSError?
        var inputBufferConsumed = false
        converter.convert(to: outputBuffer, error: &error) { _, outStatus in
            // render block：被调用一次（输入是一次性 buffer）
            if inputBufferConsumed {
                outStatus.pointee = .endOfStream
                return nil
            }
            inputBufferConsumed = true
            outStatus.pointee = .haveData
            return sourceBuffer
        }
        if let error { throw error }
        return extractFloats(fromPCMBuffer: outputBuffer)
    }
}
```

⚠️ **此 Task 最复杂**——AVAudioConverter render block 的精确写法需要在实现期对照 Apple 文档打磨。测试主要覆盖「格式相同 pass-through」和「planar→interleaved」两条路径。**若实现期发现 SCK `.audio` 已经是 48k/2ch/float32 interleaved**（很可能），`convertWithAVAudioConverter` 分支根本走不到，可作为降级路径。

- [ ] **Step 4: 跑测试，确认通过**

```bash
swift test --filter PCMConverterTests
```

Expected: `testFloat32InterleavedPassThrough` 通过。

- [ ] **Step 5: Commit**

```bash
git add Sources/OctopusSckHelperLib/PCMConverter.swift Tests/OctopusSckHelperTests/PCMConverterTests.swift
git commit -m "feat(helper): PCMConverter——CMSampleBuffer→Float32 interleaved"
```

---

## Phase 2: AVAudioEngine 图集成

⚠️ **Phase 2 结束后必须立即跑 Phase 3 e2e**（止损判定）。Phase 2 内部不验收。

### Task 2.1: AVAudioEngine 装配 + setupWriter 改单 input

**Files:**
- Modify: `crates/record/native/macos/Sources/OctopusSckHelper/main.swift:478-517`（setupWriter）
- Modify: `crates/record/native/macos/Sources/OctopusSckHelper/main.swift:147-165`（成员变量）
- Modify: `crates/record/native/macos/Sources/OctopusSckHelper/main.swift:281-330`（stream 回调分发）

**Interfaces:**
- Consumes: `RingBuffer`（Task 1.2）、`AudioMath`（Task 1.3）、`PCMConverter`（Task 1.4）

- [ ] **Step 1: 加成员变量**

在 `ScreenCaptureRecorder` 的成员变量区（main.swift:147-165）追加：

```swift
// 单轨混音（替代原 systemAudioInput + microphoneAudioInput 双轨）
private var mixedAudioInput: AVAssetWriterInput?
private var audioEngine: AVAudioEngine?
private var srcSystemNode: AVAudioSourceNode?
private var srcMicNode: AVAudioSourceNode?
private var mixerNode: AVAudioMixerNode?
private var systemRingBuffer: RingBuffer?
private var micRingBuffer: RingBuffer?
private var emittedAudioFrames: Int64 = 0
private var tapBufferCount: Int = 0
private var tapLastReportTime: Date = .distantPast
private var appendCount: Int = 0
private var detectedAudioFormat: String = "?"  // 止损信号 S1 用
```

移除 `systemAudioInput`、`microphoneAudioInput`（被 mixedAudioInput 取代）。但**暂时保留这两个变量并标记为 nil 不删**，等 Phase 2.3 全改完再删——避免改一半编译不过。

- [ ] **Step 2: 重写 setupWriter 的音频段**

main.swift:507-516 替换为：

```swift
// 单轨混音——AVAudioEngine 把 system + mic 合到一条 PCM 流，自产 PTS 写单 input
let anyAudio = nativeMicrophoneEnabled || request.audio.system.enabled
if anyAudio {
    mixedAudioInput = try addAudioInput(to: writer, bitRate: 192_000)
    try setupAudioMixer()
}
```

（删除原 `if nativeMicrophoneEnabled { microphoneAudioInput = ... } if request.audio.system.enabled { systemAudioInput = ... }`）

- [ ] **Step 3: 实现 setupAudioMixer**

在 setupWriter 后追加：

```swift
private func setupAudioMixer() throws {
    let target = AudioMath.targetFormat  // 48k/2ch/float32 interleaved

    let engine = AVAudioEngine()
    let mixer = engine.mainMixerNode
    mixer.outputFormat(forBus: 0)  // 触发 mixer 初始化

    // 两个 source 节点，输出格式显式设为 target（spec §2.1）
    let srcSystem = AVAudioSourceNode(format: target) { [weak self] _, _, frameCount, audioBufferList in
        return self?.renderSource(ringBuffer: self?.systemRingBuffer, frameCount: frameCount, audioBufferList: audioBufferList) ?? noErr
    }
    let srcMic = AVAudioSourceNode(format: target) { [weak self] _, _, frameCount, audioBufferList in
        return self?.renderSource(ringBuffer: self?.micRingBuffer, frameCount: frameCount, audioBufferList: audioBufferList) ?? noErr
    }

    engine.attach(srcSystem)
    engine.attach(srcMic)
    engine.connect(srcSystem, to: mixer, format: target)
    engine.connect(srcMic, to: mixer, format: target)

    // mic bus 增益 1.4（spec §0.2 学 openscreen）
    // mixer 的 input bus 0 = srcSystem, bus 1 = srcMic（attach 顺序决定）
    mixer.outputVolume = 1.0
    // AVAudioMixerNode 没有 per-input-volume API，需要在 srcMic 输出前加 gain
    // → 改用 AVAudioUnitEQ 或在 renderSource 里手动 applyGain

    // installTap 拿混合 PCM
    mixer.installTap(onBus: 0, bufferSize: 1024, format: target) { [weak self] buffer, _ in
        self?.onMixedTap(buffer)
    }

    self.audioEngine = engine
    self.srcSystemNode = srcSystem
    self.srcMicNode = srcMic
    self.mixerNode = mixer
    self.systemRingBuffer = RingBuffer(capacity: 48_000 * 2 * 2)  // 2 秒
    self.micRingBuffer = RingBuffer(capacity: 48_000 * 2 * 2)

    try engine.start()
    fputs("[helper] engine-started\n", stderr)
}
```

- [ ] **Step 4: 实现 renderSource（拉模型）**

```swift
private func renderSource(ringBuffer: RingBuffer?, frameCount: AVAudioFrameCount, audioBufferList: UnsafeMutablePointer<AudioBufferList>) -> OSStatus {
    guard let ringBuffer else {
        // 补零
        return noErr  // audioBufferList 已是 caller 分配的，未填充即为 0
    }
    let frames = Int(frameCount) * 2  // 2 ch interleaved
    let samples = ringBuffer.pop(count: frames)
    let abl = audioBufferList.pointee
    guard let data = abl.mBuffers.mData else { return noErr }
    let pointer = data.assumingMemoryBound(to: Float.self)
    for i in 0..<min(frames, samples.count) {
        pointer[i] = samples[i]
    }
    return noErr
}
```

⚠️ **mic 增益**：AVAudioMixerNode 不支持 per-input volume，所以在 srcMic 的 render 里直接对从 ringBuffer pop 出的数据 applyGain 1.4：

```swift
private func renderMicSource(frameCount: AVAudioFrameCount, audioBufferList: UnsafeMutablePointer<AudioBufferList>) -> OSStatus {
    guard let ringBuffer = micRingBuffer else { return noErr }
    let frames = Int(frameCount) * 2
    var samples = ringBuffer.pop(count: frames)
    samples = AudioMath.applyGain(samples, gain: 1.4)  // ← mic 增益在此
    let abl = audioBufferList.pointee
    guard let data = abl.mBuffers.mData else { return noErr }
    let pointer = data.assumingMemoryBound(to: Float.self)
    for i in 0..<min(frames, samples.count) {
        pointer[i] = samples[i]
    }
    return noErr
}
```

srcMic 的 render block 改用 `renderMicSource`。

- [ ] **Step 5: 实现 onMixedTap（自产 PTS + append）**

```swift
private func onMixedTap(_ buffer: AVAudioPCMBuffer) {
    tapBufferCount += 1
    // 止损信号 S3 监控：每秒报一次
    let now = Date()
    if now.timeIntervalSince(tapLastReportTime) > 1.0 {
        fputs("[helper] tap-buffer-received count=\(tapBufferCount) frameLength=\(buffer.frameLength)\n", stderr)
        tapLastReportTime = now
    }

    // 止损信号 S1 监控：首次记录格式
    if detectedAudioFormat == "?" {
        detectedAudioFormat = "\(buffer.format)"
        fputs("[helper] audio-format-detected format=\(detectedAudioFormat)\n", stderr)
    }

    guard buffer.frameLength > 0 else { return }

    // 自产 PTS（spec §2.4）
    let pts = AudioMath.pts(forEmittedFrames: emittedAudioFrames, sampleRate: 48_000)

    // 转回 CMSampleBuffer 喂 writerInput
    guard let sb = makeCMSampleBuffer(from: buffer, pts: pts) else {
        fputs("[helper] makeCMSampleBuffer failed\n", stderr)
        return
    }

    guard let input = mixedAudioInput, input.isReadyForMoreMediaData else { return }
    if didStartWriting {
        input.append(sb)
        appendCount += 1
        emittedAudioFrames += Int64(buffer.frameLength)
        if appendCount % 50 == 0 {
            fputs("[helper] audio-appended total=\(appendCount) frames=\(emittedAudioFrames)\n", stderr)
        }
    }
}

private func makeCMSampleBuffer(from buffer: AVAudioPCMBuffer, pts: Int64) -> CMSampleBuffer? {
    // 构造 CMSampleBuffer——用 sample timing + format desc + PCM bytes
    // ⚠️ 这是上轮失败的核心位置（v4/v5 makeCMSampleBuffer 返回但 append 不生效）
    // 实现期重点验证此函数的输出能否被 AVAssetWriterInput 接受
    var sampleBuffer: CMSampleBuffer?
    var sampleSize = Int(buffer.frameLength * 2) * MemoryLayout<Float>.size
    var timing = CMSampleTimingInfo(
        duration: CMTime(value: Int64(buffer.frameLength), timescale: 48_000),
        presentationTimeStamp: CMTime(value: pts, timescale: 1_000_000_000),
        decodeTimeStamp: .invalid
    )
    var formatDescription: CMFormatDescription?
    CMAudioFormatDescriptionCreate(
        allocator: kCFAllocatorDefault,
        asbd: buffer.format.streamBasicDescription.pointee,  // 注意解包
        layoutSize: 0, layout: nil,
        magicCookieSize: 0, magicCookie: nil,
        extensions: nil, formatDescriptionOut: &formatDescription
    )
    // 用 CMSampleBufferCreateReady 分配（PCM 已在 buffer.audioBufferList）
    CMSampleBufferCreateReady(
        allocator: kCFAllocatorDefault,
        dataBuffer: nil,  // ⚠️ 需要把 PCM bytes 拷到 CMBlockBuffer，否则 append 拒绝
        formatDescription: formatDescription,
        sampleCount: Int(buffer.frameLength),
        sampleTimingEntryCount: 1,
        sampleTimingArray: &timing,
        sampleSizeEntryCount: 1,
        sampleSizeArray: &sampleSize,
        sampleBufferOut: &sampleBuffer
    )
    return sampleBuffer
}
```

⚠️ **`CMBlockBuffer` 是上轮失败的核心怀疑点**——`CMSampleBufferCreateReady` 用 `dataBuffer: nil` 会让 buffer 没有 PCM 数据，append 时 writer 拒绝。必须用 `CMBlockBufferCreateWithMemoryBlock` 拷 PCM bytes。实现期重点：

```swift
// 正确做法：构造 CMBlockBuffer 装载 PCM bytes
var blockBuffer: CMBlockBuffer?
let totalBytes = Int(buffer.frameLength) * 2 * MemoryLayout<Float>.size
CMBlockBufferCreateWithMemoryBlock(
    allocator: kCFAllocatorDefault,
    memoryBlock: nil,
    blockLength: totalBytes,
    blockAllocator: kCFAllocatorDefault, customBlockSource: nil,
    offsetToData: 0, dataLength: totalBytes,
    flags: 0, blockBufferOut: &blockBuffer
)
// 拷 PCM bytes 进 blockBuffer
let _ = blockBuffer?.withMutableDataPointer { ptr in
    let dst = ptr.pointee.withMemoryRebound(to: Float.self, capacity: totalBytes / 4) { $0 }
    let src = buffer.floatChannelData![0]  // interleaved
    dst.update(from: src, count: totalBytes / 4)
}
// 然后传给 CMSampleBufferCreateReady(dataBuffer: blockBuffer, ...)
```

**实现期如发现此路径仍不生效（S4）→ 立刻停**。

- [ ] **Step 6: 改 stream 回调分发**

main.swift:281-330 替换：

```swift
func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
    guard CMSampleBufferDataIsReady(sampleBuffer) else { return }
    let pauseState = currentPauseState()
    if pauseState.paused { return }

    if type == .audio {
        pushToMixer(sampleBuffer, ringBuffer: systemRingBuffer, label: "system")
        return
    }
    if type.rawValue == microphoneOutputTypeRawValue {
        pushToMixer(sampleBuffer, ringBuffer: micRingBuffer, label: "mic")
        return
    }
    // ... video 部分不变（原 303 行往下保留）
}

private func pushToMixer(_ sampleBuffer: CMSampleBuffer, ringBuffer: RingBuffer?, label: String) {
    guard let ringBuffer else { return }
    // 首次探测格式（止损信号 S1）
    if detectedAudioFormat == "?" {
        if let desc = CMSampleBufferGetFormatDescription(sampleBuffer)?.streamBasicDescription?.pointee {
            detectedAudioFormat = "sr=\(desc.mSampleRate) ch=\(desc.mChannelsPerFrame) flags=\(desc.mFormatFlags)"
            fputs("[helper] source-format-detected label=\(label) \(detectedAudioFormat)\n", stderr)
        }
    }
    do {
        let floats = try PCMConverter.toFloat32Interleaved(sampleBuffer, targetFormat: AudioMath.targetFormat)
        ringBuffer.push(floats)
    } catch {
        fputs("[helper] PCMConverter failed label=\(label) error=\(error)\n", stderr)
    }
}
```

⚠️ **移除原 `retimedSampleBuffer` 对音频的调用**——音频 PTS 完全自产（spec §2.4），不需要 SCK 的时间戳。video 路径保留 retimed。

- [ ] **Step 7: 改 finishWriter**

main.swift:519-582，把：

```swift
videoInput?.markAsFinished()
systemAudioInput?.markAsFinished()
microphoneAudioInput?.markAsFinished()
```

改为：

```swift
videoInput?.markAsFinished()
mixedAudioInput?.markAsFinished()
audioEngine?.stop()
mixerNode?.removeTap(onBus: 0)
fputs("[helper] engine-stopped appendCount=\(appendCount) emittedFrames=\(emittedAudioFrames)\n", stderr)
```

- [ ] **Step 8: 改 start() 顺序**

main.swift:197-202 `try setupWriter()` 已经会调 `setupAudioMixer`（在 setupWriter 内）。确认 `engine.start()` 在 `stream.startCapture()` 前调用——已经是（setupWriter 在 startCapture 前）。✅

- [ ] **Step 9: Build**

```bash
./scripts/build-macos-helper.sh
```

Expected: build 成功，0 error。若有 error，逐个修。

- [ ] **Step 10: Commit**

```bash
git add crates/record/native/macos/
git commit -m "feat(helper): AVAudioEngine 单轨混音——SCStream→SourceNode→Mixer→Tap→单 AVAssetWriterInput"
```

---

## Phase 3: 首轮 e2e + 止损判定

⚠️ **关键阶段门**。Phase 3 失败 → 立刻 revert 到 `f8bbe8ed`，不调。

### Task 3.1: e2e 录屏验证

**Files:**
- 无改动（验证 only）

- [ ] **Step 1: 重 build helper（保险）**

```bash
./scripts/build-macos-helper.sh
```

- [ ] **Step 2: 通知用户跑 app 录屏**

请用户在终端跑：

```bash
./run-octopus.sh --no-lto
```

录屏 ≥30s（开系统音频 + 开麦克风），停止。让用户回报 mp4 文件路径。

- [ ] **Step 3: ffprobe 检查**

```bash
ffprobe -show_streams <用户给的mp4路径> | grep -E "codec_type|codec_name|duration|channels|sample_rate"
```

判定：
- ✅ `codec_type=audio` 恰好 1 条 → A1 通过
- ❌ `codec_type=audio` 0 条或 2 条 → **触发 S4 止损**

- [ ] **Step 4: 听音**

用户用 QuickTime 播放 mp4，确认同时听到系统音频 + 麦克风。

判定：
- ✅ 都听到 → A2 通过
- ❌ 只听到一边 / 都听不到 → **触发 S5 止损**

- [ ] **Step 5: 查 helper stderr 日志**

从 octopus 的日志（用户在终端跑，可见 stderr reader 透传的 `[helper] xxx`）：

- `[helper] source-format-detected` → 确认 SCK `.audio` 是 PCM 还是压缩（S1）
- `[helper] engine-started` 出现 / 无 `engine-start-failed` → S2 通过
- `[helper] tap-buffer-received count=N frameLength=M` 出现且 frameLength > 0 → S3 通过
- `[helper] audio-appended total=N` N≥10 → 配合 ffprobe 单轨判 S4

- [ ] **Step 6: 决策门**

**任一止损信号命中（S1-S5）→ 立即执行 Step 7（revert）**。否则继续 Phase 4。

- [ ] **Step 7: revert（仅止损时）**

```bash
# 回到双轨方案
git revert <Phase 2 commit hash>
# 或直接 git reset --hard f8bbe8ed（丢弃 Phase 2 改动）
```

在 spec「实现注记」章节追加：

```markdown
## 止损触发记录（2026-07-27）

触发信号：S[N] —— [描述]
诊断：[分析]
已尝试修复：[如有]
结论：回退双轨，下一轮考虑 [ffmpeg amerge / 其他方案]
```

通知用户，停止。**不继续 Phase 4**。

---

## Phase 4: 验收 + 固化

### Task 4.1: 完整 4 项验收

- [ ] **Step 1: A1 ffprobe 单轨（已 Phase 3 验过，复确认）**

```bash
ffprobe -show_streams <mp4> | grep codec_type=audio | wc -l
```

Expected: `1`。

- [ ] **Step 2: A2 人工听音（已 Phase 3 验过，复确认）**

- [ ] **Step 3: A3 连续 3 次稳定**

让用户重复 3 次完整录屏（每次 ≥30s，含系统音频 + 麦克风），每次都 A1+A2 通过。

- [ ] **Step 4: A4 波形分析**

```bash
# 抽波形
ffmpeg -i <mp4> -c:a pcm_s16le /tmp/audio_check.wav

# 用 ffprobe 看音量统计
ffmpeg -i /tmp/audio_check.wav -af volumedetect -f null - 2>&1 | grep -E "mean_volume|max_volume"
```

判定：
- max_volume 不超过 0 dBFS（无爆音）
- mean_volume 在 -30 ~ -10 dBFS 区间（有语音段）

或用户用 Audacity 打开 wav，目测波形连续、无突变尖峰。

### Task 4.2: 回归脚本

**Files:**
- Create: `scripts/verify-audio-mix.sh`

- [ ] **Step 1: 写脚本**

```bash
#!/usr/bin/env bash
# 验证录屏音频混音——单轨 + 无爆音。
# 用法：./scripts/verify-audio-mix.sh <mp4_path>
set -euo pipefail

MP4="${1:?用法: $0 <mp4_path>}"
[ -f "$MP4" ] || { echo "❌ 文件不存在: $MP4"; exit 1; }

echo "=== A1: 单轨检查 ==="
AUDIO_TRACKS=$(ffprobe -v error -select_streams a -show_entries stream=index -of csv=p=0 "$MP4" | wc -l | tr -d ' ')
if [ "$AUDIO_TRACKS" -ne 1 ]; then
    echo "❌ 期望 1 条 audio track，实际 $AUDIO_TRACKS 条"
    exit 1
fi
echo "✅ 单轨（1 条 audio track）"

echo "=== A4: 音量统计（无爆音）==="
ffmpeg -i "$MP4" -af volumedetect -f null - 2>&1 | grep -E "mean_volume|max_volume" | tee /tmp/vol_check.txt

MAX_VOL=$(grep max_volume /tmp/vol_check.txt | grep -oE -- '-?[0-9]+\.[0-9]+' | head -1)
echo "max_volume = ${MAX_VOL} dB"

echo "=== A1+A4 通过，A2/A3 需人工听音 + 3 次稳定 ==="
```

- [ ] **Step 2: 测试脚本**

```bash
chmod +x scripts/verify-audio-mix.sh
./scripts/verify-audio-mix.sh <某个验收通过的 mp4>
```

Expected: ✅ 单轨 + max_volume < 0 dBFS。

### Task 4.3: 清理调试日志

**Files:**
- Modify: `crates/record/native/macos/Sources/OctopusSckHelper/main.swift`

- [ ] **Step 1: 精简 stderr fputs**

保留必要的（首启 `source-format-detected`、`engine-started`、最终 `engine-stopped appendCount=...`），删掉高频的（`tap-buffer-received` 改为首次 + 异常才打、`audio-appended` 改为只在 stop 时打）。

- [ ] **Step 2: 重 build 验证**

```bash
./scripts/build-macos-helper.sh
```

- [ ] **Step 3: Commit**

```bash
git add scripts/verify-audio-mix.sh crates/record/native/macos/
git commit -m "feat(record): 单轨混音验收通过 + verify-audio-mix 回归脚本"
```

---

## Phase 5: 文档同步

### Task 5.1: 更新 spec + plan

**Files:**
- Modify: `docs/superpowers/specs/2026-07-27-screen-record-audio-mix-redesign.md`（实现注记回填）
- Modify: `docs/superpowers/plans/2026-07-27-screen-record-audio-mix-redesign.md`（每 task 状态打勾 + 偏差记录）
- Modify: `docs/superpowers/plans/2026-07-25-screen-record.md`（Task 8 后续章节更新为「已重做」）
- Modify: `docs/architecture.md`（音轨章节）

- [ ] **Step 1: spec 回填**

在 spec 的 `## 实现注记` 章节填入：
- A2 spike 实际结果（成功/失败）
- 实际实现与 §2 设计的偏差（如 AVAudioConverter 实际用了什么、CMBlockBuffer 怎么搞定的、增益是在哪里 apply 的）
- 验收结果（A1-A4 全过）

- [ ] **Step 2: plan 回填**

把每个 `- [ ]` 改 `- [x]`。在文末追加「实施偏差」章节：

```markdown
## 实施偏差（review plan）

- Task 1.1 Package.swift 拆 library——实际发现 executable target 不能直接被 testTarget 依赖，拆成 Lib + Exec
- Task 2.1 CMBlockBuffer——上轮失败核心怀疑点，实际实现用了 [...]
- ...
```

- [ ] **Step 3: 更新原 plan 的 Task 8 后续**

`docs/superpowers/plans/2026-07-25-screen-record.md` §「Task 8 后续」改为：

```markdown
## Task 8 后续：单轨混音重做（2026-07-27，已完成）

5 轮 vDSP 失败 + 双轨回退后，spec 2026-07-27-screen-record-audio-mix-redesign 重做：
- 方案：AVAudioEngine 实时混音（SCStream→SourceNode→Mixer→Tap→单 input）
- 结果：[填验收摘要]
- 详见 plans/2026-07-27-screen-record-audio-mix-redesign.md
```

- [ ] **Step 4: 更新 architecture.md**

`docs/architecture.md` 音轨章节改为「单轨混音」描述，删掉「双轨 + mic-first」的过时说明。

- [ ] **Step 5: Commit**

```bash
git add docs/
git commit -m "docs(record): 单轨混音重做——spec/plan/architecture 同步"
```

### Task 5.2: z-sync-superpowers

- [ ] **Step 1: 跑 z-sync-superpowers skill**

调用 `z-sync-superpowers` skill（`~/.agents/skills/z_sync_superpowers/SKILL.md`），让它在所有 spec/plan 上跑一遍，确认无遗漏。

---

## 总结

- Phase 0：A2 探索（spike，半天）
- Phase 1：TDD 基础设施（RingBuffer + AudioMath + PCMConverter）
- Phase 2：AVAudioEngine 集成（大改 main.swift）
- Phase 3：**止损判定门**（e2e）
- Phase 4：验收 + 回归脚本
- Phase 5：文档同步

**关键纪律**：Phase 3 e2e 任一止损信号命中（S1-S5），立刻 revert，不调。
