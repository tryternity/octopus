//
//  ScreenCaptureRecorder.swift
//  OctopusSckHelperLib
//
//  由 OctopusSckHelper/main.swift 拆出（Task 1.1）。
//  ScreenCaptureRecorder 整体放 library，便于后续 Phase 2 集成改动可被单测覆盖。
//  internal 访问级别（@testable import 可访问）。
//

import AVFoundation
import AppKit
import CoreGraphics
import CoreMedia
import Foundation
import ScreenCaptureKit

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

		// ⚠️ 麦克风轨先 add（变 track 1，播放器默认播放）——原顺序 system 先 add 导致
		// 播放器默认放 system audio track（常为静音），用户听不到麦克风。
		// 实时混音方案（commit 6cb6fe90 ~ f9741968）经多轮调试仍无法稳定输出，
		// 暂回退双轨 + 调整顺序，混音作为后续 P2 任务重新设计。
		if nativeMicrophoneEnabled {
			microphoneAudioInput = try addAudioInput(to: writer, bitRate: 128_000)
		}
		if request.audio.system.enabled {
			systemAudioInput = try addAudioInput(to: writer, bitRate: 192_000)
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
		// macOS 14+：用 DiscoverySession 替代 10.15 起废弃的 AVCaptureDevice.devices(for:)，
		// deviceTypes 覆盖内置 + 外接麦克风，与原 devices(for: .audio) 语义一致。
		// macOS 13：.external 不可用，回退到废弃 API（仍能枚举所有音频设备）。
		let devices: [AVCaptureDevice]
		if #available(macOS 14.0, *) {
			let session = AVCaptureDevice.DiscoverySession(deviceTypes: [.builtInMicrophone, .external], mediaType: .audio, position: .unspecified)
			devices = session.devices
		} else {
			devices = AVCaptureDevice.devices(for: .audio)
		}

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
