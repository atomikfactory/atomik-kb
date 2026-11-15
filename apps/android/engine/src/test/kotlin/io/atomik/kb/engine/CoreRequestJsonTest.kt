package io.atomik.kb.engine

import io.atomik.kb.engine.model.*
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.builtins.serializer
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

/** The JSON of each request is compared with a hand-checked string, written from `api.rs`. */
class CoreRequestJsonTest {
    private fun assertJson(expected: String, request: CoreRequest) {
        val want: JsonElement = Json.parseToJsonElement(expected)
        val got: JsonElement = Json.parseToJsonElement(request.toJson())
        assertEquals(want, got, "request ${request.type}: ${request.toJson()}")
    }

    @Test fun unitRequestsCarryNoData() {
        assertEquals("""{"type":"AppStatus"}""", AppStatusRequest.toJson())
        assertEquals("""{"type":"SpaceList"}""", SpaceList.toJson())
        assertEquals("""{"type":"SyncStatus"}""", SyncStatusRequest.toJson())
        assertEquals("""{"type":"SyncNow"}""", SyncNowRequest.toJson())
        assertEquals("""{"type":"PairingConfirm"}""", PairingConfirm.toJson())
        assertEquals("""{"type":"PairingCancel"}""", PairingCancel.toJson())
        assertEquals("""{"type":"PairingStatus"}""", PairingStatus.toJson())
        assertEquals("""{"type":"DeviceList"}""", DeviceList.toJson())
        assertEquals("""{"type":"SecurityStatus"}""", SecurityStatus.toJson())
        assertEquals("""{"type":"WorkspaceGet"}""", WorkspaceGet.toJson())
    }

    @Test fun workspaceCreate() {
        assertJson("""{"type":"WorkspaceCreate","data":{"name":"Notes"}}""", WorkspaceCreate("Notes"))
        assertJson(
            """{"type":"WorkspaceCreate","data":{"name":"Notes","firstSpaceName":"Personal"}}""",
            WorkspaceCreate("Notes", "Personal"),
        )
    }

    @Test fun settings() {
        assertJson("""{"type":"SettingsGet","data":{"key":"app.x"}}""", SettingsGet("app.x"))
        assertJson("""{"type":"SettingsSet","data":{"key":"app.x","value":true}}""", SettingsSet("app.x", JsonPrimitive(true)))
    }

    @Test fun spacesAndTree() {
        assertJson("""{"type":"SpaceCreate","data":{"name":"Work"}}""", SpaceCreate("Work"))
        assertJson("""{"type":"SpaceCreate","data":{"name":"Work","icon":"W"}}""", SpaceCreate("Work", "W"))
        assertJson("""{"type":"TreeGet","data":{"spaceId":"s1"}}""", TreeGet("s1"))
    }

    @Test fun pages() {
        assertJson("""{"type":"PageGet","data":{"pageId":"p1"}}""", PageGet("p1"))
        assertJson("""{"type":"PageCreate","data":{"spaceId":"s1","title":""}}""", PageCreate(spaceId = "s1", title = ""))
        assertJson(
            """{"type":"PageCreate","data":{"spaceId":"s1","parentId":"p1","index":2,"title":"Child"}}""",
            PageCreate("s1", "p1", 2, "Child"),
        )
        assertJson("""{"type":"PageRename","data":{"pageId":"p1","title":"New"}}""", PageRename("p1", "New"))
        assertJson("""{"type":"PageTrash","data":{"pageId":"p1"}}""", PageTrash("p1"))
        assertJson("""{"type":"PageRestore","data":{"pageId":"p1"}}""", PageRestore("p1"))
        assertJson("""{"type":"PageFavoriteSet","data":{"pageId":"p1","favorite":true}}""", PageFavoriteSet("p1", true))
        assertJson("""{"type":"PageFavoriteSet","data":{"pageId":"p1","favorite":false}}""", PageFavoriteSet("p1", false))
    }

    @Test fun search() {
        assertJson("""{"type":"Search","data":{"query":"milk"}}""", Search("milk"))
        assertJson(
            """{"type":"Search","data":{"query":"milk","spaceId":"s1","scope":"Titles","limit":10}}""",
            Search("milk", "s1", SearchScope.Titles, 10),
        )
    }

    @Test fun sync() {
        assertJson("""{"type":"SyncConfigure","data":{}}""", SyncConfigure())
        assertJson("""{"type":"SyncConfigure","data":{"serverUrl":"https://kb.example"}}""", SyncConfigure("https://kb.example"))
        assertJson(
            """{"type":"SyncConfigure","data":{"serverUrl":"https://kb.example","inviteToken":"t"}}""",
            SyncConfigure("https://kb.example", "t"),
        )
    }

    @Test fun pairing() {
        assertJson(
            """{"type":"PairingStart","data":{"role":"Joiner","serverUrl":"https://kb.example"}}""",
            PairingStart(PairingRole.Joiner, "https://kb.example"),
        )
        assertJson(
            """{"type":"PairingStart","data":{"role":"Sponsor","code":"AAAA BBBB CCCC"}}""",
            PairingStart(PairingRole.Sponsor, code = "AAAA BBBB CCCC"),
        )
    }

    @Test fun appLock() {
        assertJson("""{"type":"AppLockEnable","data":{"passphrase":"correct horse"}}""", AppLockEnable("correct horse"))
        assertJson("""{"type":"AppLockDisable","data":{"passphrase":"correct horse"}}""", AppLockDisable("correct horse"))
        assertJson(
            """{"type":"AppLockChange","data":{"current":"old old old","new":"new new new"}}""",
            AppLockChange("old old old", "new new new"),
        )
    }

    // ---- Responses ----

    @Test fun decodesOkAndIgnoresUnknownKeys() {
        val reply = """{"ok":{"type":"Spaces","data":[{"id":"s1","name":"Personal","createdAt":5,"pageCount":2,"aiEnabled":true,"extra":1}]}}"""
        val spaces = decodeReply(reply, kotlinx.serialization.builtins.ListSerializer(SpaceInfo.serializer()))
        assertEquals(listOf(SpaceInfo("s1", "Personal", null, 5, 2)), spaces)
    }

    @Test fun decodesTreeWithAbsentOptionals() {
        val reply = """{"ok":{"type":"Tree","data":{"spaceId":"s1","roots":[{"page":{"id":"p1","spaceId":"s1","title":"A","createdAt":1,"updatedAt":2},"children":[]}],"trashed":[],"undo":{"canUndo":false,"canRedo":false}}}}"""
        val tree = decodeReply(reply, TreeView.serializer())
        assertEquals("p1", tree.roots.single().page.id)
        assertEquals(emptyList(), tree.favorites)
    }

    @Test fun unitResponse() {
        assertEquals(Unit, decodeReply("""{"ok":{"type":"Ok"}}""", Unit.serializer()))
    }

    @Test fun errThrowsCoreException() {
        val e = assertFailsWith<CoreException> {
            decodeReply("""{"err":{"code":"Locked","message":"locked"}}""", SpaceInfo.serializer())
        }
        assertEquals(ErrorCode.Locked, e.error.code)
    }

    @Test fun unknownErrorCodeBecomesInternal() {
        val e = assertFailsWith<CoreException> {
            decodeReply("""{"err":{"code":"Brand New","message":"?"}}""", SpaceInfo.serializer())
        }
        assertEquals(ErrorCode.Internal, e.error.code)
    }

    @Test fun decodesPairingAndSync() {
        val pairing = decodeReply(
            """{"ok":{"type":"Pairing","data":{"role":"Joiner","state":"Confirming","code":"AAAA BBBB CCCC","sas":"123456","peerName":"Desktop"}}}""",
            PairingInfo.serializer(),
        )
        assertEquals(PairingState.Confirming, pairing.state)
        assertEquals("123456", pairing.sas)
        val sync = decodeReply(
            """{"ok":{"type":"Sync","data":{"configured":true,"serverUrl":"https://s","state":"Idle","pendingEntries":0,"parkedItems":0,"remoteCursor":3,"forkDetected":false,"devicesSeen":1,"needsInviteToken":false,"transfers":{"uploads":0,"downloads":0,"waiting":0}}}}""",
            SyncInfo.serializer(),
        )
        assertEquals(SyncState.Idle, sync.state)
        assertEquals(3L, sync.remoteCursor)
    }
}
