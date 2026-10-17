package id.or.oo.pr.session

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

class SessionManager(
    private val factory: SessionFactory,
    private val foreground: ForegroundController,
    private val notifier: EndedNotifier,
) {
    fun interface SessionFactory {
        fun create(alias: String, onEnded: (ManagedSession) -> Unit): ManagedSession
    }

    interface ForegroundController {
        fun start()
        fun stop()
    }

    fun interface EndedNotifier {
        fun sessionEnded(alias: String, exitCode: Int?)
    }

    sealed interface OpenResult {
        data class Opened(val session: ManagedSession) : OpenResult
        data class Failed(val reason: String) : OpenResult
    }

    private val lock = Any()
    private val sessions = LinkedHashMap<String, ManagedSession>()

    private val _running = MutableStateFlow<List<String>>(emptyList())
    val running: StateFlow<List<String>> = _running

    val keepCpuAwake = MutableStateFlow(false)

    private val _backgroundError = MutableStateFlow<String?>(null)
    val backgroundError: StateFlow<String?> = _backgroundError

    fun open(alias: String): OpenResult {
        val session = synchronized(lock) {
            val existing = sessions[alias]
            if (existing != null && existing.exit.value == null) return OpenResult.Opened(existing)
            if (existing != null) sessions.remove(alias)
            val created = try {
                factory.create(alias, ::onEnded)
            } catch (e: Exception) {
                return OpenResult.Failed(e.message ?: e.javaClass.simpleName)
            }
            sessions[alias] = created
            updateRunning()
            created
        }
        session.start()
        return OpenResult.Opened(session)
    }

    fun attach(session: ManagedSession, screen: ManagedSession.Screen) {
        session.attach(screen)?.close()
    }

    fun detach(session: ManagedSession, screen: ManagedSession.Screen) {
        synchronized(lock) {
            session.detach(screen)
            if (session.exit.value != null && !session.hasScreen && sessions[session.alias] === session) {
                sessions.remove(session.alias)
                updateRunning()
            }
        }
    }

    fun end(alias: String) {
        synchronized(lock) { sessions[alias] }?.end()
    }

    fun endAll() {
        synchronized(lock) { sessions.values.toList() }.forEach { it.end() }
    }

    private fun onEnded(session: ManagedSession) {
        var notify = false
        synchronized(lock) {
            if (sessions[session.alias] !== session) return
            if (!session.hasScreen) {
                sessions.remove(session.alias)
                notify = !session.endRequested
            }
            updateRunning()
        }
        if (notify) notifier.sessionEnded(session.alias, session.exit.value?.code)
    }

    private fun updateRunning() {
        val before = _running.value.size
        val now = sessions.values.filter { it.exit.value == null }.map { it.alias }
        if (before == 0 && now.isNotEmpty()) {
            _running.value = now
            startForeground()
            return
        }
        if (before > 0 && now.isEmpty()) {
            keepCpuAwake.value = false
            _backgroundError.value = null
            stopForeground()
        }
        _running.value = now
    }

    private fun stopForeground() {
        try {
            foreground.stop()
        } catch (e: Exception) {
            _backgroundError.value = e.message ?: e.javaClass.simpleName
        }
    }

    private fun startForeground() {
        try {
            foreground.start()
            _backgroundError.value = null
        } catch (e: Exception) {
            _backgroundError.value = e.message ?: e.javaClass.simpleName
        }
    }
}
