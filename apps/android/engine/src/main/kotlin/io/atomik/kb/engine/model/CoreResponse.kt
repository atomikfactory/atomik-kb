package io.atomik.kb.engine.model

import kotlinx.serialization.Serializable

/** `kb_types::Workspace`. */
@Serializable
data class Workspace(val id: String, val name: String, val createdAt: Long = 0)

/** `kb_types::AppStatus`. */
@Serializable
data class AppStatus(
    val workspaceInitialized: Boolean = false,
    val schemaVersion: Int = 0,
    val dbEncrypted: Boolean = false,
    val dbCipher: String = "",
    val deviceId: String = "",
    val dataDir: String = "",
    val coreVersion: String = "",
    val apiVersion: Int = 0,
    val platform: String = "",
)

/** `kb_types::DeviceInfo`. */
@Serializable
data class DeviceInfo(val deviceId: String = "", val name: String = "", val platform: String = "", val createdAt: Long = 0)

/** `kb_types::SpaceInfo` (the fields the phone shows). */
@Serializable
data class SpaceInfo(
    val id: String,
    val name: String,
    val icon: String? = null,
    val createdAt: Long = 0,
    val pageCount: Int = 0,
    val shareState: String? = null,
    val role: String? = null,
    val memberCount: Int? = null,
)

/** `kb_types::PageInfo` (metadata the phone shows; properties and styling are ignored). */
@Serializable
data class PageInfo(
    val id: String,
    val spaceId: String,
    val parentId: String? = null,
    val title: String = "",
    val icon: String? = null,
    val cover: String? = null,
    val locked: Boolean? = null,
    val createdAt: Long = 0,
    val updatedAt: Long = 0,
    val trashedAt: Long? = null,
    val template: Boolean? = null,
)

@Serializable
data class PageNode(val page: PageInfo, val children: List<PageNode> = emptyList())

/** `kb_types::TrashedPage`. */
@Serializable
data class TrashedPage(val page: PageInfo, val originalParentId: String? = null, val descendantCount: Int = 0)

/** `kb_types::TreeView`. */
@Serializable
data class TreeView(
    val spaceId: String,
    val roots: List<PageNode> = emptyList(),
    val trashed: List<TrashedPage> = emptyList(),
    val favorites: List<String> = emptyList(),
)

/** A range of UTF-16 code units inside a snippet. */
@Serializable
data class TextRange(val start: Int, val end: Int)

/** `kb_types::SearchHit`. */
@Serializable
data class SearchHit(
    val pageId: String,
    val spaceId: String,
    val title: String = "",
    val icon: String? = null,
    val blockId: String? = null,
    val commentId: String? = null,
    val snippet: String = "",
    val highlights: List<TextRange> = emptyList(),
)

@Serializable
enum class SyncState { Disabled, Idle, Pushing, Pulling, Offline, Error, Forked }

/** `kb_types::SyncInfo`. */
@Serializable
data class SyncInfo(
    val configured: Boolean = false,
    val serverUrl: String? = null,
    val state: SyncState = SyncState.Disabled,
    val pendingEntries: Int = 0,
    val parkedItems: Int = 0,
    val remoteCursor: Long = 0,
    val lastPushAt: Long? = null,
    val lastPullAt: Long? = null,
    val lastError: String? = null,
    val forkDetected: Boolean = false,
    val devicesSeen: Int = 0,
    val needsInviteToken: Boolean = false,
)

@Serializable
enum class PairingState { Idle, Starting, Waiting, Confirming, PeerConfirming, Transferring, Done, Failed, Cancelled }

/** `kb_types::PairingInfo`. */
@Serializable
data class PairingInfo(
    val role: PairingRole = PairingRole.Joiner,
    val state: PairingState = PairingState.Idle,
    val code: String? = null,
    val sas: String? = null,
    val peerName: String? = null,
    val serverUrl: String? = null,
    val error: String? = null,
    val expiresAt: Long? = null,
)

/** `kb_types::AppLockInfo`. */
@Serializable
data class AppLockInfo(
    val enabled: Boolean = false,
    val memoryKib: Int = 0,
    val iterations: Int = 0,
    val parallelism: Int = 0,
)

/** `kb_types::SecurityInfo`. */
@Serializable
data class SecurityInfo(
    val accountId: String? = null,
    val appLock: AppLockInfo = AppLockInfo(),
    val thisDevice: String = "",
    val devices: Int = 0,
    val rootGeneration: Int = 0,
    val rotationPending: Boolean = false,
    val thisDeviceRevoked: Boolean = false,
    val recoveryPublished: Boolean = false,
)

/** `kb_types::DeviceEntry`. */
@Serializable
data class DeviceEntry(
    val deviceId: String,
    val name: String = "",
    val addedAt: Long = 0,
    val addedViaKit: Boolean = false,
    val revokedAt: Long? = null,
    val isThisDevice: Boolean = false,
)
