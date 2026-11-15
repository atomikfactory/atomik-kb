package io.atomik.kb.features.home

import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.StickyNote2
import androidx.compose.material3.adaptive.layout.AnimatedPane
import androidx.compose.material3.adaptive.layout.ListDetailPaneScaffoldRole
import androidx.compose.material3.adaptive.navigation.NavigableListDetailPaneScaffold
import androidx.compose.material3.adaptive.navigation.rememberListDetailPaneScaffoldNavigator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import io.atomik.kb.R
import io.atomik.kb.data.OpenPage
import io.atomik.kb.features.page.PageScreen
import io.atomik.kb.ui.components.EmptyState
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.launch

/**
 * Home with the page opened beside it on wide screens (list-detail), one pane at a time on phones.
 * Every way of opening a page (a row, search, capture, share, an internal link) goes through
 * [openRequests], so the editor lives in exactly one place.
 */
@Composable
fun HomeRoute(
    openRequests: Flow<OpenPage>,
    onSearch: () -> Unit,
    onSettings: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val navigator = rememberListDetailPaneScaffoldNavigator<String>()
    val scope = rememberCoroutineScope()
    var selected by rememberSaveable { mutableStateOf<String?>(null) }
    var focus by rememberSaveable { mutableStateOf<String?>(null) }

    fun open(pageId: String, focusOn: String?) {
        selected = pageId
        focus = focusOn
        scope.launch { navigator.navigateTo(ListDetailPaneScaffoldRole.Detail, pageId) }
    }

    LaunchedEffect(openRequests) { openRequests.collect { open(it.pageId, it.focus) } }

    NavigableListDetailPaneScaffold(
        navigator = navigator,
        modifier = modifier.fillMaxSize(),
        listPane = {
            AnimatedPane {
                HomeScreen(onSearch = onSearch, onSettings = onSettings)
            }
        },
        detailPane = {
            AnimatedPane {
                val pageId = selected
                if (pageId == null) {
                    EmptyState(
                        icon = Icons.AutoMirrored.Filled.StickyNote2,
                        title = stringResource(R.string.select_page_title),
                        hint = stringResource(R.string.select_page_hint),
                    )
                } else {
                    PageScreen(
                        pageId = pageId,
                        focus = focus,
                        onBack = if (navigator.scaffoldDirective.maxHorizontalPartitions == 1) {
                            { scope.launch { navigator.navigateBack() } }
                        } else null,
                        onOpenPage = { open(it, null) },
                        onClosed = {
                            selected = null
                            scope.launch { navigator.navigateBack() }
                        },
                    )
                }
            }
        },
    )
}
