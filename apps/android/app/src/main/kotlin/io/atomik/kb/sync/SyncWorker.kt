package io.atomik.kb.sync

import android.content.Context
import androidx.hilt.work.HiltWorker
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import dagger.assisted.Assisted
import dagger.assisted.AssistedInject
import dagger.hilt.android.qualifiers.ApplicationContext
import io.atomik.kb.data.SettingsRepository
import io.atomik.kb.engine.CoreException
import io.atomik.kb.engine.Engine
import io.atomik.kb.engine.EngineState
import io.atomik.kb.engine.model.ErrorCode
import java.util.concurrent.TimeUnit
import javax.inject.Inject
import javax.inject.Singleton

/** A10: a sync round while the process lives or the system wakes us (every 15 minutes, with network). */
@HiltWorker
class SyncWorker @AssistedInject constructor(
    @Assisted context: Context,
    @Assisted params: WorkerParameters,
    private val engine: Engine,
    private val settings: SettingsRepository,
) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        // A locked or still-opening core cannot sync; the next round will.
        if (engine.state.value != EngineState.Ready) return Result.success()
        return try {
            if (engine.syncOnce(TIMEOUT_MS)) settings.setLastSyncAt(System.currentTimeMillis())
            Result.success()
        } catch (e: CoreException) {
            if (e.error.code == ErrorCode.Locked) Result.success() else Result.retry()
        }
    }

    private companion object { const val TIMEOUT_MS = 60_000L }
}

@Singleton
class SyncScheduler @Inject constructor(@ApplicationContext private val context: Context) {
    private val constraints = Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()

    /** Periodic sync (15 min, network required) and one round at app start. */
    fun schedule() {
        val work = WorkManager.getInstance(context)
        work.enqueueUniquePeriodicWork(
            PERIODIC,
            ExistingPeriodicWorkPolicy.KEEP,
            PeriodicWorkRequestBuilder<SyncWorker>(15, TimeUnit.MINUTES).setConstraints(constraints).build(),
        )
        work.enqueueUniqueWork(
            ON_START,
            ExistingWorkPolicy.REPLACE,
            OneTimeWorkRequestBuilder<SyncWorker>().setConstraints(constraints).build(),
        )
    }

    companion object {
        const val PERIODIC = "periodic-sync"
        const val ON_START = "sync-on-start"
    }
}
