package id.or.oo.pr.session

import org.junit.Assert.assertEquals
import org.junit.Test

class SessionNotificationsTest {

    @Test
    fun theRunningTitleCountsSessions() {
        assertEquals("1 session running", runningTitle(listOf("a")))
        assertEquals("2 sessions running", runningTitle(listOf("a", "b")))
    }

    @Test
    fun theRunningTitleWithNoSessionNeverSaysZeroSessions() {
        assertEquals("Stopping…", runningTitle(emptyList()))
    }

    @Test
    fun withNoSessionEndingTheTitleAndTextAreUnchanged() {
        assertEquals("2 sessions running", runningTitle(listOf("alpine", "debian"), emptyList()))
        assertEquals("alpine, debian", runningText(listOf("alpine", "debian"), emptyList(), keepAwake = false, exempt = true))
        assertEquals(
            "alpine · CPU kept awake · Battery saver may stop sessions; allow in the app",
            runningText(listOf("alpine"), emptyList(), keepAwake = true, exempt = false),
        )
    }

    @Test
    fun theOnlySessionEndingIsShownAsStopping() {
        assertEquals("Stopping 1 session…", runningTitle(listOf("web-server"), listOf("web-server")))
        assertEquals(
            "Stopping web-server…",
            runningText(listOf("web-server"), listOf("web-server"), keepAwake = false, exempt = true),
        )
    }

    @Test
    fun runningAndEndingSessionsAreCountedAndNamedApart() {
        assertEquals("1 session running", runningTitle(listOf("alpine", "web-server"), listOf("web-server")))
        assertEquals(
            "alpine · Stopping web-server… · CPU kept awake",
            runningText(listOf("alpine", "web-server"), listOf("web-server"), keepAwake = true, exempt = true),
        )
        assertEquals(
            "2 sessions running",
            runningTitle(listOf("alpine", "debian", "web-server"), listOf("web-server")),
        )
    }

    @Test
    fun everySessionEndingIsCountedAsStopping() {
        assertEquals("Stopping 2 sessions…", runningTitle(listOf("alpine", "web-server"), listOf("alpine", "web-server")))
        assertEquals(
            "Stopping alpine, web-server…",
            runningText(listOf("alpine", "web-server"), listOf("web-server", "alpine"), keepAwake = false, exempt = true),
        )
    }

    @Test
    fun anEndingKeyThatIsNoLongerRunningIsIgnored() {
        assertEquals("1 session running", runningTitle(listOf("alpine"), listOf("web-server")))
        assertEquals("alpine", runningText(listOf("alpine"), listOf("web-server"), keepAwake = false, exempt = true))
    }
}
