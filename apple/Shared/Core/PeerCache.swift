import Foundation
import WarpCore

/// Snapshot of what the app knows about nearby and trusted devices, written to
/// the App Group so the share extensions can send without discovering first.
struct PeerCache: Codable, Sendable {
    struct Peer: Codable, Sendable, Hashable {
        var deviceId: String
        var name: String
        var kind: String
        var trusted: Bool
        var path: String?
        /// TXT record in engine format (`v`, `id`, `n`, `k`, `pk`, `c`).
        var txt: [String: String]
        /// `ip:port` strings (`[fe80::1%en0]:port` for link-local).
        var addresses: [String]
    }

    struct Trusted: Codable, Sendable, Hashable {
        var deviceId: String
        var name: String
        var kind: String
        var publicKeyHex: String
        var origin: String
    }

    var updatedAt: Date
    var peers: [Peer]
    var trusted: [Trusted]

    static let empty = PeerCache(updatedAt: .distantPast, peers: [], trusted: [])

    static func load(from url: URL = AppGroup.peerCacheURL) -> PeerCache {
        guard let data = try? Data(contentsOf: url),
              let cache = try? JSONDecoder().decode(PeerCache.self, from: data)
        else { return .empty }
        return cache
    }

    func save(to url: URL = AppGroup.peerCacheURL) {
        guard let data = try? JSONEncoder().encode(self) else { return }
        try? data.write(to: url, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }

    var advertisements: [PeerAdvert] {
        peers.filter { !$0.addresses.isEmpty }.map { PeerAdvert(txt: $0.txt, addresses: $0.addresses) }
    }
}

extension PeerCache {
    /// Builds the cache from the engine's view plus the TXT records Bonjour
    /// delivered (which carry the public key the engine needs to pin).
    static func make(peers: [PeerInfo], trusted: [TrustedDevice], bonjourTXT: [String: [String: String]]) -> PeerCache {
        let keys = Dictionary(trusted.map { ($0.deviceId, $0.publicKeyHex) }, uniquingKeysWith: { a, _ in a })
        let cachedPeers = peers.map { p -> Peer in
            var txt = bonjourTXT[p.deviceId] ?? [:]
            if txt.isEmpty {
                txt = ["v": "1", "id": p.deviceId, "n": p.name, "k": p.kind.tag, "c": p.codeOpen ? "1" : "0"]
                if let pk = keys[p.deviceId] { txt["pk"] = pk }
            }
            // Tunnels (`adb:PORT`) are process-local and useless to an extension.
            let addrs = p.addresses.filter { !$0.hasPrefix("adb:") }
            return Peer(deviceId: p.deviceId, name: p.name, kind: p.kind.tag, trusted: p.trusted, path: p.bestPath?.tag, txt: txt, addresses: addrs)
        }
        let cachedTrusted = trusted.map {
            Trusted(deviceId: $0.deviceId, name: $0.name, kind: $0.kind.tag, publicKeyHex: $0.publicKeyHex, origin: $0.origin.tag)
        }
        return PeerCache(updatedAt: Date(), peers: cachedPeers, trusted: cachedTrusted)
    }
}
