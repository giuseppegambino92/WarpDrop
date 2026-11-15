import SwiftUI

/// macOS entry point. Windows and the menu-bar item are AppKit-managed by
/// `AppDelegate` (the window must stay re-openable from the status item and
/// the status item must accept drags, which `MenuBarExtra` cannot do); the
/// SwiftUI `App` contributes the main menu commands.
@main
struct WarpDropMacApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        Settings {
            EmptyView()
        }
        .commands {
            CommandGroup(replacing: .appSettings) {
                Button(L("settings") + "…") { appDelegate.showSettings() }
                    .keyboardShortcut(",", modifiers: .command)
            }
            CommandGroup(replacing: .newItem) {
                Button(L("send_files")) { appDelegate.pickFilesForSelection() }
                    .keyboardShortcut("o", modifiers: .command)
                Button(L("link_device") + "…") { appDelegate.showLinkSheet() }
                    .keyboardShortcut("l", modifiers: [.command, .shift])
            }
            CommandGroup(before: .windowList) {
                Button(L("menu_open")) { appDelegate.showMainWindow() }
                    .keyboardShortcut("0", modifiers: .command)
                Divider()
            }
        }
    }
}
