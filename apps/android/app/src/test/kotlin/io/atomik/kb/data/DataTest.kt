package io.atomik.kb.data

import io.atomik.kb.features.capture.pickCaptureSpace
import io.atomik.kb.engine.model.SpaceInfo
import io.atomik.kb.features.settings.passphraseProblem
import io.atomik.kb.security.shouldLock
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class DataTest {
    @Test fun serverUrlNormalisation() {
        assertEquals("https://kb.example.org", normalizeServerUrl("kb.example.org"))
        assertEquals("https://kb.example.org", normalizeServerUrl("  https://KB.example.org/  "))
        assertEquals("http://10.0.2.2:8080", normalizeServerUrl("http://10.0.2.2:8080/path?q=1"))
        assertEquals("https://kb.example.org:8443", normalizeServerUrl("kb.example.org:8443/x"))
        assertNull(normalizeServerUrl(""))
        assertNull(normalizeServerUrl("   "))
        assertNull(normalizeServerUrl("not a url"))
        assertNull(normalizeServerUrl("ftp://kb.example.org"))
        assertNull(normalizeServerUrl("https://"))
    }

    @Test fun autoLockDecision() {
        assertTrue(shouldLock(AutoLockDelay.Immediately, 0))
        assertFalse(shouldLock(AutoLockDelay.OneMinute, 59_999))
        assertTrue(shouldLock(AutoLockDelay.OneMinute, 60_000))
        assertFalse(shouldLock(AutoLockDelay.FiveMinutes, 299_000))
        assertTrue(shouldLock(AutoLockDelay.FifteenMinutes, 900_000))
        assertFalse(shouldLock(AutoLockDelay.Never, Long.MAX_VALUE))
        assertEquals(AutoLockDelay.OneMinute, AutoLockDelay.fromName("garbage"))
        assertEquals(AutoLockDelay.Never, AutoLockDelay.fromName("Never"))
    }

    @Test fun passphraseRules() {
        assertTrue(passphraseProblem("short"))
        assertFalse(passphraseProblem("long enough"))
        assertTrue(passphraseProblem("x".repeat(257)))
    }

    @Test fun sharedContentFromIntentParts() {
        val send = "android.intent.action.SEND"
        assertNull(SharedContent.from("android.intent.action.VIEW", "text/plain", "hi", null, emptyList()))
        assertNull(SharedContent.from(send, "text/plain", "   ", null, emptyList()))
        val text = SharedContent.from(send, "text/plain", "https://example.org/a\nsecond", "A great article", emptyList())!!
        assertEquals("A great article", text.suggestedTitle())
        assertEquals("first line", SharedContent.from(send, "text/plain", "\nfirst line\nsecond", null, emptyList())!!.suggestedTitle())
        assertEquals("", SharedContent.from(send, "text/plain", "https://example.org", null, emptyList())!!.suggestedTitle())
        assertEquals(80, SharedContent.from(send, "text/plain", "x".repeat(200), null, emptyList())!!.suggestedTitle().length)
    }

    @Test fun captureSpacePreference() {
        val a = SpaceInfo("a", "A")
        val b = SpaceInfo("b", "B")
        assertEquals(b, pickCaptureSpace(listOf(a, b), "b", "a"))
        assertEquals(a, pickCaptureSpace(listOf(a, b), "gone", "a"))
        assertEquals(a, pickCaptureSpace(listOf(a, b), null, null))
        assertNull(pickCaptureSpace(emptyList(), null, null))
    }

    @Test fun pendingInsertsAreDrainedOnce() {
        val pending = PendingInserts()
        pending.add("p1", PendingInsert.Text("a"))
        pending.add("p1", PendingInsert.Text("b"))
        assertEquals(2, pending.drain("p1").size)
        assertTrue(pending.drain("p1").isEmpty())
        assertTrue(pending.drain("other").isEmpty())
    }
}
