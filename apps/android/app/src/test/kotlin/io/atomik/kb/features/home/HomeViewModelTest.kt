package io.atomik.kb.features.home

import io.atomik.kb.MainDispatcherRule
import io.atomik.kb.TestEnv
import io.atomik.kb.engine.model.PageInfo
import io.atomik.kb.engine.model.PageNode
import kotlinx.coroutines.flow.launchIn
import kotlinx.coroutines.plus
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.test.runTest
import org.junit.Rule
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

class HomeViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    private fun vm(env: TestEnv) = HomeViewModel(env.spaces, env.pages, env.sync, env.settings, env.opener)

    private fun node(id: String, vararg children: PageNode) = PageNode(PageInfo(id, "s", null, id), children.toList())

    @Test fun flattenShowsChildrenOnlyUnderExpandedParents() {
        val roots = listOf(node("a", node("a1", node("a1x")), node("a2")), node("b"))
        assertEquals(listOf("a", "b"), flattenTree(roots, emptySet()).map { it.page.id })
        val open = flattenTree(roots, setOf("a"))
        assertEquals(listOf("a", "a1", "a2", "b"), open.map { it.page.id })
        assertEquals(listOf(0, 1, 1, 0), open.map { it.depth })
        assertEquals(listOf(true, true, false, false), open.map { it.hasChildren })
        assertEquals(listOf("a", "a1", "a1x", "a2", "b"), flattenTree(roots, setOf("a", "a1")).map { it.page.id })
    }

    @Test fun showsTheFirstSpaceItsTreeAndFavourites() = runTest {
        val env = TestEnv()
        val vm = vm(env)
        vm.state.onEach { }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        val s = vm.state.value
        assertFalse(s.loading)
        assertEquals("Personal", s.selectedSpace?.name)
        assertEquals(listOf("Welcome", "Shopping list"), s.rows.map { it.page.title })
        assertEquals(listOf("Welcome"), s.favorites.map { it.title })
    }

    @Test fun switchingSpaceAndExpanding() = runTest {
        val env = TestEnv()
        val vm = vm(env)
        vm.state.onEach { }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        val welcome = vm.state.value.rows.first { it.page.title == "Welcome" }
        assertTrue(welcome.hasChildren && !welcome.expanded)
        vm.toggleExpanded(welcome.page.id)
        assertEquals(listOf("Welcome", "Ideas", "Shopping list"), vm.state.value.rows.map { it.page.title })
        val work = vm.state.value.spaces.first { it.name == "Work" }
        vm.selectSpace(work.id)
        assertEquals("Work", vm.state.value.selectedSpace?.name)
        assertEquals(listOf("Roadmap", "Meeting notes"), vm.state.value.rows.map { it.page.title })
    }

    @Test fun newPageIsCreatedAndOpenedWithTheTitleFocused() = runTest {
        val env = TestEnv()
        val vm = vm(env)
        vm.state.onEach { }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        val opened = mutableListOf<io.atomik.kb.data.OpenPage>()
        env.opener.requests.onEach { opened += it }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        vm.newPage()
        assertEquals(1, opened.size)
        assertEquals("title", opened.single().focus)
        assertEquals(3, vm.state.value.rows.size)
    }

    @Test fun emptySpaceIsReportedEmpty() = runTest {
        val env = TestEnv(seed = false)
        env.spaces.createWorkspace("Notes", "Only")
        val vm = vm(env)
        vm.state.onEach { }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        assertTrue(vm.state.value.isEmpty)
    }

    @Test fun pullToRefreshRunsASyncRound() = runTest {
        val env = TestEnv()
        val vm = vm(env)
        vm.state.onEach { }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        vm.refresh()
        assertFalse(vm.state.value.refreshing)
        assertNotNull(vm.state.value.selectedSpace)
    }

    @Test fun lockedEngineShowsAnErrorThenRecoversAfterUnlock() = runTest {
        val env = TestEnv()
        env.engine.dispatchRaw("""{"type":"AppLockEnable","data":{"passphrase":"correct horse"}}""")
        env.engine.lock()
        val vm = vm(env)
        vm.state.onEach { }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        assertNotNull(vm.state.value.error)
        env.engine.unlock("correct horse")
        assertNull(vm.state.value.error)
        assertEquals("Personal", vm.state.value.selectedSpace?.name)
    }
}
