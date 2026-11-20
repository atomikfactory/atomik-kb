package io.atomik.kb.features.lock

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import io.atomik.kb.data.BiometricBlob
import io.atomik.kb.data.SecurityRepository
import io.atomik.kb.data.SettingsRepository
import io.atomik.kb.engine.CoreException
import io.atomik.kb.engine.model.ErrorCode
import io.atomik.kb.security.BiometricKeyInvalidatedException
import io.atomik.kb.security.BiometricVault
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import javax.crypto.Cipher
import javax.inject.Inject

enum class LockError { WrongPassphrase, Failed, BiometricInvalidated }

data class LockUiState(
    val busy: Boolean = false,
    val error: LockError? = null,
    val errorDetail: String? = null,
    val biometricAvailable: Boolean = false,
)

@HiltViewModel
class LockViewModel @Inject constructor(
    private val security: SecurityRepository,
    private val settings: SettingsRepository,
    private val vault: BiometricVault,
) : ViewModel() {
    private val _state = MutableStateFlow(LockUiState())
    val state: StateFlow<LockUiState> = _state.asStateFlow()
    private var blob: BiometricBlob? = null

    init {
        viewModelScope.launch {
            blob = settings.current().biometric
            _state.update { it.copy(biometricAvailable = blob != null && vault.isAvailable()) }
        }
    }

    fun unlock(passphrase: String) {
        if (passphrase.isEmpty() || _state.value.busy) return
        viewModelScope.launch {
            _state.update { it.copy(busy = true, error = null) }
            try {
                security.unlock(passphrase)
                _state.update { it.copy(busy = false) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: CoreException) {
                val wrong = e.error.code == ErrorCode.Forbidden || e.error.code == ErrorCode.Crypto || e.error.code == ErrorCode.InvalidInput
                _state.update { it.copy(busy = false, error = if (wrong) LockError.WrongPassphrase else LockError.Failed, errorDetail = e.error.message) }
            } catch (e: Exception) {
                _state.update { it.copy(busy = false, error = LockError.Failed, errorDetail = e.message) }
            }
        }
    }

    /** The cipher for the biometric prompt, or null when there is no (valid) biometric copy. */
    fun biometricCipher(): Cipher? {
        val stored = blob ?: return null
        return try {
            vault.decryptCipher(stored)
        } catch (e: BiometricKeyInvalidatedException) {
            viewModelScope.launch {
                settings.setBiometric(null)
                blob = null
                _state.update { it.copy(biometricAvailable = false, error = LockError.BiometricInvalidated) }
            }
            null
        }
    }

    /** The prompt succeeded: open the sealed passphrase and unlock with it. */
    fun onBiometricSuccess(cipher: Cipher) {
        val stored = blob ?: return
        val passphrase = try {
            vault.open(cipher, stored)
        } catch (e: Exception) {
            _state.update { it.copy(error = LockError.Failed, errorDetail = e.message) }
            return
        }
        unlock(passphrase)
    }

    fun onBiometricFailure(message: String, cancelled: Boolean) {
        if (!cancelled) _state.update { it.copy(error = LockError.Failed, errorDetail = message) }
    }

    fun clearError() = _state.update { it.copy(error = null, errorDetail = null) }
}
