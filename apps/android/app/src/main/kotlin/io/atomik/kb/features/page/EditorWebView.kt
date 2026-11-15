package io.atomik.kb.features.page

import android.annotation.SuppressLint
import android.content.Context
import android.net.Uri
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.viewinterop.AndroidView
import androidx.webkit.JavaScriptReplyProxy
import androidx.webkit.WebMessageCompat
import androidx.webkit.WebSettingsCompat
import androidx.webkit.WebViewAssetLoader
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import kotlinx.coroutines.flow.Flow
import java.io.ByteArrayInputStream

const val EDITOR_HOST = "appassets.androidplatform.net"
const val EDITOR_ORIGIN = "https://$EDITOR_HOST"
const val EDITOR_URL = "$EDITOR_ORIGIN/editor/index.html"
const val HOST_OBJECT = "kbHost"

/** The editor bundle is copied into `assets/editor/` by the build; it can be missing in a dev checkout. */
fun editorBundleAvailable(context: Context): Boolean =
    runCatching { context.assets.open("editor/index.html").close() }.isSuccess

fun isWebMessageSupported(): Boolean = WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)

/** Serves `assets/editor/...` at `https://appassets.androidplatform.net/editor/...`. */
private class EditorAssetsHandler(context: Context) : WebViewAssetLoader.PathHandler {
    private val assets = WebViewAssetLoader.AssetsPathHandler(context)
    override fun handle(path: String): WebResourceResponse? = assets.handle("editor/$path")
}

/** Is [url] a page of the editor itself (the only thing the WebView may load)? */
fun isEditorUrl(url: Uri): Boolean = url.scheme == "https" && url.host == EDITOR_HOST && (url.path ?: "").startsWith("/editor/")

private fun blocked() = WebResourceResponse("text/plain", "utf-8", 403, "Blocked", emptyMap(), ByteArrayInputStream(ByteArray(0)))

/**
 * The editor WebView (A2). Messages from the page arrive on `window.kbHost` through
 * `addWebMessageListener` for [EDITOR_ORIGIN] only; [outbound] strings are posted back to it.
 * The page cannot load anything from the network: other hosts get a 403, and links leave through
 * [onExternalLink].
 */
@SuppressLint("SetJavaScriptEnabled")
@Composable
fun EditorWebView(
    onMessage: (String) -> Unit,
    outbound: Flow<String>,
    onExternalLink: (String) -> Unit,
    onRenderProcessGone: () -> Unit,
    backgroundColor: Int,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    var proxy by remember { mutableStateOf<JavaScriptReplyProxy?>(null) }
    val currentOnMessage by rememberUpdatedState(onMessage)
    val currentOnExternal by rememberUpdatedState(onExternalLink)
    val currentOnGone by rememberUpdatedState(onRenderProcessGone)

    LaunchedEffect(outbound) {
        outbound.collect { message ->
            if (isWebMessageSupported()) proxy?.postMessage(message)
        }
    }

    AndroidView(
        modifier = modifier,
        factory = { ctx ->
            val loader = WebViewAssetLoader.Builder()
                .setDomain(EDITOR_HOST)
                .addPathHandler("/editor/", EditorAssetsHandler(ctx))
                .build()
            WebView(ctx).apply {
                setBackgroundColor(backgroundColor)
                settings.apply {
                    javaScriptEnabled = true
                    domStorageEnabled = true
                    allowFileAccess = false
                    allowContentAccess = false
                    setSupportZoom(false)
                    textZoom = 100
                    setSupportMultipleWindows(false)
                    mixedContentMode = android.webkit.WebSettings.MIXED_CONTENT_NEVER_ALLOW
                }
                if (WebViewFeature.isFeatureSupported(WebViewFeature.ALGORITHMIC_DARKENING)) {
                    WebSettingsCompat.setAlgorithmicDarkeningAllowed(settings, false)
                }
                webViewClient = object : WebViewClient() {
                    override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? {
                        loader.shouldInterceptRequest(request.url)?.let { return it }
                        val scheme = request.url.scheme
                        return if (scheme == "http" || scheme == "https") blocked() else null
                    }

                    override fun onRenderProcessGone(view: WebView, detail: android.webkit.RenderProcessGoneDetail): Boolean {
                        // The renderer died (crash or out of memory); the screen builds a fresh WebView.
                        currentOnGone()
                        return true
                    }

                    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                        if (isEditorUrl(request.url)) return false
                        currentOnExternal(request.url.toString())
                        return true
                    }
                }
                if (isWebMessageSupported()) {
                    WebViewCompat.addWebMessageListener(
                        this, HOST_OBJECT, setOf(EDITOR_ORIGIN),
                    ) { _, message: WebMessageCompat, sourceOrigin: Uri, isMainFrame: Boolean, replyProxy: JavaScriptReplyProxy ->
                        if (isMainFrame && sourceOrigin.toString() == EDITOR_ORIGIN) {
                            proxy = replyProxy
                            message.data?.let { currentOnMessage(it) }
                        }
                    }
                }
                loadUrl(EDITOR_URL)
            }
        },
        onRelease = { webView ->
            webView.stopLoading()
            webView.destroy()
        },
    )
}
