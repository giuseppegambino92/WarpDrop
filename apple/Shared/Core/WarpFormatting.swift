import Foundation
import WarpCore

// MARK: - Engine enum helpers

extension DeviceKind {
    /// Same tags the engine uses in TXT records (`k=`).
    var tag: String {
        switch self {
        case .mac: "mac"
        case .iPhone: "iphone"
        case .iPad: "ipad"
        case .android: "android"
        case .windows: "windows"
        case .linux: "linux"
        case .unknown: "unknown"
        }
    }

    init(tag: String) {
        switch tag {
        case "mac": self = .mac
        case "iphone": self = .iPhone
        case "ipad": self = .iPad
        case "android": self = .android
        case "windows": self = .windows
        case "linux": self = .linux
        default: self = .unknown
        }
    }

    /// SF Symbol for the platform glyph.
    var symbolName: String {
        switch self {
        case .mac: "laptopcomputer"
        case .iPhone: "iphone"
        case .iPad: "ipad.landscape"
        case .android: "smartphone"
        case .windows: "pc"
        case .linux: "desktopcomputer"
        case .unknown: "questionmark.square.dashed"
        }
    }
}

extension PathKind {
    var tag: String {
        switch self {
        case .usbCable: "usb"
        case .thunderboltBridge: "thunderbolt"
        case .adbUsb: "adb"
        case .wifiDirect: "wifidirect"
        case .ethernet: "ethernet"
        case .wifi: "wifi"
        case .lan: "lan"
        case .loopback: "loopback"
        case .internetP2p: "p2p"
        case .relay: "relay"
        }
    }

    init?(tag: String) {
        switch tag {
        case "usb": self = .usbCable
        case "thunderbolt": self = .thunderboltBridge
        case "adb": self = .adbUsb
        case "wifidirect": self = .wifiDirect
        case "ethernet": self = .ethernet
        case "wifi": self = .wifi
        case "lan": self = .lan
        case "loopback": self = .loopback
        case "p2p": self = .internetP2p
        case "relay": self = .relay
        default: return nil
        }
    }

    /// Wired links get the cable tint.
    var isCable: Bool {
        switch self {
        case .usbCable, .thunderboltBridge, .adbUsb: true
        default: false
        }
    }

    /// `i18n/strings.json` key of the connection badge.
    var badgeKey: String {
        switch self {
        case .usbCable, .adbUsb: "badge_usb"
        case .thunderboltBridge: "badge_thunderbolt"
        case .wifiDirect: "badge_wifi_direct"
        case .ethernet: "badge_ethernet"
        case .wifi: "badge_wifi"
        case .lan, .loopback: "badge_lan"
        case .internetP2p: "badge_p2p"
        case .relay: "badge_relay"
        }
    }
}

extension TransferState {
    var isTerminal: Bool {
        switch self {
        case .completed, .failed, .cancelled: true
        default: false
        }
    }

    var isActive: Bool { !isTerminal }
}

extension TrustOrigin {
    var tag: String {
        switch self {
        case .iCloud: "icloud"
        case .linked: "linked"
        case .ring: "ring"
        }
    }

    init(tag: String) {
        switch tag {
        case "icloud": self = .iCloud
        case "ring": self = .ring
        default: self = .linked
        }
    }
}

// MARK: - Human-readable numbers

/// Byte / speed / ETA formatting shared by every surface. Decimal units
/// (1 MB = 10⁶ B) like Finder; separators follow the effective app locale.
@MainActor
enum WarpFormat {
    static func bytes(_ value: UInt64) -> String {
        bytes(Double(value))
    }

    static func bytes(_ value: Double) -> String {
        let units = ["B", "KB", "MB", "GB", "TB"]
        var v = value
        var i = 0
        while v >= 1000, i < units.count - 1 {
            v /= 1000
            i += 1
        }
        let digits = (i == 0 || v >= 100) ? 0 : 1
        return "\(number(v, fraction: digits)) \(units[i])"
    }

    /// `412 MB/s`, switching to `Gbit/s` above 1000 MB/s.
    static func speed(_ bytesPerSecond: UInt64) -> String {
        let bps = Double(bytesPerSecond)
        let mbps = bps / 1_000_000
        if mbps >= 1000 {
            let gbit = bps * 8 / 1_000_000_000
            return "\(number(gbit, fraction: gbit >= 100 ? 0 : 1)) Gbit/s"
        }
        if mbps >= 1 {
            return "\(number(mbps, fraction: mbps >= 100 ? 0 : 1)) MB/s"
        }
        let kbps = bps / 1000
        return "\(number(kbps, fraction: 0)) KB/s"
    }

    /// `6 s left`, `2 min left`.
    static func eta(_ seconds: UInt64) -> String {
        if seconds < 60 {
            return L("eta_seconds", "\(max(seconds, 1))")
        }
        let minutes = Int((Double(seconds) / 60).rounded(.up))
        return L("eta_minutes", "\(minutes)")
    }

    /// `7:42` countdown.
    static func countdown(_ seconds: Int) -> String {
        let s = max(seconds, 0)
        return String(format: "%d:%02d", s / 60, s % 60)
    }

    static func percent(_ fraction: Double) -> String {
        "\(Int((fraction * 100).rounded(.down)))%"
    }

    private static func number(_ v: Double, fraction: Int) -> String {
        v.formatted(.number.precision(.fractionLength(fraction)).locale(Localizer.shared.locale))
    }
}

// MARK: - Errors

/// Maps engine error text (`WarpError` display strings, `TransferSnapshot.error`)
/// to the localized `error_*` copy.
@MainActor
enum WarpErrorText {
    static func message(for error: Error, peer: String = "") -> String {
        if let warp = error as? WarpError {
            return message(for: warp, peer: peer)
        }
        return L("error_generic", error.localizedDescription)
    }

    static func message(for error: WarpError, peer: String) -> String {
        switch error {
        case .WrongCode, .InvalidCode: L("error_wrong_code")
        case .CodeNotFound: L("error_code_not_found")
        case .Rejected(let reason): reason == "declined" ? L("error_declined", peerName(peer)) : L("error_generic", reason)
        case .PeerNotFound, .Timeout, .Unreachable: L("error_unreachable", peerName(peer))
        case .Cancelled: L("state_cancelled")
        case .Io(let msg), .Crypto(let msg), .Protocol(let msg), .Config(let msg): L("error_generic", msg)
        case .TransferNotFound: L("error_generic", "transfer not found")
        }
    }

    /// Stable `TransferSnapshot.errorCode` values → `error_*` copy; falls back
    /// to the engine text for unknown codes.
    static func message(forCode code: String?, text: String?, peer: String = "") -> String {
        switch code {
        case "wrong_code": return L("error_wrong_code")
        case "code_not_found": return L("error_code_not_found")
        case "declined": return L("error_declined", peerName(peer))
        case "unreachable", "timeout": return L("error_unreachable", peerName(peer))
        case "cancelled": return L("state_cancelled")
        case .some(let other):
            return L("error_generic", text.flatMap { $0.isEmpty ? nil : $0 } ?? other)
        case nil:
            return text.map { message(forEngineText: $0, peer: peer) } ?? L("state_failed")
        }
    }

    /// Engine messages are the `Display` strings of `WarpError` (see core-rust/libwarp/src/error.rs).
    static func message(forEngineText text: String, peer: String = "") -> String {
        let t = text.lowercased()
        if t.contains("wrong code") || t.contains("invalid code") {
            return L("error_wrong_code")
        }
        if t.contains("no device is currently showing this code") {
            return L("error_code_not_found")
        }
        if t.contains("declined") {
            return L("error_declined", peerName(peer))
        }
        if t.contains("unreachable") || t.contains("device not found") || t.contains("timed out") || t.contains("not nearby") {
            return L("error_unreachable", peerName(peer))
        }
        if t == "cancelled" {
            return L("state_cancelled")
        }
        return L("error_generic", text)
    }

    private static func peerName(_ peer: String) -> String {
        peer.isEmpty ? L("app_name") : peer
    }
}
