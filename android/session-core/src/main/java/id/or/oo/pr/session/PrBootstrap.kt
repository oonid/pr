package id.or.oo.pr.session

import android.content.Context
import android.content.SharedPreferences
import android.system.Os
import android.util.Log
import id.or.oo.pr.engine.ProotHost
import java.io.File
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
            createDirectories(host)
            links.ensure()

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

    private companion object {
        const val TAG = "PR"
        const val PREFS_NAME = "pr_prefs"
        const val KEY_INITIALIZED = "bootstrapped"
        const val KEY_VERSION = "bootstrap_version"
    }
}

internal fun createDirectories(host: ProotHost) {
    val dirs = listOf("tmp", "etc/pr", "var/lib/pr/installed-rootfs", "var/lib/pr/dlcache")
        .map { File(host.prefixDir, it) } + host.homeDir
    for (dir in dirs) {
        if (!dir.mkdirs() && !dir.isDirectory) throw IOException("cannot create $dir")
    }
}
