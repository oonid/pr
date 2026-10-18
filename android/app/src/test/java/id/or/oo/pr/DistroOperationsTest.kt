package id.or.oo.pr

import id.or.oo.pr.DistroOperations.Kind
import id.or.oo.pr.session.FakePty
import id.or.oo.pr.session.ManagedSession
import id.or.oo.pr.session.SessionManager
import id.or.oo.pr.session.eventually
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runInterruptible
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch

class DistroOperationsTest {

    private class FakeCommand : DistroCommand {
        val calls = CopyOnWriteArrayList<List<String>>()
        val runningAtCall = CopyOnWriteArrayList<List<String>>()
        @Volatile var sessions: SessionManager? = null
        @Volatile var before = listOf<String>()
        @Volatile var after = listOf<String>()
        @Volatile var exitCode = 0
        @Volatile var failure: Exception? = null
        @Volatile var gate: CountDownLatch? = null

        override suspend fun run(args: List<String>, onLine: (String) -> Unit): Int {
            calls += args
            sessions?.let { runningAtCall += it.running.value }
            before.forEach(onLine)
            gate?.let { runInterruptible(Dispatchers.IO) { it.await() } }
            after.forEach(onLine)
            failure?.let { throw it }
            return exitCode
        }
    }

    private val ptys = ConcurrentHashMap<String, FakePty>()
    @Volatile private var endGraceMs = 100L

    private val sessions = SessionManager(
        { alias, onEnded ->
            val pty = FakePty()
            ptys[alias] = pty
            ManagedSession(alias, pty, null, { _, _, _ -> }, onEnded, endGraceMs = endGraceMs, exitCodeWaitMs = 100)
        },
        object : SessionManager.ForegroundController {
            override fun start() {}
        },
    ) { _, _ -> }

    private val command = FakeCommand().also { it.sessions = sessions }
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    private fun operations(endTimeoutMs: Long = ManagedSession.END_TIMEOUT_MS) =
        DistroOperations(command, sessions, scope, endTimeoutMs)

    private fun DistroOperations.awaitFinished(): DistroOperations.Operation {
        eventually { operation.value?.running == false }
        return operation.value!!
    }

    @After
    fun tearDown() {
        command.gate?.countDown()
        ptys.values.forEach { it.exit(0) }
        scope.cancel()
    }

    @Test
    fun installStreamsTheOutputAndFinishesWithDone() {
        val gate = CountDownLatch(1)
        command.gate = gate
        command.before = listOf("Pulling alpine", "Extracting")
        command.after = listOf("Installed")
        val ops = operations()

        assertTrue(ops.install("alpine", "alpine:latest", "docker.io/library/alpine:latest"))

        eventually { ops.operation.value?.lines?.size == 3 }
        val running = ops.operation.value!!
        assertEquals(Kind.Install, running.kind)
        assertEquals("alpine", running.alias)
        assertEquals("alpine:latest", running.displayName)
        assertTrue(running.running)
        assertEquals(listOf("Installing alpine:latest…", "Pulling alpine", "Extracting"), running.lines)

        gate.countDown()
        val done = ops.awaitFinished()
        assertEquals(
            listOf("Installing alpine:latest…", "Pulling alpine", "Extracting", "Installed", "Done."),
            done.lines,
        )
        assertEquals(0, done.exitCode)
        assertEquals(
            listOf(listOf("install", "docker.io/library/alpine:latest", "--override-alias", "alpine")),
            command.calls,
        )
    }

    @Test
    fun aNonZeroExitEndsWithTheExitCode() {
        command.before = listOf("failed to pull")
        command.exitCode = 3
        val ops = operations()

        ops.test("alpine", "alpine:latest")

        val done = ops.awaitFinished()
        assertEquals(Kind.Test, done.kind)
        assertEquals(listOf("Testing alpine:latest…", "failed to pull", "Exit code: 3"), done.lines)
        assertEquals(3, done.exitCode)
        assertEquals(listOf(listOf("test", "alpine")), command.calls)
    }

    @Test
    fun aCommandThatCannotRunEndsWithTheError() {
        command.failure = IOException("Cannot run program \"pr-cli\"")
        val ops = operations()

        ops.test("alpine", "alpine:latest")

        val done = ops.awaitFinished()
        assertEquals(listOf("Testing alpine:latest…", "ERROR: Cannot run program \"pr-cli\""), done.lines)
        assertNull(done.exitCode)
    }

    @Test
    fun aSecondOperationIsRefusedWhileOneRuns() {
        command.gate = CountDownLatch(1)
        val ops = operations()

        assertTrue(ops.test("alpine", "alpine:latest"))
        assertFalse(ops.install("ubuntu", "ubuntu:latest", "docker.io/library/ubuntu:latest"))
        assertFalse(ops.remove("alpine", "alpine:latest"))
        assertFalse(ops.test("ubuntu", "ubuntu:latest"))
        assertFalse(ops.installCustom("docker.io/library/debian:stable", ""))
        assertFalse(ops.installCustom("docker.io/library/debian:stable", "/bad"))

        assertEquals(Kind.Test, ops.operation.value?.kind)
        assertEquals("alpine", ops.operation.value?.alias)
        command.gate!!.countDown()
        ops.awaitFinished()
        assertEquals(listOf(listOf("test", "alpine")), command.calls)
    }

    @Test
    fun dismissIsRefusedWhileRunningAndClearsAFinishedOperation() {
        command.gate = CountDownLatch(1)
        val ops = operations()
        ops.test("alpine", "alpine:latest")

        assertFalse(ops.dismiss())
        assertTrue(ops.operation.value!!.running)

        command.gate!!.countDown()
        ops.awaitFinished()
        assertTrue(ops.dismiss())
        assertNull(ops.operation.value)
    }

    @Test
    fun aNewOperationCanStartOnceTheLastFinished() {
        val ops = operations()
        ops.test("alpine", "alpine:latest")
        ops.awaitFinished()

        assertTrue(ops.remove("alpine", "alpine:latest"))

        val done = ops.awaitFinished()
        assertEquals(Kind.Remove, done.kind)
        assertEquals(listOf(listOf("test", "alpine"), listOf("remove", "alpine")), command.calls)
    }

    @Test
    fun removeWithoutASessionRunsRemove() {
        command.before = listOf("Removed alpine")
        val ops = operations()

        assertTrue(ops.remove("alpine", "alpine:latest"))

        val done = ops.awaitFinished()
        assertEquals(Kind.Remove, done.kind)
        assertEquals(listOf("Removing alpine:latest…", "Removed alpine", "Done."), done.lines)
        assertEquals(listOf(listOf("remove", "alpine")), command.calls)
    }

    @Test
    fun removeEndsARunningSessionBeforeRemoving() {
        sessions.open("alpine")
        sessions.open("ubuntu")
        val ops = operations()

        assertTrue(ops.remove("alpine", "alpine:latest"))

        val done = ops.awaitFinished()
        assertEquals(
            listOf("Removing alpine:latest…", "Stopping the alpine:latest session…", "Done."),
            done.lines,
        )
        assertEquals(listOf(listOf("remove", "alpine")), command.calls)
        assertEquals(listOf(listOf("ubuntu")), command.runningAtCall)
        assertTrue(ptys.getValue("alpine").eofSent)
        assertFalse(ptys.getValue("ubuntu").eofSent)
    }

    @Test
    fun removeIsCancelledWhenTheSessionDoesNotStopInTime() {
        endGraceMs = 5_000
        sessions.open("alpine")
        ptys.getValue("alpine").ignoresEof = true
        val ops = operations(endTimeoutMs = 200)

        val started = System.nanoTime()
        assertTrue(ops.remove("alpine", "alpine:latest"))

        val done = ops.awaitFinished()
        assertTrue((System.nanoTime() - started) / 1_000_000 < 3_000)
        assertEquals(
            listOf(
                "Removing alpine:latest…",
                "Stopping the alpine:latest session…",
                "Couldn't stop the alpine:latest session; removal cancelled.",
            ),
            done.lines,
        )
        assertTrue(command.calls.isEmpty())
        assertEquals(listOf("alpine"), sessions.running.value)
        assertTrue(ops.test("alpine", "alpine:latest"))
    }

    @Test
    fun customInstallWithAnAliasOverridesIt() {
        val ops = operations()

        assertTrue(ops.installCustom("  docker.io/library/debian:stable ", " debian-custom "))

        val done = ops.awaitFinished()
        assertEquals(Kind.Install, done.kind)
        assertEquals("docker.io/library/debian:stable", done.displayName)
        assertTrue(done.custom)
        assertEquals(listOf("Installing docker.io/library/debian:stable…", "Done."), done.lines)
        assertEquals(
            listOf(listOf("install", "docker.io/library/debian:stable", "--override-alias", "debian-custom")),
            command.calls,
        )
    }

    @Test
    fun customInstallWithoutAnAliasPassesOnlyTheSource() {
        val ops = operations()

        assertTrue(ops.installCustom("docker.io/library/debian:stable", "  "))

        ops.awaitFinished()
        assertEquals(listOf(listOf("install", "docker.io/library/debian:stable")), command.calls)
    }

    @Test
    fun customInstallWithABlankSourceIsIgnored() {
        val ops = operations()

        assertFalse(ops.installCustom("   ", "debian-custom"))

        assertNull(ops.operation.value)
        assertTrue(command.calls.isEmpty())
    }

    @Test
    fun anInvalidCustomAliasShowsTheErrorWithoutRunning() {
        val ops = operations()

        assertTrue(ops.installCustom("docker.io/library/debian:stable", "../etc"))

        val done = ops.operation.value!!
        assertFalse(done.running)
        assertTrue(done.custom)
        assertEquals(
            listOf(
                "ERROR: Invalid alias '../etc'.",
                "Alias must start with an alphanumeric character and use only [A-Za-z0-9_.+-].",
            ),
            done.lines,
        )
        assertTrue(command.calls.isEmpty())
    }

    @Test
    fun catalogInstallsAreNotCustom() {
        val ops = operations()
        ops.install("alpine", "alpine:latest", "docker.io/library/alpine:latest")
        assertFalse(ops.awaitFinished().custom)
    }

    @Test
    fun overrideAliasRulesAreUnchanged() {
        listOf("debian-custom", "a", "Alpine_3.20+x", "9lives").forEach {
            assertTrue(it, DistroOperations.isValidOverrideAlias(it))
        }
        listOf("", "-x", ".x", "a/b", "a\\b", "a..b", "setup.sh", "a b", "a:b").forEach {
            assertFalse(it, DistroOperations.isValidOverrideAlias(it))
        }
    }
}
