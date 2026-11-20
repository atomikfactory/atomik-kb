package io.atomik.kb.features.page

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import io.atomik.kb.data.Loadable
import io.atomik.kb.data.PagesRepository
import io.atomik.kb.data.PendingInsert
import io.atomik.kb.data.PendingInserts
import io.atomik.kb.data.SettingsRepository
import io.atomik.kb.data.SpacesRepository
import io.atomik.kb.engine.Engine
import io.atomik.kb.engine.model.PageInfo
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import javax.inject.Inject

data class PageUiState(
    val pageId: String? = null,
    val loading: Boolean = true,
    val error: String? = null,
    val page: PageInfo? = null,
    val title: String = "",
    val favorite: Boolean = false,
    val editorReady: Boolean = false,
    val dictating: Boolean = false,
    val dictationProblem: DictationProblem? = null,
    val editorError: String? = null,
    val editorCrashed: Boolean = false,
)

sealed interface PageEffect {
    data class OpenPage(val pageId: String) : PageEffect
    data class OpenExternal(val url: String) : PageEffect
    data object RequestDictation : PageEffect
    data object Closed : PageEffect
}

/** The page screen's brain: talks Contract 2 with the editor and the core, shows nothing itself. */
@HiltViewModel
class PageViewModel @Inject constructor(
    private val engine: Engine,
    private val pages: PagesRepository,
    private val spaces: SpacesRepository,
    private val pending: PendingInserts,
    settings: SettingsRepository,
) : ViewModel() {
    private val _state = MutableStateFlow(PageUiState())
    val state: StateFlow<PageUiState> = _state.asStateFlow()

    private val _outbound = MutableSharedFlow<String>(extraBufferCapacity = 128)

    /** Messages for the editor, in order; the WebView posts each one. */
    val outbound: SharedFlow<String> = _outbound.asSharedFlow()

    private val _effects = Channel<PageEffect>(Channel.BUFFERED)
    val effects: Flow<PageEffect> = _effects.receiveAsFlow()

    /** Online dictation is opt-in (A9); read when dictation starts. */
    val onlineDictation: StateFlow<Boolean> =
        settings.settings.map { it.onlineDictation }.stateIn(viewModelScope, SharingStarted.Eagerly, false)

    private val handler = EditorCallHandler(engine)
    private var focus: EditorFocus? = null
    private var theme = EditorTheme.Light
    private var locale = "en"
    private var eventJob: Job? = null
    private var favoriteJob: Job? = null

    /** Opens [pageId]; call once per page (the view model is keyed by page). */
    fun load(pageId: String, focus: String?) {
        if (_state.value.pageId == pageId) return
        this.focus = when (focus) { "title" -> EditorFocus.Title; "end" -> EditorFocus.End; else -> null }
        _state.value = PageUiState(pageId = pageId)
        viewModelScope.launch {
            try {
                val info = pages.get(pageId)
                _state.update { it.copy(loading = false, page = info, title = info.title) }
                watchFavorite(info)
            } catch (e: Exception) {
                _state.update { it.copy(loading = false, error = e.message ?: "error") }
            }
        }
    }

    fun setEnvironment(dark: Boolean, locale: String) {
        val newTheme = if (dark) EditorTheme.Dark else EditorTheme.Light
        val changed = newTheme != theme
        theme = newTheme
        this.locale = locale
        if (changed && _state.value.editorReady) _outbound.tryEmit(EditorProtocol.theme(newTheme))
    }

    /** One message from the editor (already on the main thread). */
    fun onEditorMessage(message: String) {
        when (val inbound = EditorProtocol.parse(message)) {
            is Inbound.Call -> viewModelScope.launch { _outbound.emit(handler.handle(inbound.call)) }
            is Inbound.Notification -> onNotification(inbound.notification)
            is Inbound.Invalid -> _state.update { it.copy(editorError = inbound.reason) }
        }
    }

    private fun onNotification(n: HostNotification) {
        when (n) {
            HostNotification.Ready -> onEditorReady()
            is HostNotification.Opened -> _state.update { it.copy(title = n.title) }
            is HostNotification.TitleChanged -> _state.update { it.copy(title = n.title) }
            is HostNotification.OpenPage -> _effects.trySend(PageEffect.OpenPage(n.pageId))
            is HostNotification.OpenExternal -> _effects.trySend(PageEffect.OpenExternal(n.url))
            HostNotification.Dictate -> _effects.trySend(PageEffect.RequestDictation)
            is HostNotification.Error -> _state.update { it.copy(editorError = n.message) }
        }
    }

    private fun onEditorReady() {
        val page = _state.value.page ?: return
        _state.update { it.copy(editorReady = true) }
        _outbound.tryEmit(EditorProtocol.init(page.id, page.spaceId, theme, locale, page.locked == true, focus))
        for (insert in pending.drain(page.id)) {
            _outbound.tryEmit(
                when (insert) {
                    is PendingInsert.Text -> EditorProtocol.insertText(insert.text)
                    is PendingInsert.Attachment -> EditorProtocol.insertAttachment(insert.attachmentId, insert.name, insert.mime)
                },
            )
        }
        eventJob?.cancel()
        eventJob = viewModelScope.launch { engine.events.collect { _outbound.emit(EditorProtocol.event(it.raw)) } }
    }

    private fun watchFavorite(page: PageInfo) {
        favoriteJob?.cancel()
        favoriteJob = viewModelScope.launch {
            spaces.tree(page.spaceId).collect { tree ->
                if (tree is Loadable.Data) _state.update { it.copy(favorite = page.id in tree.value.favorites) }
            }
        }
    }

    fun toggleFavorite() {
        val id = _state.value.pageId ?: return
        val target = !_state.value.favorite
        viewModelScope.launch {
            runCatching { pages.setFavorite(id, target) }.onFailure { e -> _state.update { it.copy(editorError = e.message) } }
        }
    }

    fun moveToTrash() {
        val id = _state.value.pageId ?: return
        viewModelScope.launch {
            try {
                pages.trash(id)
                _effects.send(PageEffect.Closed)
            } catch (e: Exception) {
                _state.update { it.copy(editorError = e.message) }
            }
        }
    }

    /** The WebView's renderer died; the screen builds a new one, which announces itself with `ready` again. */
    fun onEditorGone() {
        eventJob?.cancel()
        _state.update { it.copy(editorReady = false, dictating = false, editorCrashed = true) }
    }

    fun dismissEditorCrash() = _state.update { it.copy(editorCrashed = false) }
    fun dismissEditorError() = _state.update { it.copy(editorError = null) }
    fun dismissDictationProblem() = _state.update { it.copy(dictationProblem = null) }

    // ---- Dictation, driven by the UI's DictationController ----

    fun onDictationState(listening: Boolean) {
        _state.update { it.copy(dictating = listening) }
        _outbound.tryEmit(EditorProtocol.dictation(if (listening) DictationState.Listening else DictationState.Idle))
    }

    fun onDictationPartial(text: String) {
        _outbound.tryEmit(EditorProtocol.dictation(DictationState.Listening, text))
    }

    fun onDictationFinal(text: String) {
        _outbound.tryEmit(EditorProtocol.insertText(text))
        _outbound.tryEmit(EditorProtocol.dictation(DictationState.Listening))
    }

    fun onDictationProblem(problem: DictationProblem) = _state.update { it.copy(dictationProblem = problem, dictating = false) }

    override fun onCleared() {
        _state.value.pageId?.let { engine.docClose(it) }
    }
}
