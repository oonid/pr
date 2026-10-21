package id.or.oo.pr.session

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ProotProcessesTest {

    private val app = 1700
    private val proot = 1790
    private val shell = 1804

    private fun session(childrenFiles: Boolean = false) = FakeProcFs(childrenFiles).apply {
        add(app, ppid = 1)
        add(proot, ppid = app)
        add(shell, ppid = proot, tracer = proot)
        add(1810, ppid = shell, tracer = proot)
        add(1811, ppid = 1810, tracer = proot)
    }

    @Test
    fun endTargetsAreProotsTraceesExceptTheLoginShellAndProot() {
        assertEquals(setOf(1810, 1811), ProotProcesses(session()).endTargets(proot))
    }

    @Test
    fun endTargetsFindTheLoginShellThroughChildrenFilesWhenTheKernelHasThem() {
        assertEquals(setOf(1810, 1811), ProotProcesses(session(childrenFiles = true)).endTargets(proot))
    }

    @Test
    fun anotherProotsTraceesAreUntouched() {
        val proc = session().apply {
            add(2790, ppid = app)
            add(2804, ppid = 2790, tracer = 2790)
            add(2810, ppid = 2804, tracer = 2790)
        }
        assertEquals(setOf(1810, 1811), ProotProcesses(proc).endTargets(proot))
    }

    @Test
    fun aDaemonReparentedToInitIsStillFoundByItsTracer() {
        val proc = session().apply { add(1900, ppid = 1, tracer = proot) }
        assertEquals(setOf(1810, 1811, 1900), ProotProcesses(proc).endTargets(proot))
    }

    @Test
    fun endTargetsFallBackToTheShellsDescendantsWhenAStatusCannotBeRead() {
        val proc = session().apply {
            add(1820, ppid = shell, tracer = proot, statusReadable = false)
            add(1900, ppid = 1, tracer = proot)
            add(2790, ppid = app)
            add(2804, ppid = 2790, tracer = 2790)
        }
        assertEquals(setOf(1810, 1811, 1820), ProotProcesses(proc).endTargets(proot))
    }

    @Test
    fun aStatusUnreadableWhileTheProcessStillExistsForcesTheWalk() {
        val proc = session().apply {
            add(1820, ppid = shell, tracer = proot, statusReadable = false, statReadable = false)
            add(1900, ppid = 1, tracer = proot)
        }
        assertEquals(setOf(1810, 1811), ProotProcesses(proc).endTargets(proot))
    }

    @Test
    fun theFallbackWalkReadsTheChildrenFilesOfEveryTask() {
        val proc = session(childrenFiles = true).apply {
            add(1810, ppid = shell, tracer = proot, threads = listOf(1812), statusReadable = false)
            add(1830, ppid = 1810, tracer = proot, forkedBy = 1812)
        }
        assertEquals(setOf(1810, 1811, 1830), ProotProcesses(proc).endTargets(proot))
    }

    @Test
    fun aPidVanishingBetweenTheStatusListingAndItsReadIsSkippedWithoutFallingBack() {
        val proc = session().apply {
            add(1820, ppid = shell, tracer = proot)
            add(1900, ppid = 1, tracer = proot)
            vanishAfterListing(1820, listing = 2)
        }
        assertEquals(setOf(1810, 1811, 1900), ProotProcesses(proc).endTargets(proot))
        assertEquals(listOf("status 1820", "exists 1820"), proc.readsAfterVanishing)
    }

    @Test
    fun aPidVanishingDuringTheFallbackWalkIsSkipped() {
        val proc = session().apply {
            add(1820, ppid = shell, tracer = proot, statusReadable = false)
            add(1821, ppid = shell, tracer = proot)
            vanishAfterListing(1821, listing = 3)
        }
        assertEquals(setOf(1810, 1811, 1820), ProotProcesses(proc).endTargets(proot))
        assertEquals(listOf("stat 1821"), proc.readsAfterVanishing)
    }

    @Test
    fun theShellsAreProotsChildren() {
        assertEquals(listOf(shell), ProotProcesses(session()).shells(proot))
        assertEquals(listOf(shell), ProotProcesses(session(childrenFiles = true)).shells(proot))
    }

    @Test
    fun zombieDeadAndMissingProcessesCountAsExited() {
        val proc = session().apply {
            add(1900, ppid = shell, state = 'Z')
            add(1901, ppid = shell, state = 'X')
        }
        val processes = ProotProcesses(proc)
        assertFalse(processes.alive(setOf(1900, 1901, 4242)))
        assertTrue(processes.alive(setOf(1900, 1810)))
    }

    @Test
    fun aChildIsFoundByItsExactCommandLine() {
        val prCli = "/data/user/0/id.or.oo.pr/files/usr/bin/pr-cli"
        val test = listOf(prCli, "test", "alpine")
        listOf(false, true).forEach { childrenFiles ->
            val proc = session(childrenFiles).apply {
                add(2000, ppid = app, cmdline = test)
                add(2001, ppid = app, cmdline = listOf(prCli, "login", "alpine", "--user", "root"))
                add(2002, ppid = app, cmdline = listOf(prCli, "test", "alpine", "extra"))
                add(2003, ppid = app, state = 'Z', cmdline = test)
                add(2004, ppid = 2000, cmdline = test)
            }
            assertEquals(listOf(2000), ProotProcesses(proc).children(app, test))
        }
    }

    @Test
    fun descendantsAreEveryProcessBelowTheRoots() {
        listOf(false, true).forEach { childrenFiles ->
            val proc = session(childrenFiles).apply {
                add(2000, ppid = app)
                add(2010, ppid = 2000)
                add(2011, ppid = 2010)
                add(2012, ppid = 2000)
            }
            assertEquals(setOf(2010, 2011, 2012), ProotProcesses(proc).descendants(listOf(2000)).toSet())
            assertEquals(emptyList<Int>(), ProotProcesses(proc).descendants(emptyList()))
        }
    }

    @Test
    fun forceKillTargetsEveryTraceeIncludingTheLoginShellEndingWithProot() {
        val proc = session().apply {
            add(1900, ppid = 1, tracer = proot)
            add(2790, ppid = app)
            add(2804, ppid = 2790, tracer = 2790)
        }
        assertEquals(listOf(shell, 1810, 1811, 1900, proot), ProotProcesses(proc).forceKillTargets(proot))
    }

    @Test
    fun forceKillFallsBackToProotsDescendantsEndingWithProot() {
        val proc = session().apply {
            add(1820, ppid = shell, tracer = proot, statusReadable = false)
            add(2790, ppid = app)
            add(2804, ppid = 2790, tracer = 2790)
        }
        assertEquals(listOf(shell, 1810, 1820, 1811, proot), ProotProcesses(proc).forceKillTargets(proot))
    }
}
