package id.or.oo.pr

import id.or.oo.pr.session.ManagedSession
import id.or.oo.pr.session.SessionManager
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

fun interface DistroCommand {
    suspend fun run(args: List<String>, onLine: (String) -> Unit): Int
}

class DistroOperations(
    private val command: DistroCommand,
    private val sessions: SessionManager,
    private val scope: CoroutineScope,
    private val endTimeoutMs: Long = ManagedSession.END_TIMEOUT_MS,
) {
    enum class Kind(val label: String) { Install("Installing"), Remove("Removing"), Test("Testing") }

    data class Operation(
        val kind: Kind,
        val alias: String,
        val displayName: String,
        val lines: List<String>,
        val running: Boolean,
        val custom: Boolean = false,
        val exitCode: Int? = null,
    )

    private val lock = Any()
    private val _operation = MutableStateFlow<Operation?>(null)
    val operation: StateFlow<Operation?> = _operation

    fun install(alias: String, displayName: String, source: String): Boolean =
        start(Operation(Kind.Install, alias, displayName, emptyList(), true)) {
            execute(listOf("install", source, "--override-alias", alias))
        }

    fun installCustom(source: String, alias: String): Boolean {
        val ref = source.trim()
        val override = alias.trim()
        if (ref.isEmpty()) return false
        if (override.isNotEmpty() && !isValidOverrideAlias(override)) {
            val lines = listOf(
                "ERROR: Invalid alias '$override'.",
                "Alias must start with an alphanumeric character and use only [A-Za-z0-9_.+-].",
            )
            return post(Operation(Kind.Install, override, ref, lines, running = false, custom = true))
        }
        val args = listOf("install", ref) + if (override.isEmpty()) emptyList() else listOf("--override-alias", override)
        return start(Operation(Kind.Install, override, ref, emptyList(), true, custom = true)) { execute(args) }
    }

    fun remove(alias: String, displayName: String): Boolean =
        start(Operation(Kind.Remove, alias, displayName, emptyList(), true)) {
            if (alias in sessions.running.value) {
                append("Stopping the $displayName session…")
                sessions.end(alias)
                val stopped = withTimeoutOrNull(endTimeoutMs) { sessions.running.first { alias !in it } }
                if (stopped == null) {
                    append("Couldn't stop the $displayName session; removal cancelled.")
                    return@start
                }
            }
            execute(listOf("remove", alias))
        }

    fun test(alias: String, displayName: String): Boolean =
        start(Operation(Kind.Test, alias, displayName, emptyList(), true)) {
            execute(listOf("test", alias))
        }

    fun dismiss(): Boolean = synchronized(lock) {
        if (_operation.value?.running == true) return false
        _operation.value = null
        true
    }

    private fun post(operation: Operation): Boolean = synchronized(lock) {
        if (_operation.value?.running == true) return false
        _operation.value = operation
        true
    }

    private fun start(operation: Operation, work: suspend () -> Unit): Boolean {
        val first = "${operation.kind.label} ${operation.displayName}…"
        if (!post(operation.copy(lines = listOf(first)))) return false
        scope.launch {
            try {
                work()
            } finally {
                _operation.update { it?.copy(running = false) }
            }
        }
        return true
    }

    private suspend fun execute(args: List<String>) {
        try {
            val code = command.run(args, ::append)
            _operation.update { it?.copy(exitCode = code) }
            append(if (code != 0) "Exit code: $code" else "Done.")
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            append("ERROR: ${e.message}")
        }
    }

    private fun append(line: String) {
        _operation.update { it?.copy(lines = it.lines + line) }
    }

    companion object {
        fun isValidOverrideAlias(alias: String): Boolean {
            if (alias.isEmpty() || alias.endsWith(".sh")) return false
            if (alias.contains('/') || alias.contains('\\') || alias.contains("..")) return false
            if (!alias.first().isLetterOrDigit()) return false
            return alias.all { it.isLetterOrDigit() || it == '_' || it == '.' || it == '+' || it == '-' }
        }
    }
}
