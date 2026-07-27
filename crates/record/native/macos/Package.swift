// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "OctopusSckHelper",
    platforms: [.macOS(.v13)],
    products: [
        .executable(name: "octopus-sck-helper", targets: ["OctopusSckHelper"])
    ],
    targets: [
        // Library：ScreenCaptureRecorder + 子命令分派 + 类型。
        // 单测依赖此 target（SwiftPM 不允许 testTarget 直接依赖 executableTarget）。
        .target(
            name: "OctopusSckHelperLib",
            path: "Sources/OctopusSckHelperLib"
        ),
        // Executable wrapper：只持有 @main，委托给 OctopusSckHelperLibMain.run()。
        .executableTarget(
            name: "OctopusSckHelper",
            dependencies: ["OctopusSckHelperLib"],
            path: "Sources/OctopusSckHelper"
        ),
        // TestTarget：Task 1.2+ 的纯函数单测挂这里。
        .testTarget(
            name: "OctopusSckHelperTests",
            dependencies: ["OctopusSckHelperLib"],
            path: "Tests/OctopusSckHelperTests"
        ),
    ]
)
