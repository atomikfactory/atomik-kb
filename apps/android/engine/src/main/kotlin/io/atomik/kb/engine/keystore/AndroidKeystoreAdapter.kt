package io.atomik.kb.engine.keystore

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Mirrors the FFI `KeystoreFfiError`; the RustEngine adapter maps these onto the generated type. */
sealed class KeystoreException(message: String, cause: Throwable? = null) : Exception(message, cause) {
    class Unavailable(message: String, cause: Throwable? = null) : KeystoreException(message, cause)
    class Failed(message: String, cause: Throwable? = null) : KeystoreException(message, cause)
}

/**
 * The platform side of the core's `Keystore` trait (decision A6): every value is encrypted with a
 * non-exportable AES-256-GCM key held in `AndroidKeyStore`; the ciphertext (12-byte IV, then the
 * sealed bytes) lives in app-private files under [dir]. The method shapes match the FFI
 * `PlatformKeystore` callback (`get`/`set`/`delete` by name) so the RustEngine adapter is a
 * one-line delegation.
 */
class AndroidKeystoreAdapter(
    private val dir: File,
    private val keyAlias: String = KEY_ALIAS,
) {
    constructor(context: Context) : this(File(context.noBackupFilesDir, "keystore"))

    private val lock = Any()

    /** The stored bytes, or null when [name] was never set. */
    fun get(name: String): ByteArray? = synchronized(lock) {
        val file = fileFor(name)
        if (!file.exists()) return null
        val blob = try {
            file.readBytes()
        } catch (e: java.io.IOException) {
            throw KeystoreException.Failed("read $name", e)
        }
        if (blob.size <= IV_BYTES) throw KeystoreException.Failed("corrupt entry $name")
        try {
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.DECRYPT_MODE, secretKey(), GCMParameterSpec(TAG_BITS, blob, 0, IV_BYTES))
            cipher.updateAAD(name.toByteArray())
            cipher.doFinal(blob, IV_BYTES, blob.size - IV_BYTES)
        } catch (e: java.security.GeneralSecurityException) {
            throw KeystoreException.Failed("decrypt $name", e)
        }
    }

    fun set(name: String, value: ByteArray) = synchronized(lock) {
        val sealed = try {
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.ENCRYPT_MODE, secretKey())
            cipher.updateAAD(name.toByteArray())
            cipher.iv + cipher.doFinal(value)
        } catch (e: java.security.GeneralSecurityException) {
            throw KeystoreException.Failed("encrypt $name", e)
        }
        dir.mkdirs()
        val target = fileFor(name)
        val tmp = File(dir, target.name + ".tmp")
        try {
            tmp.writeBytes(sealed)
            if (!tmp.renameTo(target)) throw KeystoreException.Failed("rename $name")
        } catch (e: java.io.IOException) {
            throw KeystoreException.Failed("write $name", e)
        }
    }

    fun delete(name: String) = synchronized(lock) {
        val file = fileFor(name)
        if (file.exists() && !file.delete()) throw KeystoreException.Failed("delete $name")
    }

    private fun fileFor(name: String): File {
        // Names are core-chosen identifiers; hex-encode so nothing can escape [dir].
        val hex = name.toByteArray().joinToString("") { "%02x".format(it) }
        return File(dir, "$hex.bin")
    }

    private fun secretKey(): SecretKey {
        try {
            val ks = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }
            (ks.getKey(keyAlias, null) as? SecretKey)?.let { return it }
            val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
            generator.init(
                KeyGenParameterSpec.Builder(
                    keyAlias,
                    KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                )
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setKeySize(256)
                    .build(),
            )
            return generator.generateKey()
        } catch (e: java.security.GeneralSecurityException) {
            throw KeystoreException.Unavailable("AndroidKeyStore", e)
        } catch (e: java.io.IOException) {
            throw KeystoreException.Unavailable("AndroidKeyStore", e)
        }
    }

    private companion object {
        const val ANDROID_KEYSTORE = "AndroidKeyStore"
        const val KEY_ALIAS = "io.atomik.kb.root-wrap"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val IV_BYTES = 12
        const val TAG_BITS = 128
    }
}
