package io.atomik.kb.features.page

import io.atomik.kb.MainDispatcherRule
import io.atomik.kb.TestEnv
import io.atomik.kb.data.PendingInsert
import io.atomik.kb.engine.model.PageFavoriteSet
import io.atomik.kb.engine.send
import io.atomik.kb.engine.spaces
import io.atomik.kb.engine.tree
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.launchIn
import kotlinx.coroutines.plus
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import org.junit.Rule
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class PageViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    private fun vm(env: TestEnv) = PageViewModel(env.engine, env.pages, env.spaces, env.pending, env.settings)

    private suspend fun firstPageId(env: TestEnv): String = env.engine.tree(env.engine.spaces().first().id).roots.first().page.id

    private fun keys(message: String) = Json.parseToJsonElement(message).jsonObject.keys

    @Test fun loadsThePageAndTitle() = runTest {
        val env = TestEnv()
        val page = firstPageId(env)
        val vm = vm(env).also { it.load(page, null) }
        val s = vm.state.value
        assertFalse(s.loading)
        assertEquals("Welcome", s.title)
        assertTrue(s.favorite)
    }

    @Test fun unknownPageShowsAnError() = runTest {
        val vm = vm(TestEnv()).also { it.load("missing", null) }
        assertFalse(vm.state.value.loading)
        assertTrue(vm.state.value.error != null)
    }

    @Test fun readySendsInitThenPendingInsertsThenForwardsEvents() = runTest {
        val env = TestEnv()
        val page = firstPageId(env)
        env.pending.add(page, PendingInsert.Text("shared text"))
        env.pending.add(page, PendingInsert.Attachment("a1", "cat.png", "image/png"))
        val vm = vm(env)
        vm.setEnvironment(dark = true, locale = "fr-FR")
        vm.load(page, "title")
        val out = mutableListOf<String>()
        vm.outbound.onEach { out += it }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))

        vm.onEditorMessage("""{"host":"ready"}""")
        assertEquals(listOf(setOf("init"), setOf("insert"), setOf("insert")), out.map { keys(it) })
        val init = Json.parseToJsonElement(out[0]).jsonObject["init"]!!.jsonObject
        assertEquals("dark", init["theme"].toString().trim('"'))
        assertEquals("\"title\"", init["focus"].toString())
        assertTrue(vm.state.value.editorReady)
        assertTrue(env.pending.drain(page).isEmpty())

        out.clear()
        env.pages.rename(page, "Renamed")
        assertTrue(out.any { keys(it) == setOf("event") }, out.toString())
    }

    @Test fun callsAreAnsweredThroughTheEngine() = runTest {
        val env = TestEnv()
        val page = firstPageId(env)
        val vm = vm(env).also { it.load(page, null) }
        val out = mutableListOf<String>()
        vm.outbound.onEach { out += it }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        vm.onEditorMessage("""{"id":3,"op":"dispatch","request":{"type":"PageGet","data":{"pageId":"$page"}}}""")
        val reply = Json.parseToJsonElement(out.single()).jsonObject
        assertEquals("3", reply["id"].toString())
        assertTrue("ok" in reply)
    }

    @Test fun themeChangesAreForwardedOnlyOnceTheEditorIsReady() = runTest {
        val env = TestEnv()
        val vm = vm(env).also { it.load(firstPageId(env), null) }
        val out = mutableListOf<String>()
        vm.outbound.onEach { out += it }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        vm.setEnvironment(true, "en")
        assertTrue(out.isEmpty())
        vm.onEditorMessage("""{"host":"ready"}""")
        out.clear()
        vm.setEnvironment(false, "en")
        assertEquals(listOf(setOf("theme")), out.map { keys(it) })
    }

    @Test fun linksAndDictationBecomeEffects() = runTest {
        val env = TestEnv()
        val vm = vm(env).also { it.load(firstPageId(env), null) }
        val effects = mutableListOf<PageEffect>()
        vm.effects.onEach { effects += it }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        vm.onEditorMessage("""{"host":"openPage","pageId":"p9"}""")
        vm.onEditorMessage("""{"host":"openExternal","url":"https://example.org"}""")
        vm.onEditorMessage("""{"host":"dictate"}""")
        assertEquals(
            listOf(PageEffect.OpenPage("p9"), PageEffect.OpenExternal("https://example.org"), PageEffect.RequestDictation),
            effects,
        )
    }

    @Test fun dictationStreamsPartialAndFinalText() = runTest {
        val env = TestEnv()
        val vm = vm(env).also { it.load(firstPageId(env), null) }
        val out = mutableListOf<String>()
        vm.outbound.onEach { out += it }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        vm.onDictationState(true)
        vm.onDictationPartial("hello wor")
        vm.onDictationFinal("hello world")
        vm.onDictationState(false)
        assertTrue(out[0].contains("\"listening\""))
        assertTrue(out[1].contains("\"partial\":\"hello wor\""))
        assertTrue(out[2].contains("\"insert\"") && out[2].contains("hello world"))
        assertTrue(out.last().contains("\"idle\""))
        assertFalse(vm.state.value.dictating)
    }

    @Test fun togglingFavoriteAndTrashing() = runTest {
        val env = TestEnv()
        val space = env.engine.spaces().first()
        val page = env.pages.create(space.id, "Mine")
        val vm = vm(env).also { it.load(page.id, null) }
        assertFalse(vm.state.value.favorite)
        vm.toggleFavorite()
        assertTrue(vm.state.value.favorite)
        env.engine.send(PageFavoriteSet(page.id, false))
        assertFalse(vm.state.value.favorite)
        val effects = mutableListOf<PageEffect>()
        vm.effects.onEach { effects += it }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        vm.moveToTrash()
        assertEquals(listOf<PageEffect>(PageEffect.Closed), effects)
        assertTrue(env.engine.tree(space.id).trashed.any { it.page.id == page.id })
    }

    @Test fun invalidMessagesSurfaceAsEditorErrors() = runTest {
        val env = TestEnv()
        val vm = vm(env).also { it.load(firstPageId(env), null) }
        vm.onEditorMessage("garbage")
        assertTrue(vm.state.value.editorError != null)
        vm.dismissEditorError()
        assertNull(vm.state.value.editorError)
    }
}
