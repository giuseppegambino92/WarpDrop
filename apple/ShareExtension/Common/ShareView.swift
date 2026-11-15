import SwiftUI
import WarpCore

/// "Send with WarpDrop": pick a device or enter a code, then live progress.
struct ShareView: View {
    @Environment(ShareModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.m) {
            header
            if let payload = model.payload {
                Text(payload.summary)
                    .font(Theme.Fonts.body)
                    .foregroundStyle(Theme.textSecondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            content
            Spacer(minLength: 0)
        }
        .padding(Theme.Space.l)
        .background(Theme.bg)
        .environment(\.locale, Localizer.shared.locale)
        .animation(Theme.Motion.spring, value: model.phase)
    }

    private var header: some View {
        HStack {
            Text(L("share_choose_device"))
                .font(Theme.Fonts.title)
                .foregroundStyle(Theme.text)
            Spacer()
            if model.phase != .done {
                Button(L("cancel")) { model.cancel() }
                    .buttonStyle(QuietButtonStyle())
                    .keyboardShortcut(.cancelAction)
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch model.phase {
        case .loading:
            HStack(spacing: Theme.Space.xs) {
                PulsingDot()
                Text(L("searching")).font(Theme.Fonts.body).foregroundStyle(Theme.textSecondary)
            }
        case .choosing:
            chooser
        case .sending:
            progress
        case .done:
            HStack(spacing: Theme.Space.xs) {
                Image(systemName: "checkmark").foregroundStyle(Theme.success)
                Text(L("notif_sent_title", model.targetName))
                    .font(Theme.Fonts.bodyStrong)
                    .foregroundStyle(Theme.text)
            }
            .transition(.opacity)
        case .failed(let message):
            VStack(alignment: .leading, spacing: Theme.Space.s) {
                Text("✗ " + message)
                    .font(Theme.Fonts.body)
                    .foregroundStyle(Theme.danger)
                    .fixedSize(horizontal: false, vertical: true)
                if model.payload != nil {
                    Button(L("share_choose_device")) { model.retry() }
                        .buttonStyle(QuietButtonStyle())
                }
            }
        }
    }

    private var chooser: some View {
        VStack(alignment: .leading, spacing: Theme.Space.m) {
            if model.devices.isEmpty {
                HStack(alignment: .top, spacing: Theme.Space.xs) {
                    PulsingDot().padding(.top, 5)
                    Text(L("share_no_devices"))
                        .font(Theme.Fonts.body)
                        .foregroundStyle(Theme.textSecondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            } else {
                ScrollView {
                    VStack(spacing: Theme.Space.xs) {
                        ForEach(model.devices) { device in
                            Button { model.send(to: device) } label: { row(device) }
                                .buttonStyle(CardPressStyleShare())
                        }
                    }
                }
                .frame(maxHeight: 300)
            }
            VStack(alignment: .leading, spacing: 4) {
                Text(L("send_to_code"))
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.textSecondary)
                CodeEntryField(placeholder: "000-000") { model.send(toCode: $0) }
            }
        }
    }

    private func row(_ device: ShareModel.Device) -> some View {
        HStack(spacing: Theme.Space.s) {
            DeviceGlyph(kind: device.kind, size: 36)
            VStack(alignment: .leading, spacing: 3) {
                Text(device.name)
                    .font(Theme.Fonts.bodyStrong)
                    .foregroundStyle(Theme.text)
                    .lineLimit(1)
                HStack(spacing: Theme.Space.xxs) {
                    PathBadge(path: device.path)
                    if device.trusted { WarpBadge(text: L("badge_trusted"), tint: Theme.success) }
                }
            }
            Spacer(minLength: 0)
            Image(systemName: "arrow.up.circle")
                .foregroundStyle(Theme.accent)
        }
        .padding(Theme.Space.s)
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardSurface()
        .contentShape(Rectangle())
    }

    @ViewBuilder
    private var progress: some View {
        VStack(alignment: .leading, spacing: Theme.Space.s) {
            Text(L("share_sending_to", model.targetName))
                .font(Theme.Fonts.bodyStrong)
                .foregroundStyle(Theme.text)
            if let t = model.transfer {
                let status = TransferStatus(snapshot: t, peer: model.targetName)
                ThinProgressBar(value: status.fraction)
                HStack(spacing: Theme.Space.xs) {
                    Text(t.state == .transferring ? status.bytesText : status.stateText)
                    if let speed = status.speedText { Text(speed) }
                    if let eta = status.etaText { Text(eta) }
                    PathBadge(path: t.path)
                }
                .font(Theme.Fonts.caption.monospacedDigit())
                .foregroundStyle(Theme.textSecondary)
                .lineLimit(1)
            } else {
                Text(L("state_connecting")).font(Theme.Fonts.caption).foregroundStyle(Theme.textSecondary)
            }
        }
    }
}

private struct CardPressStyleShare: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label.opacity(configuration.isPressed ? 0.75 : 1)
    }
}
