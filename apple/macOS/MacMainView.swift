import SwiftUI

/// Main window content on macOS: the shared `MainView` on a clear background
/// (the window's NSVisualEffectView material shows through), plus the
/// settings hook of the header's ⚙ button.
struct MacMainView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        MainView()
            .frame(minWidth: 640, minHeight: 440)
            .onChange(of: model.isSettingsPresented) { _, presented in
                guard presented else { return }
                model.isSettingsPresented = false
                (NSApp.delegate as? AppDelegate)?.showSettings()
            }
    }
}
