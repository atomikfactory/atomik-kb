package io.atomik.kb.engine.fake

import io.atomik.kb.engine.CoreEvent
import io.atomik.kb.engine.CoreException
import io.atomik.kb.engine.Engine
import io.atomik.kb.engine.EngineState
import io.atomik.kb.engine.decodeReply
import io.atomik.kb.engine.model.*
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.serialization.KSerializer
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put

/**
 * In-memory [Engine] for previews, UI work and tests. It understands the v1 request subset and
 * keeps spaces, a page tree with favourites and trash, page bodies (UTF-8 bytes standing in for
 * Loro updates), search, sync, a simulated pairing session and a passphrase lock.
 *
 * Pairing advances on its own when a [scope] is given (Waiting, then Confirming after
 * [pairingStepDelayMs]); tests call [advancePairing] instead.
 */
class FakeEngine(
    seed: Boolean = false,
    private val scope: CoroutineScope? = null,
    private val pairingStepDelayMs: Long = 1_500,
    private val clock: () -> Long = System::currentTimeMillis,
) : Engine {
    private val _state = MutableStateFlow<EngineState>(EngineState.Ready)
    override val state: StateFlow<EngineState> = _state.asStateFlow()
    private val _events = MutableSharedFlow<CoreEvent>(extraBufferCapacity = 128)
    override val events: SharedFlow<CoreEvent> = _events.asSharedFlow()

    private val lock = Any()
    private var counter = 0
    private var workspace: Workspace? = null
    private val spaces = mutableListOf<SpaceInfo>()
    private val pages = linkedMapOf<String, PageInfo>()
    private val order = mutableMapOf<String, MutableList<String>>() // parentKey -> children
    private val favorites = mutableMapOf<String, MutableList<String>>() // spaceId -> pages
    private val bodies = mutableMapOf<String, ByteArray>()
    private val attachments = mutableMapOf<String, ByteArray>()
    private val settings = mutableMapOf<String, JsonElement>()
    private var syncServer: String? = null
    private var syncState = SyncState.Disabled
    private var pairing = PairingInfo(state = PairingState.Idle)
    private var passphrase: String? = null
    private var deviceName = "Fake phone"
    private val devices = mutableListOf<DeviceEntry>()

    /** Number of times [setForeground] changed the foreground flag (test hook). */
    @Volatile
    var foreground: Boolean = false
        private set

    init {
        if (seed) seedWorkspace()
    }

    // ---- Engine ----

    override suspend fun dispatchRaw(requestJson: String): String = synchronized(lock) {
        val emitted = mutableListOf<Pair<String, JsonElement?>>()
        val reply = try {
            val root = CoreJson.parseToJsonElement(requestJson).jsonObject
            val type = root["type"]?.jsonPrimitive?.content ?: throw fail(ErrorCode.InvalidInput, "missing type")
            val data = root["data"] as? JsonObject
            buildJsonObject { put("ok", handle(type, data, emitted)) }
        } catch (e: CoreException) {
            buildJsonObject { put("err", CoreJson.encodeToJsonElement(ApiError.serializer(), e.error)) }
        } catch (e: kotlinx.serialization.SerializationException) {
            buildJsonObject { put("err", CoreJson.encodeToJsonElement(ApiError.serializer(), ApiError(ErrorCode.InvalidInput, e.message ?: "bad request"))) }
        }
        emitted.forEach { (t, d) -> emit(t, d) }
        CoreJson.encodeToString(JsonObject.serializer(), reply)
    }

    override suspend fun <T> dispatch(request: CoreRequest, response: KSerializer<T>): T =
        decodeReply(dispatchRaw(request.toJson()), response)

    override suspend fun unlock(passphrase: String) {
        synchronized(lock) {
            if (this.passphrase == null) return
            if (this.passphrase != passphrase) throw fail(ErrorCode.Forbidden, "wrong passphrase")
            _state.value = EngineState.Ready
        }
        emit("SecurityChanged", null)
    }

    override fun lock() {
        synchronized(lock) { if (passphrase != null) _state.value = EngineState.Locked }
    }

    override suspend fun docOpen(pageId: String): ByteArray = synchronized(lock) {
        requireReady(); requirePage(pageId)
        bodies[pageId] ?: ByteArray(0)
    }

    override suspend fun docImport(pageId: String, update: ByteArray) {
        synchronized(lock) { requireReady(); requirePage(pageId); bodies[pageId] = update }
        emit("PageContentChanged", buildJsonObject { put("pageId", pageId); put("source", "Local") })
    }

    override suspend fun docExport(pageId: String, since: ByteArray): ByteArray = synchronized(lock) {
        requireReady(); requirePage(pageId)
        bodies[pageId] ?: ByteArray(0)
    }

    override fun docClose(pageId: String) = Unit

    override suspend fun attachmentRead(id: String, start: Long?, end: Long?): ByteArray = synchronized(lock) {
        val all = attachments[id] ?: throw fail(ErrorCode.NotFound, "attachment")
        all.copyOfRange((start ?: 0).toInt().coerceIn(0, all.size), (end ?: all.size.toLong()).toInt().coerceIn(0, all.size))
    }

    override suspend fun attachmentWrite(id: String, bytes: ByteArray) {
        synchronized(lock) { attachments[id] = bytes }
    }

    override fun setForeground(foreground: Boolean) {
        this.foreground = foreground
    }

    override suspend fun syncOnce(timeoutMs: Long): Boolean {
        synchronized(lock) { if (syncServer != null) syncState = SyncState.Idle }
        emit("SyncChanged", null)
        return true
    }

    // ---- Test and preview hooks ----

    /** Moves the simulated pairing one step: Waiting to Confirming (six digits shown). */
    fun advancePairing() {
        val changed = synchronized(lock) {
            if (pairing.state == PairingState.Waiting) {
                pairing = pairing.copy(state = PairingState.Confirming, sas = "482 913".replace(" ", ""), peerName = "Desktop")
                true
            } else false
        }
        if (changed) emit("PairingChanged", null)
    }

    /** Stores [text] as the body of [pageId] (what the editor would write), for search tests. */
    fun setBody(pageId: String, text: String) {
        synchronized(lock) { bodies[pageId] = text.toByteArray() }
    }

    fun bodyText(pageId: String): String? = synchronized(lock) { bodies[pageId]?.decodeToString() }

    // ---- Dispatch ----

    private fun handle(type: String, d: JsonObject?, out: MutableList<Pair<String, JsonElement?>>): JsonElement {
        if (_state.value is EngineState.Locked && type != "AppStatus") throw fail(ErrorCode.Locked, "locked")
        fun str(k: String) = d?.get(k)?.takeIf { it !is JsonNull }?.jsonPrimitive?.contentOrNull
        fun req(k: String) = str(k) ?: throw fail(ErrorCode.InvalidInput, k)
        return when (type) {
            "AppStatus" -> ok("AppStatus", AppStatus.serializer(), AppStatus(
                workspaceInitialized = workspace != null, deviceId = "fake-device", dataDir = "memory",
                coreVersion = "fake", apiVersion = 1, platform = "android", dbEncrypted = true, dbCipher = "fake",
            ))
            "WorkspaceGet" -> envelope("Workspace", workspace?.let { CoreJson.encodeToJsonElement(Workspace.serializer(), it) } ?: JsonNull)
            "WorkspaceCreate" -> {
                if (workspace != null) throw fail(ErrorCode.AlreadyExists, "workspace")
                val ws = Workspace("w${++counter}", req("name").trim().ifEmpty { throw fail(ErrorCode.InvalidInput, "name") }, clock())
                workspace = ws
                out += "WorkspaceCreated" to buildJsonObject { put("id", ws.id) }
                str("firstSpaceName")?.let { addSpace(it, null, out) }
                ok("Workspace", Workspace.serializer(), ws)
            }
            "SettingsGet" -> envelope("Setting", settings[req("key")] ?: JsonNull)
            "SettingsSet" -> {
                settings[req("key")] = d?.get("value") ?: JsonNull
                out += "SettingChanged" to buildJsonObject { put("key", req("key")) }
                unit()
            }
            "DeviceInfo" -> ok("DeviceInfo", DeviceInfo.serializer(), DeviceInfo("fake-device", deviceName, "android", 0))
            "SpaceList" -> ok("Spaces", ListSerializer(SpaceInfo.serializer()), spaces.map { it.withCount() })
            "SpaceCreate" -> ok("Space", SpaceInfo.serializer(), addSpace(req("name"), str("icon"), out))
            "TreeGet" -> ok("Tree", TreeView.serializer(), tree(req("spaceId")))
            "PageGet" -> ok("Page", PageInfo.serializer(), requirePage(req("pageId")))
            "PageCreate" -> {
                val space = req("spaceId")
                if (spaces.none { it.id == space }) throw fail(ErrorCode.NotFound, "space")
                val parent = str("parentId")?.also { requirePage(it) }
                val now = clock()
                val page = PageInfo(id = "p${++counter}", spaceId = space, parentId = parent, title = str("title").orEmpty().trim(), createdAt = now, updatedAt = now)
                pages[page.id] = page
                val siblings = order.getOrPut(key(space, parent)) { mutableListOf() }
                val idx = d?.get("index")?.takeIf { it !is JsonNull }?.jsonPrimitive?.contentOrNull?.toIntOrNull()
                if (idx != null) siblings.add(idx.coerceIn(0, siblings.size), page.id) else siblings.add(page.id)
                out += "StructureChanged" to buildJsonObject { put("spaceId", space) }
                ok("Page", PageInfo.serializer(), page)
            }
            "PageRename" -> {
                val p = requirePage(req("pageId"))
                val renamed = p.copy(title = str("title").orEmpty().trim(), updatedAt = clock())
                pages[p.id] = renamed
                out += "StructureChanged" to buildJsonObject { put("spaceId", p.spaceId) }
                ok("Page", PageInfo.serializer(), renamed)
            }
            "PageTrash" -> {
                val p = requirePage(req("pageId"))
                order[key(p.spaceId, p.parentId)]?.remove(p.id)
                pages[p.id] = p.copy(trashedAt = clock())
                out += "StructureChanged" to buildJsonObject { put("spaceId", p.spaceId) }
                unit()
            }
            "PageRestore" -> {
                val p = requirePage(req("pageId"))
                pages[p.id] = p.copy(trashedAt = null)
                order.getOrPut(key(p.spaceId, p.parentId)) { mutableListOf() }.add(p.id)
                out += "StructureChanged" to buildJsonObject { put("spaceId", p.spaceId) }
                unit()
            }
            "PageFavoriteSet" -> {
                val p = requirePage(req("pageId"))
                val list = favorites.getOrPut(p.spaceId) { mutableListOf() }
                if (d?.get("favorite")?.jsonPrimitive?.content == "true") { if (p.id !in list) list.add(p.id) } else list.remove(p.id)
                out += "StructureChanged" to buildJsonObject { put("spaceId", p.spaceId) }
                unit()
            }
            "Search" -> ok("Search", ListSerializer(SearchHit.serializer()), search(req("query"), str("spaceId"), d?.get("limit")?.jsonPrimitive?.contentOrNull?.toIntOrNull() ?: 20))
            "AttachmentBegin" -> envelope("AttachmentStarted", JsonPrimitive("a${++counter}"))
            "AttachmentFinish", "AttachmentAbort" -> unit()
            "SyncConfigure" -> {
                syncServer = str("serverUrl")
                syncState = if (syncServer == null) SyncState.Disabled else SyncState.Idle
                out += "SyncChanged" to null
                unit()
            }
            "SyncStatus" -> ok("Sync", SyncInfo.serializer(), syncInfo())
            "SyncNow" -> { out += "SyncRequested" to null; unit() }
            "PairingStart" -> {
                val role = if (str("role") == "Sponsor") PairingRole.Sponsor else PairingRole.Joiner
                val server = str("serverUrl")
                if (role == PairingRole.Joiner && server.isNullOrBlank()) throw fail(ErrorCode.InvalidInput, "serverUrl")
                if (workspace != null && role == PairingRole.Joiner) throw fail(ErrorCode.AlreadyExists, "workspace")
                pairing = PairingInfo(role, PairingState.Waiting, code = "KQXT 7M2B 9WFH", serverUrl = server, expiresAt = clock() + 600_000)
                out += "PairingChanged" to null
                scope?.launch { delay(pairingStepDelayMs); advancePairing() }
                ok("Pairing", PairingInfo.serializer(), pairing)
            }
            "PairingConfirm" -> {
                if (pairing.state != PairingState.Confirming) throw fail(ErrorCode.InvalidInput, "not confirming")
                pairing = pairing.copy(state = PairingState.Done)
                if (workspace == null) seedWorkspace(emitInto = out)
                syncServer = pairing.serverUrl; syncState = SyncState.Idle
                out += "PairingChanged" to null
                unit()
            }
            "PairingCancel" -> { pairing = pairing.copy(state = PairingState.Cancelled, code = null, sas = null); out += "PairingChanged" to null; unit() }
            "PairingStatus" -> ok("Pairing", PairingInfo.serializer(), pairing)
            "DeviceList" -> ok("Devices", ListSerializer(DeviceEntry.serializer()), devices.toList())
            "SecurityStatus" -> ok("Security", SecurityInfo.serializer(), SecurityInfo(
                appLock = AppLockInfo(enabled = passphrase != null, memoryKib = 65536, iterations = 3, parallelism = 4),
                thisDevice = "fake-device", devices = devices.size,
            ))
            "AppLockEnable" -> {
                if (passphrase != null) throw fail(ErrorCode.AlreadyExists, "lock")
                val p = req("passphrase")
                if (p.toByteArray().size !in 8..256) throw fail(ErrorCode.InvalidInput, "passphrase")
                passphrase = p; out += "SecurityChanged" to null; unit()
            }
            "AppLockDisable" -> {
                if (passphrase != req("passphrase")) throw fail(ErrorCode.Forbidden, "wrong passphrase")
                passphrase = null; out += "SecurityChanged" to null; unit()
            }
            "AppLockChange" -> {
                if (passphrase != req("current")) throw fail(ErrorCode.Forbidden, "wrong passphrase")
                val n = req("new")
                if (n.toByteArray().size !in 8..256) throw fail(ErrorCode.InvalidInput, "passphrase")
                passphrase = n; out += "SecurityChanged" to null; unit()
            }
            else -> throw fail(ErrorCode.InvalidInput, "unsupported request $type")
        }
    }

    // ---- Model helpers ----

    private fun key(space: String, parent: String?) = "$space/${parent.orEmpty()}"

    private fun addSpace(name: String, icon: String?, out: MutableList<Pair<String, JsonElement?>>): SpaceInfo {
        val s = SpaceInfo(id = "s${++counter}", name = name.trim().ifEmpty { throw fail(ErrorCode.InvalidInput, "name") }, icon = icon, createdAt = clock())
        spaces += s
        out += "SpacesChanged" to null
        return s
    }

    private fun SpaceInfo.withCount() = copy(pageCount = pages.values.count { it.spaceId == id && it.trashedAt == null })

    private fun node(p: PageInfo): PageNode =
        PageNode(p, order[key(p.spaceId, p.id)].orEmpty().mapNotNull { pages[it] }.map { node(it) })

    private fun tree(space: String): TreeView {
        if (spaces.none { it.id == space }) throw fail(ErrorCode.NotFound, "space")
        val roots = order[key(space, null)].orEmpty().mapNotNull { pages[it] }.map { node(it) }
        val trashed = pages.values.filter { it.spaceId == space && it.trashedAt != null && it.parentId?.let { pid -> pages[pid]?.trashedAt == null } != false }
            .sortedByDescending { it.trashedAt }.map { TrashedPage(it, it.parentId) }
        val favs = favorites[space].orEmpty().filter { pages[it]?.trashedAt == null && it in pages }
        return TreeView(space, roots, trashed, favs)
    }

    private fun search(query: String, space: String?, limit: Int): List<SearchHit> {
        val q = query.trim().lowercase()
        if (q.isEmpty()) return emptyList()
        val hits = mutableListOf<SearchHit>()
        for (p in pages.values) {
            if (p.trashedAt != null || (space != null && p.spaceId != space)) continue
            val titleAt = p.title.lowercase().indexOf(q)
            if (titleAt >= 0) {
                hits += SearchHit(p.id, p.spaceId, p.title, p.icon, snippet = p.title, highlights = listOf(TextRange(titleAt, titleAt + q.length)))
                continue
            }
            val body = bodies[p.id]?.decodeToString().orEmpty()
            val at = body.lowercase().indexOf(q)
            if (at >= 0) {
                val from = (at - 20).coerceAtLeast(0)
                val to = (at + q.length + 40).coerceAtMost(body.length)
                hits += SearchHit(p.id, p.spaceId, p.title, p.icon, blockId = "b1", snippet = body.substring(from, to), highlights = listOf(TextRange(at - from, at - from + q.length)))
            }
        }
        return hits.take(limit.coerceIn(1, 100))
    }

    private fun syncInfo() = SyncInfo(
        configured = syncServer != null, serverUrl = syncServer, state = syncState,
        lastPullAt = if (syncServer != null) clock() else null, devicesSeen = devices.size,
    )

    private fun requirePage(id: String): PageInfo = pages[id] ?: throw fail(ErrorCode.NotFound, "page")

    private fun requireReady() {
        if (_state.value is EngineState.Locked) throw fail(ErrorCode.Locked, "locked")
    }

    private fun seedWorkspace(emitInto: MutableList<Pair<String, JsonElement?>>? = null) {
        val sink = emitInto ?: mutableListOf()
        workspace = Workspace("w${++counter}", "My notes", clock())
        val personal = addSpace("Personal", null, sink)
        val work = addSpace("Work", null, sink)
        devices += DeviceEntry("fake-device", deviceName, clock(), isThisDevice = true)
        devices += DeviceEntry("desktop-1", "Desktop", clock() - 86_400_000)
        fun page(space: SpaceInfo, title: String, parent: PageInfo? = null, body: String = ""): PageInfo {
            val now = clock()
            val p = PageInfo("p${++counter}", space.id, parent?.id, title, createdAt = now, updatedAt = now)
            pages[p.id] = p
            order.getOrPut(key(space.id, parent?.id)) { mutableListOf() }.add(p.id)
            if (body.isNotEmpty()) bodies[p.id] = body.toByteArray()
            return p
        }
        val welcome = page(personal, "Welcome", body = "Welcome to atomik. Your notes stay on your devices.")
        page(personal, "Shopping list", body = "Milk, eggs, coffee")
        page(personal, "Ideas", welcome, body = "Build a small garden")
        page(work, "Roadmap", body = "Ship the Android app")
        page(work, "Meeting notes")
        favorites[personal.id] = mutableListOf(welcome.id)
        if (emitInto != null) emitInto += "StructureChanged" to buildJsonObject { put("spaceId", personal.id) }
    }

    // ---- JSON helpers ----

    private fun <T> ok(type: String, ser: KSerializer<T>, value: T): JsonElement =
        envelope(type, CoreJson.encodeToJsonElement(ser, value))

    private fun envelope(type: String, data: JsonElement): JsonElement = buildJsonObject { put("type", type); put("data", data) }
    private fun unit(): JsonElement = buildJsonObject { put("type", "Ok") }

    private fun fail(code: ErrorCode, message: String) = CoreException(ApiError(code, message))

    private fun emit(type: String, data: JsonElement?) {
        val raw = buildJsonObject { put("type", type); if (data != null) put("data", data) }
        _events.tryEmit(CoreEvent(type, data ?: JsonNull, CoreJson.encodeToString(JsonObject.serializer(), raw)))
    }
}
