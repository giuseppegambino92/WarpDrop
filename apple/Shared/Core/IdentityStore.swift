import Foundation
import Security

/// Owns the 32-byte Ed25519 identity seed.
///
/// The seed lives in the Keychain (`AfterFirstUnlockThisDeviceOnly`, shared
/// access group so the share extensions act as the same device). Unsigned
/// development builds have no keychain entitlements; they fall back to a
/// 0600 file in the (shared or private) container so identity stays stable.
enum IdentityStore {
    private static let service = "com.warpdrop.identity"
    private static let account = "identity-seed"
    private static let seedLength = 32

    /// Returns the persisted seed, creating one on first launch. `nil` only
    /// when nothing could be persisted; the engine then manages its own seed.
    static func loadOrCreateSeed() -> Data? {
        let stores = availableStores()
        for store in stores {
            if let seed = store.read(), seed.count == seedLength {
                return seed
            }
        }
        // Extensions never mint an identity: they must act as the app's device.
        guard !isExtension else { return nil }
        guard let seed = randomSeed() else { return nil }
        for store in stores where store.write(seed) {
            return seed
        }
        return nil
    }

    /// Read-only lookup used by extensions.
    static func existingSeed() -> Data? {
        for store in availableStores() {
            if let seed = store.read(), seed.count == seedLength { return seed }
        }
        return nil
    }

    private static var isExtension: Bool {
        Bundle.main.bundleURL.pathExtension == "appex"
    }

    private static func randomSeed() -> Data? {
        var bytes = [UInt8](repeating: 0, count: seedLength)
        let status = SecRandomCopyBytes(kSecRandomDefault, seedLength, &bytes)
        return status == errSecSuccess ? Data(bytes) : nil
    }

    private static func availableStores() -> [SeedStore] {
        var stores: [SeedStore] = []
        if let group = AppGroup.keychainGroup {
            stores.append(KeychainSeedStore(accessGroup: group))
        }
        stores.append(KeychainSeedStore(accessGroup: nil))
        stores.append(FileSeedStore(url: AppGroup.containerURL.appendingPathComponent(".identity-seed")))
        return stores
    }

    fileprivate static var baseQuery: [String: Any] {
        var q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        #if os(macOS)
        // Modern (iOS-style) keychain: no ACL prompts, supports access groups.
        q[kSecUseDataProtectionKeychain as String] = true
        #endif
        return q
    }
}

private protocol SeedStore {
    func read() -> Data?
    func write(_ seed: Data) -> Bool
}

private struct KeychainSeedStore: SeedStore {
    let accessGroup: String?

    private var query: [String: Any] {
        var q = IdentityStore.baseQuery
        if let accessGroup { q[kSecAttrAccessGroup as String] = accessGroup }
        return q
    }

    func read() -> Data? {
        var q = query
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess else { return nil }
        return out as? Data
    }

    func write(_ seed: Data) -> Bool {
        var q = query
        q[kSecValueData as String] = seed
        q[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        q[kSecAttrSynchronizable as String] = false
        var status = SecItemAdd(q as CFDictionary, nil)
        if status == errSecDuplicateItem {
            status = SecItemUpdate(query as CFDictionary, [kSecValueData as String: seed] as CFDictionary)
        }
        return status == errSecSuccess
    }
}

private struct FileSeedStore: SeedStore {
    let url: URL

    func read() -> Data? {
        try? Data(contentsOf: url)
    }

    func write(_ seed: Data) -> Bool {
        do {
            try seed.write(to: url, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
            return true
        } catch {
            return false
        }
    }
}
