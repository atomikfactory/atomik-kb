package io.atomik.kb.engine

import io.atomik.kb.engine.model.*
import kotlinx.serialization.KSerializer
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.builtins.nullable
import kotlinx.serialization.builtins.serializer
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonObject

/** Decodes a raw `{"ok": Response}` / `{"err": ApiError}` reply; throws [CoreException] on `err`. */
fun <T> decodeReply(reply: String, response: KSerializer<T>): T {
    val root = CoreJson.parseToJsonElement(reply).jsonObject
    root["err"]?.let { throw CoreException(CoreJson.decodeFromJsonElement(ApiError.serializer(), it)) }
    val ok = root["ok"] ?: throw CoreException(ApiError(ErrorCode.Internal, "malformed reply"))
    return decodeOk(ok, response)
}

/** Decodes the `data` of a `Response` object (`{"type":..,"data":..}`). */
fun <T> decodeOk(ok: JsonElement, response: KSerializer<T>): T {
    @Suppress("UNCHECKED_CAST")
    if (response === Unit.serializer()) return Unit as T
    val data = (ok as? JsonObject)?.get("data") ?: JsonPrimitive(null as String?)
    return CoreJson.decodeFromJsonElement(response, data)
}

/** Reads the string `value` of a `Setting` response, if any. */
fun JsonElement?.settingString(): String? = (this as? JsonPrimitive)?.contentOrNull

// Typed calls: the thin vocabulary the repositories speak.
suspend fun Engine.send(request: CoreRequest) = dispatch(request, Unit.serializer())
suspend fun Engine.appStatus(): AppStatus = dispatch(AppStatusRequest, AppStatus.serializer())
suspend fun Engine.workspace(): Workspace? = dispatch(WorkspaceGet, Workspace.serializer().nullable)
suspend fun Engine.spaces(): List<SpaceInfo> = dispatch(SpaceList, ListSerializer(SpaceInfo.serializer()))
suspend fun Engine.tree(spaceId: String): TreeView = dispatch(TreeGet(spaceId), TreeView.serializer())
suspend fun Engine.page(pageId: String): PageInfo = dispatch(PageGet(pageId), PageInfo.serializer())
suspend fun Engine.createPage(spaceId: String, title: String, parentId: String? = null): PageInfo =
    dispatch(PageCreate(spaceId = spaceId, parentId = parentId, title = title), PageInfo.serializer())
suspend fun Engine.search(query: String, spaceId: String? = null, limit: Int? = 30): List<SearchHit> =
    dispatch(Search(query, spaceId, SearchScope.All, limit), ListSerializer(SearchHit.serializer()))
suspend fun Engine.syncInfo(): SyncInfo = dispatch(SyncStatusRequest, SyncInfo.serializer())
suspend fun Engine.pairingInfo(): PairingInfo = dispatch(PairingStatus, PairingInfo.serializer())
suspend fun Engine.security(): SecurityInfo = dispatch(SecurityStatus, SecurityInfo.serializer())
suspend fun Engine.devices(): List<DeviceEntry> = dispatch(DeviceList, ListSerializer(DeviceEntry.serializer()))
