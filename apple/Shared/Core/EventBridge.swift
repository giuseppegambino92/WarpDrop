import Foundation
import WarpCore

/// Receives engine events on the core's worker thread and forwards them, in
/// order, to a MainActor handler.
final class EventBridge: WarpEventListener {
    private let deliver: @Sendable (WarpEvent) -> Void

    /// `handler` runs on the main actor; events keep their engine order
    /// because the main queue is FIFO.
    init(_ handler: @escaping @MainActor @Sendable (WarpEvent) -> Void) {
        deliver = { event in
            DispatchQueue.main.async {
                MainActor.assumeIsolated { handler(event) }
            }
        }
    }

    func onEvent(event: WarpEvent) {
        deliver(event)
    }
}
