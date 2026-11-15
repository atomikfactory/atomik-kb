package io.atomik.kb.features.settings

import android.text.format.DateUtils
import androidx.activity.compose.LocalActivity
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
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
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.fragment.app.FragmentActivity
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.atomik.kb.BuildConfig
import io.atomik.kb.R
import io.atomik.kb.data.AppSettings
import io.atomik.kb.data.AutoLockDelay
import io.atomik.kb.data.Loadable
import io.atomik.kb.engine.model.AppLockInfo
import io.atomik.kb.engine.model.DeviceEntry
import io.atomik.kb.engine.model.SecurityInfo
import io.atomik.kb.engine.model.SpaceInfo
import io.atomik.kb.engine.model.SyncInfo
import io.atomik.kb.engine.model.SyncState
import io.atomik.kb.security.authenticateWithCipher
import io.atomik.kb.ui.components.BackButton
import io.atomik.kb.ui.theme.KbTheme
import javax.crypto.Cipher

private sealed interface Dialog {
    data object Server : Dialog
    data object EnableLock : Dialog
    data object DisableLock : Dialog
    data object ChangeLock : Dialog
    data object BiometricPassphrase : Dialog
}

@Composable
fun SettingsScreen(onBack: () -> Unit, viewModel: SettingsViewModel = hiltViewModel()) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    val activity = LocalActivity.current as? FragmentActivity
    val title = stringResource(R.string.biometric_enroll_title)
    val subtitle = stringResource(R.string.biometric_enroll_subtitle)
    val cancel = stringResource(R.string.cancel)
    SettingsContent(
        state = state,
        onBack = onBack,
        onSyncNow = viewModel::syncNow,
        onSaveServer = viewModel::saveServer,
        onEnableLock = viewModel::enableLock,
        onDisableLock = viewModel::disableLock,
        onChangeLock = viewModel::changeLock,
        onAutoLock = viewModel::setAutoLock,
        onOnlineDictation = viewModel::setOnlineDictation,
        onCaptureSpace = viewModel::setCaptureSpace,
        onEnableBiometric = { passphrase, done ->
            viewModel.beginBiometric(passphrase) { cipher: Cipher ->
                done()
                activity?.authenticateWithCipher(
                    cipher, title, subtitle, cancel,
                    onSuccess = { authenticated -> viewModel.completeBiometric(authenticated, passphrase) },
                    onFailure = viewModel::biometricFailed,
                )
            }
        },
        onDisableBiometric = viewModel::disableBiometric,
        onNoticeShown = viewModel::noticeShown,
        onClearDialogError = viewModel::clearDialogError,
    )
}

@Composable
private fun Notice?.text(): String? = when (this) {
    null -> null
    is Notice.Res -> stringResource(id)
    is Notice.Raw -> text
}

@Composable
fun SettingsContent(
    state: SettingsUiState,
    onBack: () -> Unit,
    onSyncNow: () -> Unit,
    onSaveServer: (String, () -> Unit) -> Unit,
    onEnableLock: (String, () -> Unit) -> Unit,
    onDisableLock: (String, () -> Unit) -> Unit,
    onChangeLock: (String, String, () -> Unit) -> Unit,
    onAutoLock: (AutoLockDelay) -> Unit,
    onOnlineDictation: (Boolean) -> Unit,
    onCaptureSpace: (String?) -> Unit,
    onEnableBiometric: (String, () -> Unit) -> Unit,
    onDisableBiometric: () -> Unit,
    onNoticeShown: () -> Unit,
    onClearDialogError: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var dialog by remember { mutableStateOf<Dialog?>(null) }
    val snackbar = remember { SnackbarHostState() }
    val notice = state.notice.text()
    LaunchedEffect(state.notice) {
        if (notice != null) { snackbar.showSnackbar(notice); onNoticeShown() }
    }
    val close = { dialog = null; onClearDialogError() }
    val dialogError = state.dialogError.text()

    Scaffold(
        modifier = modifier,
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = { TopAppBar(title = { Text(stringResource(R.string.settings)) }, navigationIcon = { BackButton(onBack) }) },
    ) { padding ->
        LazyColumn(Modifier.padding(padding).fillMaxSize()) {
            item { SectionTitle(stringResource(R.string.settings_sync)) }
            item { SyncSection(state, onSyncNow, onChangeServer = { dialog = Dialog.Server }) }
            item { HorizontalDivider(Modifier.padding(vertical = 8.dp)) }

            item { SectionTitle(stringResource(R.string.settings_devices)) }
            item { DevicesSection(state.devices) }
            item { HorizontalDivider(Modifier.padding(vertical = 8.dp)) }

            item { SectionTitle(stringResource(R.string.settings_security)) }
            item {
                SecuritySection(
                    state = state,
                    onToggleLock = { enable -> dialog = if (enable) Dialog.EnableLock else Dialog.DisableLock },
                    onChangeLock = { dialog = Dialog.ChangeLock },
                    onToggleBiometric = { enable -> if (enable) dialog = Dialog.BiometricPassphrase else onDisableBiometric() },
                    onAutoLock = onAutoLock,
                )
            }
            item { HorizontalDivider(Modifier.padding(vertical = 8.dp)) }

            item { SectionTitle(stringResource(R.string.settings_capture)) }
            item { CaptureSpaceRow(state.spaces, state.settings.captureSpaceId, onCaptureSpace) }
            item { HorizontalDivider(Modifier.padding(vertical = 8.dp)) }

            item { SectionTitle(stringResource(R.string.settings_dictation)) }
            item {
                ToggleRow(
                    title = stringResource(R.string.dictation_online),
                    summary = stringResource(R.string.dictation_online_summary),
                    checked = state.settings.onlineDictation,
                    onCheckedChange = onOnlineDictation,
                )
            }
            item { HorizontalDivider(Modifier.padding(vertical = 8.dp)) }

            item { SectionTitle(stringResource(R.string.settings_about)) }
            item {
                Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
                    Text(stringResource(R.string.app_name), style = MaterialTheme.typography.bodyLarge)
                    Text(
                        stringResource(R.string.about_version, BuildConfig.VERSION_NAME),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }

    when (dialog) {
        Dialog.Server -> TextInputDialog(
            title = stringResource(R.string.server_dialog_title),
            label = stringResource(R.string.server_url),
            initial = (state.sync as? Loadable.Data)?.value?.serverUrl.orEmpty(),
            hint = stringResource(R.string.server_dialog_hint),
            error = dialogError, busy = state.busy,
            onConfirm = { onSaveServer(it, close) }, onDismiss = close,
        )
        Dialog.EnableLock -> PassphraseDialog(
            title = stringResource(R.string.lock_enable_title),
            fields = listOf(R.string.passphrase, R.string.passphrase_confirm),
            mustMatch = true, error = dialogError, busy = state.busy,
            onConfirm = { onEnableLock(it[0], close) }, onDismiss = close,
        )
        Dialog.DisableLock -> PassphraseDialog(
            title = stringResource(R.string.lock_disable_title),
            fields = listOf(R.string.passphrase_current), error = dialogError, busy = state.busy,
            onConfirm = { onDisableLock(it[0], close) }, onDismiss = close,
        )
        Dialog.ChangeLock -> PassphraseDialog(
            title = stringResource(R.string.lock_change_title),
            fields = listOf(R.string.passphrase_current, R.string.passphrase_new, R.string.passphrase_confirm),
            mustMatch = true, error = dialogError, busy = state.busy,
            onConfirm = { onChangeLock(it[0], it[1], close) }, onDismiss = close,
        )
        Dialog.BiometricPassphrase -> PassphraseDialog(
            title = stringResource(R.string.biometric_enable_title),
            message = stringResource(R.string.biometric_enable_message),
            fields = listOf(R.string.passphrase_current), error = dialogError, busy = state.busy,
            onConfirm = { onEnableBiometric(it[0], close) }, onDismiss = close,
        )
        null -> Unit
    }
}

@Composable
private fun SectionTitle(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp).semantics { heading() },
    )
}

@Composable
private fun SyncSection(state: SettingsUiState, onSyncNow: () -> Unit, onChangeServer: () -> Unit) {
    Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        when (val sync = state.sync) {
            Loadable.Loading -> CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
            is Loadable.Error -> Text(sync.message, color = MaterialTheme.colorScheme.error)
            is Loadable.Data -> {
                val info = sync.value
                Text(info.serverUrl ?: stringResource(R.string.sync_not_configured), style = MaterialTheme.typography.bodyLarge)
                Text(syncStatusText(info), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                info.lastError?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error) }
            }
        }
        val last = state.settings.lastSyncAt
        if (last > 0) {
            Text(
                stringResource(R.string.sync_last, DateUtils.getRelativeTimeSpanString(last).toString()),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(vertical = 8.dp)) {
            Button(
                onClick = onSyncNow,
                enabled = !state.syncing && (state.sync as? Loadable.Data)?.value?.configured == true,
                modifier = Modifier.heightIn(min = 48.dp),
            ) {
                if (state.syncing) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp) else Text(stringResource(R.string.sync_now))
            }
            OutlinedButton(onClick = onChangeServer, modifier = Modifier.heightIn(min = 48.dp)) { Text(stringResource(R.string.server_change)) }
        }
    }
}

@Composable
private fun syncStatusText(info: SyncInfo): String = when {
    !info.configured -> stringResource(R.string.sync_state_disabled)
    info.forkDetected -> stringResource(R.string.sync_state_forked)
    else -> when (info.state) {
        SyncState.Disabled -> stringResource(R.string.sync_state_disabled)
        SyncState.Idle -> stringResource(R.string.sync_state_idle)
        SyncState.Pushing -> stringResource(R.string.sync_state_pushing)
        SyncState.Pulling -> stringResource(R.string.sync_state_pulling)
        SyncState.Offline -> stringResource(R.string.sync_state_offline)
        SyncState.Error -> stringResource(R.string.sync_state_error)
        SyncState.Forked -> stringResource(R.string.sync_state_forked)
    }
}

@Composable
private fun DevicesSection(devices: Loadable<List<DeviceEntry>>) {
    Column(Modifier.padding(horizontal = 16.dp)) {
        when (devices) {
            Loadable.Loading -> CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
            is Loadable.Error -> Text(devices.message, color = MaterialTheme.colorScheme.error)
            is Loadable.Data -> if (devices.value.isEmpty()) {
                Text(stringResource(R.string.devices_none), color = MaterialTheme.colorScheme.onSurfaceVariant)
            } else devices.value.forEach { d ->
                Column(Modifier.heightIn(min = 48.dp).padding(vertical = 4.dp)) {
                    Text(
                        if (d.isThisDevice) stringResource(R.string.device_this, d.name) else d.name,
                        style = MaterialTheme.typography.bodyLarge,
                    )
                    val sub = if (d.revokedAt != null) stringResource(R.string.device_revoked)
                    else stringResource(R.string.device_added, DateUtils.getRelativeTimeSpanString(d.addedAt).toString())
                    Text(sub, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
    }
}

@Composable
private fun SecuritySection(
    state: SettingsUiState,
    onToggleLock: (Boolean) -> Unit,
    onChangeLock: () -> Unit,
    onToggleBiometric: (Boolean) -> Unit,
    onAutoLock: (AutoLockDelay) -> Unit,
) {
    ToggleRow(
        title = stringResource(R.string.lock_toggle),
        summary = stringResource(R.string.lock_toggle_summary),
        checked = state.lockEnabled,
        enabled = state.security is Loadable.Data,
        onCheckedChange = onToggleLock,
    )
    if (state.lockEnabled) {
        TextButton(onClick = onChangeLock, modifier = Modifier.padding(horizontal = 8.dp).heightIn(min = 48.dp)) {
            Text(stringResource(R.string.lock_change))
        }
        ToggleRow(
            title = stringResource(R.string.biometric_toggle),
            summary = stringResource(if (state.biometricSupported) R.string.biometric_toggle_summary else R.string.biometric_unavailable),
            checked = state.settings.biometric != null,
            enabled = state.biometricSupported,
            onCheckedChange = onToggleBiometric,
        )
        AutoLockRow(state.settings.autoLock, onAutoLock)
    }
}

@Composable
private fun AutoLockRow(current: AutoLockDelay, onSelect: (AutoLockDelay) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Row(
        Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(stringResource(R.string.autolock), Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
        androidx.compose.foundation.layout.Box {
            TextButton(onClick = { open = true }, modifier = Modifier.heightIn(min = 48.dp)) { Text(autoLockLabel(current)) }
            DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                AutoLockDelay.entries.forEach { delay ->
                    DropdownMenuItem(text = { Text(autoLockLabel(delay)) }, onClick = { open = false; onSelect(delay) })
                }
            }
        }
    }
}

@Composable
private fun autoLockLabel(delay: AutoLockDelay): String = stringResource(
    when (delay) {
        AutoLockDelay.Immediately -> R.string.autolock_immediately
        AutoLockDelay.OneMinute -> R.string.autolock_1m
        AutoLockDelay.FiveMinutes -> R.string.autolock_5m
        AutoLockDelay.FifteenMinutes -> R.string.autolock_15m
        AutoLockDelay.Never -> R.string.autolock_never
    },
)

@Composable
private fun CaptureSpaceRow(spaces: List<SpaceInfo>, selectedId: String?, onSelect: (String?) -> Unit) {
    var open by remember { mutableStateOf(false) }
    val selected = spaces.firstOrNull { it.id == selectedId }
    Row(
        Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(stringResource(R.string.capture_space), style = MaterialTheme.typography.bodyLarge)
            Text(stringResource(R.string.capture_space_summary), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        androidx.compose.foundation.layout.Box {
            TextButton(onClick = { open = true }, enabled = spaces.isNotEmpty(), modifier = Modifier.heightIn(min = 48.dp)) {
                Text(selected?.name ?: stringResource(R.string.capture_space_last_used))
            }
            DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                DropdownMenuItem(text = { Text(stringResource(R.string.capture_space_last_used)) }, onClick = { open = false; onSelect(null) })
                spaces.forEach { s -> DropdownMenuItem(text = { Text(s.name) }, onClick = { open = false; onSelect(s.id) }) }
            }
        }
    }
}

@Composable
private fun ToggleRow(
    title: String,
    summary: String,
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    enabled: Boolean = true,
) {
    Row(
        Modifier.fillMaxWidth().heightIn(min = 56.dp)
            .toggleable(value = checked, enabled = enabled, role = Role.Switch, onValueChange = onCheckedChange)
            .padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f).padding(end = 16.dp)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            Text(summary, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Switch(checked = checked, onCheckedChange = null, enabled = enabled)
    }
}

@Composable
private fun TextInputDialog(
    title: String,
    label: String,
    initial: String,
    hint: String,
    error: String?,
    busy: Boolean,
    onConfirm: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    var value by remember { mutableStateOf(initial) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            Column {
                OutlinedTextField(
                    value = value, onValueChange = { value = it }, label = { Text(label) }, singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri), isError = error != null,
                    supportingText = { Text(error ?: hint) }, modifier = Modifier.fillMaxWidth(),
                )
            }
        },
        confirmButton = { TextButton(onClick = { onConfirm(value) }, enabled = !busy) { Text(stringResource(R.string.save)) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

/** A dialog with one to three passphrase fields; when [mustMatch] the last field repeats the one before. */
@Composable
private fun PassphraseDialog(
    title: String,
    fields: List<Int>,
    error: String?,
    busy: Boolean,
    onConfirm: (List<String>) -> Unit,
    onDismiss: () -> Unit,
    message: String? = null,
    mustMatch: Boolean = false,
) {
    val values = remember { mutableStateOf(List(fields.size) { "" }) }
    val mismatch = mustMatch && fields.size >= 2 && values.value[fields.size - 1] != values.value[fields.size - 2]
    val filled = values.value.all { it.isNotEmpty() }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                message?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
                fields.forEachIndexed { i, label ->
                    OutlinedTextField(
                        value = values.value[i],
                        onValueChange = { v -> values.value = values.value.toMutableList().also { it[i] = v } },
                        label = { Text(stringResource(label)) },
                        singleLine = true,
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                        isError = error != null || (mismatch && i == fields.size - 1 && values.value[i].isNotEmpty()),
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
                if (mismatch && values.value.last().isNotEmpty()) {
                    Text(stringResource(R.string.passphrase_mismatch), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                }
                error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
            }
        },
        confirmButton = {
            TextButton(onClick = { onConfirm(values.value) }, enabled = filled && !mismatch && !busy) { Text(stringResource(R.string.continue_action)) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

@Preview(showBackground = true, heightDp = 900)
@Composable
private fun SettingsPreview() {
    KbTheme(dynamicColor = false) {
        SettingsContent(
            state = SettingsUiState(
                sync = Loadable.Data(SyncInfo(configured = true, serverUrl = "https://kb.example.org", state = SyncState.Idle)),
                devices = Loadable.Data(listOf(DeviceEntry("d1", "Pixel", 0, isThisDevice = true), DeviceEntry("d2", "Desktop", 0))),
                security = Loadable.Data(SecurityInfo(appLock = AppLockInfo(enabled = true))),
                settings = AppSettings(),
                biometricSupported = true,
            ),
            onBack = {}, onSyncNow = {}, onSaveServer = { _, _ -> }, onEnableLock = { _, _ -> }, onDisableLock = { _, _ -> },
            onChangeLock = { _, _, _ -> }, onAutoLock = {}, onOnlineDictation = {}, onCaptureSpace = {},
            onEnableBiometric = { _, _ -> }, onDisableBiometric = {}, onNoticeShown = {}, onClearDialogError = {},
        )
    }
}
