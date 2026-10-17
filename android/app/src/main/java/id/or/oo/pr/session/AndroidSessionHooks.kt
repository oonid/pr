package id.or.oo.pr.session

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.PowerManager
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat

class ServiceForegroundController(private val context: Context) : SessionManager.ForegroundController {
    override fun start() {
        ContextCompat.startForegroundService(context, Intent(context, SessionService::class.java))
    }
}

class NotificationEndedNotifier(private val context: Context) : SessionManager.EndedNotifier {
    override fun sessionEnded(alias: String, exitCode: Int?) {
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) return
        SessionNotifications.createChannels(context)
        NotificationManagerCompat.from(context)
            .notify("ended:$alias".hashCode(), SessionNotifications.ended(context, alias, exitCode))
    }
}

fun Context.isIgnoringBatteryOptimizations(): Boolean =
    getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(packageName)
