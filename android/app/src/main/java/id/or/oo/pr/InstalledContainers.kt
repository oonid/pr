package id.or.oo.pr

import java.io.File

class InstalledContainers(prefixDir: File) {

    private val legacyRootfsDir = File(prefixDir, "var/lib/pr/installed-rootfs")
    private val ociContainersDir = File(prefixDir, "var/lib/pr/containers")

    fun aliases(): Set<String> {
        val legacy = legacyRootfsDir.listFiles().orEmpty().filter { it.isDirectory }.map { it.name }
        val oci = ociContainersDir.listFiles().orEmpty().map { it.name }.filter(::isOci)
        return (legacy + oci).toSet()
    }

    fun contains(alias: String): Boolean =
        alias.isNotEmpty() && (File(legacyRootfsDir, alias).isDirectory || isOci(alias))

    private fun isOci(alias: String): Boolean = File(ociContainersDir, "$alias/rootfs").isDirectory
}
