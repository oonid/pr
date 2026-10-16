package id.or.oo.pr.engine

import java.net.Inet6Address
import java.net.InetAddress

object NetworkDns {
    /**
     * The network's DNS servers the container can actually reach. IPv6 servers are dropped when the
     * network has no IPv6 default route (seen on mobile data); glibc would otherwise wait out its
     * resolver timeout on them before every lookup reaches the public fallback.
     */
    fun usableServers(servers: List<InetAddress>, hasIpv6DefaultRoute: Boolean): List<String> =
        servers.filter { hasIpv6DefaultRoute || it !is Inet6Address }.mapNotNull { it.hostAddress }
}
