package id.or.oo.pr

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

class InstalledContainersTest {

    @get:Rule
    val prefix = TemporaryFolder()

    private fun dir(path: String) = File(prefix.root, path).apply { mkdirs() }

    @Test
    fun aContainerIsInstalledOnceItsRootfsOrLegacyDirectoryExists() {
        dir("var/lib/pr/installed-rootfs/debian")
        dir("var/lib/pr/containers/alpine/rootfs")
        dir("var/lib/pr/containers/pulling")
        File(prefix.root, "var/lib/pr/installed-rootfs/notes.txt").writeText("")
        val containers = InstalledContainers(prefix.root)

        assertEquals(setOf("debian", "alpine"), containers.aliases())
        assertTrue(containers.contains("debian"))
        assertTrue(containers.contains("alpine"))
        assertFalse(containers.contains("pulling"))
        assertFalse(containers.contains("notes.txt"))
        assertFalse(containers.contains("ubuntu"))
        assertFalse(containers.contains(""))
    }

    @Test
    fun nothingIsInstalledOnAFreshPrefix() {
        val containers = InstalledContainers(prefix.root)
        assertEquals(emptySet<String>(), containers.aliases())
        assertFalse(containers.contains("alpine"))
    }
}
