package id.or.oo.pr.session

import id.or.oo.pr.engine.ProotHost
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.nio.file.Files
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger

class CommandSessionTest {

    private val literalPrefix = File("/data/data/org.example.gateway/files/usr")
    private val root = Files.createTempDirectory("command-session").toFile()
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

    private val container = "debian"
    private val key = "web-server"
    private val command = "httpd -f -p 8080 && echo 'server stopped'"
    private val literalBinds = listOf(
        Bind("/storage/emulated/0/Download", "/root/Download"),
        Bind("/data/data/org.example.host/files/home/srv", "/srv"),
    )
    private val binds = listOf(
        Bind(File(root, "Download").apply { mkdirs() }.path, "/root/Download"),
        Bind(File(root, "srv").apply { mkdirs() }.path, "/srv"),
    )

    private val started = CopyOnWriteArrayList<List<String>>()
    private val ptys = CopyOnWriteArrayList<FakePty>()
    private val received = StringBuffer()
    private val ended = CopyOnWriteArrayList<Pair<String, Int?>>()

    private val sessions = SessionManager(
        { alias, onEnded ->
            val pty = FakePty().also { ptys += it }
            ManagedSession(alias, pty, null, { _, _, _ -> }, onEnded, exitCodeWaitMs = 100)
        },
        object : SessionManager.ForegroundController {
            override fun start() {}
        },
    ) { alias, code -> ended += alias to code }

    private val toReceived: (ByteArray, Int, Int) -> Unit = { buf, off, len -> received.append(String(buf, off, len)) }

    private fun factory(
        sink: (ByteArray, Int, Int) -> Unit = toReceived,
        binds: List<Bind> = this.binds,
        start: (List<String>) -> Pty? = { args -> started += args; FakePty().also { ptys += it } },
    ) = CommandSessionFactory(host, container, command, sink, binds, start)

    private fun openCommand(): ManagedSession =
        (sessions.open(key, factory()) as SessionManager.OpenResult.Opened).session

    private val app = 1700
    private val proot = 1790
    private val shell = 1804
    private val gateway = 1810

    private class CommandProcess(
        private val proc: FakeProcFs,
        private val proot: Int,
        private val exitsOnSigtermOf: Int,
        val pty: FakePty = FakePty().apply { ignoresEof = true },
    ) : Pty by pty {
        val sigterm = CopyOnWriteArrayList<Int>()

        override fun signalTracees(): Set<Int> {
            pty.signalTracees()
            val targets = ProotProcesses(proc).endTargets(proot, CommandSessionFactory.END_POLICY)
            sigterm += targets
            if (exitsOnSigtermOf in targets) pty.exit(143)
            return targets
        }
    }

    private fun endCommand(proc: FakeProcFs, exitsOnSigtermOf: Int): CommandProcess {
        val process = CommandProcess(proc, proot, exitsOnSigtermOf).also { ptys += it.pty }
        sessions.open(key, factory(start = { args -> started += args; process }))
        sessions.end(key)
        eventually(timeoutMs = 10_000) { sessions.running.value.isEmpty() }
        return process
    }

    @After
    fun tearDown() {
        ptys.forEach { it.exit(0) }
        root.deleteRecursively()
    }

    @Test
    fun theLoginArgumentsCarryEachBindAndTheCommandAsOneArgumentAfterTheSeparator() {
        assertEquals(
            listOf(
                "/data/data/org.example.gateway/files/usr/bin/pr-cli",
                "login",
                "debian",
                "--user",
                "root",
                "--custom-bind",
                "/storage/emulated/0/Download:/root/Download",
                "--custom-bind",
                "/data/data/org.example.host/files/home/srv:/srv",
                "--",
                "httpd -f -p 8080 && echo 'server stopped'",
            ),
            loginArgs(literalPrefix, "debian", command, literalBinds),
        )
    }

    @Test
    fun withoutBindsTheSeparatorFollowsTheUser() {
        assertEquals(
            listOf("/data/data/org.example.gateway/files/usr/bin/pr-cli", "login", "alpine", "--user", "root", "--", "top -b"),
            loginArgs(literalPrefix, "alpine", "top -b", emptyList()),
        )
    }

    @Test
    fun theFactoryLogsIntoItsContainerNotIntoTheSessionKey() {
        openCommand()

        assertEquals(listOf(loginArgs(host.prefixDir, container, command, binds)), started)
        assertEquals(container, started.single()[2])
    }

    @Test
    fun aCommandSessionHasNoEmulatorAndItsOutputReachesTheSink() {
        val session = openCommand()

        ptys.single().emit("gateway listening on 8080\n")

        eventually { received.toString() == "gateway listening on 8080\n" }
        assertNull(session.emulator)
        assertEquals(listOf(key), sessions.running.value)
    }

    @Test
    fun endStopsACommandSessionTheWayItStopsATerminalSession() {
        openCommand()
        val pty = ptys.single()

        sessions.end(key)

        eventually { sessions.running.value.isEmpty() }
        assertEquals(listOf("signal", "eof", "close"), pty.calls)
        assertTrue(ended.isEmpty())
    }

    @Test
    fun endAllStopsCommandAndTerminalSessionsTogether() {
        sessions.open("alpine")
        openCommand()

        sessions.endAll()

        eventually { sessions.running.value.isEmpty() }
        assertTrue(ptys.all { it.eofSent })
    }

    @Test
    fun aCommandThatExitsOnItsOwnPostsTheEndedNotification() {
        openCommand()

        ptys.single().exit(1)

        eventually { sessions.running.value.isEmpty() }
        assertEquals(listOf(key to 1), ended)
    }

    @Test
    fun aCommandThatCannotStartIsReportedAndNothingRuns() {
        val result = sessions.open(key, factory(start = { null }))

        assertEquals(SessionManager.OpenResult.Failed("could not start a command session for web-server"), result)
        assertTrue(sessions.running.value.isEmpty())
    }

    @Test
    fun aTerminalAndACommandSessionShareOneContainerUnderDifferentKeys() {
        val terminal = sessions.open(container)
        openCommand()

        assertTrue(terminal is SessionManager.OpenResult.Opened)
        assertEquals(listOf(container, key), sessions.running.value)
        assertEquals(container, started.single()[2])
        assertEquals(2, ptys.size)
    }

    @Test
    fun aCommandSessionCannotTakeTheKeyOfARunningTerminal() {
        sessions.open(container)

        val result = sessions.open(container, factory())

        assertEquals(SessionManager.OpenResult.Failed("debian is already open as another kind of session"), result)
        assertTrue(started.isEmpty())
        assertEquals(listOf(container), sessions.running.value)
    }

    @Test
    fun aTerminalCannotTakeTheKeyOfARunningCommandSession() {
        openCommand()

        val result = sessions.open(key)

        assertEquals(SessionManager.OpenResult.Failed("web-server is already open as another kind of session"), result)
        assertEquals(1, ptys.size)
        assertEquals(listOf(key), sessions.running.value)
    }

    @Test
    fun anEndedTerminalStillOnItsScreenGivesItsKeyToACommandSession() {
        val terminal = (sessions.open(container) as SessionManager.OpenResult.Opened).session
        sessions.attach(terminal, ManagedSession.Screen {})
        ptys.single().exit(0)
        eventually { terminal.exit.value != null }

        val result = sessions.open(container, factory())

        assertTrue(result is SessionManager.OpenResult.Opened && result.session !== terminal)
        assertEquals(listOf(container), sessions.running.value)
    }

    @Test
    fun aRetainedEndedTerminalKeepsItsKeyFromACommandSession() {
        val terminal = (sessions.open(container) as SessionManager.OpenResult.Opened).session
        val screen = ManagedSession.Screen {}
        sessions.attach(terminal, screen)
        sessions.detach(terminal, screen, retainEnded = true)
        ptys.single().exit(0)
        eventually { terminal.exit.value != null }

        val result = sessions.open(container, factory())

        assertEquals(SessionManager.OpenResult.Failed("debian is already open as another kind of session"), result)
        assertTrue(started.isEmpty())
        assertEquals(1, ptys.size)
    }

    @Test
    fun aRunningKeyIsReturnedToAnotherFactoryOfTheSameKind() {
        val first = openCommand()

        val again = sessions.open(key, factory())

        assertSame(first, (again as SessionManager.OpenResult.Opened).session)
        assertEquals(1, started.size)
    }

    @Test
    fun endSendsSigtermToACommandThatIsProotsDirectChildAndDrainsWithoutAKill() {
        val proc = FakeProcFs(childrenFiles = false).apply {
            add(app, ppid = 1)
            add(proot, ppid = app)
            add(gateway, ppid = proot, tracer = proot, cmdline = listOf("httpd", "-f", "-p", "8080"))
        }

        val process = endCommand(proc, exitsOnSigtermOf = gateway)

        assertEquals(listOf(gateway), process.sigterm)
        assertEquals(listOf("signal", "close"), process.pty.calls)
        assertFalse(process.pty.killed)
        assertFalse(process.pty.forceKilled)
    }

    @Test
    fun endSendsSigtermToAShellThatStayedAndToItsCommand() {
        val proc = FakeProcFs(childrenFiles = false).apply {
            add(app, ppid = 1)
            add(proot, ppid = app)
            add(shell, ppid = proot, tracer = proot, cmdline = listOf("/bin/sh", "-l", "-c", command))
            add(gateway, ppid = shell, tracer = proot, cmdline = listOf("httpd", "-f", "-p", "8080"))
        }

        val process = endCommand(proc, exitsOnSigtermOf = shell)

        assertEquals(setOf(shell, gateway), process.sigterm.toSet())
        assertEquals(listOf("signal", "close"), process.pty.calls)
        assertFalse(process.pty.killed)
    }

    @Test
    fun aSinkThatThrowsLosesOnlyThatOutputAndTheSessionKeepsRunning() {
        val writes = AtomicInteger()
        sessions.open(key, factory(sink = { buf, off, len ->
            if (writes.getAndIncrement() == 0) throw IllegalStateException("log full")
            toReceived(buf, off, len)
        }))

        ptys.single().emit("first\n")
        ptys.single().emit("second\n")

        eventually { received.toString() == "second\n" }
        assertEquals(listOf(key), sessions.running.value)
        assertEquals(2, writes.get())
    }

    @Test
    fun aBlankCommandIsRejectedWhenTheFactoryIsBuilt() {
        val e = assertThrows(IllegalArgumentException::class.java) {
            CommandSessionFactory(host, container, " \t", toReceived)
        }

        assertEquals("command must not be blank", e.message)
    }

    @Test
    fun aMissingPrCliFailsTheOpenAndStartsNothing() {
        prCli.delete()

        val result = sessions.open(key, factory())

        assertEquals(
            SessionManager.OpenResult.Failed("could not start a command session for web-server: no pr-cli at $prCli"),
            result,
        )
        assertTrue(started.isEmpty())
        assertTrue(sessions.running.value.isEmpty())
        assertTrue(ended.isEmpty())
    }

    @Test
    fun aMissingBindSourceFailsTheOpenAndStartsNothing() {
        val missing = File(root, "absent")

        val result = sessions.open(key, factory(binds = binds + Bind(missing.path, "/mnt")))

        assertEquals(
            SessionManager.OpenResult.Failed("could not start a command session for web-server: no bind source $missing"),
            result,
        )
        assertTrue(started.isEmpty())
        assertTrue(sessions.running.value.isEmpty())
        assertTrue(ended.isEmpty())
    }
}
