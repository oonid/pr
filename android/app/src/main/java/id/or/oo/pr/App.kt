package id.or.oo.pr

import android.app.Application
import android.content.Intent
import android.net.ConnectivityManager
import id.or.oo.pr.engine.NetworkDns
import id.or.oo.pr.engine.ProotHost
import id.or.oo.pr.session.NotificationEndedNotifier
import id.or.oo.pr.session.PrBootstrap
import id.or.oo.pr.session.PrCliCommand
import id.or.oo.pr.session.ProotSessionFactory
import id.or.oo.pr.session.ServiceForegroundController
import id.or.oo.pr.session.SessionHost
import id.or.oo.pr.session.SessionManager
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import java.io.File
import java.net.Inet6Address

class App : Application(), ProotHost, SessionHost {

    companion object {
        private const val BOOTSTRAP_VERSION = 8
    }

    override val prefixDir: File get() = File(filesDir, "usr")

    override val dnsServers: List<String>
        get() {
            val connectivity = getSystemService(ConnectivityManager::class.java) ?: return emptyList()
            val network = connectivity.activeNetwork ?: return emptyList()
            val link = connectivity.getLinkProperties(network) ?: return emptyList()
            val hasIpv6DefaultRoute = link.routes.any {
                it.destination.prefixLength == 0 && it.destination.address is Inet6Address
            }
            return NetworkDns.usableServers(link.dnsServers, hasIpv6DefaultRoute)
        }

    override val homeDir: File get() = File(filesDir, "home")

    @get:JvmName("getProotPackageName")
    override val packageName: String get() = super.getPackageName()
    
    @get:JvmName("getProotCacheDir")
    override val cacheDir: File get() = super.getCacheDir()

    override val sessions: SessionManager by lazy {
        SessionManager(
            ProotSessionFactory(this),
            ServiceForegroundController(this),
            NotificationEndedNotifier(this),
        )
    }

    override val notificationIcon: Int get() = R.drawable.ic_notification

    override fun openAppIntent(): Intent = Intent(this, MainActivity::class.java)

    val containers: InstalledContainers by lazy { InstalledContainers(prefixDir) }

    val operations: DistroOperations by lazy {
        DistroOperations(
            PrCliCommand(this),
            sessions,
            CoroutineScope(SupervisorJob() + Dispatchers.Default),
            containers::contains,
        )
    }

    override fun onCreate() {
        super.onCreate()
        PrBootstrap(this, this, BOOTSTRAP_VERSION).run()
    }
}
