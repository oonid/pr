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
    }

    fun interface EndedNotifier {
        fun sessionEnded(alias: String, exitCode: Int?)
    }

    sealed interface OpenResult {
        data class Opened(val session: ManagedSession) : OpenResult
        data class Ending(val session: ManagedSession) : OpenResult
        data class Failed(val reason: String) : OpenResult
    }

    private val lock = Any()
    private val sessions = LinkedHashMap<String, ManagedSession>()
    private val kinds = HashMap<String, Class<out SessionFactory>>()
    private val retained = HashSet<ManagedSession>()

    private val _running = MutableStateFlow<List<String>>(emptyList())
    val running: StateFlow<List<String>> = _running

    private val _keepCpuAwake = MutableStateFlow(false)
    val keepCpuAwake: StateFlow<Boolean> = _keepCpuAwake

    fun setKeepCpuAwake(on: Boolean) {
        synchronized(lock) {
            if (_running.value.isNotEmpty()) _keepCpuAwake.value = on
        }
    }

    private val _backgroundError = MutableStateFlow<String?>(null)
    val backgroundError: StateFlow<String?> = _backgroundError

    fun open(alias: String, factory: SessionFactory = this.factory): OpenResult {
        val session = synchronized(lock) {
            val existing = sessions[alias]
            if (existing != null) {
                val wasRetained = existing in retained
                if ((existing.exit.value == null || wasRetained) && kinds[alias] != factory.javaClass) {
                    return OpenResult.Failed("$alias is already open as another kind of session")
                }
                if (existing.exit.value == null && existing.endRequested) return OpenResult.Ending(existing)
                if (existing.exit.value == null || wasRetained) return OpenResult.Opened(existing)
                sessions.remove(alias)
            }
            val created = try {
                factory.create(alias, ::onEnded)
            } catch (e: Exception) {
                return OpenResult.Failed(e.message ?: e.javaClass.simpleName)
            }
            sessions[alias] = created
            kinds[alias] = factory.javaClass
            updateRunning()
            created
        }
        session.start()
        return OpenResult.Opened(session)
    }

    fun foregroundStarted() {
        synchronized(lock) { _backgroundError.value = null }
    }

    fun attach(session: ManagedSession, screen: ManagedSession.Screen) {
        val previous = synchronized(lock) {
            retained.remove(session)
            session.attach(screen)
        }
        previous?.close()
    }

    fun setVisible(session: ManagedSession, screen: ManagedSession.Screen, visible: Boolean) {
        session.setVisible(screen, visible)
    }

    fun detach(session: ManagedSession, screen: ManagedSession.Screen, retainEnded: Boolean = false) {
        synchronized(lock) {
            session.detach(screen)
            if (session.hasScreen || sessions[session.alias] !== session) return
            if (retainEnded) {
                retained += session
            } else if (session.exit.value != null) {
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
            val awaited = session in retained
            if (!session.hasScreen && !awaited) sessions.remove(session.alias)
            notify = !session.endRequested && !awaited && !session.screenVisible
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
            _keepCpuAwake.value = false
            _backgroundError.value = null
        }
        _running.value = now
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
