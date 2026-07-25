// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "OctopusSckHelper",
    platforms: [.macOS(.v13)],
    products: [
        .executable(name: "octopus-sck-helper", targets: ["OctopusSckHelper"])
    ],
    targets: [
        .executableTarget(name: "OctopusSckHelper", path: "Sources/OctopusSckHelper")
    ]
)
