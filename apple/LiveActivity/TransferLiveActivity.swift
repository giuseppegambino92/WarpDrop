import ActivityKit
import SwiftUI
import WidgetKit

@main
struct WarpDropWidgets: WidgetBundle {
    var body: some Widget {
        TransferLiveActivity()
    }
}

/// Lock Screen banner + Dynamic Island for a running transfer: progress,
/// speed, peer name and the connection (path) badge.
struct TransferLiveActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: TransferActivityAttributes.self) { context in
            LockScreenView(attributes: context.attributes, state: context.state)
                .padding(16)
                .activityBackgroundTint(Theme.surface)
                .activitySystemActionForegroundColor(Theme.text)
        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    Image(systemName: context.attributes.isOutgoing ? "arrow.up.circle.fill" : "arrow.down.circle.fill")
                        .font(.system(size: 22))
                        .foregroundStyle(tint(context.state))
                        .padding(.leading, 4)
                }
                DynamicIslandExpandedRegion(.trailing) {
                    Text(context.state.speedText.isEmpty ? percent(context.state.fraction) : context.state.speedText)
                        .font(.system(size: 14, weight: .semibold).monospacedDigit())
                        .foregroundStyle(.white)
                        .padding(.trailing, 4)
                }
                DynamicIslandExpandedRegion(.center) {
                    Text(headline(context.attributes))
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(.white)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    VStack(alignment: .leading, spacing: 6) {
                        ProgressBar(fraction: context.state.fraction, tint: tint(context.state), track: Color.white.opacity(0.2))
                        HStack(spacing: 6) {
                            Text(context.state.bytesText)
                            Spacer(minLength: 4)
                            Text(context.state.detailText).lineLimit(1)
                            if !context.state.pathText.isEmpty {
                                PathPill(text: context.state.pathText, cable: context.state.pathIsCable, onDark: true)
                            }
                        }
                        .font(.system(size: 12, weight: .medium).monospacedDigit())
                        .foregroundStyle(Color.white.opacity(0.75))
                    }
                    .padding(.horizontal, 4)
                }
            } compactLeading: {
                Image(systemName: context.attributes.isOutgoing ? "arrow.up" : "arrow.down")
                    .font(.system(size: 13, weight: .bold))
                    .foregroundStyle(tint(context.state))
            } compactTrailing: {
                ProgressRing(fraction: context.state.fraction, tint: tint(context.state))
                    .frame(width: 18, height: 18)
            } minimal: {
                ProgressRing(fraction: context.state.fraction, tint: tint(context.state))
                    .frame(width: 18, height: 18)
            }
            .keylineTint(Theme.accent)
        }
    }

    private func headline(_ a: TransferActivityAttributes) -> String {
        "\(a.title) \(a.isOutgoing ? "→" : "←") \(a.peerName)"
    }

    private func tint(_ s: TransferActivityAttributes.ContentState) -> Color {
        switch s.phase {
        case .completed: Theme.success
        case .failed: Theme.danger
        case .paused: Theme.textSecondary
        case .active: Theme.accent
        }
    }

    private func percent(_ f: Double) -> String {
        "\(Int((f * 100).rounded(.down)))%"
    }
}

private struct LockScreenView: View {
    let attributes: TransferActivityAttributes
    let state: TransferActivityAttributes.ContentState

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: attributes.isOutgoing ? "arrow.up.circle.fill" : "arrow.down.circle.fill")
                    .foregroundStyle(tint)
                Text("\(attributes.title) \(attributes.isOutgoing ? "→" : "←") \(attributes.peerName)")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(Theme.text)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 4)
                if !state.pathText.isEmpty {
                    PathPill(text: state.pathText, cable: state.pathIsCable, onDark: false)
                }
            }
            ProgressBar(fraction: state.fraction, tint: tint, track: Theme.border)
            HStack(spacing: 8) {
                Text(state.bytesText)
                if !state.speedText.isEmpty { Text(state.speedText) }
                Spacer(minLength: 4)
                Text(state.detailText).lineLimit(1)
            }
            .font(.system(size: 12, weight: .medium).monospacedDigit())
            .foregroundStyle(Theme.textSecondary)
        }
    }

    private var tint: Color {
        switch state.phase {
        case .completed: Theme.success
        case .failed: Theme.danger
        case .paused: Theme.textSecondary
        case .active: Theme.accent
        }
    }
}

/// 4 pt bar — engine values only.
private struct ProgressBar: View {
    let fraction: Double
    let tint: Color
    let track: Color

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(track)
                Capsule().fill(tint).frame(width: max(0, min(1, fraction)) * geo.size.width)
            }
        }
        .frame(height: 4)
    }
}

private struct ProgressRing: View {
    let fraction: Double
    let tint: Color

    var body: some View {
        ZStack {
            Circle().stroke(Color.white.opacity(0.25), lineWidth: 2.5)
            Circle()
                .trim(from: 0, to: max(0.02, min(1, fraction)))
                .stroke(tint, style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                .rotationEffect(.degrees(-90))
        }
    }
}

private struct PathPill: View {
    let text: String
    let cable: Bool
    let onDark: Bool

    var body: some View {
        let color: Color = cable ? Theme.cable : (onDark ? Color.white.opacity(0.75) : Theme.textSecondary)
        Text(text)
            .font(.system(size: 11, weight: .medium))
            .foregroundStyle(color)
            .lineLimit(1)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(color.opacity(0.14)))
    }
}
