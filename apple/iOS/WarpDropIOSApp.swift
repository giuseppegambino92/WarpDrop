import SwiftUI
import UIKit
import WarpCore

@main
struct WarpDropIOSApp: App {
    @State private var model = AppModel()
    @State private var liveActivities = LiveActivityController()
    @State private var background = BackgroundTransferKeeper()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            IOSRootView()
                .environment(model)
                .task {
                    let model = self.model
                    model.onTransferUpdate = { [weak model, liveActivities, background] t in
                        guard let model else { return }
                        liveActivities.update(t, peer: model.displayPeer(t), title: model.displayTitle(t))
                        background.update(model.transfers)
                    }
                    model.start()
                }
                .onOpenURL { url in
                    // warpdrop:// links (e.g. scanned with the Camera app).
                    model.tab = .quickCode
                    model.submitCode(url.absoluteString)
                }
        }
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .active: model.didBecomeActive()
            case .background: background.update(model.transfers)
            default: break
            }
        }
    }
}

/// Phone/iPad root: the shared main view on the token background, with
/// settings as a sheet.
struct IOSRootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        MainView()
            .background(Theme.bg.ignoresSafeArea())
            .sheet(isPresented: $model.isSettingsPresented) {
                NavigationStack {
                    SettingsView()
                }
                .environment(model)
            }
    }
}

/// Asks iOS for extra background time while transfers are running, so a
/// transfer started just before switching apps can finish.
@MainActor
final class BackgroundTransferKeeper {
    private var task: UIBackgroundTaskIdentifier = .invalid

    func update(_ transfers: [TransferSnapshot]) {
        let running = transfers.contains { $0.state.isActive && $0.state != .paused }
        if running, task == .invalid {
            task = UIApplication.shared.beginBackgroundTask(withName: "WarpDrop transfer") { [weak self] in
                MainActor.assumeIsolated { self?.end() }
            }
        } else if !running {
            end()
        }
    }

    private func end() {
        guard task != .invalid else { return }
        UIApplication.shared.endBackgroundTask(task)
        task = .invalid
    }
}
