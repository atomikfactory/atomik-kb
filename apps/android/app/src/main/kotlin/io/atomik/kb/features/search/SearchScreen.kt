package io.atomik.kb.features.search

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Clear
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.atomik.kb.R
import io.atomik.kb.engine.model.SearchHit
import io.atomik.kb.engine.model.TextRange
import io.atomik.kb.ui.components.BackButton
import io.atomik.kb.ui.components.EmptyState
import io.atomik.kb.ui.components.ErrorState
import io.atomik.kb.ui.components.LoadingBox
import io.atomik.kb.ui.theme.KbTheme

@Composable
fun SearchScreen(onClose: () -> Unit, viewModel: SearchViewModel = hiltViewModel()) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    SearchContent(
        state = state,
        onQueryChange = viewModel::onQueryChange,
        onOpen = { hit -> viewModel.open(hit); onClose() },
        onRetry = viewModel::retry,
        onBack = onClose,
    )
}

/** The snippet with the matched ranges in bold primary; ranges are UTF-16 offsets into the snippet. */
fun highlightedSnippet(snippet: String, highlights: List<TextRange>, color: androidx.compose.ui.graphics.Color): AnnotatedString =
    buildAnnotatedString {
        var at = 0
        for (range in highlights.sortedBy { it.start }) {
            val start = range.start.coerceIn(at, snippet.length)
            val end = range.end.coerceIn(start, snippet.length)
            append(snippet.substring(at, start))
            withStyle(SpanStyle(fontWeight = FontWeight.Bold, color = color)) { append(snippet.substring(start, end)) }
            at = end
        }
        append(snippet.substring(at))
    }

@Composable
fun SearchContent(
    state: SearchUiState,
    onQueryChange: (String) -> Unit,
    onOpen: (SearchHit) -> Unit,
    onRetry: () -> Unit,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    Scaffold(
        modifier = modifier,
        topBar = {
            TopAppBar(
                navigationIcon = { BackButton(onBack) },
                title = {
                    TextField(
                        value = state.query,
                        onValueChange = onQueryChange,
                        singleLine = true,
                        placeholder = { Text(stringResource(R.string.search_hint)) },
                        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                        keyboardActions = KeyboardActions(onSearch = { onQueryChange(state.query) }),
                        colors = TextFieldDefaults.colors(focusedContainerColor = androidx.compose.ui.graphics.Color.Transparent, unfocusedContainerColor = androidx.compose.ui.graphics.Color.Transparent),
                        trailingIcon = {
                            if (state.query.isNotEmpty()) {
                                IconButton(onClick = { onQueryChange("") }) { Icon(Icons.Filled.Clear, contentDescription = stringResource(R.string.clear)) }
                            }
                        },
                        modifier = Modifier.fillMaxWidth().focusRequester(focus),
                    )
                },
            )
        },
    ) { padding ->
        Column(Modifier.padding(padding).fillMaxSize()) {
            when {
                state.error != null -> ErrorState(state.error, onRetry)
                state.loading && state.results.isEmpty() -> LoadingBox()
                state.query.isBlank() -> EmptyState(Icons.Filled.Search, stringResource(R.string.search_empty_title), stringResource(R.string.search_empty_hint))
                state.noResults -> EmptyState(Icons.Filled.Search, stringResource(R.string.search_none_title), stringResource(R.string.search_none_hint))
                else -> LazyColumn(Modifier.fillMaxSize()) {
                    items(state.results, key = { it.pageId + ":" + (it.blockId ?: it.commentId ?: "title") }) { hit ->
                        ResultRow(hit, onOpen = { onOpen(hit) })
                        HorizontalDivider()
                    }
                }
            }
        }
    }
}

@Composable
private fun ResultRow(hit: SearchHit, onOpen: () -> Unit) {
    val title = hit.title.ifBlank { stringResource(R.string.untitled) }
    Column(
        Modifier.fillMaxWidth().heightIn(min = 56.dp).clickable(role = Role.Button, onClick = onOpen).padding(horizontal = 16.dp, vertical = 10.dp),
    ) {
        Text(title, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
        if (hit.snippet.isNotBlank() && hit.snippet != hit.title) {
            Text(
                highlightedSnippet(hit.snippet, hit.highlights, MaterialTheme.colorScheme.primary),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

@Preview(showBackground = true)
@Composable
private fun SearchPreview() {
    KbTheme(dynamicColor = false) {
        SearchContent(
            state = SearchUiState(
                query = "coffee",
                results = listOf(SearchHit("p1", "s1", "Shopping list", snippet = "Milk, eggs, coffee and bread", highlights = listOf(TextRange(11, 17)))),
            ),
            onQueryChange = {}, onOpen = {}, onRetry = {}, onBack = {},
        )
    }
}
