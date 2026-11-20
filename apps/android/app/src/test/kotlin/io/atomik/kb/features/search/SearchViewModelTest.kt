package io.atomik.kb.features.search

import io.atomik.kb.MainDispatcherRule
import io.atomik.kb.TestEnv
import io.atomik.kb.engine.model.TextRange
import androidx.compose.ui.graphics.Color
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.launchIn
import kotlinx.coroutines.plus
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Rule
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertFalse
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class SearchViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    @Test fun debouncesThenShowsHitsWithHighlights() = runTest {
        val env = TestEnv()
        val vm = SearchViewModel(env.search, env.opener)
        vm.state.onEach { }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        vm.onQueryChange("cof")
        vm.onQueryChange("coffee")
        assertTrue(vm.state.value.loading)
        assertTrue(vm.state.value.results.isEmpty())
        main.dispatcher.scheduler.advanceTimeBy(SearchViewModel.DEBOUNCE_MS + 1)
        main.dispatcher.scheduler.advanceUntilIdle()
        val s = vm.state.value
        assertFalse(s.loading)
        assertEquals(listOf("Shopping list"), s.results.map { it.title })
        assertTrue(s.results.single().highlights.isNotEmpty())
    }

    @Test fun noMatchIsReportedAndBlankClearsResults() = runTest {
        val env = TestEnv()
        val vm = SearchViewModel(env.search, env.opener)
        vm.state.onEach { }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        vm.onQueryChange("zzzzzz")
        main.dispatcher.scheduler.advanceTimeBy(SearchViewModel.DEBOUNCE_MS + 1)
        main.dispatcher.scheduler.advanceUntilIdle()
        assertTrue(vm.state.value.noResults)
        vm.onQueryChange("")
        main.dispatcher.scheduler.advanceTimeBy(SearchViewModel.DEBOUNCE_MS + 1)
        main.dispatcher.scheduler.advanceUntilIdle()
        assertFalse(vm.state.value.noResults)
        assertFalse(vm.state.value.loading)
    }

    @Test fun lockedEngineShowsAnErrorAndRetryWorksAfterUnlock() = runTest {
        val env = TestEnv()
        env.engine.dispatchRaw("""{"type":"AppLockEnable","data":{"passphrase":"correct horse"}}""")
        env.engine.lock()
        val vm = SearchViewModel(env.search, env.opener)
        vm.state.onEach { }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        vm.onQueryChange("milk")
        main.dispatcher.scheduler.advanceTimeBy(SearchViewModel.DEBOUNCE_MS + 1)
        main.dispatcher.scheduler.advanceUntilIdle()
        assertTrue(vm.state.value.error != null)
        env.engine.unlock("correct horse")
        vm.retry()
        main.dispatcher.scheduler.advanceUntilIdle()
        assertNull(vm.state.value.error)
        assertEquals(1, vm.state.value.results.size)
    }

    @Test fun openingAHitAsksTheOpener() = runTest {
        val env = TestEnv()
        val vm = SearchViewModel(env.search, env.opener)
        val opened = mutableListOf<String>()
        env.opener.requests.onEach { opened += it.pageId }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        vm.state.onEach { }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        vm.onQueryChange("welcome")
        main.dispatcher.scheduler.advanceTimeBy(SearchViewModel.DEBOUNCE_MS + 1)
        main.dispatcher.scheduler.advanceUntilIdle()
        vm.open(vm.state.value.results.first())
        assertEquals(1, opened.size)
    }

    @Test fun snippetHighlightsAreClampedToTheText() {
        val text = highlightedSnippet("hello world", listOf(TextRange(6, 11), TextRange(9, 99)), Color.Red)
        assertEquals("hello world", text.text)
        assertTrue(text.spanStyles.isNotEmpty())
        val bad = highlightedSnippet("abc", listOf(TextRange(5, 9)), Color.Red)
        assertEquals("abc", bad.text)
    }
}
