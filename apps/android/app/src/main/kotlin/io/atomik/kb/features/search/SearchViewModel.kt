package io.atomik.kb.features.search

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import io.atomik.kb.data.PageOpener
import io.atomik.kb.data.SearchRepository
import io.atomik.kb.engine.model.SearchHit
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.debounce
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import javax.inject.Inject

data class SearchUiState(
    val query: String = "",
    val loading: Boolean = false,
    val results: List<SearchHit> = emptyList(),
    val error: String? = null,
) {
    /** A query was run and found nothing. */
    val noResults: Boolean get() = query.isNotBlank() && !loading && error == null && results.isEmpty()
}

@OptIn(FlowPreview::class)
@HiltViewModel
class SearchViewModel @Inject constructor(
    private val repository: SearchRepository,
    private val opener: PageOpener,
) : ViewModel() {
    private val _state = MutableStateFlow(SearchUiState())
    val state: StateFlow<SearchUiState> = _state.asStateFlow()
    private val queries = MutableStateFlow("")

    init {
        viewModelScope.launch {
            queries.debounce(DEBOUNCE_MS).distinctUntilChanged().collectLatest { run(it.trim()) }
        }
    }

    fun onQueryChange(query: String) {
        _state.update { it.copy(query = query, error = null, loading = query.isNotBlank()) }
        queries.value = query
    }

    fun retry() = viewModelScope.launch { run(_state.value.query.trim()) }

    fun open(hit: SearchHit) = opener.open(hit.pageId)

    private suspend fun run(query: String) {
        if (query.isEmpty()) {
            _state.update { it.copy(loading = false, results = emptyList(), error = null) }
            return
        }
        _state.update { it.copy(loading = true, error = null) }
        try {
            val hits = repository.search(query)
            _state.update { it.copy(loading = false, results = hits) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            _state.update { it.copy(loading = false, results = emptyList(), error = e.message ?: "error") }
        }
    }

    companion object {
        const val DEBOUNCE_MS = 250L
    }
}
