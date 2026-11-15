// swift-tools-version: 5.10
// Local package wrapping the Rust core (WarpCoreFFI.xcframework) and its
// UniFFI-generated Swift bindings. The bindings compile in Swift 5 language
// mode; the apps themselves are Swift 6.
import PackageDescription

let package = Package(
    name: "WarpCore",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [.library(name: "WarpCore", targets: ["WarpCore"])],
    targets: [
        .binaryTarget(name: "warpFFI", path: "Frameworks/WarpCoreFFI.xcframework"),
        .target(
            name: "WarpCore",
            dependencies: ["warpFFI"],
            path: "Sources/WarpCore",
            linkerSettings: [.linkedFramework("SystemConfiguration"), .linkedFramework("Security")]
        ),
    ]
)
