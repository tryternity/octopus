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
	private var systemAudioInput: AVAssetWriterInput?
	private var microphoneAudioInput: AVAssetWriterInput?
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
			appendAudioSampleBuffer(sampleBuffer, to: systemAudioInput)
			return
		}

		if type.rawValue == microphoneOutputTypeRawValue {
			appendAudioSampleBuffer(sampleBuffer, to: microphoneAudioInput)
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

		if request.audio.system.enabled {
			systemAudioInput = try addAudioInput(to: writer, bitRate: 192_000)
		}
		if nativeMicrophoneEnabled {
			microphoneAudioInput = try addAudioInput(to: writer, bitRate: 128_000)
		}
	}

	private func finishWriter() async {
		guard let writer else {
			return
		}

		videoInput?.markAsFinished()
		systemAudioInput?.markAsFinished()
		microphoneAudioInput?.markAsFinished()

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
					//    这些不是用户想录的「应用窗口」
					if let bundleId = w.owningApplication?.bundleIdentifier {
						let systemPrefixes = [
							"com.apple.controlcenter",
							"com.apple.dock",
							"com.apple.WindowManager",
							"com.apple.WindowServer",
							"com.apple.UIEngine",
						]
						for prefix in systemPrefixes {
							if bundleId.hasPrefix(prefix) { return false }
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
