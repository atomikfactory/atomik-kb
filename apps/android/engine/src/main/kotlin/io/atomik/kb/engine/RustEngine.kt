package io.atomik.kb.engine

import io.atomik.kb.engine.ffi.EngineConfig
import io.atomik.kb.engine.ffi.EngineException
import io.atomik.kb.engine.ffi.EventSink
import io.atomik.kb.engine.ffi.KbEngine
import io.atomik.kb.engine.ffi.KeystoreFfiException
import io.atomik.kb.engine.ffi.PlatformKeystore
import io.atomik.kb.engine.keystore.AndroidKeystoreAdapter
import io.atomik.kb.engine.keystore.KeystoreException
import io.atomik.kb.engine.model.ApiError
import io.atomik.kb.engine.model.CoreJson
import io.atomik.kb.engine.model.CoreRequest
import io.atomik.kb.engine.model.ErrorCode
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.withContext
import kotlinx.serialization.KSerializer
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.util.concurrent.Executors
import io.atomik.kb.engine.ffi.EngineState as FfiState

/**
 * [Engine] over the UniFFI `KbEngine`. Core calls run on one dedicated thread (the core is behind a mutex
 * on the Rust side; serialising here keeps request order and keeps the main thread free).
 */
class RustEngine(
    dataDir: String,
    keystore: AndroidKeystoreAdapter,
    platform: String = "android",
    private val io: CoroutineDispatcher = Executors.newSingleThreadExecutor { r ->
        Thread(r, "kb-engine").apply { isDaemon = true }
    }.asCoroutineDispatcher(),
) : Engine {
    private val _state = MutableStateFlow<EngineState>(EngineState.Opening)
    override val state: StateFlow<EngineState> = _state.asStateFlow()

    private val _events = MutableSharedFlow<CoreEvent>(extraBufferCapacity = 256)
    override val events: SharedFlow<CoreEvent> = _events.asSharedFlow()

    private val sink = object : EventSink {
        override fun onEvent(eventJson: String) {
            val event = parseEvent(eventJson) ?: return
            _events.tryEmit(event)
        }

        override fun onState(state: FfiState) {
            _state.value = state.toModel()
        }
    }

    private val native: KbEngine = KbEngine(EngineConfig(dataDir, platform), KeystoreBridge(keystore), sink)

    init {
        _state.value = native.state().toModel()
    }

    override suspend fun dispatchRaw(requestJson: String): String =
        withContext(io) { native.dispatch(requestJson) }

    override suspend fun <T> dispatch(request: CoreRequest, response: KSerializer<T>): T {
        val reply = dispatchRaw(request.toJson())
        return decodeReply(reply, response)
    }

    override suspend fun unlock(passphrase: String) = call { native.unlock(passphrase) }

    override fun lock() = native.lock()

    override suspend fun docOpen(pageId: String): ByteArray = call { native.docOpen(pageId) }
    override suspend fun docImport(pageId: String, update: ByteArray) = call { native.docImport(pageId, update) }
    override suspend fun docExport(pageId: String, since: ByteArray): ByteArray =
        call { native.docExport(pageId, since) }

    override fun docClose(pageId: String) = native.docClose(pageId)

    override suspend fun attachmentRead(id: String, start: Long?, end: Long?): ByteArray =
        call { native.attachmentRead(id, start?.toULong(), end?.toULong()) }

    override suspend fun attachmentWrite(id: String, bytes: ByteArray) = call { native.attachmentWrite(id, bytes) }

    override fun setForeground(foreground: Boolean) = native.setForeground(foreground)

    override suspend fun syncOnce(timeoutMs: Long): Boolean =
        withContext(io) { native.syncOnce(timeoutMs.coerceAtLeast(0).toULong()) }

    private suspend fun <T> call(block: () -> T): T = withContext(io) {
        try {
            block()
        } catch (e: EngineException) {
            throw e.toCore()
        }
    }

    internal companion object {
        fun parseEvent(json: String): CoreEvent? = runCatching {
            val obj = CoreJson.parseToJsonElement(json).jsonObject
            CoreEvent(obj.getValue("type").jsonPrimitive.content, obj["data"] ?: JsonNull, json)
        }.getOrNull()

        fun EngineException.toCore(): Exception = when (this) {
            is EngineException.Api -> CoreException(
                runCatching { CoreJson.decodeFromString(ApiError.serializer(), json) }
                    .getOrElse { ApiError(ErrorCode.Internal, json) },
            )
            is EngineException.Locked -> CoreException(ApiError(ErrorCode.Locked, "locked"))
            is EngineException.NotReady -> CoreException(ApiError(ErrorCode.Internal, "the core is still opening"))
            is EngineException.Internal -> CoreException(ApiError(ErrorCode.Internal, reason))
        }

        fun FfiState.toModel(): EngineState = when (this) {
            is FfiState.Opening -> EngineState.Opening
            is FfiState.Ready -> EngineState.Ready
            is FfiState.Locked -> EngineState.Locked
            is FfiState.Failed -> EngineState.Failed(message)
        }
    }
}

/** Delegates the FFI keystore callback to the Android Keystore adapter. */
private class KeystoreBridge(private val adapter: AndroidKeystoreAdapter) : PlatformKeystore {
    override fun get(name: String): ByteArray? = wrap { adapter.get(name) }
    override fun set(name: String, value: ByteArray) = wrap { adapter.set(name, value) }
    override fun delete(name: String) = wrap { adapter.delete(name) }

    private inline fun <T> wrap(block: () -> T): T = try {
        block()
    } catch (e: KeystoreException.Unavailable) {
        throw KeystoreFfiException.Unavailable(e.message ?: "unavailable")
    } catch (e: KeystoreException) {
        throw KeystoreFfiException.Failed(e.message ?: "failed")
    }
}
