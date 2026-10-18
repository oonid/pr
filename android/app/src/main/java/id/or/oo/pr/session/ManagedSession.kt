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
    private val endGraceMs: Long = 2_000,
    private val exitCodeWaitMs: Long = 2_000,
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
        synchronized(ptyLock) { if (!closed) pty.sendEof() }
        thread(isDaemon = true, name = "pty-end-$alias") {
            if (ended.await(endGraceMs, TimeUnit.MILLISECONDS)) return@thread
            pty.kill()
            if (ended.await(endGraceMs, TimeUnit.MILLISECONDS)) return@thread
            pty.forceKill()
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

    private fun finish() {
        runCatching {
            val deadline = System.currentTimeMillis() + exitCodeWaitMs
            while (pty.exitCode == null && System.currentTimeMillis() < deadline) Thread.sleep(20)
        }
        synchronized(ptyLock) {
            closed = true
            runCatching { pty.close() }
        }
        _exit.value = Exit(pty.exitCode)
        ended.countDown()
        onEnded(this)
    }
}
