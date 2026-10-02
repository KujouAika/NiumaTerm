import Foundation
import Security

/// The key each host seals this phone's pushes with. The app creates one per
/// host, hands it to the host in `push.register`, and keeps it in a keychain
/// group the notification extension shares, which reads it with the same
/// service name (see NotificationService/NotificationService.swift).
enum PushKeys {
    private static let service = "push"

    private static var group: String? {
        Bundle.main.object(forInfoDictionaryKey: "NMTPushKeyGroup") as? String
    }

    /// The host's key, created on first use, base64 as the host takes it.
    static func key(for host: String) -> String? {
        if let existing = read(host) {
            return existing.base64EncodedString()
        }
        var bytes = Data(count: 32)
        let status = bytes.withUnsafeMutableBytes { SecRandomCopyBytes(kSecRandomDefault, 32, $0.baseAddress!) }
        guard status == errSecSuccess, let group else { return nil }

        let item: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: host,
            kSecAttrAccessGroup as String: group,
            kSecValueData as String: bytes,
            // The extension opens pushes that arrive while the phone is
            // locked, which it can once the phone was unlocked after boot.
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        guard SecItemAdd(item as CFDictionary, nil) == errSecSuccess else { return nil }
        return bytes.base64EncodedString()
    }

    /// Forget the host's key; pushes it still sends show the placeholder.
    static func remove(for host: String) {
        guard let group else { return }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: host,
            kSecAttrAccessGroup as String: group,
        ]
        SecItemDelete(query as CFDictionary)
    }

    private static func read(_ host: String) -> Data? {
        guard let group else { return nil }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
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
