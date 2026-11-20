package io.atomik.kb.features.share

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Image
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.atomik.kb.R
import io.atomik.kb.data.Loadable
import io.atomik.kb.data.SharedContent
import io.atomik.kb.engine.model.SpaceInfo
import io.atomik.kb.ui.components.BackButton
import io.atomik.kb.ui.components.EmptyState
import io.atomik.kb.ui.components.ErrorState
import io.atomik.kb.ui.components.LoadingBox
import io.atomik.kb.ui.theme.KbTheme

@Composable
fun ShareScreen(onClose: () -> Unit, viewModel: ShareViewModel = hiltViewModel()) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    LaunchedEffect(state.done) { if (state.done) onClose() }
    ShareContent(
        state = state,
        onSelectSpace = viewModel::selectSpace,
        onCreate = viewModel::createPage,
        onCancel = { viewModel.cancel(); onClose() },
    )
}

@Composable
fun ShareContent(
    state: ShareUiState,
    onSelectSpace: (String) -> Unit,
    onCreate: () -> Unit,
    onCancel: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Scaffold(
        modifier = modifier,
        topBar = { TopAppBar(title = { Text(stringResource(R.string.share_title)) }, navigationIcon = { BackButton(onCancel) }) },
    ) { padding ->
        val content = state.content
        val spaces = state.spaces
        Column(Modifier.padding(padding).fillMaxSize()) {
            when {
                content == null -> EmptyState(Icons.Filled.Image, stringResource(R.string.share_nothing_title), stringResource(R.string.share_nothing_hint))
                spaces is Loadable.Loading -> LoadingBox()
                spaces is Loadable.Error -> ErrorState(spaces.message, onRetry = null)
                else -> {
                    val list = (spaces as Loadable.Data).value
                    Column(Modifier.padding(16.dp)) {
                        content.text?.let {
                            Text(it, maxLines = 4, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyMedium)
                        }
                        if (content.images.isNotEmpty()) {
                            Text(
                                pluralStringResource(R.plurals.share_images, content.images.size, content.images.size),
                                style = MaterialTheme.typography.bodyMedium,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                        Text(
                            stringResource(R.string.share_choose_space),
                            style = MaterialTheme.typography.labelLarge,
                            color = MaterialTheme.colorScheme.primary,
                            modifier = Modifier.padding(top = 16.dp, bottom = 4.dp),
                        )
                    }
                    LazyColumn(Modifier.weight(1f).selectableGroup()) {
                        items(list, key = { it.id }) { space ->
                            Row(
                                Modifier.fillMaxWidth().heightIn(min = 56.dp)
                                    .selectable(selected = space.id == state.selectedSpaceId, role = Role.RadioButton, onClick = { onSelectSpace(space.id) })
                                    .padding(horizontal = 16.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                RadioButton(selected = space.id == state.selectedSpaceId, onClick = null)
                                Text(space.name, modifier = Modifier.padding(start = 16.dp))
                            }
                        }
                    }
                    state.error?.let { Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = 16.dp)) }
                    Row(Modifier.fillMaxWidth().padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                        TextButton(onClick = onCancel, modifier = Modifier.heightIn(min = 48.dp)) { Text(stringResource(R.string.cancel)) }
                        Row(Modifier.weight(1f)) {}
                        Button(onClick = onCreate, enabled = !state.busy && state.selectedSpaceId != null, modifier = Modifier.heightIn(min = 48.dp)) {
                            if (state.busy) CircularProgressIndicator(Modifier.padding(end = 8.dp).heightIn(max = 18.dp), strokeWidth = 2.dp)
                            Text(stringResource(R.string.share_create))
                        }
                    }
                }
            }
        }
    }
}

@Preview(showBackground = true)
@Composable
private fun SharePreview() {
    KbTheme(dynamicColor = false) {
        ShareContent(
            state = ShareUiState(
                content = SharedContent(text = "https://example.com/article\nAn interesting read"),
                spaces = Loadable.Data(listOf(SpaceInfo("s1", "Personal"), SpaceInfo("s2", "Work"))),
                selectedSpaceId = "s1",
            ),
            onSelectSpace = {}, onCreate = {}, onCancel = {},
        )
    }
}
