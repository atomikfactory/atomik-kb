package io.atomik.kb.features.page

import io.atomik.kb.engine.fake.FakeEngine
import io.atomik.kb.engine.spaces
import io.atomik.kb.engine.tree
import io.atomik.kb.engine.model.ApiError
import io.atomik.kb.engine.model.ErrorCode
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertTrue

class EditorProtocolTest {
    private fun call(message: String): BridgeCall = assertIs<Inbound.Call>(EditorProtocol.parse(message)).call
    private fun note(message: String): HostNotification = assertIs<Inbound.Notification>(EditorProtocol.parse(message)).notification
    private fun obj(s: String): JsonObject = Json.parseToJsonElement(s).jsonObject

    // ---- JS -> Kotlin ----

    @Test fun parsesDispatchWithTheRequestAsJson() {
        val c = call("""{"id":7,"op":"dispatch","request":{"type":"PageGet","data":{"pageId":"p1"}}}""")
        c as BridgeCall.Dispatch
        assertEquals(7, c.id)
        assertEquals(obj("""{"type":"PageGet","data":{"pageId":"p1"}}"""), obj(c.requestJson))
    }

    @Test fun parsesDocCalls() {
        assertEquals(BridgeCall.DocOpen(1, "p1"), call("""{"id":1,"op":"docOpen","pageId":"p1"}"""))
        val imp = call("""{"id":2,"op":"docImport","pageId":"p1","b64":"AQID"}""") as BridgeCall.DocImport
        assertContentEquals(byteArrayOf(1, 2, 3), imp.update)
        val exp = call("""{"id":3,"op":"docExport","pageId":"p1","b64":"BA=="}""") as BridgeCall.DocExport
        assertContentEquals(byteArrayOf(4), exp.since)
        assertEquals(BridgeCall.DocClose(4, "p1"), call("""{"id":4,"op":"docClose","pageId":"p1"}"""))
    }

    @Test fun docExportWithoutVersionMeansEverything() {
        val exp = call("""{"id":3,"op":"docExport","pageId":"p1"}""") as BridgeCall.DocExport
        assertEquals(0, exp.since.size)
    }

    @Test fun parsesAttachmentCalls() {
        assertEquals(BridgeCall.AttRead(5, "a1", null, null), call("""{"id":5,"op":"attRead","attachmentId":"a1"}"""))
        assertEquals(BridgeCall.AttRead(6, "a1", 10, 20), call("""{"id":6,"op":"attRead","attachmentId":"a1","start":10,"end":20}"""))
        val w = call("""{"id":8,"op":"attWrite","attachmentId":"a1","b64":"AAE="}""") as BridgeCall.AttWrite
        assertContentEquals(byteArrayOf(0, 1), w.bytes)
    }

    @Test fun parsesNotifications() {
        assertEquals(HostNotification.Ready, note("""{"host":"ready"}"""))
        assertEquals(HostNotification.Opened("p1", "T"), note("""{"host":"opened","pageId":"p1","title":"T"}"""))
        assertEquals(HostNotification.TitleChanged("New"), note("""{"host":"titleChanged","title":"New"}"""))
        assertEquals(HostNotification.OpenPage("p2"), note("""{"host":"openPage","pageId":"p2"}"""))
        assertEquals(HostNotification.OpenExternal("https://x.org"), note("""{"host":"openExternal","url":"https://x.org"}"""))
        assertEquals(HostNotification.Dictate, note("""{"host":"dictate"}"""))
        assertEquals(HostNotification.Error("boom"), note("""{"host":"error","message":"boom"}"""))
    }

    @Test fun rejectsMalformedMessages() {
        listOf(
            "not json",
            """{"foo":1}""",
            """{"id":1}""",
            """{"id":1,"op":"nope"}""",
            """{"id":1,"op":"docOpen"}""",
            """{"id":1,"op":"docImport","pageId":"p","b64":"***"}""",
            """{"host":"unknown"}""",
            """{"host":"openPage"}""",
            """{"id":1,"op":"dispatch"}""",
        ).forEach { assertIs<Inbound.Invalid>(EditorProtocol.parse(it), it) }
    }

    // ---- Kotlin -> JS ----

    @Test fun buildsReplies() {
        assertEquals(obj("""{"id":1,"ok":null}"""), obj(EditorProtocol.replyOk(1)))
        assertEquals(obj("""{"id":2,"ok":{"b64":"AQID"}}"""), obj(EditorProtocol.replyBytes(2, byteArrayOf(1, 2, 3))))
        assertEquals(
            obj("""{"id":3,"err":{"code":"NotFound","message":"page"}}"""),
            obj(EditorProtocol.replyErr(3, ApiError(ErrorCode.NotFound, "page"))),
        )
    }

    @Test fun buildsMessagesForTheEditor() {
        assertEquals(
            obj("""{"event":{"type":"StructureChanged","data":{"spaceId":"s1"}}}"""),
            obj(EditorProtocol.event("""{"type":"StructureChanged","data":{"spaceId":"s1"}}""")),
        )
        assertEquals(
            obj("""{"init":{"pageId":"p1","spaceId":"s1","theme":"dark","locale":"fr-FR","readOnly":false,"focus":"title"}}"""),
            obj(EditorProtocol.init("p1", "s1", EditorTheme.Dark, "fr-FR", false, EditorFocus.Title)),
        )
        assertEquals(
            obj("""{"init":{"pageId":"p1","spaceId":"s1","theme":"light","locale":"en","readOnly":true,"focus":null}}"""),
            obj(EditorProtocol.init("p1", "s1", EditorTheme.Light, "en", true, null)),
        )
        assertEquals(obj("""{"insert":{"text":"hello \"x\""}}"""), obj(EditorProtocol.insertText("hello \"x\"")))
        assertEquals(
            obj("""{"insert":{"attachmentId":"a1","name":"cat.png","mime":"image/png"}}"""),
            obj(EditorProtocol.insertAttachment("a1", "cat.png", "image/png")),
        )
        assertEquals(obj("""{"theme":"dark"}"""), obj(EditorProtocol.theme(EditorTheme.Dark)))
        assertEquals(obj("""{"dictation":{"state":"idle"}}"""), obj(EditorProtocol.dictation(DictationState.Idle)))
        assertEquals(
            obj("""{"dictation":{"state":"listening","partial":"hel"}}"""),
            obj(EditorProtocol.dictation(DictationState.Listening, "hel")),
        )
    }

    // ---- The handler over an engine ----

    @Test fun dispatchGoesToTheEngineAndRepliesWithTheResponse() = runTest {
        val engine = FakeEngine(seed = true)
        val handler = EditorCallHandler(engine)
        val reply = obj(handler.handle(BridgeCall.Dispatch(9, """{"type":"SpaceList"}""")))
        assertEquals(9, reply["id"]!!.jsonPrimitive.content.toInt())
        val ok = reply["ok"]!!.jsonObject
        assertEquals("Spaces", ok["type"]!!.jsonPrimitive.content)
    }

    @Test fun dispatchErrorsComeBackAsErr() = runTest {
        val handler = EditorCallHandler(FakeEngine(seed = true))
        val reply = obj(handler.handle(BridgeCall.Dispatch(1, """{"type":"PageGet","data":{"pageId":"nope"}}""")))
        assertEquals("NotFound", reply["err"]!!.jsonObject["code"]!!.jsonPrimitive.content)
        assertTrue("ok" !in reply)
    }

    @Test fun docAndAttachmentRoundTrip() = runTest {
        val engine = FakeEngine(seed = true)
        val handler = EditorCallHandler(engine)
        val page = engine.tree(engine.spaces().first().id).roots.first().page.id
        assertEquals(JsonNull, obj(handler.handle(BridgeCall.DocImport(1, page, "hi".toByteArray())))["ok"])
        val opened = obj(handler.handle(BridgeCall.DocOpen(2, page)))["ok"]!!.jsonObject["b64"]!!.jsonPrimitive.content
        assertEquals("hi", String(EditorProtocol.decode(opened)))
        handler.handle(BridgeCall.AttWrite(3, "att", byteArrayOf(1, 2, 3, 4)))
        val part = obj(handler.handle(BridgeCall.AttRead(4, "att", 1, 3)))["ok"]!!.jsonObject["b64"]!!.jsonPrimitive.content
        assertContentEquals(byteArrayOf(2, 3), EditorProtocol.decode(part))
    }

    @Test fun failuresNeverEscapeTheHandler() = runTest {
        val handler = EditorCallHandler(FakeEngine(seed = true))
        val reply = obj(handler.handle(BridgeCall.AttRead(1, "missing", null, null)))
        assertEquals("NotFound", reply["err"]!!.jsonObject["code"]!!.jsonPrimitive.content)
    }
}
