import SwiftUI
import WarpCore

/// Menu-bar popover: online devices as drop targets, your code, live
/// transfers and the app commands.
struct RadarView: View {
    @Environment(AppModel.self) private var model
    var onSent: () -> Void
    var onOpenApp: () -> Void
    var onSettings: () -> Void
    var onQuit: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Rectangle().fill(Theme.border).frame(height: 1)
            devices
            if let active = model.transfers.first(where: { $0.state.isActive }) {
                Rectangle().fill(Theme.border).frame(height: 1)
                MiniTransferRow(transfer: active)
            }
            Rectangle().fill(Theme.border).frame(height: 1)
            footer
        }
        .frame(width: 320)
        .environment(\.locale, Localizer.shared.locale)
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 2) {
                Text(L("your_code")).font(Theme.Fonts.caption).foregroundStyle(Theme.textSecondary)
                CodeText(text: model.code?.display ?? "———", size: 22)
            }
            Spacer()
            Button {
                if let code = model.code {
                    PlatformServices.copy(code.display)
                    model.show(L("code_copied"))
                }
            } label: { Image(systemName: "doc.on.doc") }
            .buttonStyle(IconButtonStyle())
            .help(L("copy_code"))
        }
        .padding(Theme.Space.m)
    }

    @ViewBuilder
    private var devices: some View {
        let list = model.onlineDevices
        VStack(alignment: .leading, spacing: Theme.Space.xs) {
            if let staged = model.staged {
                HStack(spacing: Theme.Space.xs) {
                    Text(L("staged_items", staged.summary))
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.accent)
                        .lineLimit(2)
                    Spacer(minLength: 0)
                    Button { model.clearStaged() } label: { Image(systemName: "xmark") }
                        .buttonStyle(IconButtonStyle())
                        .help(L("cancel"))
                }
            } else {
                Text(L(list.isEmpty ? "searching" : "drop_on_device"))
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.textSecondary)
            }
            if list.isEmpty {
                HStack(spacing: Theme.Space.xs) {
                    PulsingDot()
                    Text(L("no_nearby"))
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.textTertiary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.vertical, Theme.Space.s)
            } else {
                ScrollView {
                    VStack(spacing: Theme.Space.xxs) {
                        ForEach(list) { device in
                            RadarRow(device: device, onSent: onSent)
                        }
                    }
                }
                .frame(maxHeight: 280)
                .fixedSize(horizontal: false, vertical: list.count <= 4)
            }
        }
        .padding(Theme.Space.s)
        .animation(Theme.Motion.spring, value: list)
    }

    private var footer: some View {
        HStack(spacing: Theme.Space.xs) {
            Button(L("menu_open"), action: onOpenApp)
                .buttonStyle(QuietButtonStyle())
            Spacer()
            Button(action: onSettings) { Image(systemName: "gearshape") }
                .buttonStyle(IconButtonStyle())
                .help(L("settings"))
            Button(action: onQuit) { Image(systemName: "power") }
                .buttonStyle(IconButtonStyle())
                .help(L("menu_quit"))
        }
        .padding(Theme.Space.s)
    }
}

/// A device in the radar: click sends staged items / picks files; drop sends.
private struct RadarRow: View {
    @Environment(AppModel.self) private var model
    let device: DeviceItem
    var onSent: () -> Void
    @State private var isTargeted = false
    @State private var isHovering = false
    @State private var sent = false

    var body: some View {
        Button {
            let hadStaged = model.staged != nil
            model.activate(device)
            if hadStaged { confirm() } else { (NSApp.delegate as? AppDelegate)?.showMainWindow() }
        } label: {
            HStack(spacing: Theme.Space.s) {
                DeviceGlyph(kind: device.kind, size: 32)
                VStack(alignment: .leading, spacing: 2) {
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
                if sent {
                    Image(systemName: "checkmark")
                        .foregroundStyle(Theme.success)
                        .transition(.scale.combined(with: .opacity))
                }
            }
            .padding(Theme.Space.xs)
            .contentShape(Rectangle())
        }
        .buttonStyle(CardPressStyle())
        .background(
            RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous)
                .fill(isTargeted ? Theme.accentSoft : (isHovering ? Theme.surfaceHover : Color.clear))
        )
        .overlay(
            RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous)
                .strokeBorder(isTargeted ? Theme.accent : Color.clear, lineWidth: 1.5)
        )
        .scaleEffect(isTargeted ? 1.02 : 1)
        .animation(Theme.Motion.spring, value: isTargeted)
        .onHover { isHovering = $0 }
        .onDrop(of: ItemLoader.acceptedTypes, isTargeted: $isTargeted) { providers in
            model.handleDrop(providers, deviceID: device.id)
            confirm()
            return true
        }
        .contextMenu {
            Button(L("send_clipboard")) {
                model.sendClipboard(to: device.id)
                confirm()
            }
        }
    }

    private func confirm() {
        withAnimation(Theme.Motion.spring) { sent = true }
        onSent()
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
            sent = false
        }
    }
}

/// The most recent active transfer, compact.
private struct MiniTransferRow: View {
    @Environment(AppModel.self) private var model
    let transfer: TransferSnapshot

    var body: some View {
        let status = TransferStatus(snapshot: transfer, peer: model.displayPeer(transfer))
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("\(model.displayTitle(transfer)) \(transfer.direction == .outgoing ? "→" : "←") \(status.peer)")
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.text)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer()
                Text(transfer.state == .transferring ? (status.speedText ?? WarpFormat.percent(status.fraction)) : status.stateText)
                    .font(Theme.Fonts.caption.monospacedDigit())
                    .foregroundStyle(Theme.textSecondary)
                    .lineLimit(1)
            }
            ThinProgressBar(value: status.fraction)
        }
        .padding(Theme.Space.s)
    }
}
