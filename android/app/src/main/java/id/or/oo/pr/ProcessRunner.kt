package id.or.oo.pr

import id.or.oo.pr.session.ProotProcesses
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import kotlin.concurrent.thread

class ProcessRunner(
    private val parentPid: Int,
    private val signal: (pid: Int, signal: Int) -> Unit,
    private val processes: ProotProcesses = ProotProcesses(),
    private val stopWaitMs: Long = STOP_WAIT_MS,
) {

    suspend fun run(cmd: List<String>, env: Map<String, String>, onLine: (String) -> Unit): Int {
        val open = AtomicBoolean(true)
        val started = CompletableDeferred<Process?>()
        val code = AtomicReference<Int>()
        val exit = CompletableDeferred<Int>()
        thread(isDaemon = true, name = "process-output") {
            runCatching {
                val pb = ProcessBuilder(cmd)
                pb.environment().putAll(env)
                pb.redirectErrorStream(true)
                val proc = pb.start()
                started.complete(proc)
                proc.outputStream.close()
                proc.inputStream.bufferedReader().use { reader ->
                    var line = reader.readLine()
                    while (line != null) {
                        if (open.get()) onLine(line)
                        line = reader.readLine()
                    }
                }
                proc.waitFor().also(code::set)
            }.fold(exit::complete) {
                started.complete(null)
                exit.completeExceptionally(it)
            }
        }

        try {
            return exit.await()
        } catch (e: Throwable) {
            if (e is CancellationException) code.get()?.let { return it }
            withContext(NonCancellable) {
                started.await()?.let { proc ->
                    val stopped = CompletableDeferred<Unit>()
                    thread(isDaemon = true, name = "process-stop") {
                        try {
                            stop(proc, cmd)
                        } finally {
                            stopped.complete(Unit)
                        }
                    }
                    stopped.await()
                    if (withTimeoutOrNull(stopWaitMs) { exit.join() } == null) proc.inputStream.close()
                }
            }
            open.set(false)
            throw e
        }
    }

    private fun stop(proc: Process, cmd: List<String>) {
        val roots = processes.children(parentPid, cmd)
        val tree = processes.descendants(roots)
        tree.forEach { signal(it, SIGTERM) }
        proc.destroy()
        if (exited(proc, tree)) return
        (tree + processes.descendants(roots)).distinct()
            .filter { processes.alive(setOf(it)) }
            .forEach { signal(it, SIGKILL) }
        proc.destroyForcibly()
        proc.waitFor(stopWaitMs, TimeUnit.MILLISECONDS)
    }

    private fun exited(proc: Process, tree: List<Int>): Boolean {
        val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(stopWaitMs)
        while (proc.isAlive || processes.alive(tree.toSet())) {
            if (deadline - System.nanoTime() <= 0) return false
            Thread.sleep(POLL_MS)
        }
        return true
    }

    companion object {
        const val STOP_WAIT_MS = 2_000L
        private const val POLL_MS = 20L
        private const val SIGTERM = 15
        private const val SIGKILL = 9
    }
}
