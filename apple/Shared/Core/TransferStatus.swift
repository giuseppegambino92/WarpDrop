import Foundation
import WarpCore

/// Honest, engine-derived wording for a transfer (shared by the app HUD, the
/// share extensions and the Live Activity).
@MainActor
struct TransferStatus {
    let snapshot: TransferSnapshot
    let peer: String

    var fraction: Double {
        guard snapshot.totalBytes > 0 else { return snapshot.state == .completed ? 1 : 0 }
        return min(1, Double(snapshot.doneBytes) / Double(snapshot.totalBytes))
    }

    var isOutgoing: Bool { snapshot.direction == .outgoing }

    /// Short state line: "Connecting…", "Waiting for Pixel to accept…", "✓ Saved", "✗ …".
    var stateText: String {
        switch snapshot.state {
        case .connecting: L("state_connecting")
        case .waitingForAccept: L("state_waiting", peer)
        case .transferring: L(isOutgoing ? "state_transferring" : "state_receiving")
        case .paused: L("state_paused")
        case .reconnecting: L("state_reconnecting")
        case .verifying: L("state_verifying")
        case .completed: "✓ " + L(isOutgoing ? "state_sent" : "state_saved")
        case .failed: "✗ " + WarpErrorText.message(forCode: snapshot.errorCode, text: snapshot.error, peer: peer)
        case .cancelled: L("state_cancelled")
        }
    }

    /// `412 MB / 1.2 GB`
    var bytesText: String {
        "\(WarpFormat.bytes(snapshot.doneBytes)) / \(WarpFormat.bytes(snapshot.totalBytes))"
    }

    /// Speed only while bytes are actually moving.
    var speedText: String? {
        snapshot.state == .transferring && snapshot.speedBps > 0 ? WarpFormat.speed(snapshot.speedBps) : nil
    }

    var etaText: String? {
        guard snapshot.state == .transferring, let eta = snapshot.etaSecs else { return nil }
        return WarpFormat.eta(eta)
    }

    var pathText: String? {
        snapshot.path.map { L($0.badgeKey) }
    }
}
