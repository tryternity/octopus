//
//  main.swift
//  OctopusSckHelper
//
//  Vendor 自 openscreen（https://github.com/EtienneLescort/openscreen）
//  原文件：electron/native/screencapturekit/Sources/OpenScreenScreenCaptureKitHelper/main.swift
//  上游 commit：f57e36e25448b5af6c7b1b271066fe5beb9b8a49
//  原作者：Siddharth Vaddem（MIT License，Copyright (c) 2025）
//
//  octopus 修改点（完整声明见同目录 LICENSE）：
//  1. product/target 名 openscreen-screencapturekit-helper → octopus-sck-helper
//  2. 删除 RecordingRequest.webcam / cursor 字段（octopus MVP 不需要）
//  3. 新增 5 个子命令模式：--list-displays / --list-windows / --list-microphones / --check-permission / --request-permission
//  4. emit 事件 schema 对齐 octopus protocol.rs（snake_case，含 timestamp_ms/duration_ms/file_size）
//  5. Task 1.1：拆 library + executable wrapper。本文件只保留 @main 入口，
//     所有逻辑（ScreenCaptureRecorder / 子命令分派）搬到 OctopusSckHelperLib。
//

import Foundation
import OctopusSckHelperLib

@main
struct OctopusSckHelper {
	static func main() async {
		await OctopusSckHelperLibMain.run()
	}
}
