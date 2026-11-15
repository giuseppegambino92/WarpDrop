import SwiftUI
import WarpCore

/// My Devices / Nearby: a grid of cards (desktop, iPad) or rows (phone).
struct DeviceListView: View {
    @Environment(AppModel.self) private var model
    let compact: Bool

    private var devices: [DeviceItem] {
        model.tab == .nearby ? model.nearbyDevices : model.myDevices
    }

    private var isDragActive: Bool {
        model.isDragOverWindow || model.dropTargetDeviceID != nil
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Theme.Space.m) {
                if let staged = model.staged {
                    StagedBanner(payload: staged)
                }
                if devices.isEmpty {
                    emptyState
                        .frame(maxWidth: .infinity)
                        .padding(.top, Theme.Space.xxl)
                } else if compact {
                    LazyVStack(spacing: Theme.Space.xs) {
                        ForEach(devices) { DeviceCard(device: $0, isDragActive: isDragActive, compact: true) }
                    }
                } else {
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 200, maximum: 320), spacing: Theme.Space.s)], alignment: .leading, spacing: Theme.Space.s) {
                        ForEach(devices) { DeviceCard(device: $0, isDragActive: isDragActive, compact: false) }
                    }
                }
                if !devices.isEmpty {
                    Text(L(isDragActive ? "drop_on_device" : "drop_hint"))
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(isDragActive ? Theme.accent : Theme.textTertiary)
                        .padding(.top, Theme.Space.xs)
                        .animation(Theme.Motion.spring, value: isDragActive)
                }
            }
            .padding(Theme.Space.window)
            .animation(Theme.Motion.spring, value: devices)
        }
        .scrollContentBackground(.hidden)
    }

    @ViewBuilder
    private var emptyState: some View {
        VStack(spacing: Theme.Space.m) {
            if model.tab == .nearby {
                HStack(spacing: Theme.Space.xs) {
                    PulsingDot()
                    Text(L("searching")).font(Theme.Fonts.bodyStrong).foregroundStyle(Theme.text)
                }
                Text(L("no_nearby"))
                    .font(Theme.Fonts.body)
                    .foregroundStyle(Theme.textSecondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 380)
            } else {
                Image(systemName: "person.crop.circle.badge.checkmark")
                    .font(.system(size: 28, weight: .regular))
                    .foregroundStyle(Theme.textTertiary)
                Text(L("no_my_devices"))
                    .font(Theme.Fonts.body)
                    .foregroundStyle(Theme.textSecondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 380)
                Button(L("link_device")) { model.presentLinkSheet() }
                    .buttonStyle(QuietButtonStyle(prominent: true))
            }
            Text(L("drop_hint"))
                .font(Theme.Fonts.caption)
                .foregroundStyle(Theme.textTertiary)
                .padding(.top, Theme.Space.xs)
        }
    }
}

/// "3 items ready to send — pick a device or enter a code"
struct StagedBanner: View {
    @Environment(AppModel.self) private var model
    let payload: SendPayload

    var body: some View {
        HStack(spacing: Theme.Space.s) {
            Image(systemName: "tray.and.arrow.up")
                .foregroundStyle(Theme.accent)
            Text(L("staged_items", payload.summary))
                .font(Theme.Fonts.body)
                .foregroundStyle(Theme.text)
                .lineLimit(2)
            Spacer(minLength: Theme.Space.xs)
            Button(L("cancel")) { model.clearStaged() }
                .buttonStyle(QuietButtonStyle())
        }
        .padding(Theme.Space.s)
        .cardSurface(fill: Theme.accentSoft, border: Theme.accent.opacity(0.35))
        .transition(.move(edge: .top).combined(with: .opacity))
    }
}

/// One device: glyph, name, connection + trust badges. Click → file picker
/// (or send staged items); drop → send; context menu → clipboard / remove.
struct DeviceCard: View {
    @Environment(AppModel.self) private var model
    let device: DeviceItem
    let isDragActive: Bool
    let compact: Bool
    @State private var isTargeted = false
    @State private var isHovering = false

    private var highlighted: Bool { (isDragActive || isTargeted) && device.online }
    private var isSelected: Bool { model.selectedDeviceID == device.id && device.online }
    private var isNew: Bool { model.recentlyLinkedID == device.id }

    var body: some View {
        Button {
            model.activate(device)
        } label: {
            HStack(spacing: Theme.Space.s) {
                DeviceGlyph(kind: device.kind, size: compact ? 40 : 48)
                VStack(alignment: .leading, spacing: 4) {
                    Text(device.name)
                        .font(Theme.Fonts.bodyStrong)
                        .foregroundStyle(Theme.text)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    badges
                }
                Spacer(minLength: 0)
                if compact {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(Theme.textTertiary)
                }
            }
            .padding(compact ? Theme.Space.s : Theme.Space.m)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(RoundedRectangle(cornerRadius: Theme.Radius.card))
        }
        .buttonStyle(CardPressStyle())
        .cardSurface(fill: fill, border: border, lineWidth: highlighted || isSelected ? 1.5 : 1)
        .opacity(device.online ? 1 : 0.5)
        .scaleEffect(isTargeted && device.online ? 1.04 : (highlighted ? 1.02 : 1))
        .animation(Theme.Motion.spring, value: highlighted)
        .animation(Theme.Motion.spring, value: isTargeted)
        .transition(.scale(scale: 0.96).combined(with: .opacity))
        .onDrop(of: ItemLoader.acceptedTypes, isTargeted: $isTargeted) { providers in
            guard device.online else { return false }
            model.handleDrop(providers, deviceID: device.id)
            return true
        }
        .onChange(of: isTargeted) { _, targeted in
            if targeted {
                model.dropTargetDeviceID = device.id
            } else if model.dropTargetDeviceID == device.id {
                model.dropTargetDeviceID = nil
            }
        }
        #if os(macOS)
        .onHover { isHovering = $0 }
        #endif
        .contextMenu { menu }
        .accessibilityElement(children: .combine)
        .accessibilityHint(L("drop_on_device"))
    }

    private var fill: Color {
        if highlighted || isNew { return Theme.accentSoft }
        return isHovering ? Theme.surfaceHover : Theme.surface
    }

    private var border: Color {
        highlighted || isSelected ? Theme.accent : Theme.border
    }

    @ViewBuilder
    private var badges: some View {
        HStack(spacing: Theme.Space.xxs) {
            if device.online {
                PathBadge(path: device.path)
            } else {
                WarpBadge(text: L("not_nearby"), tint: Theme.textTertiary)
            }
            if device.trusted {
                WarpBadge(text: L("badge_trusted"), tint: Theme.success)
            }
        }
    }

    @ViewBuilder
    private var menu: some View {
        if device.online {
            Button(L("send_files")) { model.activate(device, kind: .files) }
            #if os(iOS)
            Button(L("send_folder")) { model.activate(device, kind: .folder) }
            #endif
            Button(L("send_clipboard")) { model.sendClipboard(to: device.id) }
        }
        if device.trusted {
            Divider()
            Button(L("remove_device"), role: .destructive) { model.removeDevice(device.id) }
        }
    }
}

/// Subtle press feedback for cards (no bounce, no shadow).
struct CardPressStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .opacity(configuration.isPressed ? 0.75 : 1)
    }
}
