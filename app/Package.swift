// swift-tools-version:5.9
// The Arboria Mac app's thin Swift shell (ADR 0004). Builds with Command Line Tools only:
// `app/build.sh` runs `swift build` and wraps the binary into Arboria.app.
import PackageDescription

let package = Package(
    name: "Arboria",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(name: "Arboria", path: "Sources/Arboria",
                          linkerSettings: [.linkedFramework("AppKit"), .linkedFramework("WebKit")]),
    ]
)
