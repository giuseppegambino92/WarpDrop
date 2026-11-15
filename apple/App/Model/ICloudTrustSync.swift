import Foundation
import WarpCore

/// Same-account auto-trust through iCloud key-value storage.
///
/// Every device publishes `device.<deviceId> = {publicKeyHex, name, kind}`;
/// every other entry is added to the trust ring with origin `.iCloud`.
/// Without the ubiquity-kvstore entitlement the store is simply local and
/// nothing syncs — no crash, no prompt.
@MainActor
final class ICloudTrustSync {
    private let store = NSUbiquitousKeyValueStore.default
    private var observer: NSObjectProtocol?
    private var identity: DeviceIdentity?
    private var onDevice: ((_ publicKeyHex: String, _ name: String, _ kind: DeviceKind) -> Void)?

    private static let prefix = "device."

    func start(identity: DeviceIdentity, onDevice: @escaping (_ publicKeyHex: String, _ name: String, _ kind: DeviceKind) -> Void) {
        self.identity = identity
        self.onDevice = onDevice
        if observer == nil {
            observer = NotificationCenter.default.addObserver(
                forName: NSUbiquitousKeyValueStore.didChangeExternallyNotification,
                object: store,
                queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.importDevices() }
            }
        }
        store.synchronize()
        publish(identity)
        importDevices()
    }

    /// (Re)publishes this device, e.g. after a rename.
    func publish(_ identity: DeviceIdentity) {
        self.identity = identity
        let record: [String: String] = [
            "publicKeyHex": identity.publicKeyHex,
            "name": identity.name,
            "kind": identity.kind.tag,
        ]
        let key = Self.prefix + identity.deviceId
        if (store.dictionary(forKey: key) as? [String: String]) != record {
            store.set(record, forKey: key)
            store.synchronize()
        }
    }

    private func importDevices() {
        guard let identity, let onDevice else { return }
        for (key, value) in store.dictionaryRepresentation where key.hasPrefix(Self.prefix) {
            guard key != Self.prefix + identity.deviceId,
                  let record = value as? [String: String],
                  let pk = record["publicKeyHex"], pk.lowercased() != identity.publicKeyHex.lowercased()
            else { continue }
            onDevice(pk, record["name"] ?? "", DeviceKind(tag: record["kind"] ?? ""))
        }
    }
}
