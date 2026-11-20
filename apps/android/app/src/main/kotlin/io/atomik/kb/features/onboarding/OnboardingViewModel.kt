package io.atomik.kb.features.onboarding

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import io.atomik.kb.data.Loadable
import io.atomik.kb.data.SpacesRepository
import io.atomik.kb.data.SyncRepository
import io.atomik.kb.data.normalizeServerUrl
import io.atomik.kb.engine.model.PairingInfo
import io.atomik.kb.engine.model.PairingState
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import javax.inject.Inject

enum class OnboardingStep { Welcome, Server, Pairing, Fresh }

data class OnboardingUiState(
    val step: OnboardingStep = OnboardingStep.Welcome,
    val serverUrl: String = "",
    val serverUrlInvalid: Boolean = false,
    val busy: Boolean = false,
    val pairing: PairingInfo? = null,
    val error: String? = null,
    val workspaceName: String = "",
    val spaceName: String = "",
) {
    val pairingActive: Boolean
        get() = pairing?.state in setOf(PairingState.Starting, PairingState.Waiting, PairingState.Confirming, PairingState.PeerConfirming, PairingState.Transferring)
}

@HiltViewModel
class OnboardingViewModel @Inject constructor(
    private val spaces: SpacesRepository,
    private val sync: SyncRepository,
) : ViewModel() {
    private val _state = MutableStateFlow(OnboardingUiState())
    val state: StateFlow<OnboardingUiState> = _state.asStateFlow()
    private var pairingJob: Job? = null

    fun goTo(step: OnboardingStep) = _state.update { it.copy(step = step, error = null, serverUrlInvalid = false) }

    fun onServerUrlChange(value: String) = _state.update { it.copy(serverUrl = value, serverUrlInvalid = false) }
    fun onWorkspaceNameChange(value: String) = _state.update { it.copy(workspaceName = value) }
    fun onSpaceNameChange(value: String) = _state.update { it.copy(spaceName = value) }

    /** Joiner pairing: shows the code to type on the existing device, then the six digits to compare. */
    fun startPairing() {
        val url = normalizeServerUrl(_state.value.serverUrl)
        if (url == null) {
            _state.update { it.copy(serverUrlInvalid = true) }
            return
        }
        viewModelScope.launch {
            _state.update { it.copy(busy = true, error = null, serverUrl = url) }
            try {
                val info = sync.startPairing(url)
                _state.update { it.copy(busy = false, step = OnboardingStep.Pairing, pairing = info) }
                watchPairing()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _state.update { it.copy(busy = false, error = e.message ?: "error") }
            }
        }
    }

    private fun watchPairing() {
        pairingJob?.cancel()
        pairingJob = viewModelScope.launch {
            sync.pairing.collect { loaded ->
                if (loaded is Loadable.Data) _state.update { it.copy(pairing = loaded.value) }
            }
        }
    }

    /** The six digits match on both devices. */
    fun confirmPairing() = act { sync.confirmPairing() }

    /** The digits differ or the user gave up. */
    fun cancelPairing() {
        viewModelScope.launch {
            runCatching { sync.cancelPairing() }
            pairingJob?.cancel()
            _state.update { it.copy(step = OnboardingStep.Server, pairing = null, error = null) }
        }
    }

    fun startFresh() {
        val s = _state.value
        val name = s.workspaceName.trim()
        if (name.isEmpty() || s.busy) return
        act { spaces.createWorkspace(name, s.spaceName.trim().ifEmpty { "Personal" }) }
    }

    private fun act(block: suspend () -> Unit) {
        viewModelScope.launch {
            _state.update { it.copy(busy = true, error = null) }
            try {
                block()
                _state.update { it.copy(busy = false) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _state.update { it.copy(busy = false, error = e.message ?: "error") }
            }
        }
    }
}
