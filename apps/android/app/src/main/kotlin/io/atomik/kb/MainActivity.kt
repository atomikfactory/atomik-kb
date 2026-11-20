package io.atomik.kb

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.core.content.IntentCompat
import androidx.fragment.app.FragmentActivity
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dagger.hilt.android.AndroidEntryPoint
import io.atomik.kb.data.PageOpener
import io.atomik.kb.data.SharedContent
import io.atomik.kb.data.ShareInbox
import io.atomik.kb.features.lock.LockScreen
import io.atomik.kb.features.onboarding.OnboardingScreen
import io.atomik.kb.ui.components.ErrorState
import io.atomik.kb.ui.components.LoadingBox
import io.atomik.kb.ui.nav.AppNavHost
import io.atomik.kb.ui.nav.IntentRouter
import io.atomik.kb.ui.nav.NavRequest
import io.atomik.kb.ui.theme.KbTheme
import javax.inject.Inject

/** FragmentActivity because BiometricPrompt needs one; everything else is Compose. */
@AndroidEntryPoint
class MainActivity : FragmentActivity() {
    @Inject lateinit var router: IntentRouter
    @Inject lateinit var shared: ShareInbox
    @Inject lateinit var opener: PageOpener
    private val root: RootViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) handle(intent)
        setContent {
            KbTheme {
                Surface(Modifier.fillMaxSize()) {
                    AppRoot(root, opener, router)
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handle(intent)
    }

    private fun handle(intent: Intent?) {
        intent ?: return
        when (intent.action) {
            ACTION_CAPTURE -> router.request(NavRequest.Capture)
            Intent.ACTION_SEND, Intent.ACTION_SEND_MULTIPLE -> {
                val streams: List<Uri> = if (intent.action == Intent.ACTION_SEND_MULTIPLE) {
                    IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
                } else {
                    listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
                }
                val content = SharedContent.from(
                    action = intent.action,
                    mime = intent.type,
                    text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString(),
                    subject = intent.getStringExtra(Intent.EXTRA_SUBJECT),
                    streams = streams,
                )
                if (content != null) {
                    shared.set(content)
                    router.request(NavRequest.Share)
                }
            }
        }
    }

    companion object {
        const val ACTION_CAPTURE = "io.atomik.kb.action.CAPTURE"
    }
}

@Composable
private fun AppRoot(root: RootViewModel, opener: PageOpener, router: IntentRouter) {
    val state by root.state.collectAsStateWithLifecycle()
    Box(Modifier.fillMaxSize()) {
        when (val s = state) {
            RootUiState.Loading -> LoadingBox()
            is RootUiState.Failed -> ErrorState(s.message, onRetry = root::retry)
            RootUiState.Locked -> LockScreen()
            RootUiState.Onboarding -> OnboardingScreen()
            RootUiState.Main -> AppNavHost(opener = opener, navRequests = router.requests)
        }
    }
}
