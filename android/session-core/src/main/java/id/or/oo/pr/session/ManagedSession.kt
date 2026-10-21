package id.or.oo.pr.session

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import org.connectbot.terminal.TerminalEmulator
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

class ManagedSession(
    val alias: String,
    private val pty: Pty,
    val emulator: TerminalEmulator?,
    private val output: (ByteArray, Int, Int) -> Unit,
    private val onEnded: (ManagedSession) -> Unit,
    private val endGraceMs: Long = END_GRACE_MS,
    private val signalWaitMs: Long = SIGNAL_WAIT_MS,
    private val exitCodeWaitMs: Long = EXIT_CODE_WAIT_MS,
    private val killExitWaitMs: Long = KILL_EXIT_WAIT_MS,
) {
    fun interface Screen {
        fun close()
    }

    data class Exit(val code: Int?)

    private val _exit = MutableStateFlow<Exit?>(null)
    val exit: StateFlow<Exit?> = _exit

    private val endFlag = AtomicBoolean(false)
    val endRequested: Boolean
        get() = endFlag.get()

    private val ptyLock = Any()
    private var closed = false
    private val ended = CountDownLatch(1)
    private val screenLock = Any()
    private var screen: Screen? = null
    private var visible = false

    val hasScreen: Boolean
        get() = synchronized(screenLock) { screen != null }

    val screenVisible: Boolean
        get() = synchronized(screenLock) { screen != null && visible }

    fun setVisible(screen: Screen, visible: Boolean) {
        synchronized(screenLock) {
            if (this.screen === screen) this.visible = visible
        }
    }

    fun start() {
        thread(isDaemon = true, name = "pty-reader-$alias") { readLoop() }
    }

    fun write(data: ByteArray) {
        synchronized(ptyLock) { if (!closed) pty.write(data) }
    }

    fun resize(rows: Int, cols: Int) {
        synchronized(ptyLock) { if (!closed) pty.resize(rows, cols) }
    }

    fun attach(screen: Screen): Screen? = synchronized(screenLock) {
        val previous = this.screen
        this.screen = screen
        visible = true
        previous?.takeIf { it !== screen }
    }

    fun detach(screen: Screen) {
        synchronized(screenLock) {
            if (this.screen === screen) this.screen = null
        }
    }

    fun end() {
        if (_exit.value != null) return
        if (!endFlag.compareAndSet(false, true)) return
        thread(isDaemon = true, name = "pty-end-$alias") {
            val signalled = pty.signalTracees()
            if (signalled.isNotEmpty() && awaitEndOrExit(signalled)) return@thread
            thread(isDaemon = true, name = "pty-eof-$alias") { synchronized(ptyLock) { if (!closed) pty.sendEof() } }
            if (ended.await(endGraceMs, TimeUnit.MILLISECONDS)) return@thread
            pty.kill()
            if (ended.await(endGraceMs, TimeUnit.MILLISECONDS)) return@thread
            pty.forceKill()
        }
    }

    private fun awaitEndOrExit(pids: Set<Int>): Boolean {
        val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(signalWaitMs)
        while (true) {
            val left = deadline - System.nanoTime()
            if (left <= 0) return false
            if (ended.await(minOf(left, TimeUnit.MILLISECONDS.toNanos(POLL_MS)), TimeUnit.NANOSECONDS)) return true
            if (!pty.alive(pids)) return ended.count == 0L
        }
    }

    private fun readLoop() {
        val buf = ByteArray(8192)
        try {
            while (true) {
                val n = pty.read(buf)
                if (n < 0) break
                if (n > 0) output(buf, 0, n)
            }
        } catch (e: Exception) {
        } finally {
            finish()
        }
    }

    private fun awaitExitCode(ms: Long): Boolean {
        val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(ms)
        while (pty.exitCode == null && deadline - System.nanoTime() > 0) Thread.sleep(20)
        return pty.exitCode != null
    }

    private fun finish() {
        runCatching {
            if (!awaitExitCode(exitCodeWaitMs)) {
                pty.kill()
                if (!awaitExitCode(killExitWaitMs)) pty.forceKill()
            }
        }
        synchronized(ptyLock) {
            closed = true
            runCatching { pty.close() }
        }
        _exit.value = Exit(pty.exitCode)
        ended.countDown()
        onEnded(this)
    }

    companion object {
        const val END_GRACE_MS = 2_000L
        const val SIGNAL_WAIT_MS = 10_000L
        const val EXIT_CODE_WAIT_MS = 2_000L
        const val KILL_EXIT_WAIT_MS = 500L
        private const val POLL_MS = 50L
        internal const val SCAN_MARGIN_MS = 1_000L
        private const val TIMEOUT_HEADROOM_MS = 2_500L

        fun endWorstCaseMs(
            signalWaitMs: Long = SIGNAL_WAIT_MS,
            endGraceMs: Long = END_GRACE_MS,
            exitCodeWaitMs: Long = EXIT_CODE_WAIT_MS,
            killExitWaitMs: Long = KILL_EXIT_WAIT_MS,
        ): Long = signalWaitMs + endGraceMs + endGraceMs + exitCodeWaitMs + killExitWaitMs + SCAN_MARGIN_MS

        val END_TIMEOUT_MS: Long = endWorstCaseMs() + TIMEOUT_HEADROOM_MS
    }
}
