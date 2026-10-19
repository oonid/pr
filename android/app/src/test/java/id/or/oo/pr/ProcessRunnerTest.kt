package id.or.oo.pr

import id.or.oo.pr.session.SystemProcFs
import id.or.oo.pr.session.processState
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.util.concurrent.CopyOnWriteArrayList

class ProcessRunnerTest {

    private val lines = CopyOnWriteArrayList<String>()

    private fun alive(pid: Int) = SystemProcFs.stat(pid)?.let(::processState).let { it != null && it != 'Z' && it != 'X' }

    private fun sh(script: String) = listOf("/bin/sh", "-c", script)

    private val myPid = File("/proc/self").canonicalFile.name.toInt()

    private fun runner(stopWaitMs: Long = ProcessRunner.STOP_WAIT_MS) =
        ProcessRunner(myPid, { pid, signal -> ProcessBuilder("kill", "-$signal", "$pid").start().waitFor() }, stopWaitMs = stopWaitMs)

    private fun cancelAfterTwoLines(runner: ProcessRunner, script: String): Pair<List<Int>, Long> = runBlocking {
        val job = launch(Dispatchers.Default) { runner.run(sh(script), emptyMap()) { lines += it } }
        withTimeout(10_000) { while (lines.size < 2) delay(10) }
        val started = System.nanoTime()
        withTimeout(10_000) { job.cancelAndJoin() }
        assertTrue(job.isCancelled)
        lines.map { it.toInt() } to (System.nanoTime() - started) / 1_000_000
    }

    @Test
    fun linesStreamInOrderAndTheExitCodeIsReturned() = runBlocking {
        val code = withTimeout(10_000) {
            runner().run(sh("echo one; echo two >&2; exit 3"), emptyMap()) { lines += it }
        }
        assertEquals(listOf("one", "two"), lines)
        assertEquals(3, code)
    }

    @Test
    fun theEnvironmentIsPassed() = runBlocking {
        withTimeout(10_000) {
            runner().run(sh("echo \"\$PR_TEST_VALUE\""), mapOf("PR_TEST_VALUE" to "set")) { lines += it }
        }
        assertEquals(listOf("set"), lines)
    }

    @Test
    fun standardInputIsClosedSoAReaderOfItDoesNotBlock() = runBlocking {
        val code = withTimeout(10_000) {
            runner().run(sh("cat; echo done"), emptyMap()) { lines += it }
        }
        assertEquals(listOf("done"), lines)
        assertEquals(0, code)
    }

    @Test
    fun aFailingReadStopsTheProcessBeforeRethrowing() = runBlocking {
        var pid = 0
        val failure = runCatching {
            withTimeout(10_000) {
                runner(stopWaitMs = 500).run(sh("echo \$\$; exec sleep 30"), emptyMap()) {
                    pid = it.toInt()
                    throw IllegalStateException("boom")
                }
            }
        }.exceptionOrNull()
        assertEquals("boom", failure?.message)
        assertTrue(pid > 0)
        assertFalse(alive(pid))
    }

    @Test
    fun cancellingStopsTheProcessAndItsDescendants() {
        val (pids, tookMs) = cancelAfterTwoLines(runner(), "sleep 30 & echo \$!; echo \$\$; wait")
        assertTrue(pids.none(::alive))
        assertTrue("took $tookMs ms", tookMs < ProcessRunner.STOP_WAIT_MS)
    }

    @Test
    fun aTreeThatIgnoresTermIsKilledAfterTheWait() {
        val (pids, tookMs) = cancelAfterTwoLines(runner(stopWaitMs = 300), "trap '' TERM; sleep 30 & echo \$!; echo \$\$; wait")
        assertTrue(pids.none(::alive))
        assertTrue("took $tookMs ms", tookMs >= 300)
    }
}
