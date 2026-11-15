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
import io.atomik.kb.engine.RustEngine
import io.atomik.kb.engine.keystore.AndroidKeystoreAdapter
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import java.io.File
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
 * THE SEAM. Everything in the app talks to [Engine] only. Production runs the Rust core through
 * [RustEngine]; [FakeEngine] stays for previews and tests.
 */
@Module
@InstallIn(SingletonComponent::class)
object EngineModule {
    @Provides @Singleton
    fun engine(@ApplicationContext context: Context, keystore: AndroidKeystoreAdapter): Engine =
        RustEngine(dataDir = File(context.filesDir, "kb").apply { mkdirs() }.path, keystore = keystore)
}
