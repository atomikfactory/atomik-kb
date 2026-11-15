package io.atomik.kb.features.share

import android.net.Uri
import io.atomik.kb.MainDispatcherRule
import io.atomik.kb.TestEnv
import io.atomik.kb.data.PendingInsert
import io.atomik.kb.data.ShareInbox
import io.atomik.kb.data.SharedContent
import io.atomik.kb.engine.model.TreeView
import io.atomik.kb.engine.spaces
import io.atomik.kb.engine.tree
import io.atomik.kb.features.capture.CaptureViewModel
import kotlinx.coroutines.flow.launchIn
import kotlinx.coroutines.plus
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.test.runTest
import org.junit.Rule
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

class ShareAndCaptureTest {
    @get:Rule val main = MainDispatcherRule()

    private class FakeReader(private val file: SharedFile) : SharedFileReader {
        override suspend fun read(uri: Uri): SharedFile = file
    }

    private fun shareVm(env: TestEnv, inbox: ShareInbox, reader: SharedFileReader = FakeReader(SharedFile("cat.png", "image/png", byteArrayOf(1, 2, 3)))) =
        ShareViewModel(env.spaces, env.pages, inbox, reader, env.pending, env.opener, env.settings)

    private suspend fun titles(env: TestEnv, tree: TreeView) = tree.roots.map { it.page.title }

    @Test fun sharedTextBecomesAPageWithAPendingInsert() = runTest {
        val env = TestEnv()
        val inbox = ShareInbox().also { it.set(SharedContent(text = "Remember the milk\nand eggs")) }
        val vm = shareVm(env, inbox)
        assertNotNull(vm.state.value.selectedSpaceId)
        val opened = mutableListOf<String>()
        env.opener.requests.onEach { opened += it.pageId }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))

        vm.createPage()

        assertTrue(vm.state.value.done)
        assertEquals(1, opened.size)
        val spaceId = vm.state.value.selectedSpaceId!!
        assertTrue("Remember the milk" in titles(env, env.engine.tree(spaceId)))
        assertEquals(listOf<PendingInsert>(PendingInsert.Text("Remember the milk\nand eggs")), env.pending.drain(opened.single()))
        assertNull(inbox.content.value)
    }

    @Test fun sharedImagesAreWrittenAsAttachmentsThenInserted() = runTest {
        val env = TestEnv()
        val inbox = ShareInbox().also { it.set(SharedContent(images = listOf(Uri.EMPTY, Uri.EMPTY))) }
        val vm = shareVm(env, inbox)
        val opened = mutableListOf<String>()
        env.opener.requests.onEach { opened += it.pageId }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        vm.createPage()
        val inserts = env.pending.drain(opened.single())
        assertEquals(2, inserts.size)
        inserts.forEach {
            it as PendingInsert.Attachment
            assertEquals("cat.png", it.name)
            assertEquals("image/png", it.mime)
        }
    }

    @Test fun aFailingImageReadLeavesAnErrorAndNoNavigation() = runTest {
        val env = TestEnv()
        val inbox = ShareInbox().also { it.set(SharedContent(images = listOf(Uri.EMPTY))) }
        val failing = object : SharedFileReader {
            override suspend fun read(uri: Uri): SharedFile = throw TooLargeException(1)
        }
        val vm = shareVm(env, inbox, failing)
        vm.createPage()
        assertTrue(vm.state.value.error != null)
        assertTrue(!vm.state.value.done)
        assertTrue(!vm.state.value.busy)
    }

    @Test fun chosenSpaceIsUsed() = runTest {
        val env = TestEnv()
        val inbox = ShareInbox().also { it.set(SharedContent(text = "hello")) }
        val vm = shareVm(env, inbox)
        val work = env.engine.spaces().first { it.name == "Work" }
        vm.selectSpace(work.id)
        vm.createPage()
        assertTrue("hello" in titles(env, env.engine.tree(work.id)))
    }

    @Test fun captureCreatesAnEmptyPageAndOpensItFocused() = runTest {
        val env = TestEnv()
        val opened = mutableListOf<io.atomik.kb.data.OpenPage>()
        env.opener.requests.onEach { opened += it }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        val vm = CaptureViewModel(env.spaces, env.pages, env.settings, env.opener)
        assertTrue(vm.state.value.done)
        assertEquals("title", opened.single().focus)
        val personal = env.engine.spaces().first()
        assertEquals(3, env.engine.tree(personal.id).roots.size)
    }

    @Test fun captureHonoursTheCaptureSpaceSetting() = runTest {
        val env = TestEnv()
        val work = env.engine.spaces().first { it.name == "Work" }
        env.settings.setCaptureSpace(work.id)
        CaptureViewModel(env.spaces, env.pages, env.settings, env.opener)
        assertEquals(3, env.engine.tree(work.id).roots.size)
    }
}
