package id.or.oo.pr.engine

import android.os.ParcelFileDescriptor
import android.system.Os
import android.system.OsConstants
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class PtyNativeContractTest {

    private fun readUntil(fd: Int, done: (String) -> Boolean): String {
        val buf = ByteArray(1024)
        val out = StringBuilder()
        val deadline = System.currentTimeMillis() + 10_000
        while (!done(out.toString()) && System.currentTimeMillis() < deadline) {
            val n = PtyNative.read(fd, buf, 0, buf.size)
            if (n < 0) break
            out.append(String(buf, 0, n))
        }
        return out.toString()
    }

    @Test
    fun waitPidTellsRunningFromExited() {
        val (masterFd, pid) = PtyNative.forkPtyWithPid(
            "/system/bin/sh", arrayOf("sh", "-c", "sleep 1; exit 3"), null, 24, 80)
        assertTrue("forkPty failed", masterFd >= 0 && pid > 0)
        try {
            assertEquals(PtyNative.STILL_RUNNING, PtyNative.waitPid(pid))
            var status = PtyNative.STILL_RUNNING
            val deadline = System.currentTimeMillis() + 10_000
            while (status == PtyNative.STILL_RUNNING && System.currentTimeMillis() < deadline) {
                Thread.sleep(50)
                status = PtyNative.waitPid(pid)
            }
            assertEquals(3, status)
        } finally {
            PtyNative.close(masterFd)
        }
    }

    @Test
    fun childGetsArgsAndEnvOnTopOfInheritedEnv() {
        val masterFd = PtyNative.forkPty(
            "/system/bin/sh",
            arrayOf("sh", "-c", "echo \"<\$PIT_FOO|\$0|\${PATH:+path}>\"", "zero"),
            arrayOf("PIT_FOO", "first", "PIT_FOO", "bar"),
            24, 80)
        assertTrue("forkPty failed", masterFd >= 0)
        try {
            val out = readUntil(masterFd) { it.contains(">") }
            assertTrue("unexpected child output: $out", out.contains("<bar|zero|path>"))
        } finally {
            PtyNative.close(masterFd)
        }
    }

    private fun childOutput(script: String, forkOnMainThread: Boolean = false): String {
        val args = arrayOf("sh", "-c", "$script; echo END")
        var masterFd = -1
        if (forkOnMainThread) {
            InstrumentationRegistry.getInstrumentation().runOnMainSync {
                masterFd = PtyNative.forkPty("/system/bin/sh", args, null, 24, 80)
            }
        } else {
            masterFd = PtyNative.forkPty("/system/bin/sh", args, null, 24, 80)
        }
        assertTrue("forkPty failed", masterFd >= 0)
        try {
            return readUntil(masterFd) { it.contains("END") }
        } finally {
            PtyNative.close(masterFd)
        }
    }

    private fun signalMask(out: String, field: String): Long =
        Regex("$field:\\s*([0-9a-f]+)").find(out)?.groupValues?.get(1)?.toLong(16)
            ?: throw AssertionError("no $field in: $out")

    @Test
    fun childHasOnlyTheTerminalDescriptors() {
        val (readEnd, writeEnd) = ParcelFileDescriptor.createPipe()
        val inheritable = Os.dup(readEnd.fileDescriptor)
        try {
            val out = childOutput("sleep 5 & ls -1 /proc/\$!/fd; kill \$!")
            val fds = out.lines().map { it.trim() }.filter { it.isNotEmpty() && it.all(Char::isDigit) }
            assertEquals("child descriptors: $out", listOf("0", "1", "2"), fds)
        } finally {
            Os.close(inheritable)
            readEnd.close()
            writeEnd.close()
        }
    }

    @Test
    fun childStartsWithNoBlockedOrIgnoredSignals() {
        val bionicPosixTimerSignal = 1L shl (32 - 1)
        val bionicArtProfilerSignal = 1L shl (38 - 1)
        val out = childOutput("grep -E '^Sig(Blk|Ign)' /proc/self/status", forkOnMainThread = true)
        assertEquals("blocked: $out", 0L, signalMask(out, "SigBlk") and bionicPosixTimerSignal.inv())
        assertEquals("ignored: $out", 0L, signalMask(out, "SigIgn") and bionicArtProfilerSignal.inv())
    }

    @Test
    fun writeRejectsSliceOutsideTheArray() {
        val (readEnd, writeEnd) = ParcelFileDescriptor.createPipe()
        try {
            assertThrows(ArrayIndexOutOfBoundsException::class.java) {
                PtyNative.write(writeEnd.fd, ByteArray(4), 2, 8)
            }
            assertThrows(ArrayIndexOutOfBoundsException::class.java) {
                PtyNative.write(writeEnd.fd, ByteArray(4), -1, 2)
            }
        } finally {
            readEnd.close()
            writeEnd.close()
        }
    }

    @Test
    fun readRejectsSliceOutsideTheArrayWithoutConsumingInput() {
        val (readEnd, writeEnd) = ParcelFileDescriptor.createPipe()
        try {
            val data = "0123456789abcdef".toByteArray()
            assertEquals(data.size, PtyNative.write(writeEnd.fd, data, 0, data.size))
            assertThrows(ArrayIndexOutOfBoundsException::class.java) {
                PtyNative.read(readEnd.fd, ByteArray(4), 2, 8)
            }
            val buf = ByteArray(32)
            val n = PtyNative.read(readEnd.fd, buf, 0, buf.size)
            assertEquals("0123456789abcdef", String(buf, 0, n))
        } finally {
            readEnd.close()
            writeEnd.close()
        }
    }

    @Test
    fun readAndWriteTouchOnlyTheirSlice() {
        val (readEnd, writeEnd) = ParcelFileDescriptor.createPipe()
        try {
            assertEquals(5, PtyNative.write(writeEnd.fd, "xxABCDEyy".toByteArray(), 2, 5))
            val buf = "..........".toByteArray()
            assertEquals(5, PtyNative.read(readEnd.fd, buf, 3, 6))
            assertEquals("...ABCDE..", String(buf))
        } finally {
            readEnd.close()
            writeEnd.close()
        }
    }

    @Test
    fun aLargeWriteIsWrittenWhole() {
        val (readEnd, writeEnd) = ParcelFileDescriptor.createPipe()
        try {
            val data = ByteArray(40_000) { (it % 251).toByte() }
            assertEquals(data.size, PtyNative.write(writeEnd.fd, data, 0, data.size))
            val got = java.io.ByteArrayOutputStream()
            val buf = ByteArray(4096)
            while (got.size() < data.size) {
                val n = PtyNative.read(readEnd.fd, buf, 0, buf.size)
                if (n <= 0) break
                got.write(buf, 0, n)
            }
            assertTrue("bytes read back differ", data.contentEquals(got.toByteArray()))
        } finally {
            readEnd.close()
            writeEnd.close()
        }
    }

    @Test
    fun theTerminalErasesWholeUtf8Characters() {
        val out = childOutput("stty -a")
        assertTrue("iutf8 is not set: $out", Regex("(^|\\s)iutf8(\\s|$)").containsMatchIn(out))
    }

    @Test
    fun theMasterIsClosedOnExec() {
        val masterFd = PtyNative.forkPty("/system/bin/sh", arrayOf("sh", "-c", "sleep 1"), null, 24, 80)
        assertTrue("forkPty failed", masterFd >= 0)
        val pfd = ParcelFileDescriptor.adoptFd(masterFd)
        try {
            val flags = Os.fcntlInt(pfd.fileDescriptor, OsConstants.F_GETFD, 0)
            assertTrue("FD_CLOEXEC is not set", flags and OsConstants.FD_CLOEXEC != 0)
        } finally {
            PtyNative.close(pfd.detachFd())
        }
    }

    @Test
    fun backspaceErasesAWholeUtf8Character() {
        val masterFd = PtyNative.forkPty(
            "/system/bin/sh",
            arrayOf("sh", "-c", "read x; printf '<%s>' \"\$x\" | od -An -c; echo END"),
            null, 24, 80)
        assertTrue("forkPty failed", masterFd >= 0)
        try {
            Thread.sleep(300)
            val input = "\u00e9".toByteArray(Charsets.UTF_8) + byteArrayOf(0x7f) + "a\n".toByteArray()
            PtyNative.write(masterFd, input, 0, input.size)
            val out = readUntil(masterFd) { it.contains("END") }
            assertTrue("the erase left part of the character: $out", Regex("<\\s+a\\s+>").containsMatchIn(out))
        } finally {
            PtyNative.close(masterFd)
        }
    }

    @Test
    fun closingASessionSendsNothingToTheProgram() {
        val cache = InstrumentationRegistry.getInstrumentation().targetContext.cacheDir
        val out = java.io.File(cache, "pit-close-${System.nanoTime()}")
        val (masterFd, pid) = PtyNative.forkPtyWithPid(
            "/system/bin/sh",
            arrayOf("sh", "-c", "stty raw -echo; trap '' HUP; dd bs=1 count=1 of=${out.absolutePath} 2>/dev/null"),
            null, 24, 80)
        assertTrue("forkPty failed", masterFd >= 0 && pid > 0)
        Thread.sleep(500)
        val session = ProotLauncher.Session(masterFd, pid)

        session.close()

        val deadline = System.currentTimeMillis() + 10_000
        while (session.exitCode == null && System.currentTimeMillis() < deadline) Thread.sleep(50)
        val received = if (out.exists()) out.length() else 0L
        out.delete()
        assertEquals("bytes the program received on close", 0L, received)
    }

    @Test
    fun aSizeOutOfRangeIsRefused() {
        val masterFd = PtyNative.forkPty("/system/bin/sh", arrayOf("sh", "-c", "sleep 2"), null, 24, 80)
        assertTrue("forkPty failed", masterFd >= 0)
        try {
            for ((rows, cols) in listOf(0 to 80, -1 to 80, 70_000 to 80, 24 to 0)) {
                assertEquals("resize($rows, $cols)", -1, PtyNative.resize(masterFd, rows, cols))
            }
            assertEquals(0, PtyNative.resize(masterFd, 30, 100))
        } finally {
            PtyNative.close(masterFd)
        }
    }
}
