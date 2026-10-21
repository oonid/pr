package id.or.oo.pr.session

import id.or.oo.pr.engine.ProotHost
import org.junit.Assert.assertEquals
import org.junit.Test
import java.io.File

class PrBootstrapTest {

    private val host = object : ProotHost {
        override val prefixDir = File("/data/data/org.example.gateway/files/usr")
        override val homeDir = File("/data/data/org.example.gateway/files/home")
        override val packageName = "org.example.gateway"
        override val cacheDir = File("/data/data/org.example.gateway/cache")
    }

    @Test
    fun bootstrapShRunsWithTheHostPathsAndSystemBinFirst() {
        assertEquals(
            mapOf(
                "APP_PREFIX" to "/data/data/org.example.gateway/files/usr",
                "APP_HOME" to "/data/data/org.example.gateway/files/home",
                "APP_PACKAGE" to "org.example.gateway",
                "PATH" to "/system/bin:/system/xbin:/data/data/org.example.gateway/files/usr/bin",
                "PROOT_NO_SECCOMP" to "1",
                "HOME" to "/data/data/org.example.gateway/files/home",
            ),
            bootstrapEnv(host),
        )
    }
}
