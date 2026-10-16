package id.or.oo.pr.engine

import java.io.File

interface ProotHost {
    val prefixDir: File
    val homeDir: File
    val packageName: String
    val cacheDir: File

    /** DNS servers of the active network; pr-cli writes them into the container's resolv.conf. */
    val dnsServers: List<String> get() = emptyList()
}
