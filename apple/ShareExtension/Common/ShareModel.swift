import Foundation
import Observation
import WarpCore
#if os(iOS)
import UIKit
#endif

/// Drives the share extension: a lightweight `WarpNode` acting as the same
/// device (identity seed from the shared Keychain), primed with the peers and
/// trust ring the main app cached in the App Group, plus a live Bonjour browse.
@Observable
@MainActor
final class ShareModel {
    enum Phase: Equatable {
        case loading
        case choosing
        case sending
        case done
        case failed(String)
    }

    struct Device: Identifiable, Hashable {
        let id: String
        var name: String
        var kind: DeviceKind
        var path: PathKind?
        var trusted: Bool
    }

    private(set) var phase: Phase = .loading
    private(set) var payload: SendPayload?
    private(set) var devices: [Device] = []
    private(set) var transfer: TransferSnapshot?
    private(set) var targetName = ""

    @ObservationIgnored private var node: WarpNode?
    @ObservationIgnored private var browser: BonjourBrowser?
    @ObservationIgnored private var transferID: String?
    @ObservationIgnored private var cachedPeers: [String: PeerCache.Peer] = [:]
    @ObservationIgnored private var finish: ((Bool) -> Void)?
    @ObservationIgnored private var scoped: [URL] = []

    /// `finish(true)` completes the request, `finish(false)` cancels it.
    func start(providers: [NSItemProvider], finish: @escaping (Bool) -> Void) {
        self.finish = finish
        Localizer.shared.reload()
        startNode()
        Task {
            let payload = await ItemLoader.payload(from: providers)
            self.payload = payload
            phase = payload == nil ? .failed(L("share_nothing")) : .choosing
        }
    }

    private func startNode() {
        let cache = PeerCache.load()
        cachedPeers = Dictionary(cache.peers.map { ($0.deviceId, $0) }, uniquingKeysWith: { a, _ in a })
        let inbox = FileManager.default.temporaryDirectory.appendingPathComponent("ShareInbox", isDirectory: true)
        let name = AppGroup.defaults.string(forKey: SharedKeys.deviceName) ?? L("app_name")
        var cfg = defaultNodeConfig(deviceName: name, stateDir: AppGroup.extensionStateDirectory.path, downloadDir: inbox.path)
        #if os(macOS)
        cfg.deviceKind = .mac
        #else
        cfg.deviceKind = UIDevice.current.userInterfaceIdiom == .pad ? .iPad : .iPhone
        #endif
        cfg.identitySeed = IdentityStore.existingSeed()
        cfg.port = 0
        cfg.enableMdns = false
        cfg.enableBeacon = false
        cfg.enableAdb = false
        cfg.parallelStreams = 2
        cfg.relayUrl = AppGroup.defaults.string(forKey: SharedKeys.relayURL).flatMap { $0.isEmpty ? nil : $0 }
        do {
            let node = try WarpNode(config: cfg, listener: EventBridge { [weak self] in self?.handle($0) })
            self.node = node
            for t in cache.trusted {
                try? node.trustDevice(publicKeyHex: t.publicKeyHex, name: t.name, kind: DeviceKind(tag: t.kind), origin: TrustOrigin(tag: t.origin))
            }
            for advert in cache.advertisements {
                node.injectPeer(advert: advert)
            }
            refreshDevices()
            let browser = BonjourBrowser(ownDeviceID: node.identity().deviceId)
            browser.onPeer = { [weak self] _, advert in self?.node?.injectPeer(advert: advert) }
            browser.start()
            self.browser = browser
        } catch {
            phase = .failed(WarpErrorText.message(for: error))
        }
    }

    private func refreshDevices() {
        guard let node else { return }
        let trustedIDs = Set(node.trustedDevices().map(\.deviceId))
        devices = node.peers().map { p in
            Device(id: p.deviceId, name: p.name, kind: p.kind, path: p.bestPath ?? cachedPeers[p.deviceId]?.path.flatMap(PathKind.init(tag:)), trusted: p.trusted || trustedIDs.contains(p.deviceId))
        }
        .sorted { a, b in a.trusted != b.trusted ? a.trusted : a.name.localizedStandardCompare(b.name) == .orderedAscending }
    }

    private func handle(_ event: WarpEvent) {
        switch event {
        case .peersChanged, .trustRingChanged:
            refreshDevices()
        case .transferUpdated(let t) where t.id == transferID:
            transfer = t
            if !t.peerName.isEmpty { targetName = t.peerName }
            switch t.state {
            case .completed:
                phase = .done
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) { [weak self] in
                    MainActor.assumeIsolated { self?.close(completed: true) }
                }
            case .failed:
                phase = .failed(WarpErrorText.message(forCode: t.errorCode, text: t.error, peer: targetName))
            case .cancelled:
                close(completed: false)
            default:
                break
            }
        default:
            break
        }
    }

    func send(to device: Device) {
        targetName = device.name
        start { node, payload in
            switch payload.content {
            case .files: try node.sendFiles(peerId: device.id, items: payload.sendItems)
            case .clipboard(let c): try node.sendClipboard(peerId: device.id, content: c)
            }
        }
    }

    func send(toCode code: String) {
        targetName = normalizeCode(input: code).map { formatCode(code: $0) } ?? code
        start { node, payload in
            switch payload.content {
            case .files: try node.sendFilesToCode(code: code, items: payload.sendItems)
            case .clipboard(let c): try node.sendClipboardToCode(code: code, content: c)
            }
        }
    }

    private func start(_ action: (WarpNode, SendPayload) throws -> String) {
        guard let node, let payload else { return }
        scoped = payload.securityScopedURLs.filter { $0.startAccessingSecurityScopedResource() }
        do {
            transferID = try action(node, payload)
            transfer = node.transfers().first { $0.id == transferID }
            phase = .sending
        } catch {
            phase = .failed(WarpErrorText.message(for: error, peer: targetName))
        }
    }

    func retry() {
        transfer = nil
        transferID = nil
        phase = payload == nil ? .failed(L("share_nothing")) : .choosing
    }

    func cancel() {
        if let transferID, phase == .sending {
            try? node?.cancelTransfer(id: transferID)
        }
        close(completed: false)
    }

    func close(completed: Bool) {
        guard let finish else { return }
        self.finish = nil
        browser?.stop()
        browser = nil
        node?.shutdown()
        node = nil
        scoped.forEach { $0.stopAccessingSecurityScopedResource() }
        if let payload {
            for url in payload.temporaryURLs { try? FileManager.default.removeItem(at: url) }
        }
        finish(completed)
    }
}
