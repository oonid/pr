package id.or.oo.pr

import java.io.File
import java.nio.file.Files
import java.nio.file.LinkOption

class InstalledContainers(prefixDir: File) {

    private val legacyRootfsDir = File(prefixDir, "var/lib/pr/installed-rootfs")
    private val ociContainersDir = File(prefixDir, "var/lib/pr/containers")

    fun aliases(): Set<String> {
        val legacy = legacyRootfsDir.listFiles().orEmpty().map { it.name }.filter(::isLegacy)
        val oci = ociContainersDir.listFiles().orEmpty().map { it.name }.filter(::isOci)
        return (legacy + oci).toSet()
    }

    fun unfinished(): Set<String> =
        listOf(legacyRootfsDir, ociContainersDir).flatMap { parent ->
            parent.list().orEmpty().mapNotNull { name ->
                name.takeIf { it.startsWith(".") && it.endsWith(INSTALL_MARKER_SUFFIX) }
                    ?.drop(1)?.dropLast(INSTALL_MARKER_SUFFIX.length)
                    ?.takeIf(DistroOperations::isValidOverrideAlias)
            }
        }.toSet()

    fun contains(alias: String): Boolean = alias.isNotEmpty() && (isLegacy(alias) || isOci(alias))

    fun isUnfinished(alias: String): Boolean =
        alias.isNotEmpty() && listOf(legacyRootfsDir, ociContainersDir).any { unfinished(it, alias) }

    private fun isLegacy(alias: String): Boolean =
        File(legacyRootfsDir, alias).isDirectory && !unfinished(legacyRootfsDir, alias)

    private fun isOci(alias: String): Boolean =
        File(ociContainersDir, "$alias/rootfs").isDirectory && !unfinished(ociContainersDir, alias)

    private fun unfinished(parent: File, alias: String): Boolean =
        Files.exists(File(parent, ".$alias$INSTALL_MARKER_SUFFIX").toPath(), LinkOption.NOFOLLOW_LINKS)

    private companion object {
        const val INSTALL_MARKER_SUFFIX = ".pr-cli-installing"
    }
}
