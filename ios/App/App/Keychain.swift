import Foundation
import Security

/// The identity key, in the Keychain.
///
/// `docs/keys.md#signing-happens-in-the-background` fixes the accessibility
/// class and it is load-bearing, not a default to revisit:
///
/// - `AfterFirstUnlock` so the key is readable while the device is locked. The
///   key signs during encounters, which happen in a pocket. `WhenUnlocked`
///   would kill pocket-to-pocket gossip silently.
/// - `ThisDeviceOnly` so it stays out of iCloud Keychain and encrypted backups.
///   Moving an identity is login-with-device, an explicit flow, never a sync.
/// - No `SecAccessControl`, and with it no biometric gate on the signing path. There is
///   no user present during a background wake, and a phone that cannot sign
///   cannot authenticate.
///
/// What is stored is key bytes protected at rest. A hardware-backed key is not
/// available on either platform, because the Secure Enclave does NIST P-256 and
/// nostr is secp256k1.
enum Keychain {
    /// The one item, under the app's own service name.
    private static let service = "social.coracle.dipity.identity"
    private static let account = "nostr"

    /// Whether an identity has been generated or imported yet.
    ///
    /// This is the first-run question, and it is asked without reading the key.
    static func has() -> Bool {
        var query = base()
        query[kSecReturnData as String] = false

        return SecItemCopyMatching(query as CFDictionary, nil) == errSecSuccess
    }

    /// The identity's 32 secret bytes.
    static func read() throws -> Data {
        var query = base()
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)

        guard status != errSecItemNotFound else { throw KeyError.Missing }
        guard status == errSecSuccess, let data = item as? Data else {
            throw KeyError.Unreadable(reason: "Keychain returned OSStatus \(status)")
        }

        return data
    }

    /// Store `secret`, replacing whatever was there.
    ///
    /// Replacing is deliberate: an import or a login-with-device is the user
    /// deciding which identity this device is, and two would have no way to
    /// choose between them.
    static func write(_ secret: Data) throws {
        SecItemDelete(base() as CFDictionary)

        var item = base()
        item[kSecValueData as String] = secret
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly

        let status = SecItemAdd(item as CFDictionary, nil)

        guard status == errSecSuccess else {
            throw KeyError.Unreadable(reason: "Keychain refused the write, OSStatus \(status)")
        }
    }

    /// Forget the identity. Answers whether there was one.
    @discardableResult
    static func delete() -> Bool {
        SecItemDelete(base() as CFDictionary) == errSecSuccess
    }

    private static func base() -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }
}

/// The Keychain as the core reads it.
///
/// One read per signature, which is what `dip::keys::KeyCustody` asks for: the
/// core holds the pubkey and this object, never the key.
final class KeychainCustody: KeyCustody {
    func secretKey() throws -> Data {
        try Keychain.read()
    }
}
