import Foundation
import UniformTypeIdentifiers
import WarpCore

/// Something the user wants to send: files/folders, or clipboard-style content.
struct SendPayload: Sendable {
    enum Content: Sendable {
        case files([URL])
        case clipboard(ClipboardContent)
    }

    var content: Content
    /// Copies we made (e.g. dropped image data); deleted when the transfer ends.
    var temporaryURLs: [URL] = []
    /// URLs that need `startAccessingSecurityScopedResource` while sending.
    var securityScopedURLs: [URL] = []

    var sendItems: [SendItem] {
        guard case .files(let urls) = content else { return [] }
        return urls.map { SendItem(path: $0.path, name: nil) }
    }

    var itemCount: Int {
        switch content {
        case .files(let urls): urls.count
        case .clipboard: 1
        }
    }

    /// "Holiday Photos", "3 items", the text snippet…
    @MainActor
    var summary: String {
        switch content {
        case .files(let urls):
            if urls.count == 1 { return urls[0].lastPathComponent }
            return L("items_count", "\(urls.count)")
        case .clipboard(let c):
            switch c {
            case .text(let text): return String(text.prefix(40))
            case .url(let url): return url
            case .image: return L("send_clipboard")
            }
        }
    }

    /// Merges two payloads (files win; clipboard content can't be combined).
    func merged(with other: SendPayload) -> SendPayload {
        switch (content, other.content) {
        case (.files(let a), .files(let b)):
            var urls = a
            for url in b where !urls.contains(url) { urls.append(url) }
            return SendPayload(content: .files(urls), temporaryURLs: temporaryURLs + other.temporaryURLs, securityScopedURLs: securityScopedURLs + other.securityScopedURLs)
        default:
            return other
        }
    }
}

/// Turns drag-and-drop / paste item providers into a `SendPayload`.
@MainActor
enum ItemLoader {
    static let acceptedTypes: [UTType] = [.fileURL, .folder, .item, .image, .url, .plainText, .text]

    static func payload(from providers: [NSItemProvider]) async -> SendPayload? {
        var files: [URL] = []
        var temporary: [URL] = []
        var scoped: [URL] = []
        var clip: ClipboardContent?

        for provider in providers {
            if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier),
               let url = await loadURL(provider), url.isFileURL {
                files.append(url)
                continue
            }
            if provider.hasItemConformingToTypeIdentifier(UTType.url.identifier),
               let url = await loadURL(provider), !url.isFileURL {
                clip = clip ?? .url(url: url.absoluteString)
                continue
            }
            if provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier),
               !provider.hasItemConformingToTypeIdentifier(UTType.image.identifier),
               let text = await loadText(provider) {
                if let url = URL(string: text.trimmingCharacters(in: .whitespacesAndNewlines)), let scheme = url.scheme, ["http", "https"].contains(scheme) {
                    clip = clip ?? .url(url: url.absoluteString)
                } else {
                    clip = clip ?? .text(text: text)
                }
                continue
            }
            // Anything else (Files app items on iOS, image data, folders):
            // get a file representation.
            if let (url, inPlace) = await loadFile(provider) {
                files.append(url)
                if inPlace { scoped.append(url) } else { temporary.append(url.deletingLastPathComponent()) }
            }
        }
        if !files.isEmpty {
            return SendPayload(content: .files(files), temporaryURLs: temporary, securityScopedURLs: scoped)
        }
        if let clip {
            return SendPayload(content: .clipboard(clip))
        }
        return nil
    }

    private static func loadURL(_ provider: NSItemProvider) async -> URL? {
        await withCheckedContinuation { cont in
            _ = provider.loadObject(ofClass: URL.self) { @Sendable url, _ in
                cont.resume(returning: url)
            }
        }
    }

    private static func loadText(_ provider: NSItemProvider) async -> String? {
        await withCheckedContinuation { cont in
            _ = provider.loadObject(ofClass: String.self) { @Sendable text, _ in
                cont.resume(returning: text)
            }
        }
    }

    /// In-place when the provider allows it (no copy of large files), otherwise
    /// a private copy in our temporary "Outgoing" directory.
    private static func loadFile(_ provider: NSItemProvider) async -> (URL, Bool)? {
        let types = provider.registeredTypeIdentifiers
        guard let type = types.first(where: { UTType($0)?.conforms(to: .folder) == true })
            ?? types.first(where: { UTType($0)?.conforms(to: .data) == true })
            ?? types.first
        else { return nil }
        let suggested = provider.suggestedName
        return await withCheckedContinuation { cont in
            _ = provider.loadInPlaceFileRepresentation(forTypeIdentifier: type) { @Sendable url, inPlace, _ in
                guard let url else { return cont.resume(returning: nil) }
                if inPlace { return cont.resume(returning: (url, true)) }
                cont.resume(returning: copyToOutgoing(url, suggestedName: suggested, type: type).map { ($0, false) })
            }
        }
    }

    nonisolated static func copyToOutgoing(_ url: URL, suggestedName: String?, type: String?) -> URL? {
        let fm = FileManager.default
        let dir = fm.temporaryDirectory.appendingPathComponent("Outgoing", isDirectory: true).appendingPathComponent(UUID().uuidString, isDirectory: true)
        do {
            try fm.createDirectory(at: dir, withIntermediateDirectories: true)
            var name = suggestedName ?? url.lastPathComponent
            if (name as NSString).pathExtension.isEmpty, let ext = type.flatMap({ UTType($0)?.preferredFilenameExtension }) ?? Optional(url.pathExtension), !ext.isEmpty {
                name += ".\(ext)"
            }
            let dest = dir.appendingPathComponent(name)
            try fm.copyItem(at: url, to: dest)
            return dest
        } catch {
            return nil
        }
    }

    /// Writes raw data (e.g. a pasted image) to a temporary file.
    nonisolated static func writeTemporary(_ data: Data, name: String) -> URL? {
        let fm = FileManager.default
        let dir = fm.temporaryDirectory.appendingPathComponent("Outgoing", isDirectory: true).appendingPathComponent(UUID().uuidString, isDirectory: true)
        do {
            try fm.createDirectory(at: dir, withIntermediateDirectories: true)
            let dest = dir.appendingPathComponent(name)
            try data.write(to: dest)
            return dest
        } catch {
            return nil
        }
    }
}
