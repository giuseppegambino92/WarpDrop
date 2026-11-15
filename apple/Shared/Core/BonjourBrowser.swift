import Foundation
import Network
import WarpCore

/// Browses `_warpdrop._udp` with Network.framework (including AWDL / Wi-Fi
/// peer-to-peer), resolves every result to `ip:port` strings and reports one
/// `PeerAdvert` per device to the engine.
@MainActor
final class BonjourBrowser {
    static let serviceType = "_warpdrop._udp"

    /// A device appeared or its addresses / TXT changed.
    var onPeer: ((String, PeerAdvert) -> Void)?
    /// The last Bonjour result for a device disappeared.
    var onLost: ((String) -> Void)?

    private let ownDeviceID: String
    private var browser: NWBrowser?
    /// Every Bonjour result (one per service instance × interface set).
    private var entries: [NWEndpoint: Entry] = [:]
    private var resolvers: [NWEndpoint: [NWConnection]] = [:]
    private var reported: [String: PeerAdvert] = [:]

    private struct Entry {
        var deviceID: String
        var txt: [String: String]
        var addresses: Set<String>
        var interfaces: [NWInterface]
    }

    init(ownDeviceID: String) {
        self.ownDeviceID = ownDeviceID
    }

    /// TXT records by device id (used for the extension peer cache).
    var txtByDevice: [String: [String: String]] {
        var out: [String: [String: String]] = [:]
        for e in entries.values { out[e.deviceID] = e.txt }
        return out
    }

    func start() {
        guard browser == nil else { return }
        let params = NWParameters()
        params.includePeerToPeer = true
        let browser = NWBrowser(for: .bonjourWithTXTRecord(type: Self.serviceType, domain: nil), using: params)
        browser.browseResultsChangedHandler = { [weak self] _, changes in
            let items = changes.map(Change.init)
            MainActor.assumeIsolated { self?.apply(items) }
        }
        browser.stateUpdateHandler = { [weak self] state in
            guard case .failed = state else { return }
            MainActor.assumeIsolated { self?.restart() }
        }
        browser.start(queue: .main)
        self.browser = browser
    }

    func stop() {
        browser?.cancel()
        browser = nil
        for conns in resolvers.values { conns.forEach { $0.cancel() } }
        resolvers.removeAll()
        for id in Set(entries.values.map(\.deviceID)) { onLost?(id) }
        entries.removeAll()
        reported.removeAll()
    }

    /// Re-resolves every known service (addresses may have changed, e.g.
    /// after a network switch or when the app returns to the foreground).
    func refresh() {
        for (endpoint, entry) in entries {
            entries[endpoint]?.addresses.removeAll()
            resolve(endpoint, interfaces: entry.interfaces)
        }
    }

    private func restart() {
        stop()
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) { [weak self] in
            MainActor.assumeIsolated { self?.start() }
        }
    }

    // MARK: - Results

    /// Sendable digest of an `NWBrowser.Result.Change`.
    private struct Change: @unchecked Sendable {
        enum Kind { case added, removed, changed, ignored }
        let kind: Kind
        let endpoint: NWEndpoint
        let txt: [String: String]
        let interfaces: [NWInterface]

        init(_ change: NWBrowser.Result.Change) {
            switch change {
            case .added(let r):
                self.init(kind: .added, result: r)
            case .removed(let r):
                self.init(kind: .removed, result: r)
            case .changed(_, let new, _):
                self.init(kind: .changed, result: new)
            case .identical:
                self.init(kind: .ignored, endpoint: nil)
            @unknown default:
                self.init(kind: .ignored, endpoint: nil)
            }
        }

        private init(kind: Kind, result: NWBrowser.Result) {
            self.kind = kind
            self.endpoint = result.endpoint
            if case .bonjour(let record) = result.metadata {
                self.txt = record.dictionary
            } else {
                self.txt = [:]
            }
            self.interfaces = result.interfaces
        }

        private init(kind: Kind, endpoint: NWEndpoint?) {
            self.kind = kind
            self.endpoint = endpoint ?? .hostPort(host: "0.0.0.0", port: 0)
            self.txt = [:]
            self.interfaces = []
        }
    }

    private func apply(_ changes: [Change]) {
        for change in changes {
            switch change.kind {
            case .ignored:
                continue
            case .removed:
                remove(change.endpoint)
            case .added, .changed:
                guard let id = change.txt["id"], !id.isEmpty, id != ownDeviceID else { continue }
                let previous = entries[change.endpoint]
                entries[change.endpoint] = Entry(deviceID: id, txt: change.txt, addresses: previous?.addresses ?? [], interfaces: change.interfaces)
                if previous?.deviceID != id || previous?.txt != change.txt || previous?.addresses.isEmpty != false {
                    resolve(change.endpoint, interfaces: change.interfaces)
                }
                report(id)
            }
        }
    }

    private func remove(_ endpoint: NWEndpoint) {
        resolvers.removeValue(forKey: endpoint)?.forEach { $0.cancel() }
        guard let entry = entries.removeValue(forKey: endpoint) else { return }
        if entries.values.contains(where: { $0.deviceID == entry.deviceID }) {
            report(entry.deviceID)
        } else {
            reported.removeValue(forKey: entry.deviceID)
            onLost?(entry.deviceID)
        }
    }

    /// Merges every result of a device into one advert and forwards it when it changed.
    private func report(_ deviceID: String) {
        let mine = entries.values.filter { $0.deviceID == deviceID }
        guard let txt = mine.first?.txt else { return }
        let addresses = Set(mine.flatMap(\.addresses)).sorted { lhs, rhs in
            // IPv4 first, then routable IPv6, then link-local.
            rank(lhs) == rank(rhs) ? lhs < rhs : rank(lhs) < rank(rhs)
        }
        guard !addresses.isEmpty else { return }
        let advert = PeerAdvert(txt: txt, addresses: addresses)
        if reported[deviceID] != advert {
            reported[deviceID] = advert
            onPeer?(deviceID, advert)
        }
    }

    private func rank(_ address: String) -> Int {
        if !address.hasPrefix("[") { return 0 }
        return address.contains("%") ? 2 : 1
    }

    // MARK: - Resolution

    /// Opens a throw-away UDP "connection" per interface: once `.ready`, its
    /// path's remote endpoint is the resolved address. Nothing is sent.
    private func resolve(_ endpoint: NWEndpoint, interfaces: [NWInterface]) {
        resolvers.removeValue(forKey: endpoint)?.forEach { $0.cancel() }
        let targets: [NWInterface?] = interfaces.isEmpty ? [nil] : interfaces.map { Optional($0) }
        var conns: [NWConnection] = []
        for iface in targets {
            let params = NWParameters.udp
            params.includePeerToPeer = true
            if let iface { params.requiredInterface = iface }
            let conn = NWConnection(to: endpoint, using: params)
            let ifaceName = iface?.name
            conn.stateUpdateHandler = { [weak self, weak conn] state in
                switch state {
                case .ready:
                    let resolved = conn?.currentPath?.remoteEndpoint.flatMap { Self.format($0, fallbackInterface: ifaceName) }
                    conn?.cancel()
                    MainActor.assumeIsolated { self?.resolved(endpoint, address: resolved) }
                case .failed, .waiting:
                    conn?.cancel()
                default:
                    break
                }
            }
            conn.start(queue: .main)
            conns.append(conn)
            DispatchQueue.main.asyncAfter(deadline: .now() + 8) { [weak conn] in
                conn?.cancel()
            }
        }
        resolvers[endpoint] = conns
    }

    private func resolved(_ endpoint: NWEndpoint, address: String?) {
        guard let address, var entry = entries[endpoint] else { return }
        entry.addresses.insert(address)
        entries[endpoint] = entry
        report(entry.deviceID)
    }

    /// `192.168.1.4:5000`, `[2a02::1]:5000`, `[fe80::1%awdl0]:5000`.
    nonisolated static func format(_ endpoint: NWEndpoint, fallbackInterface: String?) -> String? {
        guard case .hostPort(let host, let port) = endpoint else { return nil }
        switch host {
        case .ipv4(let a):
            return "\(ntop(a.rawValue, family: AF_INET) ?? "\(a)"):\(port.rawValue)"
        case .ipv6(let a):
            if let v4 = a.asIPv4 {
                return "\(ntop(v4.rawValue, family: AF_INET) ?? "\(v4)"):\(port.rawValue)"
            }
            guard let text = ntop(a.rawValue, family: AF_INET6) else { return nil }
            if a.isLinkLocal, let scope = a.interface?.name ?? fallbackInterface {
                return "[\(text)%\(scope)]:\(port.rawValue)"
            }
            return "[\(text)]:\(port.rawValue)"
        case .name:
            return nil
        @unknown default:
            return nil
        }
    }

    nonisolated private static func ntop(_ raw: Data, family: Int32) -> String? {
        var buffer = [CChar](repeating: 0, count: Int(INET6_ADDRSTRLEN))
        let ok = raw.withUnsafeBytes { bytes -> Bool in
            guard let base = bytes.baseAddress else { return false }
            return inet_ntop(family, base, &buffer, socklen_t(buffer.count)) != nil
        }
        guard ok else { return nil }
        return String(decoding: buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }, as: UTF8.self)
    }
}
