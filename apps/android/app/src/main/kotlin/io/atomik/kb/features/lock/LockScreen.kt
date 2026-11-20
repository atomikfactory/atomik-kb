package io.atomik.kb.features.lock

import androidx.activity.compose.LocalActivity
import android.view.WindowManager
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Fingerprint
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.fragment.app.FragmentActivity
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.atomik.kb.R
import io.atomik.kb.security.authenticateWithCipher
import io.atomik.kb.ui.theme.KbTheme

@Composable
fun LockScreen(viewModel: LockViewModel = hiltViewModel()) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    val activity = LocalActivity.current as? FragmentActivity
    val title = stringResource(R.string.lock_biometric_title)
    val subtitle = stringResource(R.string.lock_biometric_subtitle)
    val cancel = stringResource(R.string.use_passphrase)

    // FLAG_SECURE while the lock is up: no screenshots, and the app switcher shows nothing.
    DisposableEffect(activity) {
        activity?.window?.setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE)
        onDispose { activity?.window?.clearFlags(WindowManager.LayoutParams.FLAG_SECURE) }
    }

    fun promptBiometric() {
        val act = activity ?: return
        val cipher = viewModel.biometricCipher() ?: return
        act.authenticateWithCipher(cipher, title, subtitle, cancel, viewModel::onBiometricSuccess, viewModel::onBiometricFailure)
    }

    LaunchedEffect(state.biometricAvailable) { if (state.biometricAvailable) promptBiometric() }

    LockContent(
        state = state,
        onUnlock = viewModel::unlock,
        onBiometric = ::promptBiometric,
        onEdit = viewModel::clearError,
    )
}

/** The lock screen; stateless apart from the text being typed. */
@Composable
fun LockContent(
    state: LockUiState,
    onUnlock: (String) -> Unit,
    onBiometric: () -> Unit,
    onEdit: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var passphrase by remember { mutableStateOf("") }
    Surface(modifier = modifier.fillMaxSize()) {
        Column(
            Modifier.safeDrawingPadding().imePadding().verticalScroll(rememberScrollState()).padding(24.dp).fillMaxSize(),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Icon(Icons.Filled.Lock, contentDescription = null, modifier = Modifier.size(48.dp), tint = MaterialTheme.colorScheme.primary)
            Spacer(Modifier.height(16.dp))
            Text(stringResource(R.string.lock_title), style = MaterialTheme.typography.headlineSmall)
            Spacer(Modifier.height(8.dp))
            Text(
                stringResource(R.string.lock_hint),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(24.dp))
            OutlinedTextField(
                value = passphrase,
                onValueChange = { passphrase = it; onEdit() },
                label = { Text(stringResource(R.string.passphrase)) },
                singleLine = true,
                visualTransformation = PasswordVisualTransformation(),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { onUnlock(passphrase) }),
                isError = state.error != null,
                modifier = Modifier.fillMaxWidth(),
            )
            state.error?.let {
                Text(
                    stringResource(
                        when (it) {
                            LockError.WrongPassphrase -> R.string.lock_wrong
                            LockError.Failed -> R.string.lock_failed
                            LockError.BiometricInvalidated -> R.string.lock_biometric_invalidated
                        },
                    ),
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(top = 8.dp).semantics { liveRegion = LiveRegionMode.Assertive },
                )
            }
            Spacer(Modifier.height(16.dp))
            Button(
                onClick = { onUnlock(passphrase) },
                enabled = passphrase.isNotEmpty() && !state.busy,
                modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp),
            ) {
                if (state.busy) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp) else Text(stringResource(R.string.unlock))
            }
            if (state.biometricAvailable) {
                Spacer(Modifier.height(8.dp))
                OutlinedButton(onClick = onBiometric, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
                    Icon(Icons.Filled.Fingerprint, contentDescription = null, modifier = Modifier.padding(end = 8.dp))
                    Text(stringResource(R.string.unlock_biometric))
                }
            }
        }
    }
}

@Preview(showBackground = true)
@Composable
private fun LockPreview() {
    KbTheme(dynamicColor = false) {
        LockContent(LockUiState(biometricAvailable = true, error = LockError.WrongPassphrase), onUnlock = {}, onBiometric = {}, onEdit = {})
    }
}
