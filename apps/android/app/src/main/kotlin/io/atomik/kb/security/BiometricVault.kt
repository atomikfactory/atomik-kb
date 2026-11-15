package io.atomik.kb.security

import android.content.Context
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import androidx.biometric.BiometricManager
import dagger.hilt.android.qualifiers.ApplicationContext
import io.atomik.kb.data.BiometricBlob
import java.security.GeneralSecurityException
import java.security.KeyStore
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import javax.inject.Inject
import javax.inject.Singleton

/** The biometric key is gone or no longer valid (new fingerprint enrolled, lock screen removed). */
class BiometricKeyInvalidatedException(cause: Throwable? = null) : Exception("biometric key invalidated", cause)

/**
 * Decision A7: biometrics never replace the passphrase, they guard a copy of it. The copy is sealed
 * with an AES-256-GCM key in `AndroidKeyStore` that can only be used right after a strong biometric
 * authentication (BiometricPrompt with a CryptoObject). The prompt is shown by the UI; this class
 * hands out the ciphers and does the sealing.
 */
@Singleton
class BiometricVault @Inject constructor(@ApplicationContext private val context: Context) {

    /** A strong biometric is enrolled and usable. */
    fun isAvailable(): Boolean =
        BiometricManager.from(context).canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG) == BiometricManager.BIOMETRIC_SUCCESS

    /** A cipher for sealing a passphrase; generates a fresh key (replacing any old one). */
    fun newEncryptCipher(): Cipher {
        delete()
        val key = generateKey()
        return Cipher.getInstance(TRANSFORMATION).apply { init(Cipher.ENCRYPT_MODE, key) }
    }

    /** A cipher for opening [blob]; throws [BiometricKeyInvalidatedException] when the key cannot be used. */
    fun decryptCipher(blob: BiometricBlob): Cipher {
        val key = existingKey() ?: throw BiometricKeyInvalidatedException()
        return try {
            Cipher.getInstance(TRANSFORMATION).apply {
                init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(TAG_BITS, Base64.getDecoder().decode(blob.iv)))
            }
        } catch (e: KeyPermanentlyInvalidatedException) {
            delete()
            throw BiometricKeyInvalidatedException(e)
        }
    }

    /** Seals [passphrase] with an authenticated [cipher] from [newEncryptCipher]. */
    fun seal(cipher: Cipher, passphrase: String): BiometricBlob {
        val sealed = cipher.doFinal(passphrase.toByteArray(Charsets.UTF_8))
        return BiometricBlob(Base64.getEncoder().encodeToString(cipher.iv), Base64.getEncoder().encodeToString(sealed))
    }

    /** Opens [blob] with an authenticated [cipher] from [decryptCipher]. */
    fun open(cipher: Cipher, blob: BiometricBlob): String =
        String(cipher.doFinal(Base64.getDecoder().decode(blob.ciphertext)), Charsets.UTF_8)

    fun delete() {
        runCatching { KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }.deleteEntry(KEY_ALIAS) }
    }

    private fun existingKey(): SecretKey? = try {
        KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }.getKey(KEY_ALIAS, null) as? SecretKey
    } catch (e: GeneralSecurityException) {
        null
    }

    private fun generateKey(): SecretKey {
        val spec = KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .setUserAuthenticationRequired(true)
            .setInvalidatedByBiometricEnrollment(true)
            .apply {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    // Every use needs a fresh strong biometric (timeout 0 = per-operation, bound to the CryptoObject).
                    setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG)
                } else {
                    @Suppress("DEPRECATION")
                    setUserAuthenticationValidityDurationSeconds(-1)
                }
            }
            .build()
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE).apply { init(spec) }.generateKey()
    }

    private companion object {
        const val ANDROID_KEYSTORE = "AndroidKeyStore"
        const val KEY_ALIAS = "io.atomik.kb.biometric-wrap"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val TAG_BITS = 128
    }
}
