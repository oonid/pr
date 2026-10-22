package id.or.oo.pr.session

import id.or.oo.pr.engine.ProotHost
import id.or.oo.pr.engine.ProotLauncher
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.nio.file.Files
import java.util.concurrent.CopyOnWriteArrayList

class TerminalSessionTest {

    private val literalPrefix = File("/data/data/org.example.gateway/files/usr")
    private val root = Files.createTempDirectory("terminal-session").toFile()
    private val host = object : ProotHost {
        override val prefixDir = File(root, "usr")
        override val homeDir = File(root, "home")
        override val packageName = "org.example.gateway"
        override val cacheDir = File(root, "cache")
    }
    private val prCli = File(host.prefixDir, "bin/pr-cli").apply {
        parentFile!!.mkdirs()
        createNewFile()
    }
    private val profile = File(root, "profile").apply { mkdirs() }
    private val binds = listOf(Bind(profile.path, "/srv"))
    private val started = CopyOnWriteArrayList<List<String>>()

    private fun factory(binds: List<Bind> = this.binds, session: ProotLauncher.Session? = ProotLauncher.Session(-1)) =
        ProotSessionFactory(host, binds) { args -> started += args; session }

    @After
    fun tearDown() {
        root.deleteRecursively()
    }

    @Test
    fun theTerminalArgumentsCarryEachBindAndNoCommand() {
        assertEquals(
            listOf(
                "/data/data/org.example.gateway/files/usr/bin/pr-cli",
                "login",
                "debian",
                "--user",
                "root",
                "--custom-bind",
                "/data/data/org.example.host/files/home/srv:/srv",
            ),
            terminalArgs(
                literalPrefix,
                "debian",
                listOf(Bind("/data/data/org.example.host/files/home/srv", "/srv")),
            ),
        )
    }

    @Test
    fun withoutBindsTheTerminalLogsInAsRootTheWayItAlwaysHas() {
        assertEquals(
            listOf("/data/data/org.example.gateway/files/usr/bin/pr-cli", "login", "alpine", "--user", "root"),
            terminalArgs(literalPrefix, "alpine", emptyList()),
        )
    }

    @Test
    fun theCommandArgumentsAreTheTerminalArgumentsWithTheCommandAfterTheSeparator() {
        assertEquals(
            terminalArgs(literalPrefix, "debian", binds) + listOf("--", "top -b"),
            loginArgs(literalPrefix, "debian", "top -b", binds),
        )
    }

    @Test
    fun theTerminalLogsIntoTheKeyWithItsBinds() {
        factory().startLogin("debian")

        assertEquals(listOf(terminalArgs(host.prefixDir, "debian", binds)), started)
    }

    @Test
    fun aMissingBindSourceFailsTheTerminalAndStartsNothing() {
        val missing = File(root, "absent")

        val e = assertThrows(IllegalStateException::class.java) {
            factory(binds = binds + Bind(missing.path, "/mnt")).startLogin("debian")
        }

        assertEquals("could not start a terminal for debian: no bind source $missing", e.message)
        assertTrue(started.isEmpty())
    }

    @Test
    fun thePublicConstructorPassesItsBindsOn() {
        val missing = File(root, "absent")

        val e = assertThrows(IllegalStateException::class.java) {
            ProotSessionFactory(host, binds + Bind(missing.path, "/mnt")).startLogin("debian")
        }

        assertEquals("could not start a terminal for debian: no bind source $missing", e.message)
    }

    @Test
    fun aMissingPrCliFailsTheTerminalAndStartsNothing() {
        prCli.delete()

        val e = assertThrows(IllegalStateException::class.java) { factory().startLogin("debian") }

        assertEquals("could not start a terminal for debian: no pr-cli at $prCli", e.message)
        assertTrue(started.isEmpty())
    }

    @Test
    fun aTerminalThatCannotStartIsReported() {
        val e = assertThrows(IllegalStateException::class.java) { factory(session = null).startLogin("debian") }

        assertEquals("could not start a terminal for debian", e.message)
    }
}
