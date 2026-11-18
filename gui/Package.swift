// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "CrtGui",
    platforms: [
        .macOS(.v14),
    ],
    products: [
        .library(name: "CrtGui", type: .static, targets: ["CrtGui"]),
    ],
    targets: [
        .target(name: "CrtGui"),
    ]
)
