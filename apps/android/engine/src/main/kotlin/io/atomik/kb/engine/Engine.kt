package io.atomik.kb.engine

import io.atomik.kb.engine.model.ApiError
import io.atomik.kb.engine.model.CoreRequest
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.serialization.KSerializer
import kotlinx.serialization.json.JsonElement

/** Where the Rust core stands (mirrors the FFI `EngineState`). */
sealed interface EngineState {
    data object Opening : EngineState
    data object Ready : EngineState
    data object Locked : EngineState
    data class Failed(val message: String) : EngineState
}

/** One `kb_types::Event`: `{"type":..,"data":..}`; `data` is `JsonNull` for unit events. */
data class CoreEvent(val type: String, val data: JsonElement, val raw: String)

/** The core refused a request. Shells branch on [error].code. */
class CoreException(val error: ApiError) : RuntimeException("${error.code}: ${error.message}")

/**
 * The one door to the core (Contract 3). `RustEngine` implements it over the UniFFI `KbEngine`;
 * `FakeEngine` is the in-memory stand-in for previews and tests.
 */
interface Engine {
    val state: StateFlow<EngineState>
    val events: SharedFlow<CoreEvent>

    /** Request JSON in, `{"ok": Response}` or `{"err": ApiError}` out. */
    suspend fun dispatchRaw(requestJson: String): String

    /** Sends [request] and decodes the response's `data` with [response]; throws [CoreException]. */
    suspend fun <T> dispatch(request: CoreRequest, response: KSerializer<T>): T

    suspend fun unlock(passphrase: String)
    fun lock()

    suspend fun docOpen(pageId: String): ByteArray
    suspend fun docImport(pageId: String, update: ByteArray)
    suspend fun docExport(pageId: String, since: ByteArray): ByteArray
    fun docClose(pageId: String)

    suspend fun attachmentRead(id: String, start: Long? = null, end: Long? = null): ByteArray
    suspend fun attachmentWrite(id: String, bytes: ByteArray)

    fun setForeground(foreground: Boolean)

    /** Asks for a sync round and waits until idle or [timeoutMs]; true when idle. */
    suspend fun syncOnce(timeoutMs: Long): Boolean
}
