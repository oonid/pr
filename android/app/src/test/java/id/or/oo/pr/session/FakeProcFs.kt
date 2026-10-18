package id.or.oo.pr.session

class FakeProcFs(private val childrenFiles: Boolean) : ProcFs {

    private class Proc(
        val ppid: Int,
        val tracer: Int,
        val state: Char,
        val forkedBy: Int,
        val threads: List<Int>,
        val statusReadable: Boolean,
        val statReadable: Boolean,
    )

    private val procs = LinkedHashMap<Int, Proc>()
    private val vanishing = HashMap<Int, MutableList<Int>>()
    private val vanished = HashSet<Int>()
    private var listings = 0
    val readsAfterVanishing = mutableListOf<String>()

    fun add(
        pid: Int,
        ppid: Int,
        tracer: Int = 0,
        state: Char = 'S',
        forkedBy: Int = ppid,
        threads: List<Int> = emptyList(),
        statusReadable: Boolean = true,
        statReadable: Boolean = true,
    ) {
        procs[pid] = Proc(ppid, tracer, state, forkedBy, threads, statusReadable, statReadable)
    }

    fun vanishAfterListing(pid: Int, listing: Int) {
        vanishing.getOrPut(listing) { mutableListOf() } += pid
    }

    override fun pids(): List<Int> {
        val listed = procs.keys.toList()
        listings++
        vanishing.remove(listings)?.forEach {
            procs.remove(it)
            vanished += it
        }
        return listed
    }

    private fun <T> read(kind: String, pid: Int, value: (Proc) -> T?): T? {
        if (pid in vanished) readsAfterVanishing += "$kind $pid"
        return procs[pid]?.let(value)
    }

    override fun status(pid: Int): String? = read("status", pid) {
        if (!it.statusReadable) null
        else "Name:\tsh\nState:\t${it.state}\nTgid:\t$pid\nPid:\t$pid\nPPid:\t${it.ppid}\nTracerPid:\t${it.tracer}\nUid:\t10200\n"
    }

    override fun stat(pid: Int): String? = read("stat", pid) {
        if (!it.statReadable) null else "$pid (s h) ${it.state} ${it.ppid} $pid $pid 0 -1 4194304"
    }

    override fun taskIds(pid: Int): List<Int>? = read("task", pid) { listOf(pid) + it.threads }

    override fun children(pid: Int, tid: Int): String? {
        if (!childrenFiles) return null
        return read("children", pid) { _ ->
            procs.filterValues { it.ppid == pid && it.forkedBy == tid }.keys.joinToString("") { "$it " }
        }
    }

    override fun exists(pid: Int): Boolean = read("exists", pid) { true } ?: false
}
