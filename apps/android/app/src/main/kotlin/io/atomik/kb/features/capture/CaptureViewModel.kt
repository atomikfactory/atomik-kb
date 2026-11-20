package io.atomik.kb.features.capture

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import io.atomik.kb.data.Loadable
import io.atomik.kb.data.PageOpener
import io.atomik.kb.data.PagesRepository
import io.atomik.kb.data.SettingsRepository
import io.atomik.kb.data.SpacesRepository
import io.atomik.kb.engine.model.SpaceInfo
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.firstOrNull
import kotlinx.coroutines.launch
import javax.inject.Inject

data class CaptureUiState(val error: String? = null, val done: Boolean = false)

/** The space quick capture writes to: the chosen one, else the last used, else the first. */
fun pickCaptureSpace(spaces: List<SpaceInfo>, captureSpaceId: String?, lastSpaceId: String?): SpaceInfo? =
    spaces.firstOrNull { it.id == captureSpaceId } ?: spaces.firstOrNull { it.id == lastSpaceId } ?: spaces.firstOrNull()

/** Quick capture: a new empty page in the capture space, opened in the editor with the title focused. */
@HiltViewModel
class CaptureViewModel @Inject constructor(
    private val spaces: SpacesRepository,
    private val pages: PagesRepository,
    private val settings: SettingsRepository,
    private val opener: PageOpener,
) : ViewModel() {
    private val _state = MutableStateFlow(CaptureUiState())
    val state: StateFlow<CaptureUiState> = _state.asStateFlow()

    init { start() }

    fun start() {
        _state.value = CaptureUiState()
        viewModelScope.launch {
            try {
                val loaded = spaces.spaces.first { it !is Loadable.Loading }
                if (loaded is Loadable.Error) throw IllegalStateException(loaded.message)
                val list = (loaded as Loadable.Data).value
                val s = settings.current()
                val space = pickCaptureSpace(list, s.captureSpaceId, s.lastSpaceId)
                    ?: throw IllegalStateException("no space")
                val page = pages.create(space.id)
                opener.open(page.id, "title")
                _state.value = CaptureUiState(done = true)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _state.value = CaptureUiState(error = e.message ?: "error")
            }
        }
    }
}
