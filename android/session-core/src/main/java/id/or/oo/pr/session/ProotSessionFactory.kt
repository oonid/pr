package id.or.oo.pr.session

import android.os.Looper
import androidx.compose.ui.graphics.Color
import id.or.oo.pr.engine.ProotHost
import id.or.oo.pr.engine.ProotLauncher
import org.connectbot.terminal.TerminalEmulatorFactory
import java.io.File

class ProotSessionFactory internal constructor(
    private val host: ProotHost,
    private val binds: List<Bind>,
    private val startSession: (List<String>) -> ProotLauncher.Session?,
) : SessionManager.SessionFactory {

    @JvmOverloads
    constructor(host: ProotHost, binds: List<Bind> = emptyList()) :
        this(host, binds, { args -> ProotLauncher(host).startCustomSession(args, rows = 24, cols = 80) })

    override fun create(alias: String, onEnded: (ManagedSession) -> Unit): ManagedSession {
        val session = startLogin(alias)
        val pty = ProotPty(session)
        try {
            lateinit var managed: ManagedSession
            val emulator = TerminalEmulatorFactory.create(
                looper = Looper.getMainLooper(),
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
            )
            return managed
        } catch (t: Throwable) {
            pty.forceKill()
            session.close()
            throw t
        }
    }

    internal fun startLogin(alias: String): ProotLauncher.Session {
        val prCli = File(host.prefixDir, "bin/pr-cli")
        check(prCli.exists()) { "could not start a terminal for $alias: no pr-cli at $prCli" }
        binds.forEach { bind ->
            check(File(bind.hostPath).exists()) { "could not start a terminal for $alias: no bind source ${bind.hostPath}" }
        }
        return startSession(terminalArgs(host.prefixDir, alias, binds))
            ?: throw IllegalStateException("could not start a terminal for $alias")
    }
}

internal fun terminalArgs(prefixDir: File, container: String, binds: List<Bind>): List<String> =
    listOf(File(prefixDir, "bin/pr-cli").absolutePath, "login", container, "--user", "root") +
        binds.flatMap { listOf("--custom-bind", "${it.hostPath}:${it.guestPath}") }
