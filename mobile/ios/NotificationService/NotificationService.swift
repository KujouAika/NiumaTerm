import CryptoKit
import Foundation
import Security
import UserNotifications

/// Replaces a push's placeholder text with what the host sealed for this
/// phone. The relay and APNs carry only `h` (the host id) and `s` (the
/// sealed text); the key to open `s` is the one the app registered with that
/// host and keeps in the shared keychain group.
///
/// Anything that goes wrong leaves the placeholder, so a push is never lost,
/// only less specific. The extension links none of the app's Rust core: it
/// runs under a tight memory limit and needs nothing but CryptoKit.
final class NotificationService: UNNotificationServiceExtension {
    private var handler: ((UNNotificationContent) -> Void)?
    private var content: UNMutableNotificationContent?

    override func didReceive(_ request: UNNotificationRequest,
                             withContentHandler handler: @escaping (UNNotificationContent) -> Void) {
        let content = (request.content.mutableCopy() as? UNMutableNotificationContent)
            ?? UNMutableNotificationContent()
        self.handler = handler
        self.content = content

        if let host = content.userInfo["h"] as? String,
           let sealed = content.userInfo["s"] as? String,
           let message = SealedPush.open(sealed, host: host) {
            content.title = message.title
            content.body = message.body
            // One thread per session, and what a tap opens.
            content.threadIdentifier = "\(host)/\(message.session)"
            content.userInfo["host"] = host
            content.userInfo["session"] = message.session
        }

        handler(content)
    }

    override func serviceExtensionTimeWillExpire() {
        if let handler, let content {
            handler(content)
        }
    }
}

/// The text a host sealed: the JSON `PushMessage` of `nmt_remote_core::push`.
struct SealedPush: Decodable {
    let v: Int
    let host: String
    let session: String
    let kind: String
    let title: String
    let body: String
    let at: UInt64

    /// `sealed` is `base64(nonce || ciphertext || tag)`, ChaCha20-Poly1305 with
    /// the host id as associated data, which is CryptoKit's combined form.
    static func open(_ sealed: String, host: String) -> SealedPush? {
        guard let data = Data(base64Encoded: sealed),
              let key = PushKeyReader.key(for: host),
              let box = try? ChaChaPoly.SealedBox(combined: data),
              let plaintext = try? ChaChaPoly.open(box, using: SymmetricKey(data: key),
                                                   authenticating: Data(host.utf8)),
              let message = try? JSONDecoder().decode(SealedPush.self, from: plaintext),
              message.host == host
        else { return nil }
        return message
    }
}

/// Reads the push key the app stored for a host. The app's `PushKeys` writes
/// them with the same service and group.
enum PushKeyReader {
    static func key(for host: String) -> Data? {
        guard let group = Bundle.main.object(forInfoDictionaryKey: "NMTPushKeyGroup") as? String else {
            return nil
        }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "push",
            kSecAttrAccount as String: host,
            kSecAttrAccessGroup as String: group,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
              let key = result as? Data, key.count == 32
        else { return nil }
        return key
    }
}
