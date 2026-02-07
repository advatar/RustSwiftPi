// swift-tools-version: 5.9

import PackageDescription

let package = Package(
    name: "PiSwift",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .library(name: "PiSwift", targets: ["PiSwift"])
    ],
    targets: [
        .target(
            name: "PiRustFFI",
            path: "Sources/PiRustFFI",
            publicHeadersPath: "include",
            cSettings: [
                .headerSearchPath("include")
            ],
            linkerSettings: [
                .unsafeFlags(
                    [
                        "-L",
                        "Sources/PiRustFFI/lib",
                        "-lpi_swift_ffi",
                    ],
                    .when(platforms: [.macOS])
                )
            ]
        ),
        .target(
            name: "PiSwift",
            dependencies: ["PiRustFFI"],
            path: "Sources/PiSwift"
        ),
        .testTarget(
            name: "PiSwiftTests",
            dependencies: ["PiSwift"],
            path: "Tests/PiSwiftTests"
        )
    ]
)

