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
        val applets = binDir.listFiles()?.count { f ->
            val path = f.toPath()
            Files.isSymbolicLink(path) &&
                runCatching { Files.readSymbolicLink(path).toString() }.getOrNull() == "busybox" &&
                f.delete()
        } ?: 0
        if (applets > 0) Log.d(TAG, "Removed $applets busybox applet links")
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

    private companion object {
        const val TAG = "PR"
        val LINKS = mapOf(
            "proot" to "libproot.so",
            "pr-cli" to "libpr-cli.so",
        )
        val STALE = listOf("bash", "pr-test", "proot-distro", "busybox", "bootstrap.sh")
    }
}
