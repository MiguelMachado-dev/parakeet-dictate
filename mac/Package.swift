// swift-tools-version:6.0
import PackageDescription

let package = Package(
    name: "ParakeetDictate",
    platforms: [.macOS(.v14)],
    dependencies: [
        .package(url: "https://github.com/FluidInference/FluidAudio.git", exact: "0.17.3"),
    ],
    targets: [
        // Everything that runs without a microphone, a keyboard, or a window, so `swift test` covers it.
        .target(name: "DictateCore"),
        .executableTarget(
            name: "ParakeetDictate",
            dependencies: ["DictateCore", .product(name: "FluidAudio", package: "FluidAudio")]
        ),
        .testTarget(name: "DictateCoreTests", dependencies: ["DictateCore"]),
    ],
    swiftLanguageModes: [.v5]
)
