package id.or.oo.pr

import android.app.Application
import android.content.Context
import android.content.SharedPreferences
import android.net.ConnectivityManager
import android.system.Os
import android.util.Log
import id.or.oo.pr.engine.ProotHost
import java.io.File
import java.io.FileOutputStream
import java.io.IOException

class App : Application(), ProotHost {

    companion object {
        private const val TAG = "PR"
        private const val PREFS_NAME = "pr_prefs"
        private const val KEY_INITIALIZED = "bootstrapped"
        private const val KEY_VERSION = "bootstrap_version"

        private const val BOOTSTRAP_VERSION = 8
    }

    override val prefixDir: File get() = File(filesDir, "usr")

    override val dnsServers: List<String>
        get() {
            val connectivity = getSystemService(ConnectivityManager::class.java) ?: return emptyList()
            val network = connectivity.activeNetwork ?: return emptyList()
            return connectivity.getLinkProperties(network)?.dnsServers?.mapNotNull { it.hostAddress }.orEmpty()
        }

    override val homeDir: File get() = File(filesDir, "home")

    @get:JvmName("getProotPackageName")
    override val packageName: String get() = super.getPackageName()
    
    @get:JvmName("getProotCacheDir")
    override val cacheDir: File get() = super.getCacheDir()



    val nativeLibDir: File
        get() = File(applicationInfo.nativeLibraryDir)

    override fun onCreate() {
        super.onCreate()
        val prefs = getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val currentVersion = prefs.getInt(KEY_VERSION, 0)

        if (currentVersion < BOOTSTRAP_VERSION) {
            bootstrap(prefs)
        } else {
            ensureNativeLibSymlinks()
            Log.i(TAG, "Already bootstrapped (version $currentVersion)")
        }
    }

    private fun bootstrap(prefs: SharedPreferences) {
        Log.i(TAG, "Running bootstrap...")
        val t0 = System.currentTimeMillis()

        try {
            val binDir = File(prefixDir, "bin")

            binDir.mkdirs()
            homeDir.mkdirs()
            File(prefixDir, "tmp").mkdirs()

            ensureNativeLibSymlinks()
            createBusyboxSymlinks(binDir)

            copyAssetFile("scripts/bootstrap.sh", File(binDir, "bootstrap.sh"))

            executeBootstrap()

            prefs.edit()
                .putInt(KEY_VERSION, BOOTSTRAP_VERSION)
                .putBoolean(KEY_INITIALIZED, true)
                .apply()

            val elapsed = System.currentTimeMillis() - t0
            Log.i(TAG, "Bootstrap complete (${elapsed}ms)")
        } catch (e: Exception) {
            Log.e(TAG, "Bootstrap failed", e)
        }
    }

    private fun ensureNativeLibSymlinks() {
        val binDir = File(prefixDir, "bin")
        binDir.mkdirs()

        val links = mapOf(
            "busybox" to "libbusybox.so",
            "proot" to "libproot.so",
            "pr-cli" to "libpr-cli.so",
        )

        val staleNames = listOf("bash", "pr-test", "proot-distro")
        for (name in staleNames) {
            val f = File(binDir, name)
            if (f.exists() || java.nio.file.Files.isSymbolicLink(f.toPath())) {
                f.delete()
                Log.d(TAG, "Removed stale: $name")
            }
        }
        val scriptsDir = File(prefixDir, "scripts")
        if (scriptsDir.exists()) {
            scriptsDir.deleteRecursively()
            Log.d(TAG, "Removed stale scripts dir")
        }

        for ((name, lib) in links) {
            val link = File(binDir, name)
            val target = File(nativeLibDir, lib)
            if (!target.exists()) {
                Log.w(TAG, "$lib not found in $nativeLibDir")
                continue
            }

            if (link.exists()) {
                val currentTarget = link.canonicalPath
                if (currentTarget == target.canonicalPath) continue
                link.delete()
            } else if (java.nio.file.Files.isSymbolicLink(link.toPath())) {
                link.delete()
            }

            Os.symlink(target.absolutePath, link.absolutePath)
            Log.d(TAG, "Symlink: $name -> ${target.absolutePath}")
        }
    }

    private fun createBusyboxSymlinks(binDir: File) {
        Log.d(TAG, "Creating busybox applet symlinks...")

        val applets = assets.open("bin/busybox.applets").bufferedReader().readLines()
            .filter { it.isNotBlank() }
        var count = 0
        for (applet in applets) {
            val link = File(binDir, applet)
            if (link.exists()) continue
            try {
                Os.symlink("busybox", link.absolutePath)
                count++
            } catch (e: Exception) {
                Log.w(TAG, "Failed to create symlink for $applet: ${e.message}")
            }
        }

        Log.d(TAG, "Created $count busybox applet symlinks")
    }

    private fun copyAssetFile(assetPath: String, dest: File) {
        assets.open(assetPath).use { input ->
            FileOutputStream(dest).use { output ->
                input.copyTo(output)
            }
        }
        dest.setExecutable(true, false)
        dest.setReadable(true, false)
        Log.d(TAG, "Installed: ${dest.name}")
    }

    private fun executeBootstrap() {
        val bootstrapScript = File(prefixDir, "bin/bootstrap.sh")
        if (!bootstrapScript.exists()) {
            Log.e(TAG, "bootstrap.sh not found")
            return
        }

        val binDir = File(prefixDir, "bin")
        val env = mapOf(
            "APP_PREFIX" to prefixDir.absolutePath,
            "APP_HOME" to homeDir.absolutePath,
            "APP_PACKAGE" to packageName,
            "PATH" to "/system/bin:/system/xbin:${binDir.absolutePath}",
            "PROOT_NO_SECCOMP" to "1",
            "HOME" to homeDir.absolutePath,
        )

        val cmd = arrayOf(
            "/system/bin/sh",
            bootstrapScript.absolutePath,
        )

        val pb = ProcessBuilder(*cmd)
        pb.environment().putAll(env)
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
}
