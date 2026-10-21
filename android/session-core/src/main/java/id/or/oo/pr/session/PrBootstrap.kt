package id.or.oo.pr.session

import android.content.Context
import android.content.SharedPreferences
import android.system.Os
import android.util.Log
import id.or.oo.pr.engine.ProotHost
import java.io.File
import java.io.FileOutputStream
import java.io.IOException

class PrBootstrap(
    private val context: Context,
    private val host: ProotHost,
    private val version: Int,
) {

    fun run() {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val currentVersion = prefs.getInt(KEY_VERSION, 0)
        val links = NativeLinks(host.prefixDir, File(context.applicationInfo.nativeLibraryDir), Os::symlink)

        if (currentVersion < version) {
            bootstrap(prefs, links)
        } else {
            links.ensure()
            Log.i(TAG, "Already bootstrapped (version $currentVersion)")
        }
    }

    private fun bootstrap(prefs: SharedPreferences, links: NativeLinks) {
        Log.i(TAG, "Running bootstrap...")
        val t0 = System.currentTimeMillis()

        try {
            val binDir = File(host.prefixDir, "bin")

            binDir.mkdirs()
            host.homeDir.mkdirs()
            File(host.prefixDir, "tmp").mkdirs()

            links.ensure()
            Log.d(TAG, "Creating busybox applet symlinks...")
            val applets = context.assets.open("bin/busybox.applets").bufferedReader().readLines()
            Log.d(TAG, "Created ${links.applets(applets)} busybox applet symlinks")

            copyAssetFile("scripts/bootstrap.sh", File(binDir, "bootstrap.sh"))

            executeBootstrap()

            prefs.edit()
                .putInt(KEY_VERSION, version)
                .putBoolean(KEY_INITIALIZED, true)
                .apply()

            val elapsed = System.currentTimeMillis() - t0
            Log.i(TAG, "Bootstrap complete (${elapsed}ms)")
        } catch (e: Exception) {
            Log.e(TAG, "Bootstrap failed", e)
        }
    }

    private fun copyAssetFile(assetPath: String, dest: File) {
        context.assets.open(assetPath).use { input ->
            FileOutputStream(dest).use { output ->
                input.copyTo(output)
            }
        }
        dest.setExecutable(true, false)
        dest.setReadable(true, false)
        Log.d(TAG, "Installed: ${dest.name}")
    }

    private fun executeBootstrap() {
        val bootstrapScript = File(host.prefixDir, "bin/bootstrap.sh")
        if (!bootstrapScript.exists()) {
            Log.e(TAG, "bootstrap.sh not found")
            return
        }

        val cmd = arrayOf(
            "/system/bin/sh",
            bootstrapScript.absolutePath,
        )

        val pb = ProcessBuilder(*cmd)
        pb.environment().putAll(bootstrapEnv(host))
        pb.redirectErrorStream(true)

        Log.d(TAG, "Executing: ${cmd.joinToString(" ")}")

        val proc = pb.start()
        val output = proc.inputStream.bufferedReader().readText()
        val exitCode = proc.waitFor()

        if (output.isNotBlank()) {
            for (line in output.lines()) {
                Log.d(TAG, "[bootstrap] $line")
            }
        }

        if (exitCode != 0) {
            throw IOException("bootstrap.sh failed with exit code $exitCode")
        }
    }

    private companion object {
        const val TAG = "PR"
        const val PREFS_NAME = "pr_prefs"
        const val KEY_INITIALIZED = "bootstrapped"
        const val KEY_VERSION = "bootstrap_version"
    }
}

internal fun bootstrapEnv(host: ProotHost): Map<String, String> {
    val binDir = File(host.prefixDir, "bin")
    return mapOf(
        "APP_PREFIX" to host.prefixDir.absolutePath,
        "APP_HOME" to host.homeDir.absolutePath,
        "APP_PACKAGE" to host.packageName,
        "PATH" to "/system/bin:/system/xbin:${binDir.absolutePath}",
        "PROOT_NO_SECCOMP" to "1",
        "HOME" to host.homeDir.absolutePath,
    )
}
