package id.or.oo.pr.session

import android.util.Log
import id.or.oo.pr.engine.ProotHost
import id.or.oo.pr.engine.ProotLauncher
import java.io.File

class CommandSessionFactory(
    private val host: ProotHost,
    private val container: String,
    private val command: String,
    private val sink: (ByteArray, Int, Int) -> Unit,
    private val binds: List<Bind> = emptyList(),
    private val start: (List<String>) -> Pty? = { args ->
        ProotLauncher(host).startCustomSession(args)?.let(::commandPty)
    },
) : SessionManager.SessionFactory {

    init {
        require(command.isNotBlank()) { "command must not be blank" }
    }

    override fun create(alias: String, onEnded: (ManagedSession) -> Unit): ManagedSession {
        val prCli = File(host.prefixDir, "bin/pr-cli")
        check(prCli.exists()) { "could not start a command session for $alias: no pr-cli at $prCli" }
        binds.forEach { bind ->
            check(File(bind.hostPath).exists()) { "could not start a command session for $alias: no bind source ${bind.hostPath}" }
        }
        val pty = start(loginArgs(host.prefixDir, container, command, binds))
            ?: throw IllegalStateException("could not start a command session for $alias")
        return ManagedSession(alias = alias, pty = pty, emulator = null, output = isolated(alias), onEnded = onEnded)
    }

    private fun isolated(alias: String): (ByteArray, Int, Int) -> Unit {
        var logged = false
        return { buf, off, len ->
            try {
                sink(buf, off, len)
            } catch (e: Exception) {
                if (!logged) Log.w(TAG, "the output sink of $alias failed; its output is dropped while it fails", e)
                logged = true
            }
        }
    }

    internal companion object {
        val END_POLICY = EndPolicy.SIGNAL_EVERY_TRACEE
        private const val TAG = "PR"
    }
}

internal fun commandPty(session: ProotLauncher.Session, processes: ProotProcesses = ProotProcesses()): Pty =
    ProotPty(session, processes, CommandSessionFactory.END_POLICY)

internal fun loginArgs(prefixDir: File, container: String, command: String, binds: List<Bind>): List<String> =
    listOf(File(prefixDir, "bin/pr-cli").absolutePath, "login", container, "--user", "root") +
        binds.flatMap { listOf("--custom-bind", "${it.hostPath}:${it.guestPath}") } +
        listOf("--", command)
