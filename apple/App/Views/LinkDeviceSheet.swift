import SwiftUI
import WarpCore

/// Link My Device: show this device's code (link window open for 5 min) or
/// enter the other device's code.
struct LinkDeviceSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    enum Mode: String, CaseIterable, Identifiable {
        case show, enter
        var id: String { rawValue }
    }

    @State private var mode: Mode = .show

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.l) {
            HStack {
                Text(L("link_device"))
                    .font(Theme.Fonts.title)
                    .foregroundStyle(Theme.text)
                Spacer()
                Button(L("done")) { dismiss() }
                    .buttonStyle(QuietButtonStyle())
                    .keyboardShortcut(.cancelAction)
            }
            if case .linked(let name) = model.linkState {
                success(name)
            } else {
                Picker("", selection: $mode.animation(Theme.Motion.spring)) {
                    Text(L("link_show_code")).tag(Mode.show)
                    Text(L("link_enter_code")).tag(Mode.enter)
                }
                .pickerStyle(.segmented)
                .labelsHidden()

                switch mode {
                case .show: showCode
                case .enter: enterCode
                }
            }
            Spacer(minLength: 0)
        }
        .padding(Theme.Space.xl)
        .frame(minWidth: 380, minHeight: 460)
        .onAppear {
            model.resetLinkState()
            model.openLinkWindow()
        }
        .onDisappear { model.closeLinkWindow() }
        .onChange(of: mode) { _, new in
            if new == .show { model.openLinkWindow() } else { model.closeLinkWindow() }
        }
        .environment(\.locale, Localizer.shared.locale)
        .presentationDetents([.large])
    }

    private var showCode: some View {
        VStack(spacing: Theme.Space.m) {
            if let code = model.code {
                CodeText(text: code.display, size: 40)
                QRCodeView(payload: code.qrPayload)
                    .frame(width: 200, height: 200)
                    .id(code.qrPayload)
                    .padding(Theme.Space.s)
                    .cardSurface()
                if let expiry = model.linkWindowExpiry {
                    CountdownText(expiresAtMs: UInt64(min(expiry.timeIntervalSince1970 * 1000, Double(code.expiresAtMs))))
                }
            }
            Text(L("link_show_code_body"))
                .font(Theme.Fonts.body)
                .foregroundStyle(Theme.textSecondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 340)
        }
        .frame(maxWidth: .infinity)
    }

    private var enterCode: some View {
        VStack(alignment: .leading, spacing: Theme.Space.s) {
            Text(L("send_to_code_hint"))
                .font(Theme.Fonts.body)
                .foregroundStyle(Theme.textSecondary)
            CodeEntryField(placeholder: "000-000", large: true) { model.link(withCode: $0) }
            switch model.linkState {
            case .linking:
                HStack(spacing: Theme.Space.xs) {
                    ProgressView().controlSize(.small)
                    Text(L("state_connecting")).font(Theme.Fonts.caption).foregroundStyle(Theme.textSecondary)
                }
            case .failed(let message):
                Text(message).font(Theme.Fonts.caption).foregroundStyle(Theme.danger)
            default:
                EmptyView()
            }
        }
    }

    private func success(_ name: String) -> some View {
        VStack(spacing: Theme.Space.m) {
            Image(systemName: "checkmark.circle")
                .font(.system(size: 44, weight: .regular))
                .foregroundStyle(Theme.success)
                .transition(.scale.combined(with: .opacity))
            Text(L("link_success", name))
                .font(Theme.Fonts.bodyStrong)
                .foregroundStyle(Theme.text)
                .multilineTextAlignment(.center)
            Button(L("done")) { dismiss() }
                .buttonStyle(QuietButtonStyle(prominent: true))
                .keyboardShortcut(.defaultAction)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, Theme.Space.xxl)
    }
}
