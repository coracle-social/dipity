import Foundation
import UserNotifications

/// Local notifications. The core decides what to say and when, and the shell
/// posts it.
///
/// Each announcement has one identifier, so a newer count replaces the last
/// rather than stacking. `docs/storage.md#notifications`.
enum Alerts {
    /// Post what the core asked for.
    static func post(_ announcement: Announcement) {
        let content = UNMutableNotificationContent()
        let identifier: String

        switch announcement {
        case .pairing:
            identifier = "pairing"
            content.title = "Somebody nearby wants to pair"
            content.body = "Open Dipity to compare shapes with them."
        case .content(let count):
            identifier = "content"
            content.title = count == 1 ? "New post on the board" : "\(count) new posts on the board"
            content.body = "Open Dipity to read."
        }

        content.sound = .default

        let request = UNNotificationRequest(identifier: identifier, content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request)
    }

    /// The user is looking, so what was announced has been seen.
    static func clear() {
        UNUserNotificationCenter.current().removeAllDeliveredNotifications()
    }

    /// Whether the user has let the app notify: `granted`, `denied` or `prompt`.
    static func permission(_ answer: @escaping (String) -> Void) {
        UNUserNotificationCenter.current().getNotificationSettings { settings in
            DispatchQueue.main.async { answer(name(settings.authorizationStatus)) }
        }
    }

    /// Ask the user, if they have not been asked, and answer what they said.
    static func request(_ answer: @escaping (String) -> Void) {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { _, _ in
            permission(answer)
        }
    }

    private static func name(_ status: UNAuthorizationStatus) -> String {
        switch status {
        case .notDetermined: return "prompt"
        case .denied: return "denied"
        default: return "granted"
        }
    }
}
