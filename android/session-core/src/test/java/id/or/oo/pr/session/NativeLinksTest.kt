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

        assertEquals(lib("libbusybox.so"), target("busybox"))
        assertEquals(lib("libproot.so"), target("proot"))
        assertEquals(lib("libpr-cli.so"), target("pr-cli"))
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
        listOf("bash", "pr-test", "proot-distro").forEach { bin(it).writeText("old") }
        File(prefix, "scripts/proot-distro.sh").apply { parentFile!!.mkdirs() }.writeText("old")

        links().ensure()

        listOf("bash", "pr-test", "proot-distro").forEach { assertFalse(bin(it).exists()) }
        assertFalse(File(prefix, "scripts").exists())
    }

    @Test
    fun linksEachAppletToBusyboxAndKeepsExistingFiles() {
        bin("ls").parentFile!!.mkdirs()
        bin("ls").writeText("mine")

        val created = links().applets(listOf("sh", "", "ls", "tar"))

        assertEquals(2, created)
        assertEquals("busybox", target("sh"))
        assertEquals("busybox", target("tar"))
        assertEquals("mine", bin("ls").readText())
    }

    @Test
    fun anAppletThatCannotBeLinkedIsSkipped() {
        bin("sh").parentFile!!.mkdirs()

        val created = links { target, link ->
            if (link.endsWith("/bad")) throw IOException("denied") else symlink(target, link)
        }.applets(listOf("bad", "sh"))

        assertEquals(1, created)
        assertEquals("busybox", target("sh"))
    }
}
