package io.atomik.kb

import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.PreferenceDataStoreFactory
import androidx.datastore.preferences.core.Preferences
import io.atomik.kb.data.PageOpener
import io.atomik.kb.data.PagesRepository
import io.atomik.kb.data.PendingInserts
import io.atomik.kb.data.SearchRepository
import io.atomik.kb.data.SecurityRepository
import io.atomik.kb.data.SettingsRepository
import io.atomik.kb.data.SpacesRepository
import io.atomik.kb.data.SyncRepository
import io.atomik.kb.engine.fake.FakeEngine
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestDispatcher
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.rules.TestWatcher
import org.junit.runner.Description
import java.io.File

/** Runs view-model work on an unconfined test dispatcher, so launched work finishes before the next line. */
@OptIn(ExperimentalCoroutinesApi::class)
class MainDispatcherRule(val dispatcher: TestDispatcher = UnconfinedTestDispatcher()) : TestWatcher() {
    override fun starting(description: Description) = Dispatchers.setMain(dispatcher)
    override fun finished(description: Description) = Dispatchers.resetMain()
}

/** The repositories over one FakeEngine, built by hand (no Hilt in JVM tests). */
class TestEnv(seed: Boolean = true, scope: CoroutineScope? = null) {
    val engine = FakeEngine(seed = seed, scope = scope)
    private val file = File.createTempFile("settings", ".preferences_pb").also { it.delete(); it.deleteOnExit() }
    val store: DataStore<Preferences> = PreferenceDataStoreFactory.create(
        scope = CoroutineScope(Dispatchers.Unconfined),
        produceFile = { file },
    )
    val settings = SettingsRepository(store)
    val spaces = SpacesRepository(engine)
    val pages = PagesRepository(engine)
    val search = SearchRepository(engine)
    val sync = SyncRepository(engine, settings)
    val security = SecurityRepository(engine)
    val opener = PageOpener()
    val pending = PendingInserts()
}
