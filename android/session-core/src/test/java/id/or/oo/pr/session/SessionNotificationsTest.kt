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
}
