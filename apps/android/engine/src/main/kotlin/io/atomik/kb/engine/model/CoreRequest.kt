package io.atomik.kb.engine.model

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.serializer

/** `kb_types::PairingRole`. */
@Serializable
enum class PairingRole { Joiner, Sponsor }

/** `kb_types::SearchScope`. */
@Serializable
enum class SearchScope { All, Titles }

/**
 * The v1 subset of `kb_types::Request`, serialised adjacently tagged:
 * `{"type":"WorkspaceCreate","data":{"name":"x"}}`; unit requests carry no `data`.
 */
sealed interface CoreRequest {
    val type: String
    fun data(): JsonElement? = null

    fun toJsonObject(): JsonObject = buildJsonObject {
        put("type", type)
        data()?.let { put("data", it) }
    }

    fun toJson(): String = CoreJson.encodeToString(JsonObject.serializer(), toJsonObject())
}

private inline fun <reified T> T.encode(): JsonElement = CoreJson.encodeToJsonElement(serializer<T>(), this)

data object AppStatusRequest : CoreRequest { override val type = "AppStatus" }
data object WorkspaceGet : CoreRequest { override val type = "WorkspaceGet" }

@Serializable
data class WorkspaceCreate(val name: String, val firstSpaceName: String? = null) : CoreRequest {
    override val type get() = "WorkspaceCreate"
    override fun data() = encode()
}

@Serializable
data class SettingsGet(val key: String) : CoreRequest {
    override val type get() = "SettingsGet"
    override fun data() = encode()
}

@Serializable
data class SettingsSet(val key: String, val value: JsonElement) : CoreRequest {
    override val type get() = "SettingsSet"
    override fun data() = encode()
}

data object SpaceList : CoreRequest { override val type = "SpaceList" }

@Serializable
data class SpaceCreate(val name: String, val icon: String? = null) : CoreRequest {
    override val type get() = "SpaceCreate"
    override fun data() = encode()
}

@Serializable
data class TreeGet(val spaceId: String) : CoreRequest {
    override val type get() = "TreeGet"
    override fun data() = encode()
}

@Serializable
data class PageGet(val pageId: String) : CoreRequest {
    override val type get() = "PageGet"
    override fun data() = encode()
}

@Serializable
data class PageCreate(
    val spaceId: String,
    val parentId: String? = null,
    val index: Int? = null,
    val title: String,
) : CoreRequest {
    override val type get() = "PageCreate"
    override fun data() = encode()
}

@Serializable
data class PageRename(val pageId: String, val title: String) : CoreRequest {
    override val type get() = "PageRename"
    override fun data() = encode()
}

@Serializable
data class PageTrash(val pageId: String) : CoreRequest {
    override val type get() = "PageTrash"
    override fun data() = encode()
}

@Serializable
data class PageRestore(val pageId: String) : CoreRequest {
    override val type get() = "PageRestore"
    override fun data() = encode()
}

@Serializable
data class PageFavoriteSet(val pageId: String, val favorite: Boolean) : CoreRequest {
    override val type get() = "PageFavoriteSet"
    override fun data() = encode()
}

@Serializable
data class Search(
    val query: String,
    val spaceId: String? = null,
    val scope: SearchScope? = null,
    val limit: Int? = null,
) : CoreRequest {
    override val type get() = "Search"
    override fun data() = encode()
}

@Serializable
data class SyncConfigure(val serverUrl: String? = null, val inviteToken: String? = null) : CoreRequest {
    override val type get() = "SyncConfigure"
    override fun data() = encode()
}

data object SyncStatusRequest : CoreRequest { override val type = "SyncStatus" }
data object SyncNowRequest : CoreRequest { override val type = "SyncNow" }

@Serializable
data class PairingStart(
    val role: PairingRole,
    val serverUrl: String? = null,
    val code: String? = null,
) : CoreRequest {
    override val type get() = "PairingStart"
    override fun data() = encode()
}

data object PairingConfirm : CoreRequest { override val type = "PairingConfirm" }
data object PairingCancel : CoreRequest { override val type = "PairingCancel" }
data object PairingStatus : CoreRequest { override val type = "PairingStatus" }

data object DeviceList : CoreRequest { override val type = "DeviceList" }
data object DeviceInfoRequest : CoreRequest { override val type = "DeviceInfo" }
data object SecurityStatus : CoreRequest { override val type = "SecurityStatus" }

@Serializable
data class AppLockEnable(val passphrase: String) : CoreRequest {
    override val type get() = "AppLockEnable"
    override fun data() = encode()
}

@Serializable
data class AppLockDisable(val passphrase: String) : CoreRequest {
    override val type get() = "AppLockDisable"
    override fun data() = encode()
}

@Serializable
data class AppLockChange(val current: String, val new: String) : CoreRequest {
    override val type get() = "AppLockChange"
    override fun data() = encode()
}

@Serializable
data class AttachmentBegin(val spaceId: String, val name: String, val mime: String) : CoreRequest {
    override val type get() = "AttachmentBegin"
    override fun data() = encode()
}

@Serializable
data class AttachmentFinish(val attachmentId: String) : CoreRequest {
    override val type get() = "AttachmentFinish"
    override fun data() = encode()
}

@Serializable
data class AttachmentAbort(val attachmentId: String) : CoreRequest {
    override val type get() = "AttachmentAbort"
    override fun data() = encode()
}
