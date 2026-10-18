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

    override fun hangUp(): Boolean {
        if (session.exitCode != null || session.pid <= 0) return false
        val targets = descendantsOf(childrenOf(session.pid))
        targets.forEach { Process.sendSignal(it, SIGHUP) }
        return targets.isNotEmpty()
    }

    override fun sendEof() {
        session.write(byteArrayOf(0x04))
    }

    override fun kill() {
        if (session.exitCode != null || session.pid <= 0) return
        childrenOf(session.pid).forEach { Process.sendSignal(it, Process.SIGNAL_KILL) }
    }

    override fun forceKill() {
        if (session.exitCode == null && session.pid > 0) Process.sendSignal(session.pid, Process.SIGNAL_KILL)
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
        fun children(pid: Int) = byParent?.get(pid) ?: listedChildren(pid).orEmpty()
        val found = LinkedHashSet<Int>()
        val pending = ArrayDeque(roots.flatMap { children(it) })
        while (pending.isNotEmpty()) {
            val pid = pending.removeFirst()
            if (pid !in roots && found.add(pid)) pending.addAll(children(pid))
        }
        return found.toList()
    }

    private fun listedChildren(pid: Int): List<Int>? =
        runCatching { File("/proc/$pid/task/$pid/children").readText() }
            .getOrNull()
            ?.trim()
            ?.split(' ')
            ?.mapNotNull { it.toIntOrNull() }

    private fun childrenByParent(): Map<Int, List<Int>> =
        File("/proc").listFiles().orEmpty()
            .mapNotNull { it.name.toIntOrNull() }
            .mapNotNull { child ->
                runCatching { parentPid(File("/proc/$child/stat").readText()) }.getOrNull()?.let { it to child }
            }
            .groupBy({ it.first }, { it.second })

    private companion object {
        const val SIGHUP = 1
    }
}

fun parentPid(stat: String): Int? {
    val afterName = stat.substringAfterLast(')', missingDelimiterValue = "").trim()
    return afterName.split(' ').getOrNull(1)?.toIntOrNull()
}
