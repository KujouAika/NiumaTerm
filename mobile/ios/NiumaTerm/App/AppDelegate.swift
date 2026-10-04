import UIKit
import UserNotifications
import os

/// The UIKit callbacks SwiftUI has no counterpart for: the APNs device token,
/// and the notification center's presentation and tap callbacks.
final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    /// Set by the app once its model exists, before any callback can need it.
    @MainActor weak var app: AppModel?

    private let log = Logger(subsystem: "io.f32.NiumaTermMobile", category: "push")

    func application(_ application: UIApplication,
                     didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        UNUserNotificationCenter.current().delegate = self

        return true
    }

    func application(_ application: UIApplication,
                     didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        let token = deviceToken.map { String(format: "%02x", $0) }.joined()

        log.info("APNs token received")

        Task { @MainActor in self.app?.pushTokenReceived(token) }
    }

    func application(_ application: UIApplication,
                     didFailToRegisterForRemoteNotificationsWithError error: Error) {
        log.error("APNs registration failed: \(error.localizedDescription, privacy: .public)")
    }

    /// Hosts push on every event, whatever the link, so a push about the
    /// agent session on screen repeats what the transcript already shows and
    /// is dropped. One about any other session is still news: show it.
    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        let info = notification.request.content.userInfo

        guard let host = info["host"] as? String, let session = info["session"] as? String else {
            return [.banner, .list, .sound]
        }

        let showing = await MainActor.run { app?.isShowingAgent(host: host, session: session) ?? false }

        return showing ? [] : [.banner, .list, .sound]
    }

    /// A tap opens the session the push is about.
    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                didReceive response: UNNotificationResponse) async {
        let info = response.notification.request.content.userInfo

        guard let host = info["host"] as? String, let session = info["session"] as? String else { return }

        await MainActor.run { app?.openFromNotification(host: host, session: session) }
    }
}
