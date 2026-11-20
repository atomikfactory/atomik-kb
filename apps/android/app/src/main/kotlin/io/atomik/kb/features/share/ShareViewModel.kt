package io.atomik.kb.features.share

import android.content.ContentResolver
import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.Binds
import dagger.Module
import dagger.hilt.InstallIn
import dagger.hilt.android.lifecycle.HiltViewModel
import dagger.hilt.android.qualifiers.ApplicationContext
import dagger.hilt.components.SingletonComponent
import io.atomik.kb.data.Loadable
import io.atomik.kb.data.PageOpener
import io.atomik.kb.data.PagesRepository
import io.atomik.kb.data.PendingInsert
import io.atomik.kb.data.PendingInserts
import io.atomik.kb.data.SettingsRepository
import io.atomik.kb.data.SharedContent
import io.atomik.kb.data.ShareInbox
import io.atomik.kb.data.SpacesRepository
import io.atomik.kb.engine.model.SpaceInfo
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import javax.inject.Inject

/** One shared file read into memory. */
class SharedFile(val name: String, val mime: String, val bytes: ByteArray)

/** Reads the streams another app shared; a seam so tests need no content resolver. */
interface SharedFileReader {
    suspend fun read(uri: Uri): SharedFile
}

class TooLargeException(val limitBytes: Long) : Exception("file larger than $limitBytes bytes")

class ContentResolverFileReader @Inject constructor(@ApplicationContext private val context: Context) : SharedFileReader {
    override suspend fun read(uri: Uri): SharedFile = withContext(Dispatchers.IO) {
        val resolver: ContentResolver = context.contentResolver
        val mime = resolver.getType(uri) ?: "application/octet-stream"
        val name = resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
            if (c.moveToFirst()) c.getString(0) else null
        } ?: "image"
        val input = resolver.openInputStream(uri) ?: throw java.io.IOException("cannot open $uri")
        val bytes = input.use { stream ->
            val out = java.io.ByteArrayOutputStream()
            val buffer = ByteArray(64 * 1024)
            while (true) {
                val n = stream.read(buffer)
                if (n < 0) break
                out.write(buffer, 0, n)
                if (out.size() > MAX_BYTES) throw TooLargeException(MAX_BYTES)
            }
            out.toByteArray()
        }
        SharedFile(name, mime, bytes)
    }

    private companion object { const val MAX_BYTES = 25L * 1024 * 1024 }
}

@Module
@InstallIn(SingletonComponent::class)
abstract class ShareModule {
    @Binds abstract fun fileReader(impl: ContentResolverFileReader): SharedFileReader
}

data class ShareUiState(
    val content: SharedContent? = null,
    val spaces: Loadable<List<SpaceInfo>> = Loadable.Loading,
    val selectedSpaceId: String? = null,
    val busy: Boolean = false,
    val error: String? = null,
    val done: Boolean = false,
)

@HiltViewModel
class ShareViewModel @Inject constructor(
    spacesRepo: SpacesRepository,
    private val pages: PagesRepository,
    private val holder: ShareInbox,
    private val reader: SharedFileReader,
    private val pending: PendingInserts,
    private val opener: PageOpener,
    private val settings: SettingsRepository,
) : ViewModel() {
    private val _state = MutableStateFlow(ShareUiState(content = holder.content.value))
    val state: StateFlow<ShareUiState> = _state.asStateFlow()

    init {
        viewModelScope.launch {
            val s = settings.current()
            spacesRepo.spaces.collect { loaded ->
                _state.update { st ->
                    val list = (loaded as? Loadable.Data)?.value.orEmpty()
                    val keep = st.selectedSpaceId?.takeIf { id -> list.any { it.id == id } }
                    st.copy(
                        spaces = loaded,
                        selectedSpaceId = keep ?: io.atomik.kb.features.capture.pickCaptureSpace(list, s.captureSpaceId, s.lastSpaceId)?.id,
                    )
                }
            }
        }
        viewModelScope.launch { holder.content.collect { c -> _state.update { it.copy(content = c) } } }
    }

    fun selectSpace(id: String) = _state.update { it.copy(selectedSpaceId = id) }

    fun cancel() { holder.clear() }

    /** Creates the page, stores the images, queues what the editor must insert, and opens the page. */
    fun createPage() {
        val st = _state.value
        val content = st.content ?: return
        val spaceId = st.selectedSpaceId ?: return
        if (st.busy) return
        _state.update { it.copy(busy = true, error = null) }
        viewModelScope.launch {
            try {
                val page = pages.create(spaceId, content.suggestedTitle())
                content.text?.takeIf { it.isNotBlank() }?.let { pending.add(page.id, PendingInsert.Text(it)) }
                for (uri in content.images) {
                    val file = reader.read(uri)
                    val id = pages.addAttachment(spaceId, file.name, file.mime, file.bytes)
                    pending.add(page.id, PendingInsert.Attachment(id, file.name, file.mime))
                }
                holder.clear()
                opener.open(page.id, "end")
                _state.update { it.copy(busy = false, done = true) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _state.update { it.copy(busy = false, error = e.message ?: "error") }
            }
        }
    }
}
