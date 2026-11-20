package io.atomik.kb.features.home

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import io.atomik.kb.data.Loadable
import io.atomik.kb.data.PageOpener
import io.atomik.kb.data.PagesRepository
import io.atomik.kb.data.SettingsRepository
import io.atomik.kb.data.SpacesRepository
import io.atomik.kb.data.SyncRepository
import io.atomik.kb.engine.model.PageInfo
import io.atomik.kb.engine.model.PageNode
import io.atomik.kb.engine.model.SpaceInfo
import io.atomik.kb.engine.model.TreeView
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import javax.inject.Inject

/** One visible row of the page tree. */
data class TreeRow(val page: PageInfo, val depth: Int, val hasChildren: Boolean, val expanded: Boolean)

data class HomeUiState(
    val loading: Boolean = true,
    val error: String? = null,
    val spaces: List<SpaceInfo> = emptyList(),
    val selectedSpace: SpaceInfo? = null,
    val favorites: List<PageInfo> = emptyList(),
    val rows: List<TreeRow> = emptyList(),
    val refreshing: Boolean = false,
    val message: String? = null,
) {
    val isEmpty: Boolean get() = !loading && error == null && rows.isEmpty()
}

/** Flattens the tree into the rows to draw: children only under expanded parents. */
fun flattenTree(roots: List<PageNode>, expanded: Set<String>): List<TreeRow> {
    val out = mutableListOf<TreeRow>()
    fun visit(node: PageNode, depth: Int) {
        val open = node.page.id in expanded
        out += TreeRow(node.page, depth, node.children.isNotEmpty(), open)
        if (open) node.children.forEach { visit(it, depth + 1) }
    }
    roots.forEach { visit(it, 0) }
    return out
}

private fun TreeView.allPages(): Map<String, PageInfo> {
    val map = mutableMapOf<String, PageInfo>()
    fun visit(n: PageNode) { map[n.page.id] = n.page; n.children.forEach(::visit) }
    roots.forEach(::visit)
    return map
}

@OptIn(ExperimentalCoroutinesApi::class)
@HiltViewModel
class HomeViewModel @Inject constructor(
    spacesRepo: SpacesRepository,
    private val pages: PagesRepository,
    private val sync: SyncRepository,
    private val settings: SettingsRepository,
    private val opener: PageOpener,
) : ViewModel() {
    private val selectedId = MutableStateFlow<String?>(null)
    private val expanded = MutableStateFlow<Set<String>>(emptySet())
    private val refreshing = MutableStateFlow(false)
    private val message = MutableStateFlow<String?>(null)
    private val retry = MutableStateFlow(0)

    private val spaces = retry.flatMapLatest { spacesRepo.spaces }

    private val selected = combine(spaces, selectedId) { s, id ->
        val list = (s as? Loadable.Data)?.value.orEmpty()
        list.firstOrNull { it.id == id } ?: list.firstOrNull()
    }

    private val tree = combine(selected, retry) { space, _ -> space }.flatMapLatest { space ->
        if (space == null) flowOf(null) else spacesRepo.tree(space.id)
    }

    val state: StateFlow<HomeUiState> = combine(spaces, selected, tree, expanded, combine(refreshing, message) { r, m -> r to m }) { s, sel, t, exp, rm ->
        val spaceList = (s as? Loadable.Data)?.value.orEmpty()
        when {
            s is Loadable.Error -> HomeUiState(loading = false, error = s.message, refreshing = rm.first, message = rm.second)
            s is Loadable.Loading || (sel != null && t == null) || (sel != null && t is Loadable.Loading) -> HomeUiState(spaces = spaceList, selectedSpace = sel, refreshing = rm.first, message = rm.second)
            t is Loadable.Error -> HomeUiState(loading = false, error = t.message, spaces = spaceList, selectedSpace = sel, refreshing = rm.first, message = rm.second)
            t is Loadable.Data -> {
                val index = t.value.allPages()
                HomeUiState(
                    loading = false,
                    spaces = spaceList,
                    selectedSpace = sel,
                    favorites = t.value.favorites.mapNotNull { index[it] },
                    rows = flattenTree(t.value.roots, exp),
                    refreshing = rm.first,
                    message = rm.second,
                )
            }
            else -> HomeUiState(loading = false, spaces = spaceList, selectedSpace = sel, refreshing = rm.first, message = rm.second)
        }
    }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), HomeUiState())

    init {
        viewModelScope.launch { selectedId.value = settings.current().lastSpaceId }
    }

    fun selectSpace(id: String) {
        selectedId.value = id
        viewModelScope.launch { settings.setLastSpace(id) }
    }

    fun toggleExpanded(pageId: String) = expanded.update { if (pageId in it) it - pageId else it + pageId }

    fun openPage(pageId: String) = opener.open(pageId)

    fun newPage() {
        val space = state.value.selectedSpace ?: return
        viewModelScope.launch {
            try {
                val page = pages.create(space.id)
                opener.open(page.id, "title")
            } catch (e: Exception) {
                message.value = e.message ?: "error"
            }
        }
    }

    /** Pull to refresh: a sync round, then the tree reloads by itself through the core's events. */
    fun refresh() {
        if (refreshing.value) return
        viewModelScope.launch {
            refreshing.value = true
            try {
                sync.syncNow()
            } catch (e: Exception) {
                message.value = e.message ?: "error"
            } finally {
                refreshing.value = false
            }
        }
    }

    fun retryLoad() = retry.update { it + 1 }
    fun messageShown() { message.value = null }
}
