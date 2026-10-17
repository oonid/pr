package id.or.oo.pr.engine

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class PtyNativeTest {

    @Test
    fun testPtyNativeForkAndRead() {
        val args = arrayOf("/system/bin/echo", "hello pty")
        val envVars = emptyArray<String>()
        val masterFd = PtyNative.forkPty(args[0], args, envVars, 24, 80)
        
        assertTrue("masterFd should be > 0, got $masterFd", masterFd > 0)
        
        val buf = ByteArray(1024)
        // Note: PTY reads can sometimes return early with carriage returns etc., 
        // so we do a simple read which should be enough for 'echo'.
        val n = PtyNative.read(masterFd, buf, 0, buf.size)
        assertTrue("Should read some bytes, got $n", n > 0)
        
        val output = String(buf, 0, n)
        assertTrue("Output should contain our echoed text, got '$output'", output.contains("hello pty"))
        
        val pid = PtyNative.getPid()
        assertTrue("PID should be > 0, got $pid", pid > 0)
        
        // Wait for child to exit. The echo command exits almost instantly.
        var exitStatus = PtyNative.waitPid(pid)
        // If it hasn't exited yet, wait a tiny bit and retry (simple busy-wait for test)
        var retries = 50
        while (exitStatus == PtyNative.STILL_RUNNING && retries > 0) {
            Thread.sleep(10)
            exitStatus = PtyNative.waitPid(pid)
            retries--
        }
        assertEquals("Echo should exit with 0", 0, exitStatus)
        
        PtyNative.close(masterFd)
    }
}
