package id.or.oo.pr.session

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.io.IOException
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.Paths

class NativeLinksTest {

    @get:Rule
    val tmp = TemporaryFolder()

    private lateinit var prefix: File
    private lateinit var libs: File

    private val symlink: (String, String) -> Unit = { target, link ->
        Files.createSymbolicLink(Paths.get(link), Paths.get(target))
    }

    @Before
    fun setUp() {
        prefix = tmp.newFolder("usr")
        libs = tmp.newFolder("lib")
        listOf("libbusybox.so", "libproot.so", "libpr-cli.so").forEach { File(libs, it).writeText("elf") }
    }

    private fun bin(name: String) = File(prefix, "bin/$name")

    private fun target(name: String) = Files.readSymbolicLink(bin(name).toPath()).toString()

    private fun lib(name: String) = File(libs, name).absolutePath

    private fun links(symlink: (String, String) -> Unit = this.symlink) = NativeLinks(prefix, libs, symlink)

    @Test
    fun linksEachNativeLibraryIntoBin() {
        links().ensure()

        assertEquals(lib("libproot.so"), target("proot"))
        assertEquals(lib("libpr-cli.so"), target("pr-cli"))
    }

    @Test
    fun doesNotLinkBusyboxEvenIfTheApkShipsIt() {
        links().ensure()

        assertFalse(Files.exists(bin("busybox").toPath(), LinkOption.NOFOLLOW_LINKS))
    }

    @Test
    fun skipsALibraryTheApkDoesNotShip() {
        File(libs, "libproot.so").delete()

        links().ensure()

        assertFalse(Files.exists(bin("proot").toPath(), LinkOption.NOFOLLOW_LINKS))
        assertEquals(lib("libpr-cli.so"), target("pr-cli"))
    }

    @Test
    fun keepsALinkThatAlreadyPointsAtTheLibrary() {
        links().ensure()

        links { _, link -> throw IOException("relinked $link") }.ensure()

        assertEquals(lib("libproot.so"), target("proot"))
    }

    @Test
    fun replacesALinkToAnotherLibraryDirectory() {
        val old = tmp.newFolder("old-lib")
        File(old, "libproot.so").writeText("old")
        bin("proot").parentFile!!.mkdirs()
        symlink(File(old, "libproot.so").absolutePath, bin("proot").absolutePath)

        links().ensure()

        assertEquals(lib("libproot.so"), target("proot"))
    }

    @Test
    fun replacesADanglingLink() {
        bin("pr-cli").parentFile!!.mkdirs()
        symlink(File(tmp.root, "gone/libpr-cli.so").absolutePath, bin("pr-cli").absolutePath)

        links().ensure()

        assertEquals(lib("libpr-cli.so"), target("pr-cli"))
    }

    @Test
    fun removesStaleBinariesAndTheScriptsDirectory() {
        bin("bash").parentFile!!.mkdirs()
        listOf("bash", "pr-test", "proot-distro", "bootstrap.sh").forEach { bin(it).writeText("old") }
        File(prefix, "scripts/proot-distro.sh").apply { parentFile!!.mkdirs() }.writeText("old")

        links().ensure()

        listOf("bash", "pr-test", "proot-distro", "bootstrap.sh").forEach { assertFalse(bin(it).exists()) }
        assertFalse(File(prefix, "scripts").exists())
    }

    @Test
    fun removesTheBusyboxLinkAndEveryAppletLinkToIt() {
        bin("busybox").parentFile!!.mkdirs()
        symlink(lib("libbusybox.so"), bin("busybox").absolutePath)
        listOf("sh", "ls", "wget").forEach { symlink("busybox", bin(it).absolutePath) }

        links().ensure()

        listOf("busybox", "sh", "ls", "wget").forEach {
            assertFalse(it, Files.exists(bin(it).toPath(), LinkOption.NOFOLLOW_LINKS))
        }
    }

    @Test
    fun removesAppletLinksLeftDanglingByAnEarlierUpgrade() {
        bin("sh").parentFile!!.mkdirs()
        symlink("busybox", bin("sh").absolutePath)

        links().ensure()

        assertFalse(Files.exists(bin("sh").toPath(), LinkOption.NOFOLLOW_LINKS))
    }

    @Test
    fun keepsFilesAndLinksThatAreNotBusybox() {
        bin("mine").parentFile!!.mkdirs()
        bin("mine").writeText("mine")
        symlink("pr-cli", bin("prc").absolutePath)
        symlink(File(tmp.root, "busybox").absolutePath, bin("other").absolutePath)

        links().ensure()

        assertEquals("mine", bin("mine").readText())
        assertEquals("pr-cli", target("prc"))
        assertEquals(File(tmp.root, "busybox").absolutePath, target("other"))
    }
}
