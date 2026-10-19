package id.or.oo.pr

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

class ProcessRunner(private val stopWaitMs: Long = STOP_WAIT_MS) {

    suspend fun run(cmd: List<String>, env: Map<String, String>, onLine: (String) -> Unit): Int =
        withContext(Dispatchers.IO) {
            val pb = ProcessBuilder(cmd)
            pb.environment().putAll(env)
            pb.redirectErrorStream(true)
            val proc = pb.start()
            proc.outputStream.close()

            val open = AtomicBoolean(true)
            val exit = CompletableDeferred<Int>()
            thread(isDaemon = true, name = "process-output") {
                runCatching {
                    proc.inputStream.bufferedReader().use { reader ->
                        var line = reader.readLine()
                        while (line != null) {
                            if (open.get()) onLine(line)
                            line = reader.readLine()
                        }
                    }
                    proc.waitFor()
                }.fold(exit::complete, exit::completeExceptionally)
            }

            try {
                exit.await()
            } catch (e: Throwable) {
                stop(proc)
                withContext(NonCancellable) {
                    if (withTimeoutOrNull(stopWaitMs) { exit.join() } == null) proc.inputStream.close()
                }
                open.set(false)
                throw e
            }
        }

    private fun stop(proc: Process) {
        proc.destroy()
        if (!proc.waitFor(stopWaitMs, TimeUnit.MILLISECONDS)) {
            proc.destroyForcibly()
            proc.waitFor(stopWaitMs, TimeUnit.MILLISECONDS)
        }
    }

    companion object {
        const val STOP_WAIT_MS = 2_000L
    }
}
