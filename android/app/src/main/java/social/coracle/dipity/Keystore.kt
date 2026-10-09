package social.coracle.dipity

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import uniffi.dip_ffi.KeyCustody
import uniffi.dip_ffi.KeyException

/**
 * The identity key, wrapped by a Keystore key.
 *
 * `docs/keys.md#signing-happens-in-the-background` fixes what the wrapping key
 * may require, and it is load-bearing rather than a default to revisit: no
 * `setUserAuthenticationRequired`, and with it no biometric or lock-screen gate
 * on the signing path. The key signs during encounters, which happen with the
 * screen off and nobody in front of the phone; a gate there kills
 * pocket-to-pocket gossip silently.
 *
 * The identity itself cannot live in hardware, because Android Keystore dropped
 * secp256k1 years ago. What hardware holds is an AES key that wraps it, and what
 * is stored beside it is the ciphertext.
 */
class Keystore(context: Context) {
    private val prefs = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)

    /**
     * Whether an identity has been generated or imported yet.
     *
     * Ciphertext without the Keystore key that sealed it, which is what a
     * restore onto another device leaves, can never be read and counts as no
     * identity, so the user reaches first run rather than a dead end. A read
     * that fails for any other reason is not taken as absence, because first
     * run would then replace a key that may still be recoverable.
     */
    fun has(): Boolean = prefs.contains(IDENTITY) && keystore().containsAlias(WRAPPING_KEY)

    /** The identity's 32 secret bytes. */
    fun read(): ByteArray {
        val stored = prefs.getString(IDENTITY, null) ?: throw KeyException.Missing()
        val bytes = Base64.decode(stored, Base64.NO_WRAP)

        if (bytes.size <= IV_BYTES) {
            throw KeyException.Unreadable("the stored identity is truncated")
        }

        return try {
            Cipher.getInstance(TRANSFORMATION).run {
                init(
                    Cipher.DECRYPT_MODE,
                    wrappingKey(),
                    GCMParameterSpec(TAG_BITS, bytes, 0, IV_BYTES),
                )
                doFinal(bytes, IV_BYTES, bytes.size - IV_BYTES)
            }
        } catch (error: Exception) {
            throw KeyException.Unreadable(error.message ?: error.javaClass.simpleName)
        }
    }

    /**
     * Store `secret`, replacing whatever was there.
     *
     * Replacing is deliberate: an import or a login-with-device is the user
     * deciding which identity this device is, and two would have no way to
     * choose between them.
     */
    fun write(secret: ByteArray) {
        val cipher = Cipher.getInstance(TRANSFORMATION)

        cipher.init(Cipher.ENCRYPT_MODE, wrappingKey())

        val sealed = cipher.iv + cipher.doFinal(secret)

        prefs.edit().putString(IDENTITY, Base64.encodeToString(sealed, Base64.NO_WRAP)).commit()
    }

    /** Forget the identity, and the key that wrapped it. Answers whether there was one. */
    fun delete(): Boolean {
        val had = has()

        prefs.edit().remove(IDENTITY).commit()
        keystore().deleteEntry(WRAPPING_KEY)

        return had
    }

    /** The hardware key the identity is sealed under, generated on first use. */
    private fun wrappingKey(): SecretKey {
        val keystore = keystore()

        (keystore.getEntry(WRAPPING_KEY, null) as? KeyStore.SecretKeyEntry)?.let {
            return it.secretKey
        }

        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER)
            .apply {
                init(
                    KeyGenParameterSpec.Builder(
                            WRAPPING_KEY,
                            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                        )
                        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                        .setUserAuthenticationRequired(false)
                        .build()
                )
            }
            .generateKey()
    }

    private fun keystore(): KeyStore = KeyStore.getInstance(PROVIDER).apply { load(null) }

    private companion object {
        const val PROVIDER = "AndroidKeyStore"
        const val PREFERENCES = "social.coracle.dipity.identity"
        const val IDENTITY = "nostr"
        const val WRAPPING_KEY = "social.coracle.dipity.identity.wrap"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val IV_BYTES = 12
        const val TAG_BITS = 128
    }
}

/**
 * The Keystore as the core reads it.
 *
 * One read per signature, which is what `dip::keys::KeyCustody` asks for: the
 * core holds the pubkey and this object, never the key.
 */
class KeystoreCustody(private val keystore: Keystore) : KeyCustody {
    override fun secretKey(): ByteArray = keystore.read()
}
