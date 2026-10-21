package id.or.oo.pr

import id.or.oo.pr.DistroOperations.Kind
import id.or.oo.pr.session.DistroCommand
import id.or.oo.pr.session.FakePty
import id.or.oo.pr.session.ManagedSession
import id.or.oo.pr.session.SessionManager
import id.or.oo.pr.session.eventually
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.asCoroutineDispatcher
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
import java.util.concurrent.Executor

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
        @Volatile var sawCancellation = false
        @Volatile var failureOnCancel: Exception? = null
        @Volatile var gatedCommand: String? = null
        @Volatile var linesFor = mapOf<String, List<String>>()
        @Volatile var uninterruptibleGate: CountDownLatch? = null

        override suspend fun run(args: List<String>, onLine: (String) -> Unit): Int {
            calls += args
            sessions?.let { runningAtCall += it.running.value }
            before.forEach(onLine)
            linesFor[args.first()].orEmpty().forEach(onLine)
            try {
                gate?.takeIf { gatedCommand == null || gatedCommand == args.first() }
                    ?.let { runInterruptible(Dispatchers.IO) { it.await() } }
            } catch (e: CancellationException) {
                sawCancellation = true
                throw failureOnCancel ?: e
            }
            uninterruptibleGate?.await()
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
    private val installed: MutableSet<String> = ConcurrentHashMap.newKeySet()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    private fun operations(endTimeoutMs: Long = ManagedSession.END_TIMEOUT_MS) =
        DistroOperations(command, sessions, scope, { it in installed }, endTimeoutMs)

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
        assertFalse(done.cancelled)
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
    fun cancelDuringARunEndsWithCancelledAndFreesTheRunner() {
        command.gate = CountDownLatch(1)
        command.before = listOf("Running suite distro")
        command.after = listOf("never shown")
        val ops = operations()
        ops.test("alpine", "alpine:latest")
        eventually { ops.operation.value?.lines?.size == 2 }

        ops.cancel()

        val done = ops.awaitFinished()
        assertEquals(listOf("Testing alpine:latest…", "Running suite distro", "Cancelled."), done.lines)
        assertNull(done.exitCode)
        assertTrue(done.cancelled)
        assertTrue(command.sawCancellation)
        command.gate = null
        assertTrue(ops.install("ubuntu", "ubuntu:latest", "docker.io/library/ubuntu:latest"))
        assertEquals("Done.", ops.awaitFinished().lines.last())
    }

    @Test
    fun aCommandThatFinishesAsItIsCancelledKeepsItsResult() {
        val gate = CountDownLatch(1)
        command.uninterruptibleGate = gate
        command.after = listOf("All tests passed")
        val ops = operations()
        ops.install("alpine", "alpine:latest", "docker.io/library/alpine:latest")
        eventually { command.calls.isNotEmpty() }

        ops.cancel()
        gate.countDown()

        val done = ops.awaitFinished()
        assertEquals(listOf("Installing alpine:latest…", "All tests passed", "Done."), done.lines)
        assertEquals(0, done.exitCode)
        assertFalse(done.cancelled)
        assertEquals(1, command.calls.size)
    }

    @Test
    fun aCommandFailingAsItIsCancelledStillEndsWithCancelled() {
        command.gate = CountDownLatch(1)
        command.failureOnCancel = IOException("Stream closed")
        val ops = operations()
        ops.test("alpine", "alpine:latest")
        eventually { command.calls.isNotEmpty() }

        ops.cancel()

        assertEquals(listOf("Testing alpine:latest…", "Cancelled."), ops.awaitFinished().lines)
    }

    @Test
    fun cancelDuringRemovesWaitNeverRunsRemove() {
        endGraceMs = 5_000
        sessions.open("alpine")
        ptys.getValue("alpine").ignoresEof = true
        val ops = operations()
        ops.remove("alpine", "alpine:latest")
        eventually { ops.operation.value?.lines?.size == 2 }

        val started = System.nanoTime()
        ops.cancel()

        val done = ops.awaitFinished()
        assertTrue((System.nanoTime() - started) / 1_000_000 < 3_000)
        assertEquals(
            listOf("Removing alpine:latest…", "Stopping the alpine:latest session…", "Cancelled."),
            done.lines,
        )
        assertTrue(command.calls.isEmpty())
        eventually { ptys.getValue("alpine").eofSent }
        Thread.sleep(200)
        assertTrue(command.calls.isEmpty())
    }

    @Test
    fun cancelWhenNothingRunsDoesNothing() {
        val ops = operations()
        ops.cancel()
        assertNull(ops.operation.value)

        ops.test("alpine", "alpine:latest")
        val done = ops.awaitFinished()
        ops.cancel()

        assertEquals(done, ops.operation.value)
        assertEquals(listOf("Testing alpine:latest…", "Done."), done.lines)
        assertTrue(ops.dismiss())
    }

    @Test
    fun aCancelBeforeTheWorkStartsStillFinishes() {
        val queued = CopyOnWriteArrayList<Runnable>()
        val held = CoroutineScope(SupervisorJob() + Executor { queued += it }.asCoroutineDispatcher())
        val ops = DistroOperations(command, sessions, held, { it in installed })
        ops.test("alpine", "alpine:latest")

        ops.cancel()
        queued.forEach { it.run() }

        val done = ops.awaitFinished()
        assertEquals(listOf("Testing alpine:latest…", "Cancelled."), done.lines)
        assertTrue(done.cancelled)
        assertTrue(command.calls.isEmpty())
        assertTrue(ops.test("alpine", "alpine:latest"))
        held.cancel()
    }

    private fun cancelInstall(
        ops: DistroOperations,
        start: DistroOperations.() -> Boolean,
        createdAlias: String?,
        gated: String = "install",
    ) =
        run {
            command.gate = CountDownLatch(1)
            command.gatedCommand = gated
            command.linesFor = mapOf("remove" to listOf("Wiping the rootfs"))
            assertTrue(ops.start())
            eventually { command.calls.isNotEmpty() }
            createdAlias?.let { installed += it }
            ops.cancel()
            ops.awaitFinished()
        }

    @Test
    fun aCancelledInstallOfANewContainerRemovesIt() {
        val ops = operations()

        val done = cancelInstall(ops, { install("alpine", "alpine:latest", "docker.io/library/alpine:latest") }, "alpine")

        assertEquals(
            listOf(
                listOf("install", "docker.io/library/alpine:latest", "--override-alias", "alpine"),
                listOf("remove", "alpine"),
            ),
            command.calls,
        )
        assertEquals(listOf("Installing alpine:latest…", "Wiping the rootfs", "Cancelled."), done.lines)
        assertNull(done.exitCode)
        assertTrue(done.cancelled)
    }

    @Test
    fun aCancelledCustomInstallWithAnAliasRemovesIt() {
        val ops = operations()

        cancelInstall(ops, { installCustom("docker.io/library/debian:stable", "deb") }, "deb")

        assertEquals(listOf("remove", "deb"), command.calls.last())
    }

    @Test
    fun aCancelledInstallOverAContainerThatAlreadyExistedRemovesNothing() {
        installed += "alpine"
        val ops = operations()

        val done = cancelInstall(ops, { install("alpine", "alpine:latest", "docker.io/library/alpine:latest") }, null)

        assertEquals(1, command.calls.size)
        assertEquals(listOf("Installing alpine:latest…", "Cancelled."), done.lines)
    }

    @Test
    fun aCancelledInstallThatCreatedNothingRemovesNothing() {
        val ops = operations()

        cancelInstall(ops, { install("alpine", "alpine:latest", "docker.io/library/alpine:latest") }, null)

        assertEquals(1, command.calls.size)
    }

    @Test
    fun aCancelledCustomInstallWithoutAnAliasRemovesNothing() {
        val ops = operations()

        val done = cancelInstall(ops, { installCustom("docker.io/library/debian:stable", "") }, "debian")

        assertEquals(listOf(listOf("install", "docker.io/library/debian:stable")), command.calls)
        assertEquals("Cancelled.", done.lines.last())
    }

    @Test
    fun aCancelledTestRemovesNothing() {
        val ops = operations()

        cancelInstall(ops, { test("alpine", "alpine:latest") }, "alpine", gated = "test")

        assertEquals(listOf(listOf("test", "alpine")), command.calls)
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
