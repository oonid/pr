package id.or.oo.pr.session

import android.os.Process
import android.util.Log
import id.or.oo.pr.engine.ProotHost
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

class PrCliCommand(
    private val host: ProotHost,
    private val runner: CommandRunner = ProcessRunner(Process.myPid(), Process::sendSignal),
) : DistroCommand {

    override suspend fun run(args: List<String>, onLine: (String) -> Unit): Int {
        val binDir = File(host.prefixDir, "bin")

        val env = withContext(Dispatchers.IO) {
            mapOf(
                "APP_PREFIX" to host.prefixDir.absolutePath,
                "APP_HOME" to host.homeDir.absolutePath,
                "APP_PACKAGE" to host.packageName,
                "PATH" to "${binDir.absolutePath}:/system/bin:/system/xbin",
                "PROOT_NO_SECCOMP" to "1",
                "PROOT_TMP_DIR" to host.cacheDir.absolutePath,
                "HOME" to host.homeDir.absolutePath,
                "TERM" to "xterm-256color",
                "TMPDIR" to host.cacheDir.absolutePath,
                "APP_DNS_SERVERS" to host.dnsServers.joinToString(","),
            )
        }

        val prCli = File(binDir, "pr-cli").absolutePath
        val cmd = listOf(prCli) + args

        Log.d(TAG, "Running: ${cmd.joinToString(" ")}")
        val exitCode = runner.run(cmd, env) { line ->
            val cleaned = line
                .replace(ANSI_CSI, "")
                .replace(ANSI_OSC, "")
                .replace("\r", "")
                .trim()
            if (cleaned.isNotEmpty()) {
                Log.d(TAG, cleaned)
                onLine(cleaned)
            }
        }
        Log.d(TAG, "Exit code: $exitCode")
        return exitCode
    }

    private companion object {
        const val TAG = "PR"
        val ANSI_CSI = Regex("\\x1B\\[[0-9;]*[mGKHJ]")
        val ANSI_OSC = Regex("\\x1B\\]\\d+;.*?\\x07")
    }
}
