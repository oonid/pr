package id.or.oo.pr.session

import java.io.File

interface ProcFs {
    fun pids(): List<Int>
    fun status(pid: Int): String?
    fun stat(pid: Int): String?
    fun taskIds(pid: Int): List<Int>?
    fun children(pid: Int, tid: Int): String?
    fun cmdline(pid: Int): String?
    fun exists(pid: Int): Boolean
}

object SystemProcFs : ProcFs {
    override fun pids(): List<Int> = File("/proc").listFiles().orEmpty().mapNotNull { it.name.toIntOrNull() }

    override fun status(pid: Int): String? = read("/proc/$pid/status")

    override fun stat(pid: Int): String? = read("/proc/$pid/stat")

    override fun taskIds(pid: Int): List<Int>? =
        File("/proc/$pid/task").listFiles()?.mapNotNull { it.name.toIntOrNull() }

    override fun children(pid: Int, tid: Int): String? = read("/proc/$pid/task/$tid/children")

    override fun cmdline(pid: Int): String? = read("/proc/$pid/cmdline")

    override fun exists(pid: Int): Boolean = File("/proc/$pid").exists()

    private fun read(path: String): String? = runCatching { File(path).readText() }.getOrNull()
}

class ProotProcesses(private val proc: ProcFs = SystemProcFs) {

    fun shells(proot: Int): List<Int> = childrenOf(proot)

    fun endTargets(proot: Int, policy: EndPolicy = EndPolicy.SPARE_LOGIN_SHELL): Set<Int> {
        val shells = childrenOf(proot)
        val spared = if (policy == EndPolicy.SPARE_LOGIN_SHELL) shells.toSet() else emptySet()
        return traceesOf(proot, statuses(), exclude = spared + proot) ?: (shells - spared + descendantsOf(shells)).toSet()
    }

    fun alive(pids: Set<Int>): Boolean = pids.any { pid ->
        val state = proc.stat(pid)?.let(::processState)
        state != null && state != 'Z' && state != 'X'
    }

    fun forceKillTargets(proot: Int): List<Int> =
        forceKillTargets(proot, statuses()) { descendantsOf(listOf(proot)) }

    fun children(parent: Int, commandLine: List<String>): List<Int> =
        childrenOf(parent).filter { proc.cmdline(it)?.removeSuffix("\u0000")?.split('\u0000') == commandLine }

    fun descendants(roots: List<Int>): List<Int> = descendantsOf(roots)

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
        val tasks = proc.taskIds(pid) ?: return null
        val lists = tasks.mapNotNull { tid -> proc.children(pid, tid) }
        if (lists.isEmpty()) return null
        return lists.flatMap { text -> text.trim().split(' ').mapNotNull { it.toIntOrNull() } }
    }

    private fun statuses(): Map<Int, String?> = buildMap {
        for (pid in proc.pids()) {
            val status = proc.status(pid)
            if (status != null || proc.exists(pid)) put(pid, status)
        }
    }

    private fun childrenByParent(): Map<Int, List<Int>> =
        proc.pids()
            .mapNotNull { child -> proc.stat(child)?.let(::parentPid)?.let { it to child } }
            .groupBy({ it.first }, { it.second })
}
