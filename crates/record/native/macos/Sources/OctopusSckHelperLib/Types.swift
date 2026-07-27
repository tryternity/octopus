//
//  Types.swift
//  OctopusSckHelperLib
//
//  由 OctopusSckHelper/main.swift 拆出（Task 1.1：拆 library + executable wrapper）。
//  原文件 vendored 自 openscreen（见同目录 LICENSE / ../main.swift 头注释）。
//
//  这层放：纯数据类型 + HelperError。
//  都是 internal（@testable import OctopusSckHelperLib 可访问，Task 1.2+ 单测用）。
//

import Foundation

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

// MARK: - stdout emit helpers

/// 向 stdout 输出一行 JSON。lib 内部使用，exec wrapper 不直接调用。
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
