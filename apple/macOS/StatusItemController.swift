import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// The persistent menu-bar droplet.
///
/// `MenuBarExtra` cannot receive drops, so this is an `NSStatusItem` whose
/// button hosts a dragging-destination view: dragging files, folders or text
/// onto the icon immediately opens the radar popover listing devices, and
/// dropping onto a device row there sends. Dropping on the icon itself stages
/// the items until a device is picked.
@MainActor
final class StatusItemController: NSObject, NSPopoverDelegate {
    private let model: AppModel
    private weak var delegate: AppDelegate?
    private let item: NSStatusItem
    private let popover = NSPopover()
    private var openedByDrag = false

    init(model: AppModel, delegate: AppDelegate) {
        self.model = model
        self.delegate = delegate
        item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        super.init()

        if let button = item.button {
            let image = NSImage(systemSymbolName: "drop", accessibilityDescription: L("app_name"))
            image?.isTemplate = true
            button.image = image
            button.setAccessibilityLabel(L("app_name"))

            let dropView = StatusDropView(frame: button.bounds)
            dropView.autoresizingMask = [.width, .height]
            dropView.onDragEnter = { [weak self] in self?.showPopover(fromDrag: true) }
            dropView.onDrop = { [weak self] pasteboard in self?.stage(from: pasteboard) ?? false }
            dropView.onClick = { [weak self] in self?.togglePopover() }
            dropView.onRightClick = { [weak self] in self?.showMenu() }
            button.addSubview(dropView)
        }

        popover.behavior = .transient
        popover.animates = true
        popover.delegate = self
        popover.contentViewController = NSHostingController(rootView: RadarView(
            onSent: { [weak self] in self?.closeAfterSend() },
            onOpenApp: { [weak self] in
                self?.popover.performClose(nil)
                self?.delegate?.showMainWindow()
            },
            onSettings: { [weak self] in
                self?.popover.performClose(nil)
                self?.delegate?.showSettings()
            },
            onQuit: { [weak self] in self?.delegate?.quit() }
        ).environment(model))
    }

    private func togglePopover() {
        if popover.isShown {
            popover.performClose(nil)
        } else {
            showPopover(fromDrag: false)
        }
    }

    private func showPopover(fromDrag: Bool) {
        guard let button = item.button else { return }
        openedByDrag = fromDrag
        if !popover.isShown {
            popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
            if !fromDrag {
                popover.contentViewController?.view.window?.makeKey()
            }
        }
        button.highlight(true)
    }

    private func closeAfterSend() {
        // Let the row's confirmation animate briefly before closing.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { [weak self] in
            MainActor.assumeIsolated { self?.popover.performClose(nil) }
        }
    }

    func popoverDidClose(_ notification: Notification) {
        item.button?.highlight(false)
        openedByDrag = false
    }

    private func showMenu() {
        let menu = NSMenu()
        menu.addItem(withTitle: L("menu_open"), action: #selector(openApp), keyEquivalent: "").target = self
        menu.addItem(withTitle: L("settings") + "…", action: #selector(openSettings), keyEquivalent: ",").target = self
        menu.addItem(.separator())
        menu.addItem(withTitle: L("menu_quit"), action: #selector(quitApp), keyEquivalent: "q").target = self
        item.menu = menu
        item.button?.performClick(nil)
        item.menu = nil
    }

    @objc private func openApp() { delegate?.showMainWindow() }
    @objc private func openSettings() { delegate?.showSettings() }
    @objc private func quitApp() { delegate?.quit() }

    /// Dropped on the icon itself: stage and keep the radar open.
    private func stage(from pasteboard: NSPasteboard) -> Bool {
        guard let payload = StatusDropView.payload(from: pasteboard) else { return false }
        model.stage(payload)
        showPopover(fromDrag: true)
        return true
    }
}

/// Transparent view on top of the status button that accepts drags and
/// forwards clicks.
final class StatusDropView: NSView {
    var onDragEnter: (() -> Void)?
    var onDrop: ((NSPasteboard) -> Bool)?
    var onClick: (() -> Void)?
    var onRightClick: (() -> Void)?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        registerForDraggedTypes([.fileURL, .URL, .string, .png, .tiff])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func draggingEntered(_ sender: any NSDraggingInfo) -> NSDragOperation {
        onDragEnter?()
        return .copy
    }

    override func draggingUpdated(_ sender: any NSDraggingInfo) -> NSDragOperation {
        .copy
    }

    override func performDragOperation(_ sender: any NSDraggingInfo) -> Bool {
        onDrop?(sender.draggingPasteboard) ?? false
    }

    override func mouseDown(with event: NSEvent) {
        if event.modifierFlags.contains(.control) {
            onRightClick?()
        } else {
            onClick?()
        }
    }

    override func rightMouseDown(with event: NSEvent) {
        onRightClick?()
    }

    /// Reads files / image / URL / text from a drag pasteboard.
    @MainActor
    static func payload(from pasteboard: NSPasteboard) -> SendPayload? {
        if let urls = pasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL], !urls.isEmpty {
            return SendPayload(content: .files(urls))
        }
        if let png = pasteboard.data(forType: .png) {
            return SendPayload(content: .clipboard(.image(mime: "image/png", data: png)))
        }
        if let urls = pasteboard.readObjects(forClasses: [NSURL.self]) as? [URL], let url = urls.first {
            return SendPayload(content: .clipboard(.url(url: url.absoluteString)))
        }
        if let text = pasteboard.string(forType: .string), !text.isEmpty {
            return SendPayload(content: .clipboard(PlatformServices.textOrURL(text)))
        }
        return nil
    }
}
