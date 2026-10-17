package id.or.oo.pr.engine

object PtyNative {
    /** [waitPid] result while the child has not exited yet. */
    const val STILL_RUNNING = -2

    init {
        System.loadLibrary("ptyjni")
    }

    // Synchronized with forkPtyWithPid, which reads the pid this call records natively.
    @Synchronized
    fun forkPty(cmd: String, args: Array<String>?, envVars: Array<String>?, rows: Int, cols: Int): Int {
        return nativeForkPty(cmd, args, envVars, rows, cols)
    }

    /** Fork like [forkPty] and return (masterFd, childPid); the pid is read under the same lock. */
    @Synchronized
    fun forkPtyWithPid(cmd: String, args: Array<String>?, envVars: Array<String>?, rows: Int, cols: Int): Pair<Int, Int> {
        val fd = nativeForkPty(cmd, args, envVars, rows, cols)
        return fd to (if (fd >= 0) nativeGetPid() else -1)
    }

    /** Block until [pid] exits and reap it; exit code, or 128 + signal. */
    fun waitPidBlocking(pid: Int): Int {
        return nativeWaitPidBlocking(pid)
    }

    fun read(fd: Int, buf: ByteArray, offset: Int, length: Int): Int {
        return nativeRead(fd, buf, offset, length)
    }

    fun write(fd: Int, buf: ByteArray, offset: Int, length: Int): Int {
        return nativeWrite(fd, buf, offset, length)
    }

    fun resize(fd: Int, rows: Int, cols: Int): Int {
        return nativeResize(fd, rows, cols)
    }

    /** Reap [pid] if it has exited: exit code, or 128 + signal; [STILL_RUNNING]; -1 on error. */
    fun waitPid(pid: Int): Int {
        return nativeWaitPid(pid)
    }

    fun close(fd: Int) {
        nativeClose(fd)
    }

    fun getPid(): Int {
        return nativeGetPid()
    }

    private external fun nativeForkPty(cmd: String, args: Array<String>?, envVars: Array<String>?, rows: Int, cols: Int): Int
    private external fun nativeRead(fd: Int, buf: ByteArray, offset: Int, length: Int): Int
    private external fun nativeWrite(fd: Int, buf: ByteArray, offset: Int, length: Int): Int
    private external fun nativeResize(fd: Int, rows: Int, cols: Int): Int
    private external fun nativeWaitPid(pid: Int): Int
    private external fun nativeWaitPidBlocking(pid: Int): Int
    private external fun nativeClose(fd: Int)
    private external fun nativeGetPid(): Int
}
