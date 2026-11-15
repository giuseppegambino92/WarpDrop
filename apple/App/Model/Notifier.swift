import Foundation
import UserNotifications

/// Local notifications for received files / clipboard.
@MainActor
final class Notifier {
    nonisolated static let pathsKey = "paths"
    private let delegate = NotificationDelegate()

    func start() {
        let center = UNUserNotificationCenter.current()
        center.delegate = delegate
        center.requestAuthorization(options: [.alert, .sound]) { @Sendable _, _ in }
    }

    func post(id: String, title: String, body: String, paths: [String] = []) {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.sound = .default
        content.threadIdentifier = "transfers"
        if !paths.isEmpty {
            content.userInfo = [Self.pathsKey: paths]
        }
        let request = UNNotificationRequest(identifier: id, content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request)
    }
}

/// Shows banners while the app is in front and reveals received files when a
/// notification is clicked.
private final class NotificationDelegate: NSObject, UNUserNotificationCenterDelegate, Sendable {
    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        [.banner, .list, .sound]
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        let paths = response.notification.request.content.userInfo[Notifier.pathsKey] as? [String] ?? []
        guard !paths.isEmpty else { return }
        await MainActor.run { PlatformServices.reveal(paths) }
    }
}
