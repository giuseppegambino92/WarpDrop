import Foundation
import Observation
import SwiftUI
import WarpCore

enum MainTab: String, CaseIterable, Identifiable {
    case myDevices, nearby, quickCode

    var id: String { rawValue }

    var titleKey: String {
        switch self {
        case .myDevices: "tab_my_devices"
        case .nearby: "tab_nearby"
        case .quickCode: "tab_quick_code"
        }
    }
}

/// A device card.
struct DeviceItem: Identifiable, Hashable {
    let id: String
    var name: String
    var kind: DeviceKind
    var path: PathKind?
    var trusted: Bool
    var online: Bool
}

/// What the file importer will send to.
struct ImporterRequest: Identifiable, Equatable {
    enum Target: Equatable {
        case device(String)
        case code(String)
    }

    enum Kind {
        case files
        case folder
    }

    let id = UUID()
    var target: Target
    var kind: Kind = .files
}

enum LinkState: Equatable {
    case idle
    case linking
    case linked(String)
    case failed(String)
}

struct Toast: Identifiable, Equatable {
    let id = UUID()
    var message: String
    var isError = false
}

/// Single source of truth for the UI: owns the `WarpNode`, native discovery,
/// iCloud trust sync and all transfer bookkeeping. Engine events arrive in
/// order on the main actor through `EventBridge`.
@Observable
@MainActor
final class AppModel {
    // MARK: Engine state

    private(set) var node: WarpNode?
    private(set) var startupError: String?
    private(set) var identity: DeviceIdentity?
    private(set) var code: CodeInfo?
    private(set) var peers: [PeerInfo] = []
    private(set) var trustedDevices: [TrustedDevice] = []
    private(set) var transfers: [TransferSnapshot] = []
    private(set) var offers: [IncomingOffer] = []

    // MARK: UI state

    var tab: MainTab = .myDevices
    /// Last device the user targeted; ⌘V / paste sends here.
    var selectedDeviceID: String?
    /// Items dropped while no device was targeted.
    private(set) var staged: SendPayload?
    var isDragOverWindow = false
    var dropTargetDeviceID: String?
    var importerRequest: ImporterRequest?
    var isLinkSheetPresented = false
    var isSettingsPresented = false
    private(set) var linkState: LinkState = .idle
    private(set) var linkWindowExpiry: Date?
    private(set) var recentlyLinkedID: String?
    var toast: Toast?

    let settings = SettingsStore()

    /// Called for every transfer update (Live Activities on iOS).
    @ObservationIgnored var onTransferUpdate: ((TransferSnapshot) -> Void)?

    // MARK: Private

    @ObservationIgnored private let advertiser = BonjourAdvertiser()
    @ObservationIgnored private var browser: BonjourBrowser?
    @ObservationIgnored private let iCloud = ICloudTrustSync()
    @ObservationIgnored private let notifier = Notifier()
    @ObservationIgnored private var codeLabels: [String: String] = [:]
    @ObservationIgnored private var fallbackTitles: [String: String] = [:]
    @ObservationIgnored private var scopedAccess: [String: [URL]] = [:]
    @ObservationIgnored private var temporaryFiles: [String: [URL]] = [:]
    @ObservationIgnored private var finishedHandled: Set<String> = []
    @ObservationIgnored private var cacheTask: Task<Void, Never>?
    @ObservationIgnored private var toastTask: Task<Void, Never>?
    @ObservationIgnored private let debugPrint = UserDefaults.standard.bool(forKey: "WarpDebugPrintCode")

    init() {}

    // MARK: - Lifecycle

    func start() {
        guard node == nil else { return }
        notifier.start()
        startNode()
    }

    private func startNode() {
        let kind = PlatformServices.deviceKind
        var cfg = defaultNodeConfig(
            deviceName: settings.deviceName,
            stateDir: AppGroup.appStateDirectory.path,
            downloadDir: settings.downloadDirectory.path
        )
        cfg.deviceKind = kind
        cfg.identitySeed = IdentityStore.loadOrCreateSeed()
        cfg.port = 0
        cfg.enableMdns = false // Bonjour is native (DNS-SD + NWBrowser, incl. AWDL)
        #if os(macOS)
        cfg.enableBeacon = true
        cfg.enableAdb = true // Android over USB through `adb forward`
        #else
        cfg.enableBeacon = false // broadcast needs the multicast entitlement on iOS
        cfg.enableAdb = false
        #endif
        cfg.relayUrl = settings.relayURLOrNil

        let node: WarpNode
        do {
            node = try WarpNode(config: cfg, listener: EventBridge { [weak self] event in
                self?.handle(event)
            })
        } catch {
            startupError = WarpErrorText.message(for: error)
            return
        }
        self.node = node
        startupError = nil
        let me = node.identity()
        identity = me
        code = node.currentCode()
        peers = node.peers()
        trustedDevices = node.trustedDevices()
        mergeTransfers(node.transfers())
        if debugPrint {
            debugLog("WARP_CODE \(code?.code ?? "") \(code?.display ?? "") id=\(me.deviceId) port=\(node.port()) download=\(node.downloadDir())")
            debugLog("WARP_QR \(code?.qrPayload ?? "")")
        }

        advertiser.start(node.advertisement())
        let browser = BonjourBrowser(ownDeviceID: me.deviceId)
        browser.onPeer = { [weak self] _, advert in self?.node?.injectPeer(advert: advert) }
        browser.onLost = { [weak self] id in self?.node?.removeInjectedPeer(deviceId: id) }
        browser.start()
        self.browser = browser

        iCloud.start(identity: me) { [weak self] pk, name, kind in
            guard let self, !self.settings.ignoredICloudKeys.contains(pk.lowercased()) else { return }
            try? self.node?.trustDevice(publicKeyHex: pk, name: name, kind: kind, origin: .iCloud)
        }
        // First run (no devices of mine yet): start where devices actually show up.
        if trustedDevices.isEmpty {
            tab = .nearby
        }
        schedulePeerCacheWrite()
    }

    func shutdown() {
        browser?.stop()
        advertiser.stop()
        node?.shutdown()
    }

    /// Foreground again: refresh addresses that may have changed.
    func didBecomeActive() {
        browser?.refresh()
        if let node {
            advertiser.start(node.advertisement())
            peers = node.peers()
            trustedDevices = node.trustedDevices()
        }
    }

    // MARK: - Events

    private func handle(_ event: WarpEvent) {
        switch event {
        case .peersChanged(let list):
            peers = list
            schedulePeerCacheWrite()
        case .codeChanged(let info):
            code = info
            if debugPrint {
                debugLog("WARP_CODE \(info.code) \(info.display)")
                debugLog("WARP_QR \(info.qrPayload)")
            }
        case .trustRingChanged(let devices):
            trustedDevices = devices
            schedulePeerCacheWrite()
        case .offerReceived(let offer):
            offers.removeAll { $0.offerId == offer.offerId }
            offers.append(offer)
            if !PlatformServices.isAppActive {
                notifier.post(id: "offer-\(offer.offerId)", title: L("offer_title", offer.peerName), body: "\(offer.title) (\(WarpFormat.bytes(offer.totalBytes)))")
            }
        case .offerWithdrawn(let id):
            offers.removeAll { $0.offerId == id }
        case .transferUpdated(let snapshot):
            upsert(snapshot)
        case .clipboardReceived(_, let fromName, let content):
            PlatformServices.writeClipboard(content)
            notifier.post(id: "clip-\(UUID().uuidString)", title: L("clipboard_received", fromName), body: L("clipboard_received_body"))
            show(L("clipboard_received", fromName))
        case .deviceLinked(let device):
            linkState = .linked(device.name)
            recentlyLinkedID = device.deviceId
            settings.unignoreICloudKey(device.publicKeyHex)
            withAnimation(Theme.Motion.spring) { tab = .myDevices }
            node?.closeLinkWindow()
            linkWindowExpiry = nil
        case .error(let message):
            if linkState == .linking {
                linkState = .failed(WarpErrorText.message(forEngineText: message))
            } else {
                show(WarpErrorText.message(forEngineText: message), error: true)
            }
        }
    }

    // MARK: - Transfers

    private func mergeTransfers(_ list: [TransferSnapshot]) {
        for t in list { upsert(t) }
    }

    private func upsert(_ snapshot: TransferSnapshot) {
        if let i = transfers.firstIndex(where: { $0.id == snapshot.id }) {
            transfers[i] = snapshot
        } else {
            // Newest first, by engine start time.
            let i = transfers.firstIndex { $0.startedAtMs < snapshot.startedAtMs } ?? transfers.endIndex
            transfers.insert(snapshot, at: i)
        }
        onTransferUpdate?(snapshot)
        if snapshot.state.isTerminal, !finishedHandled.contains(snapshot.id) {
            finishedHandled.insert(snapshot.id)
            finish(snapshot)
        }
    }

    private func finish(_ t: TransferSnapshot) {
        releaseResources(of: t.id)
        guard t.state == .completed else { return }
        let peer = displayPeer(t)
        if t.direction == .incoming {
            if debugPrint { debugLog("WARP_SAVED \(t.savedPaths.joined(separator: "|"))") }
            notifier.post(id: "rx-\(t.id)", title: L("notif_received_title", peer), body: t.title, paths: t.savedPaths)
        } else if !PlatformServices.isAppActive {
            notifier.post(id: "tx-\(t.id)", title: L("notif_sent_title", peer), body: displayTitle(t))
        }
    }

    /// Ends security-scoped access and deletes our temporary copies.
    private func releaseResources(of id: String) {
        for url in scopedAccess.removeValue(forKey: id) ?? [] {
            url.stopAccessingSecurityScopedResource()
        }
        let temporary = temporaryFiles.removeValue(forKey: id) ?? []
        if !temporary.isEmpty {
            Task.detached(priority: .utility) {
                for url in temporary { try? FileManager.default.removeItem(at: url) }
            }
        }
    }

    func displayPeer(_ t: TransferSnapshot) -> String {
        if !t.peerName.isEmpty { return t.peerName }
        if let label = codeLabels[t.id] { return label }
        return trustedDevices.first { $0.deviceId == t.peerId }?.name ?? peers.first { $0.deviceId == t.peerId }?.name ?? "…"
    }

    func displayTitle(_ t: TransferSnapshot) -> String {
        if !t.title.isEmpty { return t.title }
        return fallbackTitles[t.id] ?? "…"
    }

    var hasFinishedTransfers: Bool {
        transfers.contains { $0.state.isTerminal }
    }

    func pause(_ id: String) {
        perform { try $0.pauseTransfer(id: id) }
    }

    func resume(_ id: String) {
        perform { try $0.resumeTransfer(id: id) }
    }

    func cancel(_ id: String) {
        perform { try $0.cancelTransfer(id: id) }
    }

    func clearFinished() {
        node?.clearFinishedTransfers()
        withAnimation(Theme.Motion.spring) {
            transfers.removeAll { $0.state.isTerminal }
        }
    }

    private func perform(_ action: (WarpNode) throws -> Void) {
        guard let node else { return }
        do {
            try action(node)
        } catch {
            show(WarpErrorText.message(for: error), error: true)
        }
    }

    // MARK: - Sending

    func send(_ payload: SendPayload, toDevice deviceID: String) {
        guard let node else { return }
        selectedDeviceID = deviceID
        let name = deviceName(deviceID)
        do {
            let id: String
            switch payload.content {
            case .files:
                let scoped = beginAccess(payload.securityScopedURLs)
                do {
                    id = try node.sendFiles(peerId: deviceID, items: payload.sendItems)
                } catch {
                    scoped.forEach { $0.stopAccessingSecurityScopedResource() }
                    throw error
                }
                register(id, payload: payload, scoped: scoped)
            case .clipboard(let content):
                id = try node.sendClipboard(peerId: deviceID, content: content)
                register(id, payload: payload, scoped: [])
            }
        } catch {
            show(WarpErrorText.message(for: error, peer: name), error: true)
        }
    }

    /// `target` is 6 digits, `482-910` or a `warpdrop://` link.
    func send(_ payload: SendPayload, toCode target: String) {
        guard let node else { return }
        guard let normalized = normalizeCode(input: target) else {
            show(L("error_wrong_code"), error: true)
            return
        }
        do {
            let id: String
            switch payload.content {
            case .files:
                let scoped = beginAccess(payload.securityScopedURLs)
                do {
                    id = try node.sendFilesToCode(code: target, items: payload.sendItems)
                } catch {
                    scoped.forEach { $0.stopAccessingSecurityScopedResource() }
                    throw error
                }
                register(id, payload: payload, scoped: scoped)
            case .clipboard(let content):
                id = try node.sendClipboardToCode(code: target, content: content)
                register(id, payload: payload, scoped: [])
            }
            codeLabels[id] = formatCode(code: normalized)
        } catch {
            show(WarpErrorText.message(for: error), error: true)
        }
    }

    private func register(_ id: String, payload: SendPayload, scoped: [URL]) {
        fallbackTitles[id] = payload.summary
        if !scoped.isEmpty { scopedAccess[id] = scoped }
        if !payload.temporaryURLs.isEmpty { temporaryFiles[id] = payload.temporaryURLs }
        // The transfer may already have finished (tiny clipboard sends).
        if finishedHandled.contains(id) {
            releaseResources(of: id)
        }
    }

    private func beginAccess(_ urls: [URL]) -> [URL] {
        urls.filter { $0.startAccessingSecurityScopedResource() }
    }

    /// Files picked in the importer (security-scoped on iOS).
    func sendPicked(_ urls: [URL], to target: ImporterRequest.Target) {
        guard !urls.isEmpty else { return }
        let payload = SendPayload(content: .files(urls), securityScopedURLs: urls)
        switch target {
        case .device(let id): send(payload, toDevice: id)
        case .code(let code): send(payload, toCode: code)
        }
    }

    /// Card click: send the staged items, or pick files.
    func activate(_ device: DeviceItem, kind: ImporterRequest.Kind = .files) {
        selectedDeviceID = device.id
        if let staged {
            self.staged = nil
            send(staged, toDevice: device.id)
        } else {
            importerRequest = ImporterRequest(target: .device(device.id), kind: kind)
        }
    }

    /// Code field completed: send the staged items, or pick files.
    func submitCode(_ target: String) {
        guard normalizeCode(input: target) != nil else {
            show(L("error_wrong_code"), error: true)
            return
        }
        if let staged {
            self.staged = nil
            send(staged, toCode: target)
        } else {
            importerRequest = ImporterRequest(target: .code(target))
        }
    }

    /// Item providers dropped on a card (`deviceID`) or anywhere else (stage).
    func handleDrop(_ providers: [NSItemProvider], deviceID: String?) {
        Task {
            guard let payload = await ItemLoader.payload(from: providers) else { return }
            if let deviceID {
                send(payload, toDevice: deviceID)
            } else {
                stage(payload)
            }
        }
    }

    func stage(_ payload: SendPayload) {
        withAnimation(Theme.Motion.spring) {
            staged = staged.map { $0.merged(with: payload) } ?? payload
        }
    }

    func clearStaged() {
        if let staged {
            let temporary = staged.temporaryURLs
            Task.detached(priority: .utility) {
                for url in temporary { try? FileManager.default.removeItem(at: url) }
            }
        }
        withAnimation(Theme.Motion.spring) { staged = nil }
    }

    /// ⌘V / paste / "Send Clipboard": the clipboard goes to `deviceID`, the
    /// selected device, or is staged when nothing is targeted.
    func sendClipboard(to deviceID: String? = nil) {
        guard let payload = PlatformServices.clipboardPayload() else {
            show(L("clipboard_empty"), error: true)
            return
        }
        sendPasted(payload, to: deviceID)
    }

    func sendPasted(_ payload: SendPayload, to deviceID: String? = nil) {
        if let target = deviceID ?? selectedDeviceID, isOnline(target) {
            send(payload, toDevice: target)
        } else {
            stage(payload)
        }
    }

    // MARK: - Devices

    var myDevices: [DeviceItem] {
        trustedDevices.map { t in
            let peer = peers.first { $0.deviceId == t.deviceId }
            return DeviceItem(id: t.deviceId, name: peer?.name ?? t.name, kind: t.kind, path: peer?.bestPath, trusted: true, online: t.online || peer != nil)
        }
        .sorted { a, b in a.online != b.online ? a.online : a.name.localizedStandardCompare(b.name) == .orderedAscending }
    }

    var nearbyDevices: [DeviceItem] {
        peers.filter { !$0.trusted }
            .map { DeviceItem(id: $0.deviceId, name: $0.name, kind: $0.kind, path: $0.bestPath, trusted: false, online: true) }
            .sorted { a, b in a.name.localizedStandardCompare(b.name) == .orderedAscending }
    }

    /// Every reachable device (menu-bar radar, share targets).
    var onlineDevices: [DeviceItem] {
        myDevices.filter(\.online) + nearbyDevices
    }

    func isOnline(_ deviceID: String) -> Bool {
        peers.contains { $0.deviceId == deviceID }
    }

    func deviceName(_ deviceID: String) -> String {
        peers.first { $0.deviceId == deviceID }?.name ?? trustedDevices.first { $0.deviceId == deviceID }?.name ?? ""
    }

    func removeDevice(_ deviceID: String) {
        if let t = trustedDevices.first(where: { $0.deviceId == deviceID }), t.origin == .iCloud {
            settings.ignoreICloudKey(t.publicKeyHex)
        }
        perform { try $0.untrustDevice(deviceId: deviceID) }
        if selectedDeviceID == deviceID { selectedDeviceID = nil }
    }

    // MARK: - Offers

    func respond(to offer: IncomingOffer, accept: Bool) {
        node?.respondToOffer(offerId: offer.offerId, accept: accept)
        offers.removeAll { $0.offerId == offer.offerId }
    }

    // MARK: - Link My Device

    func presentLinkSheet() {
        linkState = .idle
        isLinkSheetPresented = true
    }

    func openLinkWindow() {
        node?.openLinkWindow(seconds: 300)
        linkWindowExpiry = Date().addingTimeInterval(300)
    }

    func closeLinkWindow() {
        node?.closeLinkWindow()
        linkWindowExpiry = nil
    }

    func link(withCode target: String) {
        guard let node else { return }
        do {
            try node.linkWithCode(code: target)
            linkState = .linking
        } catch {
            linkState = .failed(WarpErrorText.message(for: error))
        }
    }

    func resetLinkState() {
        linkState = .idle
    }

    // MARK: - Settings

    func applyDeviceName(_ name: String) {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let node else { return }
        settings.deviceName = trimmed
        node.setDeviceName(name: trimmed)
        advertiser.start(node.advertisement())
        let me = node.identity()
        identity = me
        iCloud.publish(me)
    }

    func applyDownloadDirectory(_ url: URL) {
        guard let node else { return }
        do {
            try node.setDownloadDir(dir: url.path)
            settings.downloadDirectory = url
        } catch {
            show(WarpErrorText.message(for: error), error: true)
        }
    }

    func applyRelayURL(_ value: String) {
        let previous = settings.relayURLOrNil
        settings.relayURL = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard settings.relayURLOrNil != previous else { return }
        // Restarts the engine's rendezvous listener live.
        node?.setRelayUrl(url: settings.relayURLOrNil)
    }

    func rotateCode() {
        node?.rotateCode()
    }

    // MARK: - Feedback

    func show(_ message: String, error: Bool = false) {
        let toast = Toast(message: message, isError: error)
        withAnimation(Theme.Motion.spring) { self.toast = toast }
        toastTask?.cancel()
        toastTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(error ? 5 : 2.5))
            guard !Task.isCancelled, let self, self.toast?.id == toast.id else { return }
            withAnimation(Theme.Motion.spring) { self.toast = nil }
        }
    }

    // MARK: - Extension cache

    private func schedulePeerCacheWrite() {
        cacheTask?.cancel()
        cacheTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(1))
            guard !Task.isCancelled, let self else { return }
            let cache = PeerCache.make(peers: self.peers, trusted: self.trustedDevices, bonjourTXT: self.browser?.txtByDevice ?? [:])
            await Task.detached(priority: .utility) { cache.save() }.value
        }
    }

    private func debugLog(_ line: String) {
        FileHandle.standardOutput.write(Data((line + "\n").utf8))
    }
}
