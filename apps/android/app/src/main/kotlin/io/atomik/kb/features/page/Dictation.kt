package io.atomik.kb.features.page

import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer

/** Why dictation could not start. */
enum class DictationProblem {
    Unavailable,

    /** API 33+, no on-device recognizer installed and online dictation is off. */
    OfflineNotInstalled,

    /** API 28-32: on-device recognition cannot be guaranteed, so dictation needs the online opt-in. */
    OnlineRequired,
    Failed,
}

/**
 * Speech to text through `SpeechRecognizer` (A9): on-device only unless [onlineAllowed]. On API 33+
 * the on-device recognizer is used when installed; otherwise (and always on API 28-32) dictation needs the
 * online opt-in, and then the system recognizer is asked to prefer offline results.
 * Partial results go to [onPartial]; each final phrase to [onFinal]; recognition then restarts until
 * [stop], so a tap on the mic means "keep listening".
 */
class DictationController(
    private val context: Context,
    private val onlineAllowed: () -> Boolean,
    private val onPartial: (String) -> Unit,
    private val onFinal: (String) -> Unit,
    private val onStateChanged: (listening: Boolean) -> Unit,
    private val onProblem: (DictationProblem) -> Unit,
) {
    private var recognizer: SpeechRecognizer? = null
    private var active = false

    val isListening: Boolean get() = active

    fun start() {
        if (active) return
        val created = create() ?: return
        recognizer = created
        active = true
        created.setRecognitionListener(listener)
        created.startListening(recognizerIntent())
        onStateChanged(true)
    }

    fun stop() {
        if (!active && recognizer == null) return
        active = false
        recognizer?.let { runCatching { it.stopListening() }; runCatching { it.destroy() } }
        recognizer = null
        onStateChanged(false)
    }

    fun destroy() = stop()

    private fun create(): SpeechRecognizer? {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU && SpeechRecognizer.isOnDeviceRecognitionAvailable(context)) {
            return SpeechRecognizer.createOnDeviceSpeechRecognizer(context)
        }
        if (!SpeechRecognizer.isRecognitionAvailable(context)) {
            onProblem(DictationProblem.Unavailable)
            return null
        }
        if (!onlineAllowed()) {
            // The device can recognise speech, but only through a service that may use the network. Before API 33
            // there is no way to ask for on-device only (EXTRA_PREFER_OFFLINE is a hint), so never go online silently.
            onProblem(
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) DictationProblem.OfflineNotInstalled
                else DictationProblem.OnlineRequired,
            )
            return null
        }
        return SpeechRecognizer.createSpeechRecognizer(context)
    }

    private fun recognizerIntent() = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
        putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
        putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
        putExtra(RecognizerIntent.EXTRA_PREFER_OFFLINE, !onlineAllowed() || Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU)
        putExtra(RecognizerIntent.EXTRA_CALLING_PACKAGE, context.packageName)
    }

    private fun restart() {
        if (active) recognizer?.startListening(recognizerIntent())
    }

    private val listener = object : RecognitionListener {
        override fun onReadyForSpeech(params: Bundle?) = Unit
        override fun onBeginningOfSpeech() = Unit
        override fun onRmsChanged(rmsdB: Float) = Unit
        override fun onBufferReceived(buffer: ByteArray?) = Unit
        override fun onEndOfSpeech() = Unit
        override fun onEvent(eventType: Int, params: Bundle?) = Unit

        override fun onPartialResults(partialResults: Bundle?) {
            first(partialResults)?.let(onPartial)
        }

        override fun onResults(results: Bundle?) {
            first(results)?.let(onFinal)
            restart()
        }

        override fun onError(error: Int) {
            when (error) {
                SpeechRecognizer.ERROR_NO_MATCH, SpeechRecognizer.ERROR_SPEECH_TIMEOUT -> restart()
                SpeechRecognizer.ERROR_LANGUAGE_NOT_SUPPORTED, SpeechRecognizer.ERROR_LANGUAGE_UNAVAILABLE -> {
                    stop(); onProblem(DictationProblem.OfflineNotInstalled)
                }
                else -> { stop(); onProblem(DictationProblem.Failed) }
            }
        }

        private fun first(bundle: Bundle?): String? =
            bundle?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull()?.takeIf { it.isNotBlank() }
    }
}
