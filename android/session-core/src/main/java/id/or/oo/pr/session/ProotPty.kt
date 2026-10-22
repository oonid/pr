package id.or.oo.pr.session

import android.os.Process
import id.or.oo.pr.engine.ProotLauncher

class ProotPty(
    private val session: ProotLauncher.Session,
    private val processes: ProotProcesses = ProotProcesses(),
    private val endPolicy: EndPolicy = EndPolicy.SPARE_LOGIN_SHELL,
) : Pty {

    override fun read(buf: ByteArray): Int = session.read(buf)

    override fun write(data: ByteArray): Int = session.write(data)

    override fun resize(rows: Int, cols: Int) {
        session.resize(rows, cols)
    }

    override fun signalTracees(): Set<Int> {
        if (session.exitCode != null || session.pid <= 0) return emptySet()
        val targets = processes.endTargets(session.pid, endPolicy)
        targets.forEach { pid -> END_SIGNALS.forEach { Process.sendSignal(pid, it) } }
        return targets
    }

    override fun alive(pids: Set<Int>): Boolean = processes.alive(pids)

    override fun sendEof() {
        session.write(byteArrayOf(0x04))
    }

    override fun kill() {
        if (session.exitCode != null || session.pid <= 0) return
        processes.shells(session.pid).forEach { Process.sendSignal(it, Process.SIGNAL_KILL) }
    }

    override fun forceKill() {
        if (session.exitCode != null || session.pid <= 0) return
        processes.forceKillTargets(session.pid).forEach { Process.sendSignal(it, Process.SIGNAL_KILL) }
    }

    override fun close() {
        session.close()
    }

    override val exitCode: Int?
        get() = session.exitCode
}

private val END_SIGNALS = listOf(15, 18)

internal fun parentPid(stat: String): Int? {
    val afterName = stat.substringAfterLast(')', missingDelimiterValue = "").trim()
    return afterName.split(' ').getOrNull(1)?.toIntOrNull()
}

internal fun processState(stat: String): Char? =
    stat.substringAfterLast(')', missingDelimiterValue = "").trim().split(' ').firstOrNull()?.singleOrNull()

internal fun tracerPid(status: String): Int? =
    status.lineSequence().firstOrNull { it.startsWith("TracerPid:") }?.substringAfter(':')?.trim()?.toIntOrNull()

internal fun traceesOf(tracer: Int, statuses: Map<Int, String?>, exclude: Set<Int>): Set<Int>? {
    val tracees = LinkedHashSet<Int>()
    for ((pid, status) in statuses) {
        val tracedBy = status?.let(::tracerPid) ?: return null
        if (tracedBy == tracer && pid !in exclude) tracees += pid
    }
    return tracees
}

internal fun forceKillTargets(proot: Int, statuses: Map<Int, String?>, walk: () -> List<Int>): List<Int> {
    val tracees = traceesOf(proot, statuses, exclude = setOf(proot)) ?: walk()
    return tracees.filter { it != proot }.distinct() + proot
}
