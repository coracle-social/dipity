package social.coracle.dip

import android.app.Activity
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricManager.Authenticators.BIOMETRIC_STRONG
import android.hardware.biometrics.BiometricManager.Authenticators.DEVICE_CREDENTIAL
import android.hardware.biometrics.BiometricPrompt
import android.os.CancellationSignal
import com.getcapacitor.PluginCall

/**
 * Asks whoever holds the phone to prove they own it before the key leaves.
 *
 * Export and transfer both hand the identity to somebody, so both sit behind
 * the device's own biometric or credential. A phone with neither set has
 * nothing to ask and goes straight through. `docs/keys.md#backup`.
 */
object OwnerCheck {
    private const val AUTHENTICATORS = BIOMETRIC_STRONG or DEVICE_CREDENTIAL

    /** Run [then] on the main thread once the owner confirms, or reject [call]. */
    fun confirm(activity: Activity, reason: String, call: PluginCall, then: () -> Unit) {
        val manager = activity.getSystemService(BiometricManager::class.java)

        if (manager?.canAuthenticate(AUTHENTICATORS) != BiometricManager.BIOMETRIC_SUCCESS) {
            return activity.runOnUiThread(then)
        }

        BiometricPrompt.Builder(activity)
            .setTitle(reason)
            .setAllowedAuthenticators(AUTHENTICATORS)
            .build()
            .authenticate(
                CancellationSignal(),
                activity.mainExecutor,
                object : BiometricPrompt.AuthenticationCallback() {
                    override fun onAuthenticationSucceeded(
                        result: BiometricPrompt.AuthenticationResult
                    ) = then()

                    override fun onAuthenticationError(code: Int, message: CharSequence) =
                        call.reject("the owner did not confirm")
                },
            )
    }
}
