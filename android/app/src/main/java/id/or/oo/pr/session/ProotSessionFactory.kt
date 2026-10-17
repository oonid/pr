package id.or.oo.pr.session

import android.os.HandlerThread
import androidx.compose.ui.graphics.Color
import id.or.oo.pr.engine.ProotHost
import id.or.oo.pr.engine.ProotLauncher
import org.connectbot.terminal.TerminalEmulatorFactory

class ProotSessionFactory(private val host: ProotHost) : SessionManager.SessionFactory {

    override fun create(alias: String, onEnded: (ManagedSession) -> Unit): ManagedSession {
        val session = ProotLauncher(host).startSession(alias)
            ?: throw IllegalStateException("could not start a terminal for $alias")
        val pty = ProotPty(session)
        val emulatorThread = HandlerThread("emulator-$alias").apply { start() }
        try {
            lateinit var managed: ManagedSession
            val emulator = TerminalEmulatorFactory.create(
                looper = emulatorThread.looper,
                initialRows = 24,
                initialCols = 80,
                defaultForeground = Color.White,
                defaultBackground = Color(0xFF1a1a2e),
                onKeyboardInput = { data -> managed.write(data) },
                onResize = { dims -> managed.resize(dims.rows, dims.columns) },
            )
            managed = ManagedSession(
                alias = alias,
                pty = pty,
                emulator = emulator,
                output = { buf, off, len -> emulator.writeInput(buf, off, len) },
                onEnded = onEnded,
                release = { emulatorThread.quitSafely() },
            )
            return managed
        } catch (t: Throwable) {
            emulatorThread.quitSafely()
            pty.forceKill()
            session.close()
            throw t
        }
    }
}
