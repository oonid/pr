package id.or.oo.pr

import org.junit.Assert.assertEquals
import org.junit.Test

class TerminalStartTest {

    @Test
    fun aNewTerminalOpensItsSession() {
        assertEquals(TerminalStart.OPEN, terminalStart(restored = false, sessionOpen = false))
        assertEquals(TerminalStart.OPEN, terminalStart(restored = false, sessionOpen = true))
    }

    @Test
    fun aRecreatedTerminalGetsItsSessionBack() {
        assertEquals(TerminalStart.OPEN, terminalStart(restored = true, sessionOpen = true))
    }

    @Test
    fun aTerminalRestoredWithoutItsSessionSaysItEndedInsteadOfStartingOne() {
        assertEquals(TerminalStart.SHOW_ENDED, terminalStart(restored = true, sessionOpen = false))
    }
}
