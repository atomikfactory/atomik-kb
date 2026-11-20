package io.atomik.kb.data

import io.atomik.kb.engine.CoreEvent
import io.atomik.kb.engine.CoreException
import io.atomik.kb.engine.Engine
import io.atomik.kb.engine.EngineState
import io.atomik.kb.engine.appStatus
import io.atomik.kb.engine.createPage
import io.atomik.kb.engine.devices
import io.atomik.kb.engine.model.*
import io.atomik.kb.engine.page
import io.atomik.kb.engine.pairingInfo
import io.atomik.kb.engine.search
import io.atomik.kb.engine.security
import io.atomik.kb.engine.send
import io.atomik.kb.engine.spaces
import io.atomik.kb.engine.syncInfo
import io.atomik.kb.engine.tree
import io.atomik.kb.engine.writeAttachment
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.mapLatest
import kotlinx.coroutines.flow.merge
import kotlinx.coroutines.flow.onStart
import kotlinx.coroutines.CancellationException
import javax.inject.Inject
import javax.inject.Singleton

/** Something being loaded from the core: the three states every screen must draw. */
sealed interface Loadable<out T> {
    data object Loading : Loadable<Nothing>
    data class Data<T>(val value: T) : Loadable<T>
    data class Error(val message: String, val locked: Boolean = false) : Loadable<Nothing>
}

fun Throwable.toLoadError(): Loadable.Error =
    Loadable.Error(message ?: "error", locked = (this as? CoreException)?.error?.code == ErrorCode.Locked)

private suspend fun <T> attempt(load: suspend () -> T): Loadable<T> = try {
    Loadable.Data(load())
} catch (e: CancellationException) {
    throw e
} catch (e: Exception) {
    e.toLoadError()
}

/**
 * Loads once, then again after every core event for which [matches] holds and whenever the engine
 * becomes Ready (an unlock must refresh what the lock hid). A newer trigger cancels a slower load.
 */
@OptIn(ExperimentalCoroutinesApi::class)
fun <T> Engine.watch(matches: (CoreEvent) -> Boolean, load: suspend () -> T): Flow<Loadable<T>> =
    merge(
        events.filter(matches).map { },
        state.filter { it is EngineState.Ready }.map { },
    ).onStart { emit(Unit) }
        .mapLatest { attempt(load) }
        .onStart { emit(Loadable.Loading) }

private val structureEvents = setOf("SpacesChanged", "StructureChanged", "WorkspaceCreated", "PagesMovedSpace", "SecurityChanged")

@Singleton
class SpacesRepository @Inject constructor(private val engine: Engine) {
    val spaces: Flow<Loadable<List<SpaceInfo>>> = engine.watch({ it.type in structureEvents }) { engine.spaces() }

    fun tree(spaceId: String): Flow<Loadable<TreeView>> =
        engine.watch({ it.type in structureEvents }) { engine.tree(spaceId) }

    /** `true` once the workspace exists (onboarding done); follows pairing and workspace creation. */
    val onboarded: Flow<Loadable<Boolean>> =
        engine.watch({ it.type == "WorkspaceCreated" || it.type == "PairingChanged" || it.type == "SecurityChanged" }) {
            engine.appStatus().workspaceInitialized
        }

    suspend fun createWorkspace(name: String, firstSpace: String) {
        engine.dispatch(WorkspaceCreate(name.trim(), firstSpace.trim().ifEmpty { null }), Workspace.serializer())
    }

    suspend fun createSpace(name: String): SpaceInfo = engine.dispatch(SpaceCreate(name.trim()), SpaceInfo.serializer())
}

@Singleton
class PagesRepository @Inject constructor(private val engine: Engine) {
    suspend fun get(pageId: String): PageInfo = engine.page(pageId)
    suspend fun create(spaceId: String, title: String = "", parentId: String? = null): PageInfo = engine.createPage(spaceId, title, parentId)
    suspend fun rename(pageId: String, title: String) = engine.send(PageRename(pageId, title))
    suspend fun trash(pageId: String) = engine.send(PageTrash(pageId))
    suspend fun restore(pageId: String) = engine.send(PageRestore(pageId))
    suspend fun setFavorite(pageId: String, favorite: Boolean) = engine.send(PageFavoriteSet(pageId, favorite))

    /** Stores [bytes] as an attachment of [spaceId]; returns its id for the editor to insert. */
    suspend fun addAttachment(spaceId: String, name: String, mime: String, bytes: ByteArray): String =
        engine.writeAttachment(spaceId, name, mime, bytes)
}

@Singleton
class SearchRepository @Inject constructor(private val engine: Engine) {
    suspend fun search(query: String, spaceId: String? = null): List<SearchHit> = engine.search(query, spaceId)
}

@Singleton
class SyncRepository @Inject constructor(
    private val engine: Engine,
    private val settings: SettingsRepository,
) {
    val info: Flow<Loadable<SyncInfo>> =
        engine.watch({ it.type == "SyncChanged" || it.type == "SyncRequested" }) { engine.syncInfo() }

    val pairing: Flow<Loadable<PairingInfo>> =
        engine.watch({ it.type == "PairingChanged" }) { engine.pairingInfo() }

    /** Asks the driver for a round and waits a little for it to settle. Returns true when idle. */
    suspend fun syncNow(timeoutMs: Long = 20_000): Boolean {
        engine.send(SyncNowRequest)
        val idle = engine.syncOnce(timeoutMs)
        if (idle) settings.setLastSyncAt(System.currentTimeMillis())
        return idle
    }

    suspend fun configure(serverUrl: String?, inviteToken: String? = null) {
        engine.send(SyncConfigure(serverUrl?.trim()?.ifEmpty { null }, inviteToken))
    }

    suspend fun startPairing(serverUrl: String): PairingInfo =
        engine.dispatch(PairingStart(PairingRole.Joiner, serverUrl.trim()), PairingInfo.serializer())

    suspend fun confirmPairing() = engine.send(PairingConfirm)
    suspend fun cancelPairing() = engine.send(PairingCancel)
}

@Singleton
class SecurityRepository @Inject constructor(private val engine: Engine) {
    val state = engine.state

    val info: Flow<Loadable<SecurityInfo>> =
        engine.watch({ it.type == "SecurityChanged" }) { engine.security() }

    val devices: Flow<Loadable<List<DeviceEntry>>> =
        engine.watch({ it.type == "DevicesChanged" || it.type == "SecurityChanged" }) { engine.devices() }

    suspend fun appLockEnabled(): Boolean = engine.security().appLock.enabled

    suspend fun enableLock(passphrase: String) = engine.send(AppLockEnable(passphrase))
    suspend fun disableLock(passphrase: String) = engine.send(AppLockDisable(passphrase))
    suspend fun changeLock(current: String, new: String) = engine.send(AppLockChange(current, new))
    suspend fun unlock(passphrase: String) = engine.unlock(passphrase)
    fun lock() = engine.lock()

    /**
     * Checks a passphrase without changing anything: lock, then unlock with it. A wrong passphrase
     * throws and leaves the app locked, which is where a failed biometric enrolment ends up anyway.
     */
    suspend fun verifyPassphrase(passphrase: String) {
        engine.lock()
        engine.unlock(passphrase)
    }
}
