#if os(iOS)
import ActivityKit
import Foundation

/// Live Activity / Dynamic Island payload for one transfer. Shared by the app
/// (which starts/updates/ends it) and the widget extension (which renders it).
/// Strings arrive pre-localized so the in-app language override applies.
struct TransferActivityAttributes: ActivityAttributes {
    struct ContentState: Codable, Hashable {
        enum Phase: String, Codable, Hashable {
            case active, paused, completed, failed
        }

        var phase: Phase
        /// 0…1, straight from the engine.
        var fraction: Double
        /// "412 MB / 1.2 GB"
        var bytesText: String
        /// "412 MB/s" (empty when not moving)
        var speedText: String
        /// "6 s left" / "Waiting for Pixel to accept…" / "✓ Saved"
        var detailText: String
        /// "⚡ USB Cable", "Wi-Fi"… (empty when unknown)
        var pathText: String
        var pathIsCable: Bool
    }

    var transferID: String
    var title: String
    var peerName: String
    var isOutgoing: Bool
}
#endif
