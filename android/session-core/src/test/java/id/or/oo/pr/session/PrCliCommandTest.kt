package id.or.oo.pr.session

import id.or.oo.pr.engine.ProotHost
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Test
import java.io.File

class PrCliCommandTest {

    private val host = object : ProotHost {
        override val prefixDir = File("/data/data/org.example.gateway/files/usr")
        override val homeDir = File("/data/data/org.example.gateway/files/home")
        override val packageName = "org.example.gateway"
        override val cacheDir = File("/data/data/org.example.gateway/cache")
        override val dnsServers = listOf("192.0.2.53", "2001:db8::53")
    }

    private class FakeRunner(private val lines: List<String>, private val exitCode: Int) : CommandRunner {
        var cmd: List<String>? = null
        var env: Map<String, String>? = null

        override suspend fun run(cmd: List<String>, env: Map<String, String>, onLine: (String) -> Unit): Int {
            this.cmd = cmd
            this.env = env
            lines.forEach(onLine)
            return exitCode
        }
    }

    @Test
    fun runsPrCliFromTheHostPrefixWithTheHostEnvironment() = runBlocking {
        val runner = FakeRunner(emptyList(), 0)

        PrCliCommand(host, runner).run(listOf("install", "alpine")) { }

        assertEquals(listOf("/data/data/org.example.gateway/files/usr/bin/pr-cli", "install", "alpine"), runner.cmd)
        assertEquals(
            mapOf(
                "APP_PREFIX" to "/data/data/org.example.gateway/files/usr",
                "APP_HOME" to "/data/data/org.example.gateway/files/home",
                "APP_PACKAGE" to "org.example.gateway",
                "PATH" to "/data/data/org.example.gateway/files/usr/bin:/system/bin:/system/xbin",
                "PROOT_NO_SECCOMP" to "1",
                "PROOT_TMP_DIR" to "/data/data/org.example.gateway/cache",
                "HOME" to "/data/data/org.example.gateway/files/home",
                "TERM" to "xterm-256color",
                "TMPDIR" to "/data/data/org.example.gateway/cache",
                "APP_DNS_SERVERS" to "192.0.2.53,2001:db8::53",
            ),
            runner.env,
        )
    }

    @Test
    fun stripsEscapesAndCarriageReturnsAndDropsBlankLines() = runBlocking {
        val runner = FakeRunner(
            listOf("\u001B[1;32mInstalled\u001B[0m\r", "   ", "\u001B]0;pr-cli\u0007Done", ""),
            0,
        )
        val seen = mutableListOf<String>()

        PrCliCommand(host, runner).run(listOf("install", "alpine")) { seen += it }

        assertEquals(listOf("Installed", "Done"), seen)
    }

    @Test
    fun returnsTheRunnerExitCode() = runBlocking {
        assertEquals(3, PrCliCommand(host, FakeRunner(emptyList(), 3)).run(listOf("remove", "alpine")) { })
    }
}
