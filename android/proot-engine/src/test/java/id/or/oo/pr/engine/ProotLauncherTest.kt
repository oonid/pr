package id.or.oo.pr.engine

import org.junit.Assert.*
import org.junit.Test
import java.io.File
import java.lang.reflect.Method

class ProotLauncherTest {

    private val fakeHost = object : ProotHost {
        override val prefixDir = File("/fake/prefix")
        override val homeDir = File("/fake/home")
        override val packageName = "id.or.oo.test"
        override val cacheDir = File("/fake/cache")
    }

    @Test
    fun testBuildEnvVars() {
        val launcher = ProotLauncher(fakeHost)
        
        // buildEnvVars is private, we can use reflection for this quick win test
        val method: Method = ProotLauncher::class.java.getDeclaredMethod("buildEnvVars")
        method.isAccessible = true
        val envVars = method.invoke(launcher) as Array<String>
        
        // Convert to map for easier assertions
        val envMap = mutableMapOf<String, String>()
        for (i in envVars.indices step 2) {
            envMap[envVars[i]] = envVars[i + 1]
        }
        
        assertEquals("/fake/prefix", envMap["APP_PREFIX"])
        assertEquals("/fake/home", envMap["APP_HOME"])
        assertEquals("id.or.oo.test", envMap["APP_PACKAGE"])
        assertEquals("/fake/prefix/bin:/system/bin:/system/xbin", envMap["PATH"])
        assertEquals("/fake/home", envMap["HOME"])
        assertEquals("1", envMap["PROOT_NO_SECCOMP"])
        assertEquals("/fake/cache", envMap["PROOT_TMP_DIR"])
        assertEquals("/fake/cache", envMap["TMPDIR"])
        assertEquals("xterm-256color", envMap["TERM"])
        assertEquals("en_US.UTF-8", envMap["LANG"])
    }

    @Test
    fun testBuildEnvVarsPassesNetworkDnsServers() {
        val host = object : ProotHost by fakeHost {
            override val dnsServers = listOf("192.168.1.1", "2400:9800:2:2::245")
        }
        val method: Method = ProotLauncher::class.java.getDeclaredMethod("buildEnvVars")
        method.isAccessible = true
        val envVars = method.invoke(ProotLauncher(host)) as Array<String>
        val envMap = envVars.toList().chunked(2).associate { it[0] to it[1] }

        assertEquals("192.168.1.1,2400:9800:2:2::245", envMap["APP_DNS_SERVERS"])
    }
}
