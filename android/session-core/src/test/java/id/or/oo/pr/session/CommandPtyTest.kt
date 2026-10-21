package id.or.oo.pr.session

import id.or.oo.pr.engine.ProotLauncher
import org.junit.Assert.assertEquals
import org.junit.Test

class CommandPtyTest {

    private val proot = 1790
    private val server = 1804

    private fun processes() = ProotProcesses(
        FakeProcFs(childrenFiles = false).apply {
            add(1700, ppid = 1)
            add(proot, ppid = 1700)
            add(server, ppid = proot, tracer = proot)
            add(1810, ppid = server, tracer = proot)
        },
    )

    @Test
    fun aCommandPtySignalsTheExecdServerProotRunsDirectly() {
        val pty = commandPty(ProotLauncher.Session(-1, proot), processes())
        assertEquals(setOf(server, 1810), pty.signalTracees())
    }

    @Test
    fun aTerminalPtySparesTheLoginShellInTheSameProcessTree() {
        val pty = ProotPty(ProotLauncher.Session(-1, proot), processes())
        assertEquals(setOf(1810), pty.signalTracees())
    }
}
