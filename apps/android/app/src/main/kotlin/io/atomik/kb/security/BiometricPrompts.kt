package io.atomik.kb.security

import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import javax.crypto.Cipher

/**
 * Shows a strong-biometric prompt bound to [cipher] (a CryptoObject). [onSuccess] gets the
 * authenticated cipher; [onFailure] gets a message unless the user just cancelled ([cancelled]).
 */
fun FragmentActivity.authenticateWithCipher(
    cipher: Cipher,
    title: String,
    subtitle: String?,
    negativeText: String,
    onSuccess: (Cipher) -> Unit,
    onFailure: (message: String, cancelled: Boolean) -> Unit,
) {
    val callback = object : BiometricPrompt.AuthenticationCallback() {
        override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
            val authenticated = result.cryptoObject?.cipher
            if (authenticated != null) onSuccess(authenticated) else onFailure("no cipher", false)
        }

        override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
            val cancelled = errorCode == BiometricPrompt.ERROR_USER_CANCELED ||
                errorCode == BiometricPrompt.ERROR_NEGATIVE_BUTTON ||
                errorCode == BiometricPrompt.ERROR_CANCELED
            onFailure(errString.toString(), cancelled)
        }
    }
    val prompt = BiometricPrompt(this, ContextCompat.getMainExecutor(this), callback)
    val info = BiometricPrompt.PromptInfo.Builder()
        .setTitle(title)
        .apply { if (subtitle != null) setSubtitle(subtitle) }
        .setNegativeButtonText(negativeText)
        .setAllowedAuthenticators(androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_STRONG)
        .setConfirmationRequired(false)
        .build()
    prompt.authenticate(info, BiometricPrompt.CryptoObject(cipher))
}
