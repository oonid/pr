package id.or.oo.pr.session

import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.LinkedBlockingQueue

class FakePty : Pty {
    private val chunks = LinkedBlockingQueue<ByteArray>()
    private val end = ByteArray(0)

    val written = CopyOnWriteArrayList<String>()
    val resizes = CopyOnWriteArrayList<Pair<Int, Int>>()

    @Volatile var ignoresEof = false
    @Volatile var ignoresKill = false
    @Volatile var eofSent = false
    @Volatile var killed = false
    @Volatile var forceKilled = false
    @Volatile var closed = false
    @Volatile var eofCount = 0
    @Volatile var writesAfterClose = 0
    @Volatile var resizesAfterClose = 0
    @Volatile var onClose: () -> Unit = {}

    @Volatile
    override var exitCode: Int? = null

    fun emit(text: String) = chunks.put(text.toByteArray())

    fun exit(code: Int) {
        exitCode = code
        chunks.put(end)
    }

    override fun read(buf: ByteArray): Int {
        val chunk = chunks.take()
        if (chunk === end) return -1
        System.arraycopy(chunk, 0, buf, 0, chunk.size)
        return chunk.size
    }

    override fun write(data: ByteArray): Int {
        if (closed) writesAfterClose++
        written += String(data)
        return data.size
    }

    override fun resize(rows: Int, cols: Int) {
        if (closed) resizesAfterClose++
        resizes += rows to cols
    }

    override fun sendEof() {
        eofSent = true
        eofCount++
        if (!ignoresEof) exit(0)
    }

    override fun kill() {
        killed = true
        if (!ignoresKill) exit(137)
    }

    override fun forceKill() {
        forceKilled = true
        exit(137)
    }

    override fun close() {
        closed = true
        onClose()
    }
}
