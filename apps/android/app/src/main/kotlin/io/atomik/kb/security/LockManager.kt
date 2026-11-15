package io.atomik.kb.security

import io.atomik.kb.data.AutoLockDelay
import io.atomik.kb.data.SecurityRepository
import io.atomik.kb.data.SettingsRepository
import io.atomik.kb.di.AppScope
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import javax.inject.Inject
import javax.inject.Singleton

/** Whether a stay in the background of [elapsedMs] should lock the app. Pure, for tests. */
fun shouldLock(delay: AutoLockDelay, elapsedMs: Long): Boolean = when (delay) {
    AutoLockDelay.Never -> false
    else -> elapsedMs >= delay.seconds * 1000
}

/**
 * Auto-lock: the process lifecycle tells us when the whole app leaves and returns to the
 * foreground (KbApplication registers it). The lock itself is the core's: `Engine.lock()`.
 */
@Singleton
class LockManager @Inject constructor(
    private val security: SecurityRepository,
    private val settings: SettingsRepository,
    @AppScope private val scope: CoroutineScope,
) {
    /** Replaced in tests. */
    internal var clock: () -> Long = { android.os.SystemClock.elapsedRealtime() }

    @Volatile private var backgroundedAt: Long? = null

    fun onAppBackgrounded() {
        backgroundedAt = clock()
        scope.launch {
            if (settings.current().autoLock == AutoLockDelay.Immediately && security.appLockEnabled()) security.lock()
        }
    }

    fun onAppForegrounded() {
        val since = backgroundedAt ?: return
        backgroundedAt = null
        val elapsed = clock() - since
        scope.launch {
            val delay = settings.current().autoLock
            if (shouldLock(delay, elapsed) && security.appLockEnabled()) security.lock()
        }
    }
}
