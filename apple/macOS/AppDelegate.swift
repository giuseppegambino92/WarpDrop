import AppKit
import SwiftUI
import WarpCore

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    let model = AppModel()
    private var mainWindow: NSWindow?
    private var settingsWindow: NSWindow?
    private var statusItem: StatusItemController?
    private var keyMonitor: Any?
    private var activeObserver: NSObjectProtocol?

    func applicationDidFinishLaunching(_ notification: Notification) {
        model.start()
        statusItem = StatusItemController(model: model, delegate: self)
        installPasteMonitor()
        activeObserver = NotificationCenter.default.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.model.didBecomeActive() }
        }
        // Started as a login item: stay in the menu bar only.
        if !UserDefaults.standard.bool(forKey: "WarpLaunchHidden") {
            showMainWindow()
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false // keep running in the menu bar
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { showMainWindow() }
        return true
    }

    /// `warpdrop://` links (QR tickets opened from a browser or Messages).
    func application(_ application: NSApplication, open urls: [URL]) {
        guard let url = urls.first(where: { $0.scheme?.lowercased() == "warpdrop" }) else { return }
        showMainWindow()
        model.tab = .quickCode
        model.submitCode(url.absoluteString)
    }

    func applicationWillTerminate(_ notification: Notification) {
        model.shutdown()
    }

    // MARK: - Windows

    func showMainWindow() {
        if mainWindow == nil {
            mainWindow = makeMainWindow()
        }
        NSApp.setActivationPolicy(.regular)
        mainWindow?.makeKeyAndOrderFront(nil)
        NSApp.activate()
    }

    private func makeMainWindow() -> NSWindow {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 860, height: 620),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title = L("app_name")
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.isMovableByWindowBackground = true
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 640, height: 480)
        window.setFrameAutosaveName("WarpDropMain")
        window.delegate = self
        window.tabbingMode = .disallowed

        // Translucent native material behind everything (incl. the titlebar);
        // SwiftUI content respects the titlebar safe area.
        let material = NSVisualEffectView()
        material.material = .underWindowBackground
        material.blendingMode = .behindWindow
        material.state = .followsWindowActiveState
        let hosting = NSHostingView(rootView: MacMainView().environment(model))
        hosting.translatesAutoresizingMaskIntoConstraints = false
        material.addSubview(hosting)
        NSLayoutConstraint.activate([
            hosting.leadingAnchor.constraint(equalTo: material.leadingAnchor),
            hosting.trailingAnchor.constraint(equalTo: material.trailingAnchor),
            hosting.topAnchor.constraint(equalTo: material.topAnchor),
            hosting.bottomAnchor.constraint(equalTo: material.bottomAnchor),
        ])
        window.contentView = material
        if window.frame.origin == .zero { window.center() }
        return window
    }

    func showSettings() {
        if settingsWindow == nil {
            let window = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 480, height: 560),
                styleMask: [.titled, .closable, .fullSizeContentView],
                backing: .buffered,
                defer: false
            )
            window.isReleasedWhenClosed = false
            window.titlebarAppearsTransparent = true
            window.contentViewController = NSHostingController(rootView: SettingsWindowContent().environment(model))
            window.center()
            settingsWindow = window
        }
        settingsWindow?.title = L("settings")
        NSApp.setActivationPolicy(.regular)
        settingsWindow?.makeKeyAndOrderFront(nil)
        NSApp.activate()
    }

    func showLinkSheet() {
        showMainWindow()
        model.presentLinkSheet()
    }

    /// ⌘O: pick files for the selected device (or a code target later).
    func pickFilesForSelection() {
        showMainWindow()
        if let id = model.selectedDeviceID, let device = model.onlineDevices.first(where: { $0.id == id }) {
            model.activate(device)
        } else if let device = model.onlineDevices.first, model.onlineDevices.count == 1 {
            model.activate(device)
        } else {
            model.tab = .quickCode
        }
    }

    func quit() {
        NSApp.terminate(nil)
    }

    func windowWillClose(_ notification: Notification) {
        guard (notification.object as? NSWindow) === mainWindow else { return }
        // Window closed: live on as a menu-bar app without a Dock icon.
        DispatchQueue.main.async {
            if NSApp.windows.allSatisfy({ !$0.isVisible || $0.level == .statusBar || $0 is NSPanel }) {
                NSApp.setActivationPolicy(.accessory)
            }
        }
    }

    // MARK: - ⌘V

    /// ⌘V with focus outside a text field sends the clipboard to the
    /// selected device (or stages it).
    private func installPasteMonitor() {
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command,
                  event.charactersIgnoringModifiers?.lowercased() == "v"
            else { return event }
            let windowNumber = event.windowNumber
            let handled = MainActor.assumeIsolated { self?.handlePaste(inWindow: windowNumber) ?? false }
            return handled ? nil : event
        }
    }

    private func handlePaste(inWindow number: Int) -> Bool {
        guard let window = mainWindow, window.windowNumber == number, window.isKeyWindow,
              window.attachedSheet == nil,
              !(window.firstResponder is NSText)
        else { return false }
        model.sendClipboard()
        return true
    }
}

/// Settings in its own window, with the shared background.
private struct SettingsWindowContent: View {
    var body: some View {
        SettingsView()
            .background(Theme.bg)
    }
}
