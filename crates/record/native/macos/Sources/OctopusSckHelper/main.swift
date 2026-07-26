//
//  main.swift
//  OctopusSckHelper
//
//  Vendor 自 openscreen（https://github.com/EtienneLescot/openscreen）
//  原文件：electron/native/screencapturekit/Sources/OpenScreenScreenCaptureKitHelper/main.swift
//  上游 commit：f57e36e25448b5af6c7b1b271066fe5beb9b8a49
//  原作者：Siddharth Vaddem（MIT License，Copyright (c) 2025）
//
//  octopus 修改点（完整声明见本目录 LICENSE）：
//  1. product/target 名 openscreen-screencapturekit-helper → octopus-sck-helper
//  2. 删除 RecordingRequest.webcam / cursor 字段（octopus MVP 不需要）
//  3. 新增 5 个子命令模式：--list-displays / --list-windows / --list-microphones / --check-permission / --request-permission
//  4. emit 事件 schema 对齐 octopus protocol.rs（snake_case，含 timestamp_ms/duration_ms/file_size）
//

import AVFoundation
import Accelerate
import AppKit
import CoreGraphics
import CoreMedia
import Foundation
import ScreenCaptureKit

struct Rectangle: Decodable {
	let x: Double
	let y: Double
	let width: Double
	let height: Double
}

struct RecordingRequest: Decodable {
	struct Source: Decodable {
		let type: String
		let sourceId: String?
		let displayId: UInt32?
		let windowId: UInt32?
		let bounds: Rectangle?
		// Area capture（与 octopus protocol.rs::Source::Area 对齐）
		// 坐标单位：显示器内物理像素（与 DisplayInfo.width/height = CGDisplayPixelsWide 同体系）。
		// makeCaptureTarget 转 sourceRect 时 / scale 得逻辑 points（SCStreamConfiguration.sourceRect 要求）。
		let x: Int32?
		let y: Int32?
		let width: UInt32?
		let height: UInt32?
	}

	struct Video: Decodable {
		let fps: Int
		let width: Int
		let height: Int
		let bitrate: Int?
		let hideSystemCursor: Bool
	}

	struct Audio: Decodable {
		struct SystemAudio: Decodable {
			let enabled: Bool
			// octopus protocol.rs::SystemAudioConfig.excludes_current_process（避免录到 octopus 自己的提示音）。
			// 上游 openscreen 没这字段；vendor 后补，Optional 兼容旧 JSON。
			let excludesCurrentProcess: Bool?
		}

		struct Microphone: Decodable {
			let enabled: Bool
			let deviceId: String?
			let deviceName: String?
			let gain: Double?
		}

		let system: SystemAudio
		let microphone: Microphone
	}

	struct Outputs: Decodable {
		let screenPath: String
		let manifestPath: String?
	}

	let schemaVersion: Int?
	let recordingId: Int?
	let source: Source
	let video: Video
	let audio: Audio
	let outputs: Outputs
}

enum HelperError: Error, CustomStringConvertible {
	case invalidArguments
	case unsupportedMacOS
	case unsupportedFeature(String)
	case sourceNotFound(String)
	case invalidSourceType(String)
	case permissionDenied(String)
	case writerSetupFailed(String)

	var description: String {
		switch self {
		case .invalidArguments:
			return "Expected one JSON recording request argument."
		case .unsupportedMacOS:
			return "ScreenCaptureKit recording requires macOS 13 or newer."
		case .unsupportedFeature(let message):
			return message
		case .sourceNotFound(let message):
			return message
		case .invalidSourceType(let sourceType):
			return "Unsupported source type: \(sourceType)."
		case .permissionDenied(let message):
			return message
		case .writerSetupFailed(let message):
			return message
		}
	}
}

func emit(_ fields: [String: Any]) {
	if let data = try? JSONSerialization.data(withJSONObject: fields, options: []),
		let line = String(data: data, encoding: .utf8)
	{
		print(line)
		fflush(stdout)
	}
}

func emitError(code: String, message: String) {
	emit([
		"event": "error",
		"code": code,
		"message": message,
	])
}

@available(macOS 13.0, *)
final class ScreenCaptureRecorder: NSObject, SCStreamOutput, SCStreamDelegate {
	private struct CaptureTarget {
		let filter: SCContentFilter
		let width: Int
		let height: Int
		/// Area capture 的源裁剪矩形（逻辑 points）。nil = 全屏捕获（display/window 模式）。
		/// 仅 macOS 14+ 生效（SCStreamConfiguration.sourceRect 是 14 API）。
		let sourceRect: CGRect?
	}

	private let request: RecordingRequest
	private let sampleQueue = DispatchQueue(label: "app.octopus.sck-helper.samples")
	private let stateQueue = DispatchQueue(label: "app.octopus.sck-helper.state")
	private var stream: SCStream?
	private var writer: AVAssetWriter?
	private var videoInput: AVAssetWriterInput?
	// 混音输出：系统音频 + 麦克风实时混合成单条 AAC 轨（2026-07-26）。
	// 原本分 systemAudioInput / microphoneAudioInput 两条轨，但播放器默认只放第一条 →
	// 用户听不到麦克风（误以为没采集）。改为单 mixedAudioInput 符合主流录屏软件行为。
	private var mixedAudioInput: AVAssetWriterInput?
	/// 混音目标格式——锁定自首个样本（期望 48k/stereo/float32 non-interleaved）。
	/// 不符则用 AVAudioConverter 转换。
	private var mixedAudioFormat: AVAudioFormat?
	/// 双 deque 按 PTS 等待配对——**所有访问必须在 sampleQueue**（已是串行队列，无需额外锁）。
	private var pendingSystem: [(CMTime, AVAudioPCMBuffer)] = []
	private var pendingMic: [(CMTime, AVAudioPCMBuffer)] = []
	/// 背压上限（秒）：deque 内样本总时长超此则丢最旧 + emit warning（防一边卡住时无限堆积）。
	private let pendingMaxSeconds: Double = 2.0
	/// PTS 对齐窗口（秒）：若两边 PTS 落后超此仍无配对，则单独 passthrough（不混音直接写）。
	/// 这同时处理「只开 system 或只开 mic」的场景——另一边永远空，永远走 passthrough。
	private let mixAlignWindowSeconds: Double = 0.2
	private var didStartWriting = false
	private var didEmitRecordingStarted = false
	private var isStopping = false
	private var isPaused = false
	private var pauseStartedAt: CMTime?
	private var totalPausedDuration = CMTime.zero
	private var nativeMicrophoneEnabled = false
	private var outputWidth = 1920
	private var outputHeight = 1080
	/// Area capture 的源裁剪矩形（makeCaptureTarget 设，makeStreamConfiguration 读）。
	/// nil = 全屏（display/window 模式）；非 nil = area 模式（macOS 14+ sourceRect）。
	private var sourceRect: CGRect? = nil
	private let microphoneOutputTypeRawValue = 2
	private let hostClock = CMClockGetHostTimeClock()

	init(request: RecordingRequest) {
		self.request = request
	}

	func start() async throws {
		try ensureRequestedPermissions()

		let content = try await SCShareableContent.excludingDesktopWindows(
			false,
			onScreenWindowsOnly: true
		)
		let target = try makeCaptureTarget(from: content)
		outputWidth = target.width
		outputHeight = target.height
		sourceRect = target.sourceRect
		let configuration = makeStreamConfiguration()
		let stream = SCStream(filter: target.filter, configuration: configuration, delegate: self)

		try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: sampleQueue)
		if request.audio.system.enabled {
			try stream.addStreamOutput(self, type: .audio, sampleHandlerQueue: sampleQueue)
		}
		if nativeMicrophoneEnabled {
			guard let microphoneOutputType = SCStreamOutputType(rawValue: microphoneOutputTypeRawValue) else {
				throw HelperError.unsupportedFeature(
					"Native microphone capture requires a macOS version with ScreenCaptureKit microphone output."
				)
			}
			try stream.addStreamOutput(self, type: microphoneOutputType, sampleHandlerQueue: sampleQueue)
		}
		try setupWriter()

		self.stream = stream
		emit(["event": "ready", "schema_version": 1])
		try await stream.startCapture()
	}

	func stop() async {
		let shouldStop = stateQueue.sync {
			if isStopping {
				return false
			}
			isStopping = true
			return true
		}
		if !shouldStop {
			return
		}

		do {
			try await stream?.stopCapture()
		} catch {
			emit([
				"event": "warning",
				"code": "stop-capture-failed",
				"message": "\(error)",
			])
		}

		await finishWriter()
	}

	func pause() {
		let didPause = stateQueue.sync {
			if isStopping || isPaused {
				return false
			}

			isPaused = true
			pauseStartedAt = CMClockGetTime(hostClock)
			return true
		}

		if didPause {
			emit([
				"event": "recording-paused",
				"timestamp_ms": Int(Date().timeIntervalSince1970 * 1000),
			])
		}
	}

	func resume() {
		let didResume = stateQueue.sync {
			if isStopping || !isPaused {
				return false
			}

			if let pauseStartedAt {
				let now = CMClockGetTime(hostClock)
				totalPausedDuration = CMTimeAdd(
					totalPausedDuration,
					CMTimeSubtract(now, pauseStartedAt)
				)
			}
			isPaused = false
			pauseStartedAt = nil
			return true
		}

		if didResume {
			emit([
				"event": "recording-resumed",
				"timestamp_ms": Int(Date().timeIntervalSince1970 * 1000),
			])
		}
	}

	func stream(_ stream: SCStream, didStopWithError error: Error) {
		emitError(code: "capture-stopped-with-error", message: "\(error)")
		Task {
			await stop()
		}
	}

	func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
		guard CMSampleBufferDataIsReady(sampleBuffer) else {
			return
		}
		let pauseState = currentPauseState()
		if pauseState.paused {
			return
		}
		guard let sampleBuffer = retimedSampleBuffer(sampleBuffer, subtracting: pauseState.offset) else {
			return
		}

		if type == .audio {
			enqueueForMix(sampleBuffer, into: &pendingSystem)
			drainMixableSamples()
			return
		}

		if type.rawValue == microphoneOutputTypeRawValue {
			enqueueForMix(sampleBuffer, into: &pendingMic)
			drainMixableSamples()
			return
		}

		guard type == .screen else {
			return
		}
		guard isCompleteFrame(sampleBuffer) else {
			return
		}
		guard let videoInput, let writer else {
			return
		}
		let presentationTime = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
		if !didStartWriting {
			writer.startWriting()
			writer.startSession(atSourceTime: presentationTime)
			didStartWriting = true
		}

		if videoInput.isReadyForMoreMediaData {
			if videoInput.append(sampleBuffer), !didEmitRecordingStarted {
				didEmitRecordingStarted = true
				emit([
					"event": "recording-started",
					"timestamp_ms": Int(Date().timeIntervalSince1970 * 1000),
					"width": outputWidth,
					"height": outputHeight,
				])
			}
		}
	}

	private func ensureRequestedPermissions() throws {
		if !CGPreflightScreenCaptureAccess() {
			let granted = CGRequestScreenCaptureAccess()
			if !granted {
				throw HelperError.permissionDenied("Screen recording permission is required for ScreenCaptureKit capture.")
			}
		}

		if request.audio.microphone.enabled {
			switch AVCaptureDevice.authorizationStatus(for: .audio) {
			case .authorized:
				break
			case .notDetermined:
				let semaphore = DispatchSemaphore(value: 0)
				AVCaptureDevice.requestAccess(for: .audio) { _ in
					semaphore.signal()
				}
				let waitResult = semaphore.wait(timeout: .now() + 30)
				if waitResult == .timedOut || AVCaptureDevice.authorizationStatus(for: .audio) != .authorized {
					throw HelperError.permissionDenied("Microphone permission is required for native microphone capture.")
				}
			default:
				throw HelperError.permissionDenied("Microphone permission is required for native microphone capture.")
			}
		}
	}

	private func makeCaptureTarget(from content: SCShareableContent) throws -> CaptureTarget {
		switch request.source.type {
		case "display":
			guard let displayId = request.source.displayId else {
				throw HelperError.sourceNotFound("Display capture requires source.displayId.")
			}
			guard let display = content.displays.first(where: { $0.displayID == displayId }) else {
				throw HelperError.sourceNotFound("No ScreenCaptureKit display found for id \(displayId).")
			}
			let width = Int(CGDisplayPixelsWide(display.displayID))
			let height = Int(CGDisplayPixelsHigh(display.displayID))
			return CaptureTarget(
				filter: SCContentFilter(display: display, excludingWindows: []),
				width: clampCaptureDimension(width, fallback: request.video.width),
				height: clampCaptureDimension(height, fallback: request.video.height),
				sourceRect: nil
			)
		case "window":
			guard let windowId = request.source.windowId else {
				throw HelperError.sourceNotFound("Window capture requires source.windowId.")
			}
			guard let window = content.windows.first(where: { $0.windowID == windowId }) else {
				throw HelperError.sourceNotFound("No ScreenCaptureKit window found for id \(windowId).")
			}
			let candidateDisplay = content.displays.first {
				$0.frame.intersects(window.frame) || $0.frame.contains(CGPoint(x: window.frame.midX, y: window.frame.midY))
			}
			let scaleFactor = Self.scaleFactor(for: candidateDisplay?.displayID ?? CGMainDisplayID())
			let width = Int(window.frame.width) * scaleFactor
			let height = Int(window.frame.height) * scaleFactor
			return CaptureTarget(
				filter: SCContentFilter(desktopIndependentWindow: window),
				width: clampCaptureDimension(width, fallback: request.video.width),
				height: clampCaptureDimension(height, fallback: request.video.height),
				sourceRect: nil
			)
		case "area":
			// Area capture：用 display filter + sourceRect 裁剪到指定区域。
			// SCStreamConfiguration.sourceRect 是 macOS 14+ API（CGAffineTransform 裁剪源）。
			// 输入 x/y/width/height 是显示器内物理像素，转 sourceRect 需 / scale 得逻辑 points。
			guard let displayId = request.source.displayId,
			      let areaX = request.source.x,
			      let areaY = request.source.y,
			      let areaW = request.source.width,
			      let areaH = request.source.height else {
				throw HelperError.sourceNotFound("Area capture requires source.displayId + x/y/width/height.")
			}
			guard let display = content.displays.first(where: { $0.displayID == displayId }) else {
				throw HelperError.sourceNotFound("No ScreenCaptureKit display found for id \(displayId).")
			}
			let scale = Self.scaleFactor(for: displayId)
			let sourceRect = CGRect(
				x: CGFloat(areaX) / CGFloat(scale),
				y: CGFloat(areaY) / CGFloat(scale),
				width: CGFloat(areaW) / CGFloat(scale),
				height: CGFloat(areaH) / CGFloat(scale)
			)
			return CaptureTarget(
				filter: SCContentFilter(display: display, excludingWindows: []),
				width: clampCaptureDimension(Int(areaW), fallback: request.video.width),
				height: clampCaptureDimension(Int(areaH), fallback: request.video.height),
				sourceRect: sourceRect
			)
		default:
			throw HelperError.invalidSourceType(request.source.type)
		}
	}

	private func makeStreamConfiguration() -> SCStreamConfiguration {
		let configuration = SCStreamConfiguration()
		configuration.width = outputWidth
		configuration.height = outputHeight
		configuration.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(max(1, request.video.fps)))
		configuration.queueDepth = 6
		configuration.showsCursor = !request.video.hideSystemCursor
		configuration.pixelFormat = kCVPixelFormatType_32BGRA
		configuration.sampleRate = 48_000
		configuration.channelCount = 2
		configuration.excludesCurrentProcessAudio = request.audio.system.excludesCurrentProcess ?? true
		configuration.capturesAudio = request.audio.system.enabled

		// Area capture：应用 sourceRect 裁剪（macOS 14+ API）。
		// macOS 13 不支持 sourceRect，调用方（desktop 层）应在选 area 时检查版本；
		// 这里兜底——若 13.x 误传 area，emit warning + 不裁剪（录整个 display）。
		if let rect = sourceRect {
			if #available(macOS 14.0, *) {
				configuration.sourceRect = rect
			} else {
				emit([
					"event": "warning",
					"code": "area-capture-requires-macos-14",
					"message": "Area capture (sourceRect) requires macOS 14+. Capturing full display.",
				])
			}
		}

		if request.audio.microphone.enabled {
			guard supportsNativeMicrophoneCapture(streamConfig: configuration) else {
				nativeMicrophoneEnabled = false
				emit([
					"event": "warning",
					"code": "microphone-unavailable",
					"message": "Native microphone capture requires ScreenCaptureKit microphone support on this macOS version.",
				])
				return configuration
			}
			nativeMicrophoneEnabled = true
			configuration.capturesAudio = true
			configuration.setValue(true, forKey: "captureMicrophone")
			if let deviceId = resolveMicrophoneCaptureDeviceID() {
				configuration.setValue(deviceId, forKey: "microphoneCaptureDeviceID")
			}
		} else {
			nativeMicrophoneEnabled = false
		}

		return configuration
	}

	private func setupWriter() throws {
		let outputUrl = URL(fileURLWithPath: request.outputs.screenPath)
		try? FileManager.default.removeItem(at: outputUrl)
		try FileManager.default.createDirectory(
			at: outputUrl.deletingLastPathComponent(),
			withIntermediateDirectories: true
		)

		let writer = try AVAssetWriter(outputURL: outputUrl, fileType: .mp4)
		let settings: [String: Any] = [
			AVVideoCodecKey: AVVideoCodecType.h264,
			AVVideoWidthKey: outputWidth,
			AVVideoHeightKey: outputHeight,
			AVVideoCompressionPropertiesKey: [
				AVVideoAverageBitRateKey: request.video.bitrate ?? 18_000_000,
				AVVideoExpectedSourceFrameRateKey: request.video.fps,
			],
		]
		let input = AVAssetWriterInput(mediaType: .video, outputSettings: settings)
		input.expectsMediaDataInRealTime = true

		guard writer.canAdd(input) else {
			throw HelperError.writerSetupFailed("Unable to add H.264 video input to AVAssetWriter.")
		}

		writer.add(input)
		self.writer = writer
		self.videoInput = input

		// 混音输出单条轨（系统音频 + 麦克风任一开启即建）。bitRate 用系统音频的 192k
		// （混音后信息量比单 mic 高，192k 更稳；若只开 mic 也是合理上限）。
		if request.audio.system.enabled || nativeMicrophoneEnabled {
			mixedAudioInput = try addAudioInput(to: writer, bitRate: 192_000)
		}
	}

	private func finishWriter() async {
		guard let writer else {
			return
		}

		// 混音收尾：先在 sampleQueue 上 flush 两边 deque 剩余样本（避免尾部丢失），再 markAsFinished。
		sampleQueue.sync {
			flushPendingMixSamples()
		}
		videoInput?.markAsFinished()
		mixedAudioInput?.markAsFinished()

		await withCheckedContinuation { continuation in
			writer.finishWriting {
				continuation.resume()
			}
		}

		guard writer.status == .completed else {
			emitError(
				code: "writer-failed",
				message: writer.error.map { "\($0)" } ?? "AVAssetWriter failed with status \(writer.status.rawValue)."
			)
			return
		}

		let outputPath = request.outputs.screenPath

		var durationMs: Int64 = 0
		var fileSize: UInt64 = 0

		// file_size：从文件属性读取
		do {
			let attributes = try FileManager.default.attributesOfItem(atPath: outputPath)
			if let size = attributes[.size] as? NSNumber {
				fileSize = size.uint64Value
			}
		} catch {
			emit([
				"event": "warning",
				"code": "file-size-unavailable",
				"message": "Unable to read output file size: \(error)",
			])
		}

		// duration_ms：从输出文件的 AVAsset 读取
		do {
			let asset = AVAsset(url: URL(fileURLWithPath: outputPath))
			let duration = try await asset.load(.duration)
			if duration.isValid && duration.timescale > 0 {
				durationMs = Int64((Int64(duration.value) * 1000) / Int64(duration.timescale))
			}
		} catch {
			emit([
				"event": "warning",
				"code": "duration-unavailable",
				"message": "Unable to read output file duration: \(error)",
			])
		}

		emit([
			"event": "recording-stopped",
			"screen_path": outputPath,
			"duration_ms": durationMs,
			"file_size": fileSize,
		])
	}

	private func addAudioInput(to writer: AVAssetWriter, bitRate: Int) throws -> AVAssetWriterInput {
		let settings: [String: Any] = [
			AVFormatIDKey: kAudioFormatMPEG4AAC,
			AVSampleRateKey: 48_000,
			AVNumberOfChannelsKey: 2,
			AVEncoderBitRateKey: bitRate,
		]
		let input = AVAssetWriterInput(mediaType: .audio, outputSettings: settings)
		input.expectsMediaDataInRealTime = true

		guard writer.canAdd(input) else {
			throw HelperError.writerSetupFailed("Unable to add AAC audio input to AVAssetWriter.")
		}

		writer.add(input)
		return input
	}

	private func appendAudioSampleBuffer(_ sampleBuffer: CMSampleBuffer, to input: AVAssetWriterInput?) {
		guard didStartWriting else {
			return
		}
		guard let input, input.isReadyForMoreMediaData else {
			return
		}

		input.append(sampleBuffer)
	}

	// ── 实时混音：系统音频 + 麦克风 → 单轨（2026-07-26）──────────────────────
	// 设计见 plan「实时混音：系统音频 + 麦克风 → 单音轨」。
	// 所有函数假定调用方在 sampleQueue（已是串行队列，无需额外锁）。

	/// PTS → 48k sample index（用于近似对齐比较）。
	private func sampleIndex(_ pts: CMTime) -> Int64 {
		// CMTime.value / timescale → 秒，× 48000 → sample index。用 Int64 避免精度丢失。
		guard pts.timescale > 0 else { return 0 }
		return Int64(Double(pts.value) * 48000.0 / Double(pts.timescale))
	}

	/// 把 CMSampleBuffer 转 AVAudioPCMBuffer 入队。首次锁定 mixedAudioFormat。
	/// 超背压上限（pendingMaxSeconds）丢最旧 + emit warning。
	private func enqueueForMix(_ sampleBuffer: CMSampleBuffer, into deque: inout [(CMTime, AVAudioPCMBuffer)]) {
		guard mixedAudioInput != nil else { return }  // 没开音轨（理论上不会进来，防御）

		// 首次：从样本锁定 mixedAudioFormat（期望 48k/stereo/float32 non-interleaved）
		if mixedAudioFormat == nil {
			guard let fmtDesc = CMSampleBufferGetFormatDescription(sampleBuffer) else {
				emit(["event": "warning", "code": "mix-format-unknown", "message": "CMSampleBuffer has no format description, dropping audio sample"])
				return
			}
			let asbdPtr = CMAudioFormatDescriptionGetStreamBasicDescription(fmtDesc)
			guard let asbd = asbdPtr?.pointee else {
				emit(["event": "warning", "code": "mix-format-unknown", "message": "No ASBD in format description"])
				return
			}
			// 锁定 commonFormat（float32 是 SCK 默认）
			let commonFormat: AVAudioCommonFormat = (asbd.mFormatFlags & kAudioFormatFlagIsFloat) != 0
				? .pcmFormatFloat32
				: .pcmFormatInt16
			let format = AVAudioFormat(
				commonFormat: commonFormat,
				sampleRate: asbd.mSampleRate,
				channels: asbd.mChannelsPerFrame,
				interleaved: (asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved) == 0
			)
			mixedAudioFormat = format
		}
		guard let targetFormat = mixedAudioFormat else { return }

		// 转 AVAudioPCMBuffer
		guard let pcmBuffer = pcmBuffer(from: sampleBuffer, to: targetFormat) else {
			emit(["event": "warning", "code": "mix-convert-failed", "message": "Failed to convert CMSampleBuffer to AVAudioPCMBuffer"])
			return
		}

		let pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
		deque.append((pts, pcmBuffer))

		// 背压：超 pendingMaxSeconds 丢最旧
		let maxCount = max(1, Int(pendingMaxSeconds * 10))  // 粗略按 deque 条目数（每条 ~100ms 量级）
		while deque.count > maxCount {
			deque.removeFirst()
			emit(["event": "warning", "code": "mix-overflow", "message": "Audio mix deque overflow, dropping oldest sample"])
		}
	}

	/// CMSampleBuffer → AVAudioPCMBuffer。若原格式与 targetFormat 不一致，用 AVAudioConverter 转换。
	private func pcmBuffer(from sampleBuffer: CMSampleBuffer, to targetFormat: AVAudioFormat) -> AVAudioPCMBuffer? {
		guard let formatDesc = CMSampleBufferGetFormatDescription(sampleBuffer) else { return nil }
		guard let blockBuffer = CMSampleBufferGetDataBuffer(sampleBuffer) else { return nil }
		guard let asbdPtr = CMAudioFormatDescriptionGetStreamBasicDescription(formatDesc) else { return nil }
		let asbd = asbdPtr.pointee

		let srcFormat = AVAudioFormat(
			commonFormat: (asbd.mFormatFlags & kAudioFormatFlagIsFloat) != 0 ? .pcmFormatFloat32 : .pcmFormatInt16,
			sampleRate: asbd.mSampleRate,
			channels: asbd.mChannelsPerFrame,
			interleaved: (asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved) == 0
		)
		guard let srcFormat = srcFormat else { return nil }

		let frameLength = AVAudioFrameCount(CMSampleBufferGetNumSamples(sampleBuffer))
		guard let srcBuffer = AVAudioPCMBuffer(pcmFormat: srcFormat, frameCapacity: frameLength) else { return nil }
		srcBuffer.frameLength = frameLength

		// 拷贝原始字节到 srcBuffer
		let length = CMBlockBufferGetDataLength(blockBuffer)
		guard let dst = srcBuffer.audioBufferList.pointee.mBuffers.mData else { return nil }
		let copyOk = CMBlockBufferCopyDataBytes(blockBuffer, atOffset: 0, dataLength: length, destination: dst)
		guard copyOk == kCMBlockBufferNoErr else { return nil }

		// 格式一致直接返回
		if srcFormat == targetFormat {
			return srcBuffer
		}

		// 格式不一致用 AVAudioConverter 转换（采样率/通道/交错差异）
		guard let converter = AVAudioConverter(from: srcFormat, to: targetFormat) else { return nil }
		let ratio = targetFormat.sampleRate / srcFormat.sampleRate
		let outCapacity = AVAudioFrameCount(Double(frameLength) * ratio) + 1024
		guard let outBuffer = AVAudioPCMBuffer(pcmFormat: targetFormat, frameCapacity: outCapacity) else { return nil }

		var consumed = false
		let inputBlock: AVAudioConverterInputBlock = { _, outStatus in
			if consumed {
				outStatus.pointee = .endOfStream
				return nil
			}
			consumed = true
			outStatus.pointee = .haveData
			return srcBuffer
		}
		var conversionError: NSError?
		let status = converter.convert(to: outBuffer, error: &conversionError, withInputFrom: inputBlock)
		guard status != .error, conversionError == nil else { return nil }
		return outBuffer
	}

	/// 配对 + 混音 + 写入。处理 3 种情况：
	/// 1. 两边 PTS 近似对齐（sampleIndex 相等）→ 混音写出
	/// 2. 某边 PTS 落后超 mixAlignWindowSeconds → 落后边单独 passthrough
	/// 3. 某边空 → 另一边直接 passthrough（处理「只开一边」场景）
	private func drainMixableSamples() {
		guard mixedAudioInput != nil else { return }

		while true {
			let sysHead = pendingSystem.first
			let micHead = pendingMic.first

			// 情况 3：某边空——另一边直接 passthrough（只开一边音频）
			if sysHead == nil && micHead == nil { return }
			if sysHead == nil {
				// mic 单独 passthrough（system 未开 / 已 drain 完）
				if let (pts, buf) = pendingMic.first {
					pendingMic.removeFirst()
					appendMixed(buf: buf, pts: pts)
				}
				continue
			}
			if micHead == nil {
				if let (pts, buf) = pendingSystem.first {
					pendingSystem.removeFirst()
					appendMixed(buf: buf, pts: pts)
				}
				continue
			}

			// 两边都有——比较 PTS
			let sysIdx = sampleIndex(sysHead!.0)
			let micIdx = sampleIndex(micHead!.0)

			if sysIdx == micIdx {
				// 对齐——混音
				let (sysPts, sysBuf) = pendingSystem.removeFirst()
				let (_, micBuf) = pendingMic.removeFirst()
				let mixed = mixPair(sysBuf, micBuf)
				appendMixed(buf: mixed, pts: sysPts)
			} else if sysIdx < micIdx {
				// system 落后——检查对齐窗口
				let diff = micIdx - sysIdx
				if Double(diff) / 48000.0 > mixAlignWindowSeconds {
					// 超 200ms 仍无 mic 配对 → system 单独 passthrough
					let (pts, buf) = pendingSystem.removeFirst()
					appendMixed(buf: buf, pts: pts)
				} else {
					// 窗口内，等 mic 赶上
					return
				}
			} else {
				// mic 落后
				let diff = sysIdx - micIdx
				if Double(diff) / 48000.0 > mixAlignWindowSeconds {
					let (pts, buf) = pendingMic.removeFirst()
					appendMixed(buf: buf, pts: pts)
				} else {
					return
				}
			}
		}
	}

	/// vDSP 混音：两路 float32 求和 + 0.5 衰减防削波。长度不齐短的补零。
	private func mixPair(_ sysBuf: AVAudioPCMBuffer, _ micBuf: AVAudioPCMBuffer) -> AVAudioPCMBuffer {
		guard let format = mixedAudioFormat else { return sysBuf }  // 不会到这
		let outFrames = max(sysBuf.frameLength, micBuf.frameLength)
		guard let outBuf = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: outFrames) else {
			return sysBuf
		}
		outBuf.frameLength = outFrames

		let channelCount = Int(format.channelCount)
		for ch in 0..<channelCount {
			let outCh = outBuf.floatChannelData![ch]
			let sysCh = sysBuf.floatChannelData?[ch]
			let micCh = micBuf.floatChannelData?[ch]
			let sysLen = Int(sysBuf.frameLength)
			let micLen = Int(micBuf.frameLength)

			// 先把 sys 拷到 out（不足补零由 frameLength > sysLen 时天然为零，但显式清零更安全）
			if let sysCh = sysCh {
				for i in 0..<sysLen { outCh[i] = sysCh[i] }
				for i in sysLen..<Int(outFrames) { outCh[i] = 0 }  // 补零
			} else {
				for i in 0..<Int(outFrames) { outCh[i] = 0 }
			}
			// 加 mic
			if let micCh = micCh {
				for i in 0..<micLen { outCh[i] += micCh[i] }
			}
			// 0.5 衰减防削波
			var half: Float = 0.5
			vDSP_vsmul(outCh, 1, &half, outCh, 1, vDSP_Length(outFrames))
		}
		return outBuf
	}

	/// 把 AVAudioPCMBuffer 封回 CMSampleBuffer（保留原 PTS timing）并写入 mixedAudioInput。
	private func appendMixed(buf: AVAudioPCMBuffer, pts: CMTime) {
		guard didStartWriting else { return }
		guard let input = mixedAudioInput, input.isReadyForMoreMediaData else { return }
		guard let format = mixedAudioFormat,
			  let formatDesc = format.formatDescription as CMAudioFormatDescription? else { return }

		let frameLength = buf.frameLength
		// 用 PCM 非压缩 packet（每帧 1 packet）封 CMSampleBuffer
		var sampleSize = UInt32(buf.format.streamDescription.pointee.mBytesPerFrame)
		if sampleSize == 0 { sampleSize = UInt32(buf.format.streamDescription.pointee.mBytesPerPacket) }

		guard let sampleBuffer = makeSampleBuffer(from: buf, formatDesc: formatDesc, pts: pts, sampleSize: sampleSize, frameLength: frameLength) else {
			return
		}
		input.append(sampleBuffer)
	}

	/// 构造 CMSampleBuffer（PCM ready）。
	private func makeSampleBuffer(from buf: AVAudioPCMBuffer, formatDesc: CMAudioFormatDescription, pts: CMTime, sampleSize: UInt32, frameLength: AVAudioFrameCount) -> CMSampleBuffer? {
		// 拷贝 PCM 数据到 CMBlockBuffer
		var blockBuffer: CMBlockBuffer?
		let totalBytes = Int(sampleSize) * Int(frameLength)
		guard let dataPtr = buf.audioBufferList.pointee.mBuffers.mData else { return nil }

		let ok = CMBlockBufferCreateWithMemoryBlock(
			allocator: kCFAllocatorDefault,
			memoryBlock: nil,
			blockLength: totalBytes,
			blockAllocator: kCFAllocatorDefault, customBlockSource: nil,
			offsetToData: 0, dataLength: totalBytes,
			flags: 0, blockBufferOut: &blockBuffer
		)
		guard ok == kCMBlockBufferNoErr, let bb = blockBuffer else { return nil }
		let copyOk = CMBlockBufferCopyDataBytes(bb, atOffset: 0, dataLength: totalBytes, destination: dataPtr)
		guard copyOk == kCMBlockBufferNoErr else { return nil }

		// PCM 用 CMSampleBufferCreateReady（非压缩）——每帧 1 sample，sampleSize 固定。
		var sampleBuffer: CMSampleBuffer?
		var timing = CMSampleTimingInfo(
			duration: CMTime(value: 1, timescale: 48000),
			presentationTimeStamp: pts,
			decodeTimeStamp: .invalid
		)
		var size = Int(sampleSize)
		let status = CMSampleBufferCreateReady(
			allocator: kCFAllocatorDefault,
			dataBuffer: bb,
			formatDescription: formatDesc,
			sampleCount: CMItemCount(frameLength),
			sampleTimingEntryCount: 1,
			sampleTimingArray: &timing,
			sampleSizeEntryCount: 1,
			sampleSizeArray: &size,
			sampleBufferOut: &sampleBuffer
		)
		guard status == noErr else { return nil }
		return sampleBuffer
	}

	/// 收尾时把两边 deque 剩余样本全部 passthrough 写出（避免尾部 ~100-200ms 丢失）。
	/// 在 finishWriter markAsFinished 之前调，必须在 sampleQueue。
	private func flushPendingMixSamples() {
		while let (pts, buf) = pendingSystem.first {
			pendingSystem.removeFirst()
			appendMixed(buf: buf, pts: pts)
		}
		while let (pts, buf) = pendingMic.first {
			pendingMic.removeFirst()
			appendMixed(buf: buf, pts: pts)
		}
	}

	private func currentPauseState() -> (paused: Bool, offset: CMTime) {
		stateQueue.sync {
			(isPaused, totalPausedDuration)
		}
	}

	private func retimedSampleBuffer(_ sampleBuffer: CMSampleBuffer, subtracting offset: CMTime) -> CMSampleBuffer? {
		if !offset.isValid || offset == .zero {
			return sampleBuffer
		}

		let sampleCount = CMSampleBufferGetNumSamples(sampleBuffer)
		if sampleCount <= 0 {
			return sampleBuffer
		}

		var timing = Array(repeating: CMSampleTimingInfo(), count: sampleCount)
		let timingStatus = CMSampleBufferGetSampleTimingInfoArray(
			sampleBuffer,
			entryCount: sampleCount,
			arrayToFill: &timing,
			entriesNeededOut: nil
		)
		if timingStatus != noErr {
			emit([
				"event": "warning",
				"code": "sample-retime-failed",
				"message": "Unable to read sample timing info: \(timingStatus).",
			])
			return sampleBuffer
		}

		for index in timing.indices {
			if timing[index].presentationTimeStamp.isValid {
				timing[index].presentationTimeStamp = CMTimeSubtract(
					timing[index].presentationTimeStamp,
					offset
				)
			}
			if timing[index].decodeTimeStamp.isValid {
				timing[index].decodeTimeStamp = CMTimeSubtract(timing[index].decodeTimeStamp, offset)
			}
		}

		var retimedBuffer: CMSampleBuffer?
		let copyStatus = CMSampleBufferCreateCopyWithNewTiming(
			allocator: kCFAllocatorDefault,
			sampleBuffer: sampleBuffer,
			sampleTimingEntryCount: sampleCount,
			sampleTimingArray: &timing,
			sampleBufferOut: &retimedBuffer
		)
		if copyStatus != noErr {
			emit([
				"event": "warning",
				"code": "sample-retime-failed",
				"message": "Unable to copy sample timing info: \(copyStatus).",
			])
			return sampleBuffer
		}

		return retimedBuffer
	}

	private func isCompleteFrame(_ sampleBuffer: CMSampleBuffer) -> Bool {
		guard let attachments = CMSampleBufferGetSampleAttachmentsArray(
			sampleBuffer,
			createIfNecessary: false
		) as? [[SCStreamFrameInfo: Any]],
			let attachment = attachments.first,
			let statusRawValue = attachment[SCStreamFrameInfo.status] as? Int,
			let status = SCFrameStatus(rawValue: statusRawValue)
		else {
			return true
		}

		return status == .complete
	}

	private func clampCaptureDimension(_ value: Int, fallback: Int) -> Int {
		let requested = max(2, fallback)
		let candidate = value > 0 ? value : requested
		let clamped = min(candidate, requested)
		return max(2, clamped - (clamped % 2))
	}

	private static func scaleFactor(for displayId: CGDirectDisplayID) -> Int {
		guard let mode = CGDisplayCopyDisplayMode(displayId) else {
			return 1
		}

		return max(1, mode.pixelWidth / max(1, mode.width))
	}

	private func supportsNativeMicrophoneCapture(streamConfig: SCStreamConfiguration) -> Bool {
		streamConfig.responds(to: Selector(("setCaptureMicrophone:"))) &&
			streamConfig.responds(to: Selector(("setMicrophoneCaptureDeviceID:"))) &&
			SCStreamOutputType(rawValue: microphoneOutputTypeRawValue) != nil
	}

	private func resolveMicrophoneCaptureDeviceID() -> String? {
		let devices = AVCaptureDevice.devices(for: .audio)

		if let deviceName = request.audio.microphone.deviceName?.trimmingCharacters(in: .whitespacesAndNewlines),
			!deviceName.isEmpty,
			let device = devices.first(where: { $0.localizedName == deviceName })
		{
			return device.uniqueID
		}

		if let deviceId = request.audio.microphone.deviceId?.trimmingCharacters(in: .whitespacesAndNewlines),
			!deviceId.isEmpty,
			devices.contains(where: { $0.uniqueID == deviceId })
		{
			return deviceId
		}

		return nil
	}
}

// MARK: - 显示器名称解析辅助

/// SCDisplay 没有直接暴露 nsScreen/localizedName，需要通过 deviceDescription 中的 NSScreenNumber
/// 反查 NSScreen.localizedName。
func localizedDisplayName(for displayID: CGDirectDisplayID) -> String {
	for screen in NSScreen.screens {
		let number = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber
		if number?.uint32Value == displayID {
			return screen.localizedName
		}
	}
	return "Display \(displayID)"
}

@main
struct OctopusSckHelper {
	static func main() async {
		let args = CommandLine.arguments

		// ── 子命令模式（不进入录制流程）──────────────────────────────
		if args.contains("--check-permission") {
			let granted = CGPreflightScreenCaptureAccess()
			emit([
				"event": "permission-status",
				"granted": granted,
			])
			exit(granted ? 0 : 1)
		}

		if args.contains("--request-permission") {
			let granted = CGRequestScreenCaptureAccess()
			emit([
				"event": "permission-status",
				"granted": granted,
			])
			exit(granted ? 0 : 1)
		}

		if args.contains("--list-displays") {
			listDisplaysAndExit()
			return
		}

		if args.contains("--list-windows") {
			listWindowsAndExit()
			return
		}

		if args.contains("--list-microphones") {
			listMicrophonesAndExit()
			return
		}

		// ── 录制模式（原 openscreen 行为）──────────────────────────
		do {
			guard CommandLine.arguments.count == 2 else {
				throw HelperError.invalidArguments
			}

			guard #available(macOS 13.0, *) else {
				throw HelperError.unsupportedMacOS
			}

			let requestData = Data(CommandLine.arguments[1].utf8)
			let decoder = JSONDecoder()
			// octopus protocol.rs 用 serde 默认 snake_case 序列化（schema_version / hide_system_cursor / screen_path 等），
			// 上游 openscreen TS 端发 camelCase JSON 所以不需要这行；vendor 后必须显式转换。
			decoder.keyDecodingStrategy = .convertFromSnakeCase
			let request = try decoder.decode(RecordingRequest.self, from: requestData)
			let recorder = ScreenCaptureRecorder(request: request)
			let stopTask = Task.detached {
				while let line = readLine() {
					let command = line.trimmingCharacters(in: .whitespacesAndNewlines)
					switch command {
					case "pause":
						recorder.pause()
					case "resume":
						recorder.resume()
					case "stop":
						await recorder.stop()
						exit(0)
					default:
						break
					}
				}
			}

			try await recorder.start()
			await stopTask.value
		} catch let error as HelperError {
			emitError(code: "helper-error", message: error.description)
			exit(1)
		} catch {
			emitError(code: "helper-error", message: "\(error)")
			exit(1)
		}
	}

	// ── 子命令实现 ──────────────────────────────────────────────

	private static func listDisplaysAndExit() {
		guard #available(macOS 13.0, *) else {
			emitError(code: "unsupported-macos", message: "ScreenCaptureKit requires macOS 13+.")
			exit(1)
		}
		Task {
			do {
				let content = try await SCShareableContent.excludingDesktopWindows(
					false,
					onScreenWindowsOnly: true
				)
				let displays = content.displays.map { d -> [String: Any] in
					let displayID = d.displayID
					return [
						"id": displayID,
						"name": localizedDisplayName(for: displayID),
						"width": Int(CGDisplayPixelsWide(displayID)),
						"height": Int(CGDisplayPixelsHigh(displayID)),
						"is_primary": displayID == CGMainDisplayID(),
					]
				}
				emit(["displays": displays])
				exit(0)
			} catch {
				emitError(code: "list-displays-failed", message: "\(error)")
				exit(1)
			}
		}
		// Task 是异步的；main() 是 async 但 listDisplays 是同步调用方。
		// 跑 RunLoop 等 Task 完成（5 秒超时兜底）。正常 Task 完成时 exit(0) 早就触发了。
		RunLoop.main.run(until: Date(timeIntervalSinceNow: 5))
		exit(0)
	}

	private static func listWindowsAndExit() {
		guard #available(macOS 13.0, *) else {
			emitError(code: "unsupported-macos", message: "ScreenCaptureKit requires macOS 13+.")
			exit(1)
		}
		Task {
			do {
				// onScreenWindowsOnly=true 仍会返回大量「控制中心」menu bar item + 后台窗口，
				// 实测 58 个里大部分是 statusBarItem / 系统菜单。需多层过滤：
				let content = try await SCShareableContent.excludingDesktopWindows(
					false,
					onScreenWindowsOnly: true
				)
				let windows = content.windows.filter { w in
					// ① isOnScreen：macOS 窗口服务器判定可见（排除隐藏/最小化）
					guard w.isOnScreen else { return false }
					// ② 尺寸过滤：排除状态栏 item / 菜单（这些 width/height 通常 < 100）
					//    正常应用窗口至少 200x150
					guard w.frame.width >= 200 && w.frame.height >= 150 else { return false }
					// ③ 排除系统 UI app（控制中心、Dock、Window Server、UIEngine）
					if let app = w.owningApplication {
						let bid = app.bundleIdentifier  // String（cargo run 模式可能为空串）
						let systemPrefixes = [
							"com.apple.controlcenter",
							"com.apple.dock",
							"com.apple.WindowManager",
							"com.apple.WindowServer",
							"com.apple.UIEngine",
						]
						for prefix in systemPrefixes {
							if bid.hasPrefix(prefix) { return false }
						}

						// ④ 排除 octopus 的「录制设置」浮窗（设置完即关，用户无法录它）。
						//    必须双重条件：app 是 octopus（bundleId 或 PID 匹配）+ 标题匹配。
						//    单独按标题排除会误伤其他 app 的"录制设置"窗口。
						//    其他 octopus 窗口（语音识别框/compact editor/剪贴板等）不排除——用户可能要录。
						let isOctopus = bid.hasPrefix("com.octopus")
							|| app.processID == getppid()  // cargo run 模式（bid 为空）
						if isOctopus {
							let title = w.title ?? ""
							if title.contains("录制设置") || title.contains("Record Config") || title.contains("octopus-record-config") {
								return false
							}
						}
					}
					return true
				}.map { w -> [String: Any] in
					[
						"id": w.windowID,
						"title": w.title ?? "",
						"app_name": w.owningApplication?.applicationName ?? "",
						"width": Int(w.frame.width),
						"height": Int(w.frame.height),
					]
				}
				emit(["windows": windows])
				exit(0)
			} catch {
				emitError(code: "list-windows-failed", message: "\(error)")
				exit(1)
			}
		}
		RunLoop.main.run(until: Date(timeIntervalSinceNow: 5))
		exit(0)
	}

	private static func listMicrophonesAndExit() {
		// AVCaptureDevice.devices(for: .audio) 同步可用，不需 RunLoop
		let devices = AVCaptureDevice.devices(for: .audio)
		let microphones = devices.map { d -> [String: Any] in
			[
				"id": d.uniqueID,
				"name": d.localizedName,
			]
		}
		emit(["microphones": microphones])
		exit(0)
	}
}
