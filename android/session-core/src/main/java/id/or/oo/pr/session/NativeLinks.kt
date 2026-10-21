package id.or.oo.pr.session

import android.util.Log
import java.io.File
import java.nio.file.Files

internal class NativeLinks(
    private val prefixDir: File,
    private val nativeLibDir: File,
    private val symlink: (target: String, link: String) -> Unit,
) {
    private val binDir = File(prefixDir, "bin")

    fun ensure() {
        binDir.mkdirs()

        for (name in STALE) {
            val f = File(binDir, name)
            if (f.exists() || Files.isSymbolicLink(f.toPath())) {
                f.delete()
                Log.d(TAG, "Removed stale: $name")
            }
        }
        val scriptsDir = File(prefixDir, "scripts")
        if (scriptsDir.exists()) {
            scriptsDir.deleteRecursively()
            Log.d(TAG, "Removed stale scripts dir")
        }

        for ((name, lib) in LINKS) {
            val link = File(binDir, name)
            val target = File(nativeLibDir, lib)
            if (!target.exists()) {
                Log.w(TAG, "$lib not found in $nativeLibDir")
                continue
            }

            if (link.exists()) {
                if (link.canonicalPath == target.canonicalPath) continue
                link.delete()
            } else if (Files.isSymbolicLink(link.toPath())) {
                link.delete()
            }

            symlink(target.absolutePath, link.absolutePath)
            Log.d(TAG, "Symlink: $name -> ${target.absolutePath}")
        }
    }

    fun applets(names: List<String>): Int {
        var count = 0
        for (applet in names.filter { it.isNotBlank() }) {
            val link = File(binDir, applet)
            if (link.exists()) continue
            try {
                symlink("busybox", link.absolutePath)
                count++
            } catch (e: Exception) {
                Log.w(TAG, "Failed to create symlink for $applet: ${e.message}")
            }
        }
        return count
    }

    private companion object {
        const val TAG = "PR"
        val LINKS = mapOf(
            "busybox" to "libbusybox.so",
            "proot" to "libproot.so",
            "pr-cli" to "libpr-cli.so",
        )
        val STALE = listOf("bash", "pr-test", "proot-distro")
    }
}
