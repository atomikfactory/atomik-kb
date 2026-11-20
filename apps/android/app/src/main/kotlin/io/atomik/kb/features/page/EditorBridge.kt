package io.atomik.kb.features.page

import io.atomik.kb.engine.CoreException
import io.atomik.kb.engine.Engine
import io.atomik.kb.engine.model.ApiError
import io.atomik.kb.engine.model.CoreJson
import io.atomik.kb.engine.model.ErrorCode
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put
import java.util.Base64

/** A request from the editor that wants a reply carrying the same [id] (Contract 2, calls). */
sealed interface BridgeCall {
    val id: Long

    data class Dispatch(override val id: Long, val requestJson: String) : BridgeCall
    data class DocOpen(override val id: Long, val pageId: String) : BridgeCall
    data class DocImport(override val id: Long, val pageId: String, val update: ByteArray) : BridgeCall
    data class DocExport(override val id: Long, val pageId: String, val since: ByteArray) : BridgeCall
    data class DocClose(override val id: Long, val pageId: String) : BridgeCall
    data class AttRead(override val id: Long, val attachmentId: String, val start: Long?, val end: Long?) : BridgeCall
    data class AttWrite(override val id: Long, val attachmentId: String, val bytes: ByteArray) : BridgeCall
}

/** A one-way message from the editor (Contract 2, notifications). */
sealed interface HostNotification {
    data object Ready : HostNotification
    data class Opened(val pageId: String, val title: String) : HostNotification
    data class TitleChanged(val title: String) : HostNotification
    data class OpenPage(val pageId: String) : HostNotification
    data class OpenExternal(val url: String) : HostNotification
    data object Dictate : HostNotification
    data class Error(val message: String) : HostNotification
}

sealed interface Inbound {
    data class Call(val call: BridgeCall) : Inbound
    data class Notification(val notification: HostNotification) : Inbound
    data class Invalid(val reason: String) : Inbound
}

enum class EditorTheme(val wire: String) { Light("light"), Dark("dark") }
enum class EditorFocus(val wire: String) { Title("title"), End("end") }
enum class DictationState(val wire: String) { Listening("listening"), Idle("idle") }

/** Parses what the editor posts and builds what Kotlin posts back. Pure; no Android types. */
object EditorProtocol {
    fun parse(message: String): Inbound = try {
        val root = CoreJson.parseToJsonElement(message).jsonObject
        val host = root["host"]?.jsonPrimitive?.contentOrNull
        if (host != null) parseNotification(host, root) else parseCall(root)
    } catch (e: Exception) {
        Inbound.Invalid(e.message ?: "unparseable message")
    }

    private fun parseNotification(host: String, o: JsonObject): Inbound {
        fun s(k: String) = o[k]?.jsonPrimitive?.contentOrNull
        val n: HostNotification = when (host) {
            "ready" -> HostNotification.Ready
            "opened" -> HostNotification.Opened(s("pageId") ?: return Inbound.Invalid("opened without pageId"), s("title").orEmpty())
            "titleChanged" -> HostNotification.TitleChanged(s("title").orEmpty())
            "openPage" -> HostNotification.OpenPage(s("pageId") ?: return Inbound.Invalid("openPage without pageId"))
            "openExternal" -> HostNotification.OpenExternal(s("url") ?: return Inbound.Invalid("openExternal without url"))
            "dictate" -> HostNotification.Dictate
            "error" -> HostNotification.Error(s("message").orEmpty())
            else -> return Inbound.Invalid("unknown host message: $host")
        }
        return Inbound.Notification(n)
    }

    private fun parseCall(o: JsonObject): Inbound {
        val id = o["id"]?.jsonPrimitive?.longOrNull ?: return Inbound.Invalid("call without id")
        fun s(k: String) = o[k]?.jsonPrimitive?.contentOrNull
        fun req(k: String) = s(k) ?: throw IllegalArgumentException("missing $k")
        fun bytes(k: String) = decode(req(k))
        val call: BridgeCall = when (val op = s("op")) {
            "dispatch" -> BridgeCall.Dispatch(id, CoreJson.encodeToString(JsonElement.serializer(), o["request"] ?: throw IllegalArgumentException("missing request")))
            "docOpen" -> BridgeCall.DocOpen(id, req("pageId"))
            "docImport" -> BridgeCall.DocImport(id, req("pageId"), bytes("b64"))
            "docExport" -> BridgeCall.DocExport(id, req("pageId"), s("b64")?.let(::decode) ?: ByteArray(0))
            "docClose" -> BridgeCall.DocClose(id, req("pageId"))
            "attRead" -> BridgeCall.AttRead(id, req("attachmentId"), o["start"]?.jsonPrimitive?.longOrNull, o["end"]?.jsonPrimitive?.longOrNull)
            "attWrite" -> BridgeCall.AttWrite(id, req("attachmentId"), bytes("b64"))
            else -> return Inbound.Invalid("unknown op: $op")
        }
        return Inbound.Call(call)
    }

    fun encode(bytes: ByteArray): String = Base64.getEncoder().encodeToString(bytes)
    fun decode(b64: String): ByteArray = Base64.getDecoder().decode(b64)

    // ---- Kotlin -> JS ----

    fun replyOk(id: Long, value: JsonElement = JsonNull): String = json {
        put("id", id); put("ok", value)
    }

    fun replyBytes(id: Long, bytes: ByteArray): String = replyOk(id, buildJsonObject { put("b64", encode(bytes)) })

    fun replyErr(id: Long, error: ApiError): String = json {
        put("id", id); put("err", CoreJson.encodeToJsonElement(ApiError.serializer(), error))
    }

    fun event(rawEventJson: String): String = json { put("event", CoreJson.parseToJsonElement(rawEventJson)) }

    fun init(pageId: String, spaceId: String, theme: EditorTheme, locale: String, readOnly: Boolean, focus: EditorFocus?): String = json {
        put("init", buildJsonObject {
            put("pageId", pageId); put("spaceId", spaceId); put("theme", theme.wire)
            put("locale", locale); put("readOnly", readOnly)
            put("focus", focus?.wire?.let { JsonPrimitive(it) } ?: JsonNull)
        })
    }

    fun insertText(text: String): String = json { put("insert", buildJsonObject { put("text", text) }) }

    fun insertAttachment(attachmentId: String, name: String, mime: String): String = json {
        put("insert", buildJsonObject { put("attachmentId", attachmentId); put("name", name); put("mime", mime) })
    }

    fun theme(theme: EditorTheme): String = json { put("theme", theme.wire) }

    fun dictation(state: DictationState, partial: String? = null): String = json {
        put("dictation", buildJsonObject {
            put("state", state.wire)
            if (partial != null) put("partial", partial)
        })
    }

    private inline fun json(block: kotlinx.serialization.json.JsonObjectBuilder.() -> Unit): String =
        CoreJson.encodeToString(JsonObject.serializer(), buildJsonObject(block))
}

/** Answers the editor's calls from the [Engine]; one reply string per call, never throws. */
class EditorCallHandler(private val engine: Engine) {
    suspend fun handle(call: BridgeCall): String = try {
        when (call) {
            is BridgeCall.Dispatch -> {
                val reply = CoreJson.parseToJsonElement(engine.dispatchRaw(call.requestJson)).jsonObject
                val err = reply["err"]
                if (err != null) EditorProtocol.replyErr(call.id, CoreJson.decodeFromJsonElement(ApiError.serializer(), err))
                else EditorProtocol.replyOk(call.id, reply["ok"] ?: JsonNull)
            }
            is BridgeCall.DocOpen -> EditorProtocol.replyBytes(call.id, engine.docOpen(call.pageId))
            is BridgeCall.DocImport -> { engine.docImport(call.pageId, call.update); EditorProtocol.replyOk(call.id) }
            is BridgeCall.DocExport -> EditorProtocol.replyBytes(call.id, engine.docExport(call.pageId, call.since))
            is BridgeCall.DocClose -> { engine.docClose(call.pageId); EditorProtocol.replyOk(call.id) }
            is BridgeCall.AttRead -> EditorProtocol.replyBytes(call.id, engine.attachmentRead(call.attachmentId, call.start, call.end))
            is BridgeCall.AttWrite -> { engine.attachmentWrite(call.attachmentId, call.bytes); EditorProtocol.replyOk(call.id) }
        }
    } catch (e: CoreException) {
        EditorProtocol.replyErr(call.id, e.error)
    } catch (e: kotlinx.coroutines.CancellationException) {
        throw e
    } catch (e: Exception) {
        EditorProtocol.replyErr(call.id, ApiError(ErrorCode.Internal, e.message ?: "internal error"))
    }
}
