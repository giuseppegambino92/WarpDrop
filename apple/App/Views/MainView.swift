import SwiftUI
import UniformTypeIdentifiers
import WarpCore

/// The one screen: header, code bar, devices, transfers.
struct MainView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.horizontalSizeClass) private var sizeClass

    /// Phone layout (vertical code bar, device rows) vs. desktop / iPad grid.
    private var isCompact: Bool {
        #if os(iOS)
        sizeClass == .compact
        #else
        false
        #endif
    }

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            HeaderView(compact: isCompact)
            Rectangle().fill(Theme.border).frame(height: 1)
            CodeBarView(compact: isCompact)
            Rectangle().fill(Theme.border).frame(height: 1)
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            if !model.transfers.isEmpty {
                TransferHUD(compact: isCompact)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
        .animation(Theme.Motion.spring, value: model.transfers.isEmpty)
        .onDrop(of: ItemLoader.acceptedTypes, isTargeted: $model.isDragOverWindow) { providers in
            model.handleDrop(providers, deviceID: nil)
            return true
        }
        .overlay(alignment: .bottom) { ToastView() }
        .modifier(ImporterModifier())
        .sheet(item: offerBinding) { item in
            OfferSheet(offer: item.offer)
                .interactiveDismissDisabled()
        }
        .sheet(isPresented: $model.isLinkSheetPresented) {
            LinkDeviceSheet()
        }
        .environment(\.locale, Localizer.shared.locale)
    }

    @ViewBuilder
    private var content: some View {
        switch model.tab {
        case .myDevices, .nearby:
            DeviceListView(compact: isCompact)
        case .quickCode:
            QuickCodeView(compact: isCompact)
        }
    }

    private var offerBinding: Binding<OfferItem?> {
        Binding(
            get: { model.offers.first.map(OfferItem.init) },
            set: { _ in }
        )
    }
}

struct OfferItem: Identifiable {
    let offer: IncomingOffer
    var id: String { offer.offerId }
}

/// File importer driven by `AppModel.importerRequest`.
struct ImporterModifier: ViewModifier {
    @Environment(AppModel.self) private var model

    func body(content: Content) -> some View {
        let request = model.importerRequest
        content.fileImporter(
            isPresented: Binding(
                get: { model.importerRequest != nil },
                set: { if !$0 { model.importerRequest = nil } }
            ),
            allowedContentTypes: request?.kind == .folder ? [.folder] : allowedTypes,
            allowsMultipleSelection: true
        ) { result in
            guard let request else { return }
            if case .success(let urls) = result {
                model.sendPicked(urls, to: request.target)
            }
            model.importerRequest = nil
        }
    }

    private var allowedTypes: [UTType] {
        #if os(macOS)
        [.item, .folder]
        #else
        [.item]
        #endif
    }
}

// MARK: - Header

struct HeaderView: View {
    @Environment(AppModel.self) private var model
    let compact: Bool

    var body: some View {
        @Bindable var model = model
        Group {
            if compact {
                VStack(alignment: .leading, spacing: Theme.Space.s) {
                    HStack {
                        Text(L("app_name")).font(Theme.Fonts.title).foregroundStyle(Theme.text)
                        Spacer()
                        actions
                    }
                    tabs($model.tab)
                }
            } else {
                HStack(spacing: Theme.Space.m) {
                    Text(L("app_name")).font(Theme.Fonts.title).foregroundStyle(Theme.text)
                    Spacer(minLength: Theme.Space.m)
                    tabs($model.tab).frame(maxWidth: 420)
                    Spacer(minLength: Theme.Space.m)
                    actions
                }
            }
        }
        .padding(.horizontal, Theme.Space.window)
        .padding(.vertical, Theme.Space.s)
    }

    private func tabs(_ selection: Binding<MainTab>) -> some View {
        Picker("", selection: selection.animation(Theme.Motion.spring)) {
            ForEach(MainTab.allCases) { tab in
                Text(L(tab.titleKey)).tag(tab)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
    }

    private var actions: some View {
        HStack(spacing: Theme.Space.xxs) {
            #if os(iOS)
            PasteButton(supportedContentTypes: [.fileURL, .image, .url, .plainText]) { providers in
                Task { @MainActor in
                    if let payload = await ItemLoader.payload(from: providers) {
                        model.sendPasted(payload)
                    }
                }
            }
            .labelStyle(.iconOnly)
            .buttonBorderShape(.roundedRectangle(radius: Theme.Radius.control))
            .tint(Theme.accent)
            .accessibilityLabel(L("send_clipboard"))
            #endif
            Button { model.presentLinkSheet() } label: {
                Image(systemName: "plus")
            }
            .buttonStyle(IconButtonStyle())
            .help(L("link_device"))
            .accessibilityLabel(L("link_device"))

            Button { model.isSettingsPresented = true } label: {
                Image(systemName: "gearshape")
            }
            .buttonStyle(IconButtonStyle())
            .help(L("settings"))
            .accessibilityLabel(L("settings"))
        }
    }
}

// MARK: - Toast

struct ToastView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if let toast = model.toast {
            HStack(spacing: Theme.Space.xs) {
                Image(systemName: toast.isError ? "exclamationmark.circle" : "checkmark")
                    .foregroundStyle(toast.isError ? Theme.danger : Theme.success)
                Text(toast.message)
                    .font(Theme.Fonts.body)
                    .foregroundStyle(Theme.text)
                    .lineLimit(3)
            }
            .padding(.horizontal, Theme.Space.m)
            .padding(.vertical, Theme.Space.s)
            .cardSurface(radius: Theme.Radius.card)
            .padding(.bottom, model.transfers.isEmpty ? Theme.Space.l : 96)
            .padding(.horizontal, Theme.Space.window)
            .transition(.move(edge: .bottom).combined(with: .opacity))
            .id(toast.id)
            .onTapGesture { model.toast = nil }
            .accessibilityAddTraits(.isStaticText)
        }
    }
}
