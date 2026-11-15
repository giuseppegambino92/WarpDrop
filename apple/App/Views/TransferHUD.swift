import SwiftUI
import WarpCore

/// Live transfers pinned to the bottom of the window.
struct TransferHUD: View {
    @Environment(AppModel.self) private var model
    let compact: Bool

    var body: some View {
        VStack(spacing: 0) {
            Rectangle().fill(Theme.border).frame(height: 1)
            if model.hasFinishedTransfers {
                HStack {
                    Spacer()
                    Button(L("clear_finished")) { model.clearFinished() }
                        .buttonStyle(.plain)
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.textSecondary)
                }
                .padding(.horizontal, Theme.Space.window)
                .padding(.top, Theme.Space.xs)
            }
            ScrollView {
                VStack(spacing: 0) {
                    ForEach(model.transfers, id: \.id) { transfer in
                        TransferRow(transfer: transfer, compact: compact)
                        if transfer.id != model.transfers.last?.id {
                            Rectangle().fill(Theme.border).frame(height: 1).padding(.leading, Theme.Space.window)
                        }
                    }
                }
            }
            .frame(maxHeight: compact ? 260 : 196)
            .fixedSize(horizontal: false, vertical: model.transfers.count <= 2)
        }
        .background(Theme.surface.opacity(0.92))
    }
}

struct TransferRow: View {
    @Environment(AppModel.self) private var model
    let transfer: TransferSnapshot
    let compact: Bool

    private var status: TransferStatus {
        TransferStatus(snapshot: transfer, peer: model.displayPeer(transfer))
    }

    private var arrow: String { transfer.direction == .outgoing ? "→" : "←" }

    var body: some View {
        let status = status
        HStack(alignment: .center, spacing: Theme.Space.s) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: Theme.Space.xs) {
                    Text("\(model.displayTitle(transfer)) \(arrow) \(status.peer)")
                        .font(Theme.Fonts.bodyStrong)
                        .foregroundStyle(Theme.text)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: Theme.Space.xs)
                    if transfer.state == .transferring {
                        Text(WarpFormat.percent(status.fraction))
                            .font(Theme.Fonts.caption.monospacedDigit())
                            .foregroundStyle(Theme.textSecondary)
                    } else {
                        stateLabel(status)
                    }
                }
                if transfer.state != .failed && transfer.state != .cancelled {
                    ThinProgressBar(value: status.fraction, tint: transfer.state == .completed ? Theme.success : Theme.accent)
                }
                HStack(spacing: Theme.Space.xs) {
                    Text(status.bytesText)
                    if let speed = status.speedText { Text(speed) }
                    if let eta = status.etaText { Text(eta) }
                    PathBadge(path: transfer.path)
                        .animation(Theme.Motion.spring, value: transfer.path)
                }
                .font(Theme.Fonts.caption.monospacedDigit())
                .foregroundStyle(Theme.textSecondary)
                .lineLimit(1)
            }
            controls
        }
        .padding(.horizontal, Theme.Space.window)
        .padding(.vertical, Theme.Space.s)
        .contentShape(Rectangle())
        .onTapGesture {
            if transfer.state == .completed, !transfer.savedPaths.isEmpty {
                PlatformServices.reveal(transfer.savedPaths)
            }
        }
    }

    @ViewBuilder
    private func stateLabel(_ status: TransferStatus) -> some View {
        let tint: Color = switch transfer.state {
        case .completed: Theme.success
        case .failed: Theme.danger
        default: Theme.textSecondary
        }
        Text(status.stateText)
            .font(Theme.Fonts.caption)
            .foregroundStyle(tint)
            .lineLimit(2)
            .multilineTextAlignment(.trailing)
    }

    @ViewBuilder
    private var controls: some View {
        HStack(spacing: 2) {
            if transfer.state.isActive {
                if transfer.direction == .outgoing {
                    if transfer.state == .paused {
                        Button { model.resume(transfer.id) } label: { Image(systemName: "play.fill") }
                            .buttonStyle(IconButtonStyle())
                            .help(L("resume"))
                            .accessibilityLabel(L("resume"))
                    } else if transfer.state == .transferring {
                        Button { model.pause(transfer.id) } label: { Image(systemName: "pause.fill") }
                            .buttonStyle(IconButtonStyle())
                            .help(L("pause"))
                            .accessibilityLabel(L("pause"))
                    }
                }
                Button { model.cancel(transfer.id) } label: { Image(systemName: "xmark") }
                    .buttonStyle(IconButtonStyle())
                    .help(L("cancel"))
                    .accessibilityLabel(L("cancel"))
            } else if transfer.state == .completed, !transfer.savedPaths.isEmpty {
                Button { PlatformServices.reveal(transfer.savedPaths) } label: {
                    #if os(macOS)
                    Image(systemName: "magnifyingglass")
                    #else
                    Image(systemName: "folder")
                    #endif
                }
                .buttonStyle(IconButtonStyle())
                #if os(macOS)
                .help(L("show_in_folder"))
                .accessibilityLabel(L("show_in_folder"))
                #else
                .accessibilityLabel(L("open_in_files"))
                #endif
            }
        }
    }
}
