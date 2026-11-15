import ActivityKit
import Foundation
import WarpCore

/// Mirrors transfers into Live Activities / the Dynamic Island: one activity
/// per running transfer, updated at most once per second (state changes are
/// pushed immediately), ended with the final state.
@MainActor
final class LiveActivityController {
    /// Transfer id → ActivityKit activity id. (`Activity` is not `Sendable`;
    /// the async update/end calls look it up off the main actor by id.)
    private var activities: [String: String] = [:]
    private var lastUpdate: [String: Date] = [:]
    private var lastPhase: [String: TransferActivityAttributes.ContentState.Phase] = [:]
    private var ended: Set<String> = []

    func update(_ t: TransferSnapshot, peer: String, title: String) {
        guard !ended.contains(t.id) else { return }
        let status = TransferStatus(snapshot: t, peer: peer)
        let state = contentState(t, status: status)

        if t.state.isTerminal {
            ended.insert(t.id)
            guard let activityID = activities.removeValue(forKey: t.id) else { return }
            lastUpdate[t.id] = nil
            lastPhase[t.id] = nil
            let content = ActivityContent(state: state, staleDate: nil)
            Task { await Self.end(activityID, content: content) }
            return
        }

        if let activityID = activities[t.id] {
            let now = Date()
            let phaseChanged = lastPhase[t.id] != state.phase
            guard phaseChanged || now.timeIntervalSince(lastUpdate[t.id] ?? .distantPast) >= 1 else { return }
            lastUpdate[t.id] = now
            lastPhase[t.id] = state.phase
            let content = ActivityContent(state: state, staleDate: now.addingTimeInterval(30))
            Task { await Self.update(activityID, content: content) }
            return
        }

        // Only bytes actually moving deserve a Live Activity.
        guard t.state == .transferring, t.totalBytes > 0,
              ActivityAuthorizationInfo().areActivitiesEnabled
        else { return }
        let attributes = TransferActivityAttributes(transferID: t.id, title: title, peerName: peer, isOutgoing: t.direction == .outgoing)
        do {
            let activity = try Activity.request(attributes: attributes, content: ActivityContent(state: state, staleDate: Date().addingTimeInterval(30)))
            activities[t.id] = activity.id
            lastUpdate[t.id] = Date()
            lastPhase[t.id] = state.phase
        } catch {
            // Activities disabled or limit reached: the in-app HUD still shows progress.
        }
    }

    private nonisolated static func update(_ id: String, content: ActivityContent<TransferActivityAttributes.ContentState>) async {
        guard let activity = Activity<TransferActivityAttributes>.activities.first(where: { $0.id == id }) else { return }
        await activity.update(content)
    }

    private nonisolated static func end(_ id: String, content: ActivityContent<TransferActivityAttributes.ContentState>) async {
        guard let activity = Activity<TransferActivityAttributes>.activities.first(where: { $0.id == id }) else { return }
        await activity.end(content, dismissalPolicy: .after(.now + 8))
    }

    private func contentState(_ t: TransferSnapshot, status: TransferStatus) -> TransferActivityAttributes.ContentState {
        let phase: TransferActivityAttributes.ContentState.Phase = switch t.state {
        case .completed: .completed
        case .failed, .cancelled: .failed
        case .paused: .paused
        default: .active
        }
        let detail = t.state == .transferring ? (status.etaText ?? WarpFormat.percent(status.fraction)) : status.stateText
        return .init(
            phase: phase,
            fraction: status.fraction,
            bytesText: status.bytesText,
            speedText: status.speedText ?? "",
            detailText: detail,
            pathText: status.pathText ?? "",
            pathIsCable: t.path?.isCable ?? false
        )
    }
}
