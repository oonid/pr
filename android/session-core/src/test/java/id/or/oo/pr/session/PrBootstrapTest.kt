package id.or.oo.pr.session

import id.or.oo.pr.engine.ProotHost
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.io.IOException

class PrBootstrapTest {

    @get:Rule
    val tmp = TemporaryFolder()

    private fun host(root: File) = object : ProotHost {
        override val prefixDir = File(root, "files/usr")
        override val homeDir = File(root, "files/home")
        override val packageName = "org.example.gateway"
        override val cacheDir = File(root, "cache")
    }

    @Test
    fun createsTheDirectoriesPrCliExpects() {
        val host = host(tmp.root)

        createDirectories(host)

        listOf(
            "files/usr/tmp",
            "files/usr/etc/pr",
            "files/usr/var/lib/pr/installed-rootfs",
            "files/usr/var/lib/pr/dlcache",
            "files/home",
        ).forEach { assertTrue(it, File(tmp.root, it).isDirectory) }
    }

    @Test
    fun keepsWhatIsAlreadyInTheDirectories() {
        val host = host(tmp.root)
        createDirectories(host)
        val kept = File(host.homeDir, ".profile").apply { writeText("mine") }

        createDirectories(host)

        assertEquals("mine", kept.readText())
    }

    @Test(expected = IOException::class)
    fun failsWhenADirectoryCannotBeCreated() {
        val host = host(tmp.root)
        host.prefixDir.mkdirs()
        File(host.prefixDir, "var").writeText("not a directory")

        createDirectories(host)
    }
}
