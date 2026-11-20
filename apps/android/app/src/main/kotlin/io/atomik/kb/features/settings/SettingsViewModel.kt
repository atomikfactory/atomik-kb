package io.atomik.kb.features.settings

import androidx.annotation.StringRes
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import io.atomik.kb.R
import io.atomik.kb.data.AppSettings
import io.atomik.kb.data.AutoLockDelay
import io.atomik.kb.data.Loadable
import io.atomik.kb.data.SecurityRepository
import io.atomik.kb.data.SettingsRepository
import io.atomik.kb.data.SpacesRepository
import io.atomik.kb.data.SyncRepository
import io.atomik.kb.data.normalizeServerUrl
import io.atomik.kb.engine.model.DeviceEntry
import io.atomik.kb.engine.model.SecurityInfo
import io.atomik.kb.engine.model.SpaceInfo
import io.atomik.kb.engine.model.SyncInfo
import io.atomik.kb.security.BiometricVault
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import javax.crypto.Cipher
import javax.inject.Inject

/** A one-line notice: a string resource of ours or text from the core (safe to show). */
sealed interface Notice {
    data class Res(@StringRes val id: Int) : Notice
    data class Raw(val text: String) : Notice
}

/** Passphrases must be 8 to 256 bytes (the core's rule). */
fun passphraseProblem(passphrase: String): Boolean = passphrase.toByteArray().size !in 8..256

data class SettingsUiState(
    val sync: Loadable<SyncInfo> = Loadable.Loading,
    val devices: Loadable<List<DeviceEntry>> = Loadable.Loading,
    val security: Loadable<SecurityInfo> = Loadable.Loading,
    val settings: AppSettings = AppSettings(),
    val spaces: List<SpaceInfo> = emptyList(),
    val biometricSupported: Boolean = false,
    val busy: Boolean = false,
    val syncing: Boolean = false,
    val notice: Notice? = null,
    val dialogError: Notice? = null,
) {
    val lockEnabled: Boolean get() = (security as? Loadable.Data)?.value?.appLock?.enabled == true
}

@HiltViewModel
class SettingsViewModel @Inject constructor(
    private val syncRepo: SyncRepository,
    private val securityRepo: SecurityRepository,
    private val settingsRepo: SettingsRepository,
    spacesRepo: SpacesRepository,
    private val vault: BiometricVault,
) : ViewModel() {
    private val local = MutableStateFlow(SettingsUiState())

    val state: StateFlow<SettingsUiState> = combine(
        local, syncRepo.info, securityRepo.devices, securityRepo.info, settingsRepo.settings,
    ) { l, sync, devices, security, settings ->
        l.copy(sync = sync, devices = devices, security = security, settings = settings)
    }.combine(spacesRepo.spaces) { s, spaces ->
        s.copy(spaces = (spaces as? Loadable.Data)?.value.orEmpty())
    }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), SettingsUiState())

    init {
        local.update { it.copy(biometricSupported = vault.isAvailable()) }
    }

    fun noticeShown() = local.update { it.copy(notice = null) }
    fun clearDialogError() = local.update { it.copy(dialogError = null) }

    // ---- Sync ----

    fun syncNow() {
        if (local.value.syncing) return
        viewModelScope.launch {
            local.update { it.copy(syncing = true) }
            val notice = try {
                if (syncRepo.syncNow()) Notice.Res(R.string.sync_done) else Notice.Res(R.string.sync_timeout)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Notice.Raw(e.message ?: "error")
            }
            local.update { it.copy(syncing = false, notice = notice) }
        }
    }

    /** Points sync at [input] (blank turns it off); [onDone] runs when it was accepted. */
    fun saveServer(input: String, onDone: () -> Unit) {
        val url = if (input.isBlank()) null else normalizeServerUrl(input)
        if (input.isNotBlank() && url == null) {
            local.update { it.copy(dialogError = Notice.Res(R.string.server_url_invalid)) }
            return
        }
        launchBusy(onDone, Notice.Res(R.string.server_saved)) { syncRepo.configure(url) }
    }

    // ---- App lock ----

    fun enableLock(passphrase: String, onDone: () -> Unit) {
        if (passphraseProblem(passphrase)) { local.update { it.copy(dialogError = Notice.Res(R.string.passphrase_length)) }; return }
        launchBusy(onDone, Notice.Res(R.string.lock_enabled)) { securityRepo.enableLock(passphrase) }
    }

    fun disableLock(passphrase: String, onDone: () -> Unit) =
        launchBusy(onDone, Notice.Res(R.string.lock_disabled)) {
            securityRepo.disableLock(passphrase)
            settingsRepo.setBiometric(null)
            vault.delete()
        }

    fun changeLock(current: String, new: String, onDone: () -> Unit) {
        if (passphraseProblem(new)) { local.update { it.copy(dialogError = Notice.Res(R.string.passphrase_length)) }; return }
        launchBusy(onDone, Notice.Res(R.string.lock_changed)) {
            securityRepo.changeLock(current, new)
            // The biometric copy holds the old passphrase; drop it, the user can enable it again.
            settingsRepo.setBiometric(null)
            vault.delete()
        }
    }

    fun setAutoLock(delay: AutoLockDelay) { viewModelScope.launch { settingsRepo.setAutoLock(delay) } }

    // ---- Biometric (A7) ----

    /** Checks [passphrase], then hands [onCipher] the cipher the biometric prompt must authenticate. */
    fun beginBiometric(passphrase: String, onCipher: (Cipher) -> Unit) {
        viewModelScope.launch {
            local.update { it.copy(busy = true, dialogError = null) }
            try {
                securityRepo.verifyPassphrase(passphrase)
                val cipher = vault.newEncryptCipher()
                local.update { it.copy(busy = false) }
                onCipher(cipher)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                local.update { it.copy(busy = false, dialogError = Notice.Raw(e.message ?: "error")) }
            }
        }
    }

    fun completeBiometric(cipher: Cipher, passphrase: String) {
        viewModelScope.launch {
            try {
                settingsRepo.setBiometric(vault.seal(cipher, passphrase))
                local.update { it.copy(notice = Notice.Res(R.string.biometric_enabled)) }
            } catch (e: Exception) {
                local.update { it.copy(notice = Notice.Raw(e.message ?: "error")) }
            }
        }
    }

    fun biometricFailed(message: String, cancelled: Boolean) {
        vault.delete()
        if (!cancelled) local.update { it.copy(notice = Notice.Raw(message)) }
    }

    fun disableBiometric() {
        viewModelScope.launch {
            settingsRepo.setBiometric(null)
            vault.delete()
            local.update { it.copy(notice = Notice.Res(R.string.biometric_disabled)) }
        }
    }

    // ---- Preferences ----

    fun setOnlineDictation(value: Boolean) { viewModelScope.launch { settingsRepo.setOnlineDictation(value) } }
    fun setCaptureSpace(id: String?) { viewModelScope.launch { settingsRepo.setCaptureSpace(id) } }

    private fun launchBusy(onDone: () -> Unit, success: Notice, block: suspend () -> Unit) {
        viewModelScope.launch {
            local.update { it.copy(busy = true, dialogError = null) }
            try {
                block()
                local.update { it.copy(busy = false, notice = success) }
                onDone()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                local.update { it.copy(busy = false, dialogError = Notice.Raw(e.message ?: "error")) }
            }
        }
    }
}
