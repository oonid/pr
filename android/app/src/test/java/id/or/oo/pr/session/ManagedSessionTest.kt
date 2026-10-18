package id.or.oo.pr.session

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CopyOnWriteArrayList

class ManagedSessionTest {

    private val pty = FakePty()
    private val output = StringBuffer()
    private val ended = CopyOnWriteArrayList<ManagedSession>()

    private fun session(
        endGraceMs: Long = 100,
        signalWaitMs: Long = 100,
        sink: (ByteArray, Int, Int) -> Unit = { buf, off, len -> output.append(String(buf, off, len)) },
    ) = ManagedSession(
        alias = "alpine",
        pty = pty,
        emulator = null,
        output = sink,
        onEnded = { ended += it },
        endGraceMs = endGraceMs,
        signalWaitMs = signalWaitMs,
        exitCodeWaitMs = 100,
    ).also { it.start() }

    @Test
    fun drainsOutputWithNoScreenAttached() {
        val s = session()
        pty.emit("hello ")
        pty.emit("world")
        eventually { output.toString() == "hello world" }
        assertFalse(s.hasScreen)
    }

    @Test
    fun programExitRecordsTheCodeAndClosesThePty() {
        val s = session()
        pty.exit(3)
        eventually { ended.isNotEmpty() }
        assertEquals(ManagedSession.Exit(3), s.exit.value)
        assertTrue(pty.closed)
        assertSame(s, ended.single())
        assertFalse(s.endRequested)
    }

    @Test
    fun aReaderFailureWithNoExitCodeKillsTheShellBeforeClosing() {
        session()
        pty.failRead()
        eventually { ended.isNotEmpty() }
        assertEquals(listOf("kill", "close"), pty.calls)
    }

    @Test
    fun aReaderFailureForceKillsWhenTheKillDoesNotEndIt() {
        pty.ignoresKill = true
        session()
        pty.failRead()
        eventually { ended.isNotEmpty() }
        assertEquals(listOf("kill", "forceKill", "close"), pty.calls)
    }

    @Test
    fun aProgramExitDoesNotForceKill() {
        session()
        pty.exit(0)
        eventually { ended.isNotEmpty() }
        assertEquals(listOf("close"), pty.calls)
    }

    @Test
    fun endSendsEofAndMarksTheEndRequested() {
        val s = session(endGraceMs = 1_000)
        s.end()
        eventually { s.exit.value != null }
        assertTrue(pty.eofSent)
        assertFalse(pty.killed)
        assertTrue(s.endRequested)
    }

    @Test
    fun endKillsTheProgramWhenItIgnoresEof() {
        pty.ignoresEof = true
        val s = session()
        s.end()
        eventually { s.exit.value != null }
        assertTrue(pty.killed)
        assertFalse(pty.forceKilled)
        assertEquals(ManagedSession.Exit(137), s.exit.value)
    }

    @Test
    fun endForceKillsWhenTheKillDoesNotEndIt() {
        pty.ignoresEof = true
        pty.ignoresKill = true
        val s = session()
        s.end()
        eventually { s.exit.value != null }
        assertTrue(pty.forceKilled)
    }

    @Test
    fun endSignalsThenWaitsThenSendsEofThenKills() {
        pty.signalled = setOf(42)
        pty.ignoresEof = true
        pty.ignoresKill = true
        val s = session(endGraceMs = 1_000, signalWaitMs = 300)
        s.end()
        eventually(timeoutMs = 10_000) { s.exit.value != null }
        assertEquals(listOf("signal", "eof", "kill", "forceKill", "close"), pty.calls)
        assertTrue(pty.msBetween("signal", "eof") >= 300)
        assertTrue(pty.aliveQueries.isNotEmpty())
        assertTrue(pty.aliveQueries.all { it == setOf(42) })
    }

    @Test
    fun endSendsEofOnceTheSignalledProgramsHaveExited() {
        pty.signalled = setOf(42)
        pty.signalledExitAfterMs = 200
        val s = session(endGraceMs = 2_000, signalWaitMs = 3_000)
        s.end()
        eventually { s.exit.value != null }
        assertEquals(listOf("signal", "eof", "close"), pty.calls)
        assertTrue(pty.msBetween("signal", "eof") >= 150)
        assertTrue(pty.msBetween("signal", "eof") < 1_500)
    }

    @Test
    fun theSlowestEndFinishesWithinTheDerivedWorstCase() {
        pty.signalled = setOf(42)
        pty.ignoresEof = true
        pty.ignoresKill = true
        pty.forceKillLeavesNoExitCode = true
        val s = ManagedSession(
            alias = "alpine",
            pty = pty,
            emulator = null,
            output = { _, _, _ -> },
            onEnded = { ended += it },
            endGraceMs = 1_000,
            signalWaitMs = 1_000,
            exitCodeWaitMs = 1_000,
            killExitWaitMs = 1_000,
        ).also { it.start() }
        val worstWithoutMargin = ManagedSession.endWorstCaseMs(
            signalWaitMs = 1_000,
            endGraceMs = 1_000,
            exitCodeWaitMs = 1_000,
            killExitWaitMs = 1_000,
        ) - ManagedSession.SCAN_MARGIN_MS
        val started = System.nanoTime()
        s.end()
        eventually(timeoutMs = 15_000) { s.exit.value != null }
        val tookMs = (System.nanoTime() - started) / 1_000_000
        assertEquals(listOf("signal", "eof", "kill", "forceKill", "kill", "forceKill", "close"), pty.calls)
        assertTrue("took $tookMs ms", tookMs in worstWithoutMargin..worstWithoutMargin + 800)
    }

    @Test
    fun theEndTimeoutExceedsTheDefaultWorstCase() {
        assertTrue(ManagedSession.END_TIMEOUT_MS > ManagedSession.endWorstCaseMs())
        assertTrue(ManagedSession.endWorstCaseMs() >= 10_000 + 2_000 + 2_000 + 2_000)
    }

    @Test
    fun endSendsEofAtOnceWhenNothingWasSignalled() {
        val s = session(endGraceMs = 2_000, signalWaitMs = 3_000)
        s.end()
        eventually { s.exit.value != null }
        assertEquals(listOf("signal", "eof", "close"), pty.calls)
        assertTrue(pty.msBetween("signal", "eof") < 500)
        assertTrue(pty.aliveQueries.isEmpty())
    }

    @Test
    fun aBlockedEofDoesNotDelayTheKill() {
        pty.eofBlocksUntilExit = true
        val s = session(endGraceMs = 1_000)
        s.end()
        eventually { s.exit.value != null }
        assertEquals(listOf("signal", "eof", "kill", "close"), pty.calls)
        assertEquals(ManagedSession.Exit(137), s.exit.value)
    }

    @Test
    fun endStopsWhenTheSignalEndsTheSession() {
        pty.signalled = setOf(42)
        pty.exitsOnSignal = true
        val s = session(endGraceMs = 2_000, signalWaitMs = 2_000)
        s.end()
        eventually { s.exit.value != null }
        Thread.sleep(300)
        assertEquals(listOf("signal", "close"), pty.calls)
        assertEquals(ManagedSession.Exit(129), s.exit.value)
    }

    @Test
    fun endStopsWhenTheEofEndsTheSession() {
        pty.signalled = setOf(42)
        pty.signalledExitAfterMs = 0
        val s = session(endGraceMs = 2_000)
        s.end()
        eventually { s.exit.value != null }
        Thread.sleep(300)
        assertEquals(listOf("signal", "eof", "close"), pty.calls)
    }

    @Test
    fun inputAndResizeAfterExitAreIgnored() {
        val s = session()
        pty.exit(0)
        eventually { s.exit.value != null }
        s.write("ls\n".toByteArray())
        s.resize(30, 90)
        assertTrue(pty.written.isEmpty())
        assertTrue(pty.resizes.isEmpty())
    }

    @Test
    fun inputAndResizeReachThePtyWhileRunning() {
        val s = session()
        s.write("ls\n".toByteArray())
        s.resize(30, 90)
        assertEquals(listOf("ls\n"), pty.written)
        assertEquals(listOf(30 to 90), pty.resizes)
    }

    @Test
    fun attachReturnsTheScreenItReplaces() {
        val s = session()
        val first = ManagedSession.Screen {}
        val second = ManagedSession.Screen {}
        assertNull(s.attach(first))
        assertSame(first, s.attach(second))
        assertNull(s.attach(second))
        assertTrue(s.hasScreen)
    }

    @Test
    fun detachOnlyRemovesTheAttachedScreen() {
        val s = session()
        val first = ManagedSession.Screen {}
        val second = ManagedSession.Screen {}
        s.attach(first)
        s.attach(second)
        s.detach(first)
        assertTrue(s.hasScreen)
        s.detach(second)
        assertFalse(s.hasScreen)
    }

    @Test
    fun visibilityFollowsTheAttachedScreenOnly() {
        val s = session()
        val first = ManagedSession.Screen {}
        val second = ManagedSession.Screen {}
        assertFalse(s.screenVisible)
        s.attach(first)
        assertTrue(s.screenVisible)
        s.setVisible(first, false)
        assertFalse(s.screenVisible)
        s.attach(second)
        assertTrue(s.screenVisible)
        s.setVisible(first, false)
        assertTrue(s.screenVisible)
        s.detach(second)
        assertFalse(s.screenVisible)
    }

    @Test
    fun throwingOutputStillEndsTheSession() {
        val s = session(sink = { _, _, _ -> throw IllegalStateException("boom") })
        pty.emit("x")
        eventually { ended.isNotEmpty() }
        assertTrue(s.exit.value != null)
        assertTrue(pty.closed)
    }

    @Test
    fun endTwiceSendsEofOnce() {
        pty.ignoresEof = true
        val s = session(endGraceMs = 1_000)
        s.end()
        s.end()
        eventually { s.exit.value != null }
        assertEquals(1, pty.eofCount)
    }

    @Test
    fun noInputReachesThePtyOnceItIsClosed() {
        val s = session()
        pty.onClose = {
            s.write("late".toByteArray())
            s.resize(1, 1)
            s.end()
        }
        pty.exit(0)
        eventually { ended.isNotEmpty() }
        assertEquals(0, pty.writesAfterClose)
        assertEquals(0, pty.resizesAfterClose)
        assertEquals(0, pty.eofCount)
    }
}
