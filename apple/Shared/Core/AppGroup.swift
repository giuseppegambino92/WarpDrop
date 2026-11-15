import Foundation
import Security

/// Locations shared between the app and its extensions.
///
/// The App Group / Keychain group identifiers come from Info.plist
/// (`WarpAppGroup`, `WarpKeychainGroup`) so the team prefix can be expanded by
/// the build system. Every accessor degrades gracefully when the binary is not
/// signed with the matching entitlements (local `CODE_SIGNING_ALLOWED=NO`
/// builds): it then falls back to a per-process location instead of failing or
/// triggering macOS "access data from other apps" prompts.
enum AppGroup {
    static let identifier: String = {
        (Bundle.main.object(forInfoDictionaryKey: "WarpAppGroup") as? String).flatMap(nonEmpty) ?? "group.com.warpdrop.shared"
    }()

    static let keychainGroup: String? = {
        (Bundle.main.object(forInfoDictionaryKey: "WarpKeychainGroup") as? String).flatMap(nonEmpty)
    }()

    /// True when the running binary is entitled to the shared container.
    static let isEntitled: Bool = {
        #if os(macOS)
        guard let task = SecTaskCreateFromSelf(nil),
              let value = SecTaskCopyValueForEntitlement(task, "com.apple.security.application-groups" as CFString, nil)
        else { return false }
        return (value as? [String])?.contains(identifier) ?? false
        #else
        return FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: identifier) != nil
        #endif
    }()

    /// Root directory shared with the extensions (or a private fallback).
    static let containerURL: URL = {
        let fm = FileManager.default
        if isEntitled, let url = fm.containerURL(forSecurityApplicationGroupIdentifier: identifier) {
            return url
        }
        let support = (try? fm.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true))
            ?? fm.temporaryDirectory
        #if os(macOS)
        let url = support.appendingPathComponent("WarpDrop", isDirectory: true)
        #else
        let url = support
        #endif
        try? fm.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }()

    /// Settings visible to the app and its extensions. (Computed: UserDefaults
    /// is not `Sendable`; suite instances are cached by Foundation anyway.)
    static var defaults: UserDefaults {
        if isEntitled, let suite = UserDefaults(suiteName: identifier) {
            return suite
        }
        return .standard
    }

    /// Engine state (trust ring, resume state) of the main app.
    static var appStateDirectory: URL { directory("State") }

    /// Engine state of the share extensions (kept apart from the app's so two
    /// nodes never write the same trust-ring file).
    static var extensionStateDirectory: URL { directory("ExtensionState") }

    /// Cache of nearby peers + trust ring the app publishes for the extensions.
    static var peerCacheURL: URL { containerURL.appendingPathComponent("peer-cache.json") }

    private static func directory(_ name: String) -> URL {
        let url = containerURL.appendingPathComponent(name, isDirectory: true)
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    private static func nonEmpty(_ s: String) -> String? {
        let t = s.trimmingCharacters(in: .whitespaces)
        // An unexpanded `$(AppIdentifierPrefix)` collapses to "." prefixed ids in unsigned builds.
        let cleaned = t.hasPrefix(".") ? String(t.dropFirst()) : t
        return cleaned.isEmpty ? nil : cleaned
    }
}

/// Keys of `AppGroup.defaults`.
enum SharedKeys {
    static let language = "warp.language"
    static let relayURL = "warp.relayURL"
    static let deviceName = "warp.deviceName"
}
