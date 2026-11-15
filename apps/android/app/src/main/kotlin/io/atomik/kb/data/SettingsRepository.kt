package io.atomik.kb.data

import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.longPreferencesKey
import androidx.datastore.preferences.core.stringPreferencesKey
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import javax.inject.Inject
import javax.inject.Singleton

/** How long the app may stay in the background before the lock engages again. */
enum class AutoLockDelay(val seconds: Long) {
    Immediately(0), OneMinute(60), FiveMinutes(300), FifteenMinutes(900), Never(Long.MAX_VALUE);

    companion object {
        fun fromName(name: String?): AutoLockDelay = entries.firstOrNull { it.name == name } ?: OneMinute
    }
}

/** The biometric-wrapped passphrase (A7): AES-GCM IV and ciphertext, Base64. */
data class BiometricBlob(val iv: String, val ciphertext: String)

data class AppSettings(
    val onlineDictation: Boolean = false,
    val autoLock: AutoLockDelay = AutoLockDelay.OneMinute,
    val biometric: BiometricBlob? = null,
    val lastSpaceId: String? = null,
    val captureSpaceId: String? = null,
    val lastSyncAt: Long = 0,
)

/** App-level preferences on DataStore (nothing secret except the biometric-wrapped blob). */
@Singleton
class SettingsRepository @Inject constructor(private val store: DataStore<Preferences>) {
    private object Keys {
        val onlineDictation = booleanPreferencesKey("online_dictation")
        val autoLock = stringPreferencesKey("auto_lock")
        val bioIv = stringPreferencesKey("bio_iv")
        val bioCipher = stringPreferencesKey("bio_cipher")
        val lastSpace = stringPreferencesKey("last_space")
        val captureSpace = stringPreferencesKey("capture_space")
        val lastSync = longPreferencesKey("last_sync")
    }

    val settings: Flow<AppSettings> = store.data.map { p ->
        val iv = p[Keys.bioIv]
        val ct = p[Keys.bioCipher]
        AppSettings(
            onlineDictation = p[Keys.onlineDictation] ?: false,
            autoLock = AutoLockDelay.fromName(p[Keys.autoLock]),
            biometric = if (iv != null && ct != null) BiometricBlob(iv, ct) else null,
            lastSpaceId = p[Keys.lastSpace],
            captureSpaceId = p[Keys.captureSpace],
            lastSyncAt = p[Keys.lastSync] ?: 0,
        )
    }

    suspend fun current(): AppSettings = settings.first()

    suspend fun setOnlineDictation(value: Boolean) = store.edit { it[Keys.onlineDictation] = value }
    suspend fun setAutoLock(value: AutoLockDelay) = store.edit { it[Keys.autoLock] = value.name }
    suspend fun setLastSpace(id: String) = store.edit { it[Keys.lastSpace] = id }
    suspend fun setCaptureSpace(id: String?) = store.edit { if (id == null) it.remove(Keys.captureSpace) else it[Keys.captureSpace] = id }
    suspend fun setLastSyncAt(at: Long) = store.edit { it[Keys.lastSync] = at }

    suspend fun setBiometric(blob: BiometricBlob?) = store.edit {
        if (blob == null) {
            it.remove(Keys.bioIv); it.remove(Keys.bioCipher)
        } else {
            it[Keys.bioIv] = blob.iv; it[Keys.bioCipher] = blob.ciphertext
        }
    }
}
