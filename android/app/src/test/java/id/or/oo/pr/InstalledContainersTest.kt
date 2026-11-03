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
    fun aContainerWhoseInstallDidNotFinishIsNotInstalledButHasFiles() {
        dir("var/lib/pr/installed-rootfs/debian")
        File(prefix.root, "var/lib/pr/installed-rootfs/.debian.pr-cli-installing").writeText("")
        dir("var/lib/pr/containers/alpine/rootfs")
        File(prefix.root, "var/lib/pr/containers/.alpine.pr-cli-installing").writeText("")
        File(prefix.root, "var/lib/pr/containers/.early.pr-cli-installing").writeText("")
        dir("var/lib/pr/containers/ubuntu/rootfs")
        dir("var/lib/pr/containers/fedora/rootfs/.fedora.pr-cli-installing")
        File(prefix.root, "var/lib/pr/containers/.pr-cli-installing").writeText("")
        File(prefix.root, "var/lib/pr/containers/.a b.pr-cli-installing").writeText("")
        val containers = InstalledContainers(prefix.root)

        assertEquals(setOf("ubuntu", "fedora"), containers.aliases())
        assertEquals(setOf("debian", "alpine", "early"), containers.unfinished())
        listOf("debian", "alpine", "early").forEach {
            assertFalse(it, containers.contains(it))
            assertTrue(it, containers.hasFiles(it))
        }
        assertTrue(containers.hasFiles("ubuntu"))
        assertFalse(containers.hasFiles("arch"))
        assertFalse(containers.hasFiles(""))
    }

    @Test
    fun nothingIsInstalledOnAFreshPrefix() {
        val containers = InstalledContainers(prefix.root)
        assertEquals(emptySet<String>(), containers.aliases())
        assertEquals(emptySet<String>(), containers.unfinished())
        assertFalse(containers.contains("alpine"))
    }
}
