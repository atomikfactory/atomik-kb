package io.atomik.kb.di

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.PreferenceDataStoreFactory
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.preferencesDataStoreFile
import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.android.qualifiers.ApplicationContext
import dagger.hilt.components.SingletonComponent
import io.atomik.kb.engine.Engine
import io.atomik.kb.engine.fake.FakeEngine
import io.atomik.kb.engine.keystore.AndroidKeystoreAdapter
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import javax.inject.Qualifier
import javax.inject.Singleton

@Qualifier
@Retention(AnnotationRetention.BINARY)
annotation class AppScope

@Module
@InstallIn(SingletonComponent::class)
object AppModule {
    @Provides @Singleton @AppScope
    fun appScope(): CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    @Provides @Singleton
    fun preferences(@ApplicationContext context: Context): DataStore<Preferences> =
        PreferenceDataStoreFactory.create { context.preferencesDataStoreFile("settings") }

    /** The platform half of the core's `Keystore` trait (A6); RustEngine hands it to `KbEngine`. */
    @Provides @Singleton
    fun keystore(@ApplicationContext context: Context): AndroidKeystoreAdapter = AndroidKeystoreAdapter(context)
}

/**
 * THE SEAM. Everything in the app talks to [Engine] only. The integration step replaces the body of
 * [engine] with the Rust-backed implementation:
 *
 * ```
 * @Provides @Singleton
 * fun engine(@ApplicationContext c: Context, keystore: AndroidKeystoreAdapter): Engine =
 *     RustEngine(dataDir = c.noBackupFilesDir.resolve("kb").path, keystore = keystore)
 * ```
 *
 * where `RustEngine` implements [Engine] over the generated `io.atomik.kb.engine.ffi.KbEngine`
 * (a `PlatformKeystore` that delegates to `keystore.get/set/delete`, an `EventSink` feeding
 * `Engine.events` and `Engine.state`). Until then the app runs against the in-memory [FakeEngine].
 */
@Module
@InstallIn(SingletonComponent::class)
object EngineModule {
    @Provides @Singleton
    fun engine(@AppScope scope: CoroutineScope): Engine = FakeEngine(seed = false, scope = scope)
}
