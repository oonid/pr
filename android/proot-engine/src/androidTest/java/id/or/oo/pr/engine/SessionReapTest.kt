package id.or.oo.pr.engine

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** A session's child must be reaped as soon as it exits, not left as a zombie until the app dies. */
@RunWith(AndroidJUnit4::class)
class SessionReapTest {

    @Test
    fun exitedSessionChildIsReaped() {
        val (masterFd, pid) = PtyNative.forkPtyWithPid(
            "/system/bin/sh", arrayOf("sh", "-c", "exit 3"), null, 24, 80)
        assertTrue("forkPty failed", masterFd >= 0 && pid > 0)
        val session = ProotLauncher.Session(masterFd, pid)

        val deadline = System.currentTimeMillis() + 10_000
        while (session.exitCode == null && System.currentTimeMillis() < deadline) {
            Thread.sleep(50)
        }
        session.close()

        assertEquals(3, session.exitCode)
        assertFalse("pid $pid is still a zombie", File("/proc/$pid").exists())
    }
}
