package id.or.oo.pr

import id.or.oo.pr.session.SystemProcFs
import id.or.oo.pr.session.processState
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CopyOnWriteArrayList

class ProcessRunnerTest {

    private val lines = CopyOnWriteArrayList<String>()

    private fun alive(pid: Int) = SystemProcFs.stat(pid)?.let(::processState).let { it != null && it != 'Z' && it != 'X' }

    private fun sh(script: String) = listOf("/bin/sh", "-c", script)

    @Test
    fun linesStreamInOrderAndTheExitCodeIsReturned() = runBlocking {
        val code = withTimeout(10_000) {
            ProcessRunner().run(sh("echo one; echo two >&2; exit 3"), emptyMap()) { lines += it }
        }
        assertEquals(listOf("one", "two"), lines)
        assertEquals(3, code)
    }

    @Test
    fun theEnvironmentIsPassed() = runBlocking {
        withTimeout(10_000) {
            ProcessRunner().run(sh("echo \"\$PR_TEST_VALUE\""), mapOf("PR_TEST_VALUE" to "set")) { lines += it }
        }
        assertEquals(listOf("set"), lines)
    }

    @Test
    fun standardInputIsClosedSoAReaderOfItDoesNotBlock() = runBlocking {
        val code = withTimeout(10_000) {
            ProcessRunner().run(sh("cat; echo done"), emptyMap()) { lines += it }
        }
        assertEquals(listOf("done"), lines)
        assertEquals(0, code)
    }

    @Test
    fun aFailingReadStopsTheProcessBeforeRethrowing() = runBlocking {
        var pid = 0
        val failure = runCatching {
            withTimeout(10_000) {
                ProcessRunner(stopWaitMs = 500).run(sh("echo \$\$; exec sleep 30"), emptyMap()) {
                    pid = it.toInt()
                    throw IllegalStateException("boom")
                }
            }
        }.exceptionOrNull()
        assertEquals("boom", failure?.message)
        assertTrue(pid > 0)
        assertFalse(alive(pid))
    }
}
