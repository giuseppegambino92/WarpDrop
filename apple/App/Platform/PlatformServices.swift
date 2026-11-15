import Foundation
import SwiftUI
import UniformTypeIdentifiers
import WarpCore
#if os(macOS)
import AppKit
#else
import UIKit
#endif

/// Small platform seams (pasteboard, Finder/Files, device facts) so the model
/// and views stay platform-neutral.
@MainActor
enum PlatformServices {
    // MARK: Device

    static var deviceKind: DeviceKind {
        #if os(macOS)
        return .mac
        #else
        return UIDevice.current.userInterfaceIdiom == .pad ? .iPad : .iPhone
        #endif
    }

    static var defaultDeviceName: String {
        #if os(macOS)
        return Host.current().localizedName ?? "Mac"
        #else
        return UIDevice.current.name
        #endif
    }

    static var defaultDownloadDirectory: URL {
        let fm = FileManager.default
        #if os(macOS)
        let downloads = fm.urls(for: .downloadsDirectory, in: .userDomainMask).first ?? fm.homeDirectoryForCurrentUser.appendingPathComponent("Downloads")
        return downloads.appendingPathComponent("WarpDrop", isDirectory: true)
        #else
        // Documents is visible in the Files app (UIFileSharingEnabled + LSSupportsOpeningDocumentsInPlace).
        return fm.urls(for: .documentDirectory, in: .userDomainMask)[0]
        #endif
    }

    static var isAppActive: Bool {
        #if os(macOS)
        return NSApp?.isActive ?? false
        #else
        return UIApplication.shared.applicationState == .active
        #endif
    }

    // MARK: Pasteboard

    static func copy(_ string: String) {
        #if os(macOS)
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(string, forType: .string)
        #else
        UIPasteboard.general.string = string
        #endif
    }

    /// Current clipboard as something sendable: copied files, an image (PNG),
    /// a URL or text.
    static func clipboardPayload() -> SendPayload? {
        #if os(macOS)
        let pb = NSPasteboard.general
        if let urls = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL], !urls.isEmpty {
            return SendPayload(content: .files(urls))
        }
        if let png = pb.data(forType: .png) {
            return SendPayload(content: .clipboard(.image(mime: "image/png", data: png)))
        }
        if let tiff = pb.data(forType: .tiff), let rep = NSBitmapImageRep(data: tiff), let png = rep.representation(using: .png, properties: [:]) {
            return SendPayload(content: .clipboard(.image(mime: "image/png", data: png)))
        }
        if let string = pb.string(forType: .string), !string.isEmpty {
            return SendPayload(content: .clipboard(textOrURL(string)))
        }
        return nil
        #else
        let pb = UIPasteboard.general
        if pb.hasImages, let image = pb.image, let png = image.pngData() {
            return SendPayload(content: .clipboard(.image(mime: "image/png", data: png)))
        }
        if pb.hasURLs, let url = pb.url {
            if url.isFileURL { return SendPayload(content: .files([url])) }
            return SendPayload(content: .clipboard(.url(url: url.absoluteString)))
        }
        if pb.hasStrings, let string = pb.string, !string.isEmpty {
            return SendPayload(content: .clipboard(textOrURL(string)))
        }
        return nil
        #endif
    }

    static func textOrURL(_ string: String) -> ClipboardContent {
        let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.contains(where: \.isWhitespace), let url = URL(string: trimmed), let scheme = url.scheme?.lowercased(), ["http", "https"].contains(scheme) {
            return .url(url: trimmed)
        }
        return .text(text: string)
    }

    /// Puts received clipboard content on the local pasteboard.
    static func writeClipboard(_ content: ClipboardContent) {
        #if os(macOS)
        let pb = NSPasteboard.general
        pb.clearContents()
        switch content {
        case .text(let text):
            pb.setString(text, forType: .string)
        case .url(let url):
            if let u = URL(string: url) { pb.writeObjects([u as NSURL]) }
            pb.setString(url, forType: .string)
        case .image(let mime, let data):
            if mime == "image/png" {
                pb.setData(data, forType: .png)
            }
            if let image = NSImage(data: data) {
                pb.writeObjects([image])
            }
        }
        #else
        let pb = UIPasteboard.general
        switch content {
        case .text(let text):
            pb.string = text
        case .url(let url):
            if let u = URL(string: url) { pb.url = u } else { pb.string = url }
        case .image(_, let data):
            if let image = UIImage(data: data) { pb.image = image }
        }
        #endif
    }

    // MARK: Files

    /// Reveals received items in Finder / opens their folder in Files.
    static func reveal(_ paths: [String]) {
        let urls = paths.map { URL(fileURLWithPath: $0) }
        #if os(macOS)
        if urls.isEmpty { return }
        NSWorkspace.shared.activateFileViewerSelecting(urls)
        #else
        guard let first = urls.first else { return }
        // `shareddocuments://` opens the Files app at that location.
        let folder = first.hasDirectoryPath ? first : first.deletingLastPathComponent()
        if let url = URL(string: "shareddocuments://" + folder.path.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed)!) {
            UIApplication.shared.open(url)
        }
        #endif
    }

    static func openFolder(_ url: URL) {
        #if os(macOS)
        NSWorkspace.shared.open(url)
        #else
        reveal([url.path])
        #endif
    }
}
