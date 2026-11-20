package io.atomik.kb.features.onboarding

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Cloud
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.activity.compose.BackHandler
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.compose.runtime.getValue
import io.atomik.kb.R
import io.atomik.kb.engine.model.PairingInfo
import io.atomik.kb.engine.model.PairingState
import io.atomik.kb.ui.theme.KbTheme

@Composable
fun OnboardingScreen(viewModel: OnboardingViewModel = hiltViewModel()) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    BackHandler(enabled = state.step != OnboardingStep.Welcome) {
        if (state.step == OnboardingStep.Pairing) viewModel.cancelPairing() else viewModel.goTo(OnboardingStep.Welcome)
    }
    OnboardingContent(
        state = state,
        onStep = viewModel::goTo,
        onServerUrl = viewModel::onServerUrlChange,
        onStartPairing = viewModel::startPairing,
        onConfirm = viewModel::confirmPairing,
        onCancelPairing = viewModel::cancelPairing,
        onWorkspaceName = viewModel::onWorkspaceNameChange,
        onSpaceName = viewModel::onSpaceNameChange,
        onStartFresh = viewModel::startFresh,
    )
}

@Composable
fun OnboardingContent(
    state: OnboardingUiState,
    onStep: (OnboardingStep) -> Unit,
    onServerUrl: (String) -> Unit,
    onStartPairing: () -> Unit,
    onConfirm: () -> Unit,
    onCancelPairing: () -> Unit,
    onWorkspaceName: (String) -> Unit,
    onSpaceName: (String) -> Unit,
    onStartFresh: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Surface(modifier.fillMaxSize()) {
        Column(
            Modifier.safeDrawingPadding().imePadding().verticalScroll(rememberScrollState()).padding(24.dp).fillMaxSize(),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            when (state.step) {
                OnboardingStep.Welcome -> Welcome(onConnect = { onStep(OnboardingStep.Server) }, onFresh = { onStep(OnboardingStep.Fresh) })
                OnboardingStep.Server -> ServerStep(state, onServerUrl, onStartPairing, onBack = { onStep(OnboardingStep.Welcome) })
                OnboardingStep.Pairing -> PairingStep(state, onConfirm, onCancelPairing)
                OnboardingStep.Fresh -> FreshStep(state, onWorkspaceName, onSpaceName, onStartFresh, onBack = { onStep(OnboardingStep.Welcome) })
            }
        }
    }
}

@Composable
private fun Welcome(onConnect: () -> Unit, onFresh: () -> Unit) {
    Icon(Icons.Filled.Cloud, contentDescription = null, modifier = Modifier.size(64.dp), tint = MaterialTheme.colorScheme.primary)
    Spacer(Modifier.height(16.dp))
    Text(stringResource(R.string.welcome_title), style = MaterialTheme.typography.headlineMedium, textAlign = TextAlign.Center)
    Spacer(Modifier.height(8.dp))
    Text(
        stringResource(R.string.welcome_body),
        style = MaterialTheme.typography.bodyLarge,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        textAlign = TextAlign.Center,
    )
    Spacer(Modifier.height(32.dp))
    Button(onClick = onConnect, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { Text(stringResource(R.string.welcome_connect)) }
    Spacer(Modifier.height(8.dp))
    OutlinedButton(onClick = onFresh, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { Text(stringResource(R.string.welcome_fresh)) }
}

@Composable
private fun ServerStep(state: OnboardingUiState, onUrl: (String) -> Unit, onContinue: () -> Unit, onBack: () -> Unit) {
    Text(stringResource(R.string.server_step_title), style = MaterialTheme.typography.headlineSmall)
    Spacer(Modifier.height(8.dp))
    Text(stringResource(R.string.server_step_body), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
    Spacer(Modifier.height(16.dp))
    OutlinedTextField(
        value = state.serverUrl,
        onValueChange = onUrl,
        label = { Text(stringResource(R.string.server_url)) },
        singleLine = true,
        isError = state.serverUrlInvalid,
        supportingText = { Text(stringResource(if (state.serverUrlInvalid) R.string.server_url_invalid else R.string.server_dialog_hint)) },
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go),
        keyboardActions = KeyboardActions(onGo = { onContinue() }),
        modifier = Modifier.fillMaxWidth(),
    )
    state.error?.let { ErrorLine(it) }
    Spacer(Modifier.height(16.dp))
    Button(onClick = onContinue, enabled = state.serverUrl.isNotBlank() && !state.busy, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
        if (state.busy) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp) else Text(stringResource(R.string.continue_action))
    }
    TextButton(onClick = onBack, modifier = Modifier.heightIn(min = 48.dp)) { Text(stringResource(R.string.back)) }
}

@Composable
private fun PairingStep(state: OnboardingUiState, onConfirm: () -> Unit, onCancel: () -> Unit) {
    val pairing = state.pairing ?: PairingInfo()
    when (pairing.state) {
        PairingState.Confirming -> {
            Text(stringResource(R.string.pairing_compare_title), style = MaterialTheme.typography.headlineSmall)
            Spacer(Modifier.height(8.dp))
            Text(
                stringResource(R.string.pairing_compare_body, pairing.peerName ?: stringResource(R.string.pairing_other_device)),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(24.dp))
            val digits = pairing.sas.orEmpty()
            BigCode(digits.chunked(3).joinToString(" "), speak = digits.toList().joinToString(", "))
            Spacer(Modifier.height(24.dp))
            Button(onClick = onConfirm, enabled = !state.busy, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { Text(stringResource(R.string.pairing_match)) }
            Spacer(Modifier.height(8.dp))
            OutlinedButton(onClick = onCancel, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { Text(stringResource(R.string.pairing_no_match)) }
        }
        PairingState.Failed, PairingState.Cancelled -> {
            Text(stringResource(R.string.pairing_failed_title), style = MaterialTheme.typography.headlineSmall)
            ErrorLine(pairing.error ?: stringResource(R.string.pairing_failed_body))
            Spacer(Modifier.height(16.dp))
            Button(onClick = onCancel, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { Text(stringResource(R.string.try_again)) }
        }
        PairingState.PeerConfirming, PairingState.Transferring, PairingState.Done -> {
            CircularProgressIndicator()
            Spacer(Modifier.height(16.dp))
            Text(
                stringResource(if (pairing.state == PairingState.PeerConfirming) R.string.pairing_waiting_peer else R.string.pairing_transferring),
                style = MaterialTheme.typography.bodyLarge,
                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
            )
        }
        else -> {
            Text(stringResource(R.string.pairing_code_title), style = MaterialTheme.typography.headlineSmall)
            Spacer(Modifier.height(8.dp))
            Text(
                stringResource(R.string.pairing_code_body),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(24.dp))
            val code = pairing.code
            if (code == null) CircularProgressIndicator() else BigCode(code, speak = code.replace(" ", ", ").toList().joinToString(" "))
            Spacer(Modifier.height(24.dp))
            TextButton(onClick = onCancel, modifier = Modifier.heightIn(min = 48.dp)) { Text(stringResource(R.string.cancel)) }
        }
    }
}

@Composable
private fun BigCode(text: String, speak: String) {
    Text(
        text,
        style = MaterialTheme.typography.displaySmall.copy(fontFamily = FontFamily.Monospace, letterSpacing = 4.sp),
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.semantics { contentDescription = speak; liveRegion = LiveRegionMode.Polite },
    )
}

@Composable
private fun FreshStep(state: OnboardingUiState, onName: (String) -> Unit, onSpace: (String) -> Unit, onStart: () -> Unit, onBack: () -> Unit) {
    Text(stringResource(R.string.fresh_title), style = MaterialTheme.typography.headlineSmall)
    Spacer(Modifier.height(8.dp))
    Text(stringResource(R.string.fresh_body), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
    Spacer(Modifier.height(16.dp))
    OutlinedTextField(
        value = state.workspaceName, onValueChange = onName, singleLine = true,
        label = { Text(stringResource(R.string.workspace_name)) }, modifier = Modifier.fillMaxWidth(),
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Next),
    )
    Spacer(Modifier.height(8.dp))
    OutlinedTextField(
        value = state.spaceName, onValueChange = onSpace, singleLine = true,
        label = { Text(stringResource(R.string.first_space_name)) }, modifier = Modifier.fillMaxWidth(),
        placeholder = { Text(stringResource(R.string.first_space_default)) },
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done), keyboardActions = KeyboardActions(onDone = { onStart() }),
    )
    state.error?.let { ErrorLine(it) }
    Spacer(Modifier.height(16.dp))
    Button(onClick = onStart, enabled = state.workspaceName.isNotBlank() && !state.busy, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
        if (state.busy) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp) else Text(stringResource(R.string.fresh_create))
    }
    TextButton(onClick = onBack, modifier = Modifier.heightIn(min = 48.dp)) { Text(stringResource(R.string.back)) }
}

@Composable
private fun ErrorLine(text: String) {
    Text(
        text, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium,
        modifier = Modifier.padding(top = 8.dp).semantics { liveRegion = LiveRegionMode.Assertive },
    )
}

@Preview(showBackground = true)
@Composable
private fun WelcomePreview() {
    KbTheme(dynamicColor = false) {
        OnboardingContent(OnboardingUiState(), {}, {}, {}, {}, {}, {}, {}, {})
    }
}

@Preview(showBackground = true)
@Composable
private fun PairingPreview() {
    KbTheme(dynamicColor = false) {
        OnboardingContent(
            OnboardingUiState(step = OnboardingStep.Pairing, pairing = PairingInfo(state = PairingState.Confirming, sas = "482913", peerName = "Desktop")),
            {}, {}, {}, {}, {}, {}, {}, {},
        )
    }
}
