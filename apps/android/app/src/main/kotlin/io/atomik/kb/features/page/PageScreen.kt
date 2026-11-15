package io.atomik.kb.features.page

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.StickyNote2
import androidx.compose.material.icons.filled.DeleteOutline
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Star
import androidx.compose.material.icons.filled.StarBorder
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.tooling.preview.Preview
import androidx.core.content.ContextCompat
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.atomik.kb.R
import io.atomik.kb.ui.components.BackButton
import io.atomik.kb.ui.components.EmptyState
import io.atomik.kb.ui.components.ErrorState
import io.atomik.kb.ui.components.LoadingBox
import io.atomik.kb.ui.components.openExternal
import io.atomik.kb.ui.theme.KbTheme
import java.util.Locale

@Composable
fun PageScreen(
    pageId: String,
    focus: String?,
    onBack: (() -> Unit)?,
    onOpenPage: (String) -> Unit,
    onClosed: () -> Unit,
    onOpenSettings: () -> Unit = {},
    modifier: Modifier = Modifier,
    viewModel: PageViewModel = hiltViewModel(key = pageId),
) {
    val context = LocalContext.current
    val dark = isSystemInDarkTheme()
    val state by viewModel.state.collectAsStateWithLifecycle()
    val onlineDictation by viewModel.onlineDictation.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    var showRationale by remember { mutableStateOf(false) }
    var reloadKey by remember { mutableIntStateOf(0) }
    val onlineRef by rememberUpdatedState(onlineDictation)

    LaunchedEffect(pageId, focus) { viewModel.load(pageId, focus) }
    LaunchedEffect(dark) { viewModel.setEnvironment(dark, Locale.getDefault().toLanguageTag()) }

    val dictation = remember(context) {
        DictationController(
            context = context,
            onlineAllowed = { onlineRef },
            onPartial = viewModel::onDictationPartial,
            onFinal = viewModel::onDictationFinal,
            onStateChanged = viewModel::onDictationState,
            onProblem = viewModel::onDictationProblem,
        )
    }
    DisposableEffect(dictation) { onDispose { dictation.destroy() } }

    val permissionLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) dictation.start() else viewModel.onDictationProblem(DictationProblem.Unavailable)
    }

    LaunchedEffect(viewModel) {
        viewModel.effects.collect { effect ->
            when (effect) {
                is PageEffect.OpenPage -> onOpenPage(effect.pageId)
                is PageEffect.OpenExternal -> openExternal(context, effect.url)
                PageEffect.RequestDictation -> when {
                    dictation.isListening -> dictation.stop()
                    ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED -> dictation.start()
                    else -> showRationale = true
                }
                PageEffect.Closed -> onClosed()
            }
        }
    }

    val settingsLabel = stringResource(R.string.dictation_open_settings)
    val dictationMessage = state.dictationProblem?.let {
        stringResource(
            when (it) {
                DictationProblem.Unavailable -> R.string.dictation_unavailable
                DictationProblem.OfflineNotInstalled -> R.string.dictation_offline_missing
                DictationProblem.OnlineRequired -> R.string.dictation_online_required
                DictationProblem.Failed -> R.string.dictation_failed
            },
        )
    }
    LaunchedEffect(dictationMessage) {
        if (dictationMessage != null) {
            val needsOptIn = state.dictationProblem == DictationProblem.OnlineRequired ||
                state.dictationProblem == DictationProblem.OfflineNotInstalled
            val result = snackbar.showSnackbar(
                message = dictationMessage,
                actionLabel = if (needsOptIn) settingsLabel else null,
                withDismissAction = needsOptIn,
                duration = if (needsOptIn) SnackbarDuration.Long else SnackbarDuration.Short,
            )
            viewModel.dismissDictationProblem()
            if (result == SnackbarResult.ActionPerformed) onOpenSettings()
        }
    }
    val crashedMessage = if (state.editorCrashed) stringResource(R.string.editor_crashed) else null
    LaunchedEffect(crashedMessage) {
        if (crashedMessage != null) {
            snackbar.showSnackbar(crashedMessage)
            viewModel.dismissEditorCrash()
        }
    }
    LaunchedEffect(state.editorError) {
        state.editorError?.let {
            snackbar.showSnackbar(it)
            viewModel.dismissEditorError()
        }
    }

    if (showRationale) {
        AlertDialog(
            onDismissRequest = { showRationale = false },
            title = { Text(stringResource(R.string.dictation_permission_title)) },
            text = { Text(stringResource(R.string.dictation_permission_body)) },
            confirmButton = {
                TextButton(onClick = { showRationale = false; permissionLauncher.launch(Manifest.permission.RECORD_AUDIO) }) {
                    Text(stringResource(R.string.continue_action))
                }
            },
            dismissButton = { TextButton(onClick = { showRationale = false }) { Text(stringResource(R.string.cancel)) } },
        )
    }

    PageContent(
        state = state,
        snackbar = snackbar,
        onBack = onBack,
        onToggleFavorite = viewModel::toggleFavorite,
        onTrash = viewModel::moveToTrash,
        modifier = modifier,
    ) { padding ->
        when {
            state.loading -> LoadingBox(Modifier.padding(padding))
            state.error != null -> ErrorState(state.error.orEmpty(), onRetry = null, modifier = Modifier.padding(padding))
            !editorBundleAvailable(context) -> EmptyState(
                icon = Icons.AutoMirrored.Filled.StickyNote2,
                title = stringResource(R.string.editor_missing_title),
                hint = stringResource(R.string.editor_missing_hint),
                modifier = Modifier.padding(padding),
            )
            !isWebMessageSupported() -> ErrorState(stringResource(R.string.editor_unsupported), onRetry = null, modifier = Modifier.padding(padding))
            else -> key(reloadKey) {
                EditorWebView(
                    onMessage = viewModel::onEditorMessage,
                    outbound = viewModel.outbound,
                    onExternalLink = { openExternal(context, it) },
                    onRenderProcessGone = { viewModel.onEditorGone(); reloadKey++ },
                    backgroundColor = MaterialTheme.colorScheme.surface.toArgb(),
                    modifier = Modifier.fillMaxSize().padding(padding).imePadding(),
                )
            }
        }
    }
}

/** Top bar and frame around the editor; stateless. */
@Composable
fun PageContent(
    state: PageUiState,
    snackbar: SnackbarHostState,
    onBack: (() -> Unit)?,
    onToggleFavorite: () -> Unit,
    onTrash: () -> Unit,
    modifier: Modifier = Modifier,
    body: @Composable (PaddingValues) -> Unit,
) {
    var menu by remember { mutableStateOf(false) }
    Scaffold(
        modifier = modifier,
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            TopAppBar(
                title = { Text(state.title.ifBlank { stringResource(R.string.untitled) }, maxLines = 1) },
                navigationIcon = { if (onBack != null) BackButton(onBack) },
                actions = {
                    if (state.page != null) {
                        IconButton(onClick = onToggleFavorite) {
                            Icon(
                                if (state.favorite) Icons.Filled.Star else Icons.Filled.StarBorder,
                                contentDescription = stringResource(if (state.favorite) R.string.favorite_remove else R.string.favorite_add),
                            )
                        }
                        Box {
                            IconButton(onClick = { menu = true }) {
                                Icon(Icons.Filled.MoreVert, contentDescription = stringResource(R.string.more_options))
                            }
                            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                                DropdownMenuItem(
                                    text = { Text(stringResource(R.string.move_to_trash)) },
                                    leadingIcon = { Icon(Icons.Filled.DeleteOutline, contentDescription = null) },
                                    onClick = { menu = false; onTrash() },
                                )
                            }
                        }
                    }
                },
            )
        },
        content = body,
    )
}

@Preview
@Composable
private fun PageContentPreview() {
    KbTheme(dynamicColor = false) {
        PageContent(
            state = PageUiState(loading = false, title = "Shopping list", favorite = true),
            snackbar = remember { SnackbarHostState() },
            onBack = {}, onToggleFavorite = {}, onTrash = {},
        ) { padding ->
            EmptyState(Icons.AutoMirrored.Filled.StickyNote2, "Editor", "The editor appears here.", Modifier.padding(padding))
        }
    }
}
