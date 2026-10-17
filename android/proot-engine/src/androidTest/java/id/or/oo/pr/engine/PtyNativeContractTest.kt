package id.or.oo.pr.engine

import android.os.ParcelFileDescriptor
import androidx.test.ext.junit.runners.AndroidJUnit4
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
}
