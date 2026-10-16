package id.or.oo.pr.engine

import org.junit.Assert.assertEquals
import org.junit.Test
import java.net.InetAddress

class NetworkDnsTest {

    private val v4 = InetAddress.getByName("192.168.100.1")
    private val v6 = InetAddress.getByName("2400:9800:2:2::245")

    @Test
    fun ipv6ServersAreDroppedWithoutAnIpv6DefaultRoute() {
        assertEquals(listOf("192.168.100.1"), NetworkDns.usableServers(listOf(v6, v4), hasIpv6DefaultRoute = false))
    }

    @Test
    fun ipv6ServersAreKeptWithAnIpv6DefaultRoute() {
        assertEquals(listOf(v6.hostAddress, "192.168.100.1"),
            NetworkDns.usableServers(listOf(v6, v4), hasIpv6DefaultRoute = true))
    }
}
