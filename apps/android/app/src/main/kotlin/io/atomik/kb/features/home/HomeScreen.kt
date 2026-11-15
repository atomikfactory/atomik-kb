package io.atomik.kb.features.home

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.NoteAdd
import androidx.compose.material.icons.automirrored.filled.StickyNote2
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.ArrowDropDown
import androidx.compose.material.icons.filled.ChevronRight
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Star
import androidx.compose.material.icons.outlined.Description
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.atomik.kb.R
import io.atomik.kb.engine.model.PageInfo
import io.atomik.kb.engine.model.SpaceInfo
import io.atomik.kb.ui.components.EmptyState
import io.atomik.kb.ui.components.ErrorState
import io.atomik.kb.ui.components.LoadingBox
import io.atomik.kb.ui.theme.KbTheme

@Composable
fun HomeScreen(
    onSearch: () -> Unit,
    onSettings: () -> Unit,
    modifier: Modifier = Modifier,
    viewModel: HomeViewModel = hiltViewModel(),
) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    HomeContent(
        state = state,
        onSelectSpace = viewModel::selectSpace,
        onToggleExpanded = viewModel::toggleExpanded,
        onOpenPage = viewModel::openPage,
        onNewPage = viewModel::newPage,
        onSearch = onSearch,
        onSettings = onSettings,
        onRefresh = viewModel::refresh,
        onRetry = viewModel::retryLoad,
        onMessageShown = viewModel::messageShown,
        modifier = modifier,
    )
}

/** Stateless home: space switcher, favourites, the page tree, FAB for a new page. */
@Composable
fun HomeContent(
    state: HomeUiState,
    onSelectSpace: (String) -> Unit,
    onToggleExpanded: (String) -> Unit,
    onOpenPage: (String) -> Unit,
    onNewPage: () -> Unit,
    onSearch: () -> Unit,
    onSettings: () -> Unit,
    onRefresh: () -> Unit,
    onRetry: () -> Unit,
    onMessageShown: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val snackbar = remember { SnackbarHostState() }
    LaunchedEffect(state.message) {
        state.message?.let { snackbar.showSnackbar(it); onMessageShown() }
    }
    Scaffold(
        modifier = modifier,
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            TopAppBar(
                title = { SpaceSwitcher(state.spaces, state.selectedSpace, onSelectSpace) },
                actions = {
                    IconButton(onClick = onSearch) { Icon(Icons.Filled.Search, contentDescription = stringResource(R.string.search)) }
                    IconButton(onClick = onSettings) { Icon(Icons.Filled.Settings, contentDescription = stringResource(R.string.settings)) }
                },
            )
        },
        floatingActionButton = {
            if (state.selectedSpace != null) {
                ExtendedFloatingActionButton(
                    onClick = onNewPage,
                    icon = { Icon(Icons.Filled.Add, contentDescription = null) },
                    text = { Text(stringResource(R.string.new_page)) },
                )
            }
        },
    ) { padding ->
        PullToRefreshBox(isRefreshing = state.refreshing, onRefresh = onRefresh, modifier = Modifier.padding(padding).fillMaxSize()) {
            when {
                state.loading -> LoadingBox()
                state.error != null -> ErrorState(state.error, onRetry)
                state.selectedSpace == null -> EmptyState(
                    icon = Icons.AutoMirrored.Filled.StickyNote2,
                    title = stringResource(R.string.no_spaces_title),
                    hint = stringResource(R.string.no_spaces_hint),
                )
                state.isEmpty -> EmptyState(
                    icon = Icons.AutoMirrored.Filled.NoteAdd,
                    title = stringResource(R.string.no_pages_title),
                    hint = stringResource(R.string.no_pages_hint),
                    actionLabel = stringResource(R.string.new_page),
                    onAction = onNewPage,
                )
                else -> PageList(state, onToggleExpanded, onOpenPage)
            }
        }
    }
}

@Composable
private fun PageList(state: HomeUiState, onToggleExpanded: (String) -> Unit, onOpenPage: (String) -> Unit) {
    LazyColumn(modifier = Modifier.fillMaxSize(), contentPadding = androidx.compose.foundation.layout.PaddingValues(bottom = 96.dp)) {
        if (state.favorites.isNotEmpty()) {
            item(key = "favorites-header") { SectionHeader(stringResource(R.string.favorites)) }
            items(state.favorites, key = { "fav-" + it.id }) { page ->
                PageRowItem(page, depth = 0, hasChildren = false, expanded = false, favorite = true, onToggle = {}, onOpen = { onOpenPage(page.id) })
            }
        }
        item(key = "pages-header") { SectionHeader(stringResource(R.string.pages)) }
        items(state.rows, key = { it.page.id }) { row ->
            PageRowItem(row.page, row.depth, row.hasChildren, row.expanded, favorite = false, onToggle = { onToggleExpanded(row.page.id) }, onOpen = { onOpenPage(row.page.id) })
        }
    }
}

@Composable
private fun SectionHeader(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp).semantics { heading() },
    )
}

@Composable
private fun PageRowItem(
    page: PageInfo,
    depth: Int,
    hasChildren: Boolean,
    expanded: Boolean,
    favorite: Boolean,
    onToggle: () -> Unit,
    onOpen: () -> Unit,
) {
    val title = page.title.ifBlank { stringResource(R.string.untitled) }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clickable(role = Role.Button, onClick = onOpen)
            .padding(start = (8 + depth * 20).dp, end = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (hasChildren) {
            val label = stringResource(if (expanded) R.string.collapse else R.string.expand)
            IconButton(onClick = onToggle, modifier = Modifier.semantics { contentDescription = "$label $title" }) {
                Icon(if (expanded) Icons.Filled.ExpandMore else Icons.Filled.ChevronRight, contentDescription = null)
            }
        } else {
            Spacer(Modifier.width(48.dp))
        }
        // The page's own emoji when it has one; named icons ("icon:...") and attachment covers have no glyph here.
        val emoji = page.icon.orEmpty().takeIf { it.isNotBlank() && !it.startsWith("icon:") && !it.startsWith("attachment:") }
        if (emoji != null) {
            Text(emoji, style = MaterialTheme.typography.titleMedium, modifier = Modifier.width(28.dp))
        } else {
            Icon(Icons.Outlined.Description, contentDescription = null, modifier = Modifier.width(28.dp).size(20.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Spacer(Modifier.width(8.dp))
        Text(title, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        if (favorite) Icon(Icons.Filled.Star, contentDescription = stringResource(R.string.favorite), modifier = Modifier.size(16.dp), tint = MaterialTheme.colorScheme.primary)
    }
}

@Composable
private fun SpaceSwitcher(spaces: List<SpaceInfo>, selected: SpaceInfo?, onSelect: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        TextButton(
            onClick = { open = true },
            enabled = spaces.size > 1,
            // A single space is not a switcher, but the name is still the title: keep it readable, not dimmed.
            colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.onSurface, disabledContentColor = MaterialTheme.colorScheme.onSurface),
            modifier = Modifier.heightIn(min = 48.dp),
        ) {
            Text(selected?.name ?: stringResource(R.string.app_name), style = MaterialTheme.typography.titleLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (spaces.size > 1) {
                Icon(Icons.Filled.ArrowDropDown, contentDescription = stringResource(R.string.switch_space))
            }
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            spaces.forEach { space ->
                DropdownMenuItem(
                    text = { Text(space.name) },
                    onClick = { open = false; onSelect(space.id) },
                    modifier = Modifier.semantics { if (space.id == selected?.id) stateDescription = "selected" },
                )
            }
        }
    }
}

@Preview(showBackground = true)
@Composable
private fun HomePreview() {
    val page = { id: String, title: String -> PageInfo(id, "s1", null, title) }
    KbTheme(dynamicColor = false) {
        HomeContent(
            state = HomeUiState(
                loading = false,
                spaces = listOf(SpaceInfo("s1", "Personal"), SpaceInfo("s2", "Work")),
                selectedSpace = SpaceInfo("s1", "Personal"),
                favorites = listOf(page("p1", "Welcome")),
                rows = listOf(
                    TreeRow(page("p1", "Welcome"), 0, true, true),
                    TreeRow(page("p2", "Ideas"), 1, false, false),
                    TreeRow(page("p3", "Shopping list"), 0, false, false),
                ),
            ),
            onSelectSpace = {}, onToggleExpanded = {}, onOpenPage = {}, onNewPage = {}, onSearch = {}, onSettings = {},
            onRefresh = {}, onRetry = {}, onMessageShown = {},
        )
    }
}

@Preview(showBackground = true)
@Composable
private fun HomeEmptyPreview() {
    KbTheme(dynamicColor = false) {
        HomeContent(
            state = HomeUiState(loading = false, spaces = listOf(SpaceInfo("s1", "Personal")), selectedSpace = SpaceInfo("s1", "Personal")),
            onSelectSpace = {}, onToggleExpanded = {}, onOpenPage = {}, onNewPage = {}, onSearch = {}, onSettings = {},
            onRefresh = {}, onRetry = {}, onMessageShown = {},
        )
    }
}
