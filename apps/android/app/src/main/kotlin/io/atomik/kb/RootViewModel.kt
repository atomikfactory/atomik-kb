package io.atomik.kb

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import io.atomik.kb.data.Loadable
import io.atomik.kb.data.SecurityRepository
import io.atomik.kb.data.SpacesRepository
import io.atomik.kb.engine.EngineState
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.ExperimentalCoroutinesApi
import javax.inject.Inject

/** What the whole window shows. */
sealed interface RootUiState {
    data object Loading : RootUiState
    data class Failed(val message: String) : RootUiState
    data object Locked : RootUiState
    data object Onboarding : RootUiState
    data object Main : RootUiState
}

@OptIn(ExperimentalCoroutinesApi::class)
@HiltViewModel
class RootViewModel @Inject constructor(
    security: SecurityRepository,
    spaces: SpacesRepository,
) : ViewModel() {
    private val retry = MutableStateFlow(0)

    val state: StateFlow<RootUiState> = combine(
        security.state,
        retry.flatMapLatest { spaces.onboarded },
    ) { engine, onboarded ->
        when (engine) {
            EngineState.Opening -> RootUiState.Loading
            is EngineState.Failed -> RootUiState.Failed(engine.message)
            EngineState.Locked -> RootUiState.Locked
            EngineState.Ready -> when (onboarded) {
                Loadable.Loading -> RootUiState.Loading
                is Loadable.Error -> if (onboarded.locked) RootUiState.Locked else RootUiState.Failed(onboarded.message)
                is Loadable.Data -> if (onboarded.value) RootUiState.Main else RootUiState.Onboarding
            }
        }
    }.stateIn(viewModelScope, SharingStarted.Eagerly, RootUiState.Loading)

    fun retry() = retry.update { it + 1 }
}
