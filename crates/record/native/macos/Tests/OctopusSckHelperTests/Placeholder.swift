//
//  Placeholder.swift
//  OctopusSckHelperTests
//
//  Task 1.1：占位 sanity test，确认 XCTest testTarget 能跑通。
//  Task 1.2+ 加纯函数单测（RingBuffer / AudioMath / PCMConverter）后保留或删。
//

import XCTest
@testable import OctopusSckHelperLib

final class Placeholder: XCTestCase {
	func testSanity() {
		XCTAssertTrue(true)
	}

	/// 额外确认 lib target 通过 @testable import 可访问 internal 符号
	/// （这是后续 Task 1.2+ 单测能跑的前提，本 task 顺便验证一遍）。
	func testLibraryIsImportable() {
		// HelperError 是 internal，@testable import 应能访问。
		let error = HelperError.invalidArguments
		XCTAssertFalse(error.description.isEmpty)
	}
}
