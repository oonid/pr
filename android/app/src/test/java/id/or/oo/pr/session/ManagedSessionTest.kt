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

    private fun session(endGraceMs: Long = 100) = ManagedSession(
        alias = "alpine",
        pty = pty,
        emulator = null,
        output = { buf, off, len -> output.append(String(buf, off, len)) },
        onEnded = { ended += it },
        endGraceMs = endGraceMs,
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
    fun endSendsEofAndMarksTheEndRequested() {
        val s = session()
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
}
