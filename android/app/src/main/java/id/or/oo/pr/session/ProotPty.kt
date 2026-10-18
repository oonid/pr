package id.or.oo.pr.session

import android.os.Process
import id.or.oo.pr.engine.ProotLauncher
import java.io.File

class ProotPty(private val session: ProotLauncher.Session) : Pty {

    override fun read(buf: ByteArray): Int = session.read(buf)

    override fun write(data: ByteArray): Int = session.write(data)

    override fun resize(rows: Int, cols: Int) {
        session.resize(rows, cols)
    }

    override fun signalTracees(): Set<Int> {
        if (session.exitCode != null || session.pid <= 0) return emptySet()
        val shells = childrenOf(session.pid)
        val targets = traceesOf(session.pid, statuses(), exclude = shells.toSet() + session.pid)
            ?: descendantsOf(shells).toSet()
        targets.forEach { pid -> END_SIGNALS.forEach { Process.sendSignal(pid, it) } }
        return targets
    }

    override fun alive(pids: Set<Int>): Boolean = pids.any { pid ->
        val state = runCatching { File("/proc/$pid/stat").readText() }.getOrNull()?.let(::processState)
        state != null && state != 'Z' && state != 'X'
    }

    override fun sendEof() {
        session.write(byteArrayOf(0x04))
    }

    override fun kill() {
        if (session.exitCode != null || session.pid <= 0) return
        childrenOf(session.pid).forEach { Process.sendSignal(it, Process.SIGNAL_KILL) }
    }

    override fun forceKill() {
        if (session.exitCode != null || session.pid <= 0) return
        forceKillTargets(session.pid, statuses()) { descendantsOf(listOf(session.pid)) }
            .forEach { Process.sendSignal(it, Process.SIGNAL_KILL) }
    }

    override fun close() {
        session.close()
    }

    override val exitCode: Int?
        get() = session.exitCode

    private fun childrenOf(pid: Int): List<Int> {
        val listed = listedChildren(pid)
        if (!listed.isNullOrEmpty()) return listed
        return childrenByParent()[pid].orEmpty()
    }

    private fun descendantsOf(roots: List<Int>): List<Int> {
        if (roots.isEmpty()) return emptyList()
        val byParent = if (listedChildren(roots.first()) == null) childrenByParent() else null
        fun children(pid: Int) = if (byParent != null) byParent[pid].orEmpty() else listedChildren(pid).orEmpty()
        val found = LinkedHashSet<Int>()
        val pending = ArrayDeque(roots.flatMap { children(it) })
        while (pending.isNotEmpty()) {
            val pid = pending.removeFirst()
            if (pid !in roots && found.add(pid)) pending.addAll(children(pid))
        }
        return found.toList()
    }

    private fun listedChildren(pid: Int): List<Int>? {
        val tasks = File("/proc/$pid/task").listFiles() ?: return null
        val lists = tasks.mapNotNull { task -> runCatching { File(task, "children").readText() }.getOrNull() }
        if (lists.isEmpty()) return null
        return lists.flatMap { text -> text.trim().split(' ').mapNotNull { it.toIntOrNull() } }
    }

    private fun procPids(): List<Int> = File("/proc").listFiles().orEmpty().mapNotNull { it.name.toIntOrNull() }

    private fun statuses(): Map<Int, String?> = buildMap {
        for (pid in procPids()) {
            val status = runCatching { File("/proc/$pid/status").readText() }.getOrNull()
            if (status != null || File("/proc/$pid").exists()) put(pid, status)
        }
    }

    private fun childrenByParent(): Map<Int, List<Int>> =
        procPids()
            .mapNotNull { child ->
                runCatching { parentPid(File("/proc/$child/stat").readText()) }.getOrNull()?.let { it to child }
            }
            .groupBy({ it.first }, { it.second })

}

private val END_SIGNALS = listOf(15, 18)

fun parentPid(stat: String): Int? {
    val afterName = stat.substringAfterLast(')', missingDelimiterValue = "").trim()
    return afterName.split(' ').getOrNull(1)?.toIntOrNull()
}

fun processState(stat: String): Char? =
    stat.substringAfterLast(')', missingDelimiterValue = "").trim().split(' ').firstOrNull()?.singleOrNull()

fun tracerPid(status: String): Int? =
    status.lineSequence().firstOrNull { it.startsWith("TracerPid:") }?.substringAfter(':')?.trim()?.toIntOrNull()

fun traceesOf(tracer: Int, statuses: Map<Int, String?>, exclude: Set<Int>): Set<Int>? {
    val tracees = LinkedHashSet<Int>()
    for ((pid, status) in statuses) {
        val tracedBy = status?.let(::tracerPid) ?: return null
        if (tracedBy == tracer && pid !in exclude) tracees += pid
    }
    return tracees
}

fun forceKillTargets(proot: Int, statuses: Map<Int, String?>, walk: () -> List<Int>): List<Int> {
    val tracees = traceesOf(proot, statuses, exclude = setOf(proot)) ?: walk()
    return tracees.filter { it != proot }.distinct() + proot
}
