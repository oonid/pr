package id.or.oo.pr.session

interface Pty {
    fun read(buf: ByteArray): Int
    fun write(data: ByteArray): Int
    fun resize(rows: Int, cols: Int)
    fun hangUp(): Boolean
    fun sendEof()
    fun kill()
    fun forceKill()
    fun close()
    val exitCode: Int?
}
