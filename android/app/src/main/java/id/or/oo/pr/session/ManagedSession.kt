package id.or.oo.pr.session

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import org.connectbot.terminal.TerminalEmulator
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

class ManagedSession(
    val alias: String,
    private val pty: Pty,
    val emulator: TerminalEmulator?,
    private val output: (ByteArray, Int, Int) -> Unit,
    private val onEnded: (ManagedSession) -> Unit,
    private val endGraceMs: Long = 2_000,
    private val exitCodeWaitMs: Long = 2_000,
    private val release: () -> Unit = {},
) {
    fun interface Screen {
        fun close()
    }

    data class Exit(val code: Int?)

    private val _exit = MutableStateFlow<Exit?>(null)
    val exit: StateFlow<Exit?> = _exit

    @Volatile
    var endRequested = false
        private set

    private val ended = CountDownLatch(1)
    private val screenLock = Any()
    private var screen: Screen? = null

    val hasScreen: Boolean
        get() = synchronized(screenLock) { screen != null }

    fun start() {
        thread(isDaemon = true, name = "pty-reader-$alias") { readLoop() }
    }

    fun write(data: ByteArray) {
        if (_exit.value == null) pty.write(data)
    }

    fun resize(rows: Int, cols: Int) {
        if (_exit.value == null) pty.resize(rows, cols)
    }

    fun attach(screen: Screen): Screen? = synchronized(screenLock) {
        val previous = this.screen
        this.screen = screen
        previous?.takeIf { it !== screen }
    }

    fun detach(screen: Screen) {
        synchronized(screenLock) {
            if (this.screen === screen) this.screen = null
        }
    }

    fun end() {
        if (_exit.value != null) return
        endRequested = true
        pty.sendEof()
        thread(isDaemon = true, name = "pty-end-$alias") {
            if (ended.await(endGraceMs, TimeUnit.MILLISECONDS)) return@thread
            pty.kill()
            if (ended.await(endGraceMs, TimeUnit.MILLISECONDS)) return@thread
            pty.forceKill()
        }
    }

    private fun readLoop() {
        val buf = ByteArray(8192)
        while (true) {
            val n = try {
                pty.read(buf)
            } catch (e: Exception) {
                -1
            }
            if (n < 0) break
            if (n > 0) output(buf, 0, n)
        }
        val deadline = System.currentTimeMillis() + exitCodeWaitMs
        while (pty.exitCode == null && System.currentTimeMillis() < deadline) Thread.sleep(20)
        pty.close()
        release()
        _exit.value = Exit(pty.exitCode)
        ended.countDown()
        onEnded(this)
    }
}
