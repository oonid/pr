package id.or.oo.pr

import android.util.Log
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

class PrCliCommand(private val app: App) : DistroCommand {

    override suspend fun run(args: List<String>, onLine: (String) -> Unit): Int = withContext(Dispatchers.IO) {
        val binDir = File(app.prefixDir, "bin")

        val env = mapOf(
            "APP_PREFIX" to app.prefixDir.absolutePath,
            "APP_HOME" to app.homeDir.absolutePath,
            "APP_PACKAGE" to app.packageName,
            "PATH" to "${binDir.absolutePath}:/system/bin:/system/xbin",
            "PROOT_NO_SECCOMP" to "1",
            "PROOT_TMP_DIR" to app.cacheDir.absolutePath,
            "HOME" to app.homeDir.absolutePath,
            "TERM" to "xterm-256color",
            "TMPDIR" to app.cacheDir.absolutePath,
            "APP_DNS_SERVERS" to app.dnsServers.joinToString(","),
        )

        val prCli = File(binDir, "pr-cli").absolutePath
        val cmd = arrayOf(prCli, *args.toTypedArray())

        Log.d(TAG, "Running: ${cmd.joinToString(" ")}")
        val pb = ProcessBuilder(*cmd)
        pb.environment().putAll(env)
        pb.redirectErrorStream(true)

        val proc = pb.start()
        val reader = proc.inputStream.bufferedReader()
        var line: String? = reader.readLine()
        while (line != null) {
            val cleaned = line
                .replace(ANSI_CSI, "")
                .replace(ANSI_OSC, "")
                .replace("\r", "")
                .trim()
            if (cleaned.isNotEmpty()) {
                Log.d(TAG, cleaned)
                onLine(cleaned)
            }
            line = reader.readLine()
        }
        val exitCode = proc.waitFor()
        Log.d(TAG, "Exit code: $exitCode")
        exitCode
    }

    private companion object {
        const val TAG = "PR"
        val ANSI_CSI = Regex("\\x1B\\[[0-9;]*[mGKHJ]")
        val ANSI_OSC = Regex("\\x1B\\]\\d+;.*?\\x07")
    }
}
