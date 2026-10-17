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

    override fun sendEof() {
        session.write(byteArrayOf(0x04))
    }

    override fun kill() {
        if (session.exitCode != null) return
        childrenOf(session.pid).forEach { Process.sendSignal(it, Process.SIGNAL_KILL) }
    }

    override fun forceKill() {
        if (session.exitCode == null) Process.sendSignal(session.pid, Process.SIGNAL_KILL)
    }

    override fun close() {
        session.close()
    }

    override val exitCode: Int?
        get() = session.exitCode

    private fun childrenOf(pid: Int): List<Int> {
        val listed = runCatching { File("/proc/$pid/task/$pid/children").readText() }
            .getOrNull()
            ?.trim()
            ?.split(' ')
            ?.mapNotNull { it.toIntOrNull() }
        if (!listed.isNullOrEmpty()) return listed
        return File("/proc").listFiles().orEmpty()
            .mapNotNull { it.name.toIntOrNull() }
            .filter { child -> runCatching { parentPid(File("/proc/$child/stat").readText()) }.getOrNull() == pid }
    }
}

fun parentPid(stat: String): Int? {
    val afterName = stat.substringAfterLast(')', missingDelimiterValue = "").trim()
    return afterName.split(' ').getOrNull(1)?.toIntOrNull()
}
