package io.atomik.kb

import android.app.Application
import android.webkit.WebView
import androidx.hilt.work.HiltWorkerFactory
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import androidx.work.Configuration
import dagger.hilt.android.HiltAndroidApp
import io.atomik.kb.engine.Engine
import io.atomik.kb.security.LockManager
import io.atomik.kb.sync.SyncScheduler
import javax.inject.Inject

@HiltAndroidApp
class KbApplication : Application(), Configuration.Provider {
    @Inject lateinit var workerFactory: HiltWorkerFactory
    @Inject lateinit var lockManager: LockManager
    @Inject lateinit var syncScheduler: SyncScheduler
    @Inject lateinit var engine: Engine

    override val workManagerConfiguration: Configuration
        get() = Configuration.Builder().setWorkerFactory(workerFactory).build()

    override fun onCreate() {
        super.onCreate()
        WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG)
        syncScheduler.schedule()
        ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onStart(owner: LifecycleOwner) {
                engine.setForeground(true)
                lockManager.onAppForegrounded()
            }

            override fun onStop(owner: LifecycleOwner) {
                engine.setForeground(false)
                lockManager.onAppBackgrounded()
            }
        })
    }
}
