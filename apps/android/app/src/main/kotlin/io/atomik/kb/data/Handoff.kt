package io.atomik.kb.data

import android.net.Uri
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.receiveAsFlow
import java.util.concurrent.ConcurrentHashMap
import javax.inject.Inject
import javax.inject.Singleton

/** Which page to show next, from anywhere in the app (search, capture, share, links). */
data class OpenPage(val pageId: String, val focus: String? = null)

@Singleton
class PageOpener @Inject constructor() {
    private val channel = Channel<OpenPage>(Channel.BUFFERED)
    val requests: Flow<OpenPage> = channel.receiveAsFlow()
    fun open(pageId: String, focus: String? = null) {
        channel.trySend(OpenPage(pageId, focus))
    }
}

/** Content to put into a page once its editor is ready (A8: through the editor, not the core). */
sealed interface PendingInsert {
    data class Text(val text: String) : PendingInsert
    data class Attachment(val attachmentId: String, val name: String, val mime: String) : PendingInsert
}

@Singleton
class PendingInserts @Inject constructor() {
    private val map = ConcurrentHashMap<String, MutableList<PendingInsert>>()
    fun add(pageId: String, insert: PendingInsert) {
        map.getOrPut(pageId) { java.util.Collections.synchronizedList(mutableListOf()) }.add(insert)
    }

    /** Removes and returns what waits for [pageId]. */
    fun drain(pageId: String): List<PendingInsert> = map.remove(pageId)?.toList().orEmpty()
}

/** What another app shared with us. */
data class SharedContent(
    val text: String? = null,
    val subject: String? = null,
    val images: List<Uri> = emptyList(),
) {
    val isEmpty: Boolean get() = text.isNullOrBlank() && images.isEmpty()

    /** First line of the text (or the subject) as a page title, at most 80 characters. */
    fun suggestedTitle(): String {
        val line = subject?.takeIf { it.isNotBlank() }
            ?: text?.lineSequence()?.map { it.trim() }?.firstOrNull { it.isNotEmpty() }
            ?: ""
        return if (line.startsWith("http://") || line.startsWith("https://")) subject.orEmpty().trim() else line.take(80)
    }

    companion object {
        /**
         * Builds the content of an `ACTION_SEND` or `ACTION_SEND_MULTIPLE` intent from its parts.
         * Images are the shared streams, when the MIME type is an image type or streams came along.
         */
        fun from(action: String?, mime: String?, text: String?, subject: String?, streams: List<Uri>): SharedContent? {
            if (action != "android.intent.action.SEND" && action != "android.intent.action.SEND_MULTIPLE") return null
            val images = if (mime?.startsWith("image/") == true || streams.isNotEmpty()) streams else emptyList()
            val content = SharedContent(text?.takeIf { it.isNotBlank() }, subject?.takeIf { it.isNotBlank() }, images)
            return content.takeUnless { it.isEmpty }
        }
    }
}
