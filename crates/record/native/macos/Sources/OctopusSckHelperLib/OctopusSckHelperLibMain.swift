//
//  OctopusSckHelperLibMain.swift
//  OctopusSckHelperLib
//
//  原 OctopusSckHelper/main.swift 的 @main struct OctopusSckHelper 的 main() 函数体搬到这里，
//  作为 library 的 public 入口（Task 1.1）。
//
//  注意：lib 里 **不能** 用 @main（每个产品只能有一个 @main，由 executable wrapper 持有）。
//  这里是普通 public enum，public static func run() 由 exec wrapper 调用。
//

import AVFoundation
import AppKit
import CoreGraphics
import Foundation
import ScreenCaptureKit

public enum OctopusSckHelperLibMain {
	/// executable wrapper 委托入口。等价于原 `@main struct OctopusSckHelper.main()` 的函数体。
	public static func run() async {
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
		// macOS 14+：用 DiscoverySession 替代 10.15 起废弃的 AVCaptureDevice.devices(for:)，
		// deviceTypes 同时覆盖内置 + 外接（USB）麦克风，与原 devices(for: .audio) 语义一致。
		// macOS 13：.external deviceType 不可用，回退到废弃 API（仍能枚举所有音频设备）。
		let devices: [AVCaptureDevice]
		if #available(macOS 14.0, *) {
			let session = AVCaptureDevice.DiscoverySession(deviceTypes: [.builtInMicrophone, .external], mediaType: .audio, position: .unspecified)
			devices = session.devices
		} else {
			devices = AVCaptureDevice.devices(for: .audio)
		}
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
