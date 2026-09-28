// swift-tools-version: 6.0
// The Rust core of the app (crates/mobile), built by scripts/build-ios-core.sh
// into an xcframework plus its generated Swift bindings.
import PackageDescription

let package = Package(
    name: "NiumaTermCore",
    platforms: [.iOS(.v18)],
    products: [
        .library(name: "NiumaTermCore", targets: ["NiumaTermCore"]),
    ],
    targets: [
        .binaryTarget(name: "NiumaTermCoreFFI", path: "NiumaTermCoreFFI.xcframework"),
        .target(
            name: "NiumaTermCore",
            dependencies: ["NiumaTermCoreFFI"],
            // The generated bindings predate Swift 6 strict concurrency.
            swiftSettings: [.swiftLanguageMode(.v5)],
            // A Rust static library records no link dependencies of its own;
            // these are what `--print native-static-libs` reports for it.
            linkerSettings: [
                .linkedFramework("Security"),
                .linkedFramework("CoreFoundation"),
                .linkedLibrary("util"),
                .linkedLibrary("iconv"),
            ]
        ),
    ]
)
