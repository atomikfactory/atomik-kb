package io.atomik.kb.engine

import app.cash.turbine.test
import io.atomik.kb.engine.fake.FakeEngine
import io.atomik.kb.engine.model.*
import kotlinx.coroutines.test.runTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull
import kotlin.test.assertTrue

class FakeEngineTest {
    @Test fun onboardingCreatesWorkspaceAndSpace() = runTest {
        val e = FakeEngine()
        assertNull(e.workspace())
        e.dispatch(WorkspaceCreate("Notes", "Personal"), Workspace.serializer())
        assertEquals("Notes", e.workspace()?.name)
        assertEquals(listOf("Personal"), e.spaces().map { it.name })
        assertFailsWith<CoreException> { e.dispatch(WorkspaceCreate("Again"), Workspace.serializer()) }
    }

    @Test fun pageTreeFavoritesAndTrash() = runTest {
        val e = FakeEngine(seed = true)
        val space = e.spaces().first()
        val page = e.createPage(space.id, "Fresh")
        e.send(PageFavoriteSet(page.id, true))
        assertTrue(page.id in e.tree(space.id).favorites)
        e.send(PageRename(page.id, "Renamed"))
        assertEquals("Renamed", e.page(page.id).title)
        e.send(PageTrash(page.id))
        val tree = e.tree(space.id)
        assertTrue(tree.roots.none { it.page.id == page.id })
        assertTrue(page.id !in tree.favorites)
        assertEquals(page.id, tree.trashed.single().page.id)
        e.send(PageRestore(page.id))
        assertTrue(e.tree(space.id).roots.any { it.page.id == page.id })
    }

    @Test fun searchFindsTitlesAndBodies() = runTest {
        val e = FakeEngine(seed = true)
        val titleHits = e.search("shopping")
        assertEquals("Shopping list", titleHits.single().title)
        val bodyHits = e.search("coffee")
        assertEquals("Shopping list", bodyHits.single().title)
        assertTrue(bodyHits.single().highlights.isNotEmpty())
        assertEquals(emptyList(), e.search("zzzz"))
    }

    @Test fun mutationsEmitEvents() = runTest {
        val e = FakeEngine(seed = true)
        val space = e.spaces().first()
        e.events.test {
            e.createPage(space.id, "x")
            assertEquals("StructureChanged", awaitItem().type)
            cancelAndIgnoreRemainingEvents()
        }
    }

    @Test fun pairingJoinerFlow() = runTest {
        val e = FakeEngine()
        val started = e.dispatch(PairingStart(PairingRole.Joiner, "https://kb.example"), PairingInfo.serializer())
        assertEquals(PairingState.Waiting, started.state)
        assertTrue(started.code!!.isNotBlank())
        e.advancePairing()
        val confirming = e.pairingInfo()
        assertEquals(PairingState.Confirming, confirming.state)
        assertEquals(6, confirming.sas!!.length)
        e.send(PairingConfirm)
        assertEquals(PairingState.Done, e.pairingInfo().state)
        assertTrue(e.spaces().isNotEmpty())
        assertEquals("https://kb.example", e.syncInfo().serverUrl)
    }

    @Test fun lockAndUnlock() = runTest {
        val e = FakeEngine(seed = true)
        e.send(AppLockEnable("correct horse"))
        assertTrue(e.security().appLock.enabled)
        e.lock()
        assertEquals(EngineState.Locked, e.state.value)
        val ex = assertFailsWith<CoreException> { e.spaces() }
        assertEquals(ErrorCode.Locked, ex.error.code)
        assertFailsWith<CoreException> { e.unlock("wrong") }
        e.unlock("correct horse")
        assertEquals(EngineState.Ready, e.state.value)
        assertTrue(e.spaces().isNotEmpty())
    }

    @Test fun docRoundTrip() = runTest {
        val e = FakeEngine(seed = true)
        val page = e.tree(e.spaces().first().id).roots.first().page
        e.docImport(page.id, "hello".toByteArray())
        assertEquals("hello", e.docOpen(page.id).decodeToString())
    }
}
