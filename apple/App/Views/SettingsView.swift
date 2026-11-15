import SwiftUI
import UniformTypeIdentifiers
import WarpCore

/// Device name, download folder, language, relay, fingerprint.
struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var relay = ""
    @State private var pickingFolder = false

    var body: some View {
        @Bindable var localizer = Localizer.shared
        Form {
            Section {
                TextField(L("settings_device_name"), text: $name)
                    .onSubmit { model.applyDeviceName(name) }
                    #if os(iOS)
                    .submitLabel(.done)
                    #endif
            } header: {
                Text(L("settings_device_name"))
            }

            Section {
                HStack {
                    Label(folderName, systemImage: "folder")
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer()
                    #if os(macOS)
                    Button(L("settings_change")) { pickingFolder = true }
                    #else
                    Button(L("open_in_files")) { PlatformServices.openFolder(model.settings.downloadDirectory) }
                    #endif
                }
            } header: {
                Text(L("settings_download_folder"))
            }

            Section {
                Picker(L("settings_language"), selection: $localizer.language) {
                    Text(L("settings_language_system")).tag(Localizer.Language.system)
                    Text(L("settings_language_english")).tag(Localizer.Language.english)
                    Text(L("settings_language_italian")).tag(Localizer.Language.italian)
                }
            } header: {
                Text(L("settings_language"))
            }

            Section {
                TextField("wss://relay.example.com", text: $relay)
                    .autocorrectionDisabled()
                    #if os(iOS)
                    .textInputAutocapitalization(.never)
                    .keyboardType(.URL)
                    .submitLabel(.done)
                    #endif
                    .onSubmit { model.applyRelayURL(relay) }
            } header: {
                Text(L("settings_relay"))
            } footer: {
                Text(L("settings_relay_hint"))
            }

            #if os(macOS)
            Section {
                Toggle(L("settings_launch_at_login"), isOn: Binding(
                    get: { model.settings.launchAtLogin },
                    set: { model.settings.launchAtLogin = $0 }
                ))
            }
            #endif

            Section {
                Text(model.identity?.fingerprint ?? "—")
                    .font(.system(.body, design: .monospaced))
                    .textSelection(.enabled)
            } header: {
                Text(L("settings_fingerprint"))
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .background(Theme.bg)
        .onAppear {
            name = model.settings.deviceName
            relay = model.settings.relayURL
        }
        .onDisappear {
            commit()
        }
        #if os(macOS)
        .fileImporter(isPresented: $pickingFolder, allowedContentTypes: [.folder]) { result in
            if case .success(let url) = result {
                model.applyDownloadDirectory(url)
            }
        }
        .frame(minWidth: 460, minHeight: 480)
        #else
        .navigationTitle(L("settings"))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .confirmationAction) {
                Button(L("done")) {
                    commit()
                    dismiss()
                }
            }
        }
        #endif
        .environment(\.locale, Localizer.shared.locale)
    }

    private var folderName: String {
        #if os(macOS)
        (model.settings.downloadDirectory.path as NSString).abbreviatingWithTildeInPath
        #else
        L("app_name")
        #endif
    }

    private func commit() {
        if name != model.settings.deviceName { model.applyDeviceName(name) }
        if relay != model.settings.relayURL { model.applyRelayURL(relay) }
    }
}
