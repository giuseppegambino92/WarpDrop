import Foundation
import Observation
#if os(macOS)
import ServiceManagement
#endif

/// User preferences. Values the extensions need (relay, language, name) live
/// in the App Group defaults; the rest in the app's own defaults.
@Observable
@MainActor
final class SettingsStore {
    private let shared = AppGroup.defaults
    private let local = UserDefaults.standard

    var deviceName: String {
        didSet { shared.set(deviceName, forKey: SharedKeys.deviceName) }
    }

    var relayURL: String {
        didSet { shared.set(relayURL, forKey: SharedKeys.relayURL) }
    }

    /// Folder for received files.
    var downloadDirectory: URL {
        didSet { local.set(downloadDirectory.path, forKey: Keys.downloadDirectory) }
    }

    /// Public keys removed by the user that iCloud sync must not re-add.
    private(set) var ignoredICloudKeys: Set<String> {
        didSet { local.set(Array(ignoredICloudKeys), forKey: Keys.ignoredICloudKeys) }
    }

    #if os(macOS)
    var launchAtLogin: Bool {
        get {
            access(keyPath: \.launchAtLogin)
            return SMAppService.mainApp.status == .enabled
        }
        set {
            withMutation(keyPath: \.launchAtLogin) {
                do {
                    if newValue { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
                } catch {
                    // Unsigned / non-installed builds cannot register login items.
                }
            }
        }
    }
    #endif

    private enum Keys {
        static let downloadDirectory = "warp.downloadDirectory"
        static let ignoredICloudKeys = "warp.ignoredICloudKeys"
    }

    init() {
        let name = AppGroup.defaults.string(forKey: SharedKeys.deviceName)
        deviceName = (name?.isEmpty == false ? name : nil) ?? PlatformServices.defaultDeviceName
        relayURL = AppGroup.defaults.string(forKey: SharedKeys.relayURL) ?? ""
        #if os(macOS)
        let stored = UserDefaults.standard.string(forKey: Keys.downloadDirectory)
        downloadDirectory = stored.map { URL(fileURLWithPath: $0, isDirectory: true) } ?? PlatformServices.defaultDownloadDirectory
        #else
        // The sandbox path changes between installs: always derive it.
        downloadDirectory = PlatformServices.defaultDownloadDirectory
        #endif
        ignoredICloudKeys = Set(UserDefaults.standard.stringArray(forKey: Keys.ignoredICloudKeys) ?? [])
        // Make sure the extensions see the effective name even before the first edit.
        AppGroup.defaults.set(deviceName, forKey: SharedKeys.deviceName)
    }

    var relayURLOrNil: String? {
        let trimmed = relayURL.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    func ignoreICloudKey(_ publicKeyHex: String) {
        ignoredICloudKeys.insert(publicKeyHex.lowercased())
    }

    func unignoreICloudKey(_ publicKeyHex: String) {
        ignoredICloudKeys.remove(publicKeyHex.lowercased())
    }
}
