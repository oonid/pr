package id.or.oo.pr.session

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ProcStatTest {

    @Test
    fun readsTheParentPidAfterTheCommandName() {
        assertEquals(1794, parentPid("1804 (sh) S 1794 1804 1794 34816 1804 4194560 900"))
    }

    @Test
    fun handlesSpacesAndParenthesesInTheCommandName() {
        assertEquals(7, parentPid("42 (a (b) c) R 7 42 42 0 -1 4194304"))
    }

    @Test
    fun rejectsAMalformedLine() {
        assertNull(parentPid("garbage"))
    }

    private fun status(pid: Int, tracer: Int) =
        "Name:\tsh\nState:\tS (sleeping)\nTgid:\t$pid\nPid:\t$pid\nPPid:\t1\nTracerPid:\t$tracer\nUid:\t10200\n"

    @Test
    fun readsTheTracerPidFromStatus() {
        assertEquals(1790, tracerPid(status(1804, 1790)))
    }

    @Test
    fun aStatusWithoutTracerPidHasNone() {
        assertNull(tracerPid("Name:\tsh\nPid:\t1804\n"))
    }

    @Test
    fun traceesAreTheProcessesTracedByProotExceptTheExcludedOnes() {
        val statuses = mapOf(
            1790 to status(1790, 0),
            1804 to status(1804, 1790),
            1810 to status(1810, 1804),
            1811 to status(1811, 1790),
            1900 to status(1900, 1790),
            2000 to status(2000, 0),
            2100 to status(2100, 3000),
        )
        assertEquals(setOf(1811, 1900), traceesOf(1790, statuses, exclude = setOf(1790, 1804)))
    }

    @Test
    fun traceesAreUnknownWhenAStatusCannotBeRead() {
        assertNull(traceesOf(1790, mapOf(1804 to status(1804, 1790), 1900 to null), exclude = setOf(1804)))
    }

    @Test
    fun traceesAreUnknownWhenAStatusHasNoTracerPid() {
        assertNull(traceesOf(1790, mapOf(1900 to "Name:\tsh\nPid:\t1900\n"), exclude = emptySet()))
    }

    @Test
    fun readsTheProcessState() {
        assertEquals('Z', processState("1900 (a (b) c) Z 1 1900 1900 0 -1 4194304"))
        assertEquals('S', processState("1804 (sh) S 1794 1804 1794 34816 1804 4194560 900"))
        assertNull(processState("garbage"))
    }

    @Test
    fun forceKillTargetsEveryTraceeIncludingTheLoginShellBeforeProot() {
        val statuses = mapOf(
            1790 to status(1790, 0),
            1804 to status(1804, 1790),
            1811 to status(1811, 1790),
            1900 to status(1900, 1790),
            2000 to status(2000, 0),
            2100 to status(2100, 3000),
        )
        assertEquals(listOf(1804, 1811, 1900, 1790), forceKillTargets(1790, statuses) { listOf(5000) })
    }

    @Test
    fun forceKillFallsBackToTheWalkWhenTraceesAreUnknown() {
        val statuses = mapOf(1804 to status(1804, 1790), 1900 to null)
        assertEquals(listOf(1804, 1900, 1790), forceKillTargets(1790, statuses) { listOf(1804, 1790, 1900) })
    }

    @Test
    fun endSendsSigtermThenSigcontAndNoSighup() {
        assertEquals(listOf(15, 18), END_SIGNALS)
    }
}
