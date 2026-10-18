package id.or.oo.pr.session

class FakeProcFs(private val childrenFiles: Boolean) : ProcFs {

    private class Proc(
        val ppid: Int,
        val tracer: Int,
        val state: Char,
        val forkedBy: Int,
        val threads: List<Int>,
        val statusReadable: Boolean,
    )

    private val procs = LinkedHashMap<Int, Proc>()
    private val vanishing = HashSet<Int>()

    fun add(
        pid: Int,
        ppid: Int,
        tracer: Int = 0,
        state: Char = 'S',
        forkedBy: Int = ppid,
        threads: List<Int> = emptyList(),
        statusReadable: Boolean = true,
    ) {
        procs[pid] = Proc(ppid, tracer, state, forkedBy, threads, statusReadable)
    }

    fun vanishAfterListing(pid: Int) {
        vanishing += pid
    }

    override fun pids(): List<Int> {
        val listed = procs.keys.toList()
        vanishing.forEach { procs.remove(it) }
        vanishing.clear()
        return listed
    }

    override fun status(pid: Int): String? = procs[pid]?.takeIf { it.statusReadable }?.let {
        "Name:\tsh\nState:\t${it.state}\nTgid:\t$pid\nPid:\t$pid\nPPid:\t${it.ppid}\nTracerPid:\t${it.tracer}\nUid:\t10200\n"
    }

    override fun stat(pid: Int): String? = procs[pid]?.let { "$pid (s h) ${it.state} ${it.ppid} $pid $pid 0 -1 4194304" }

    override fun taskIds(pid: Int): List<Int>? = procs[pid]?.let { listOf(pid) + it.threads }

    override fun children(pid: Int, tid: Int): String? {
        if (!childrenFiles || procs[pid] == null) return null
        return procs.filterValues { it.ppid == pid && it.forkedBy == tid }.keys.joinToString("") { "$it " }
    }
}
