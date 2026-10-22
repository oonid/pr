package id.or.oo.pr.session

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationCompat

object SessionNotifications {
    const val RUNNING_ID = 1
    private const val CHANNEL_RUNNING = "sessions_running"
    private const val CHANNEL_ENDED = "sessions_ended"

    fun createChannels(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_RUNNING, "Running sessions", NotificationManager.IMPORTANCE_LOW)
        )
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_ENDED, "Session ended", NotificationManager.IMPORTANCE_DEFAULT)
        )
    }

    fun running(context: Context, aliases: List<String>, keepAwake: Boolean, exempt: Boolean): Notification {
        val title = runningTitle(aliases)
        val details = buildList {
            if (aliases.isNotEmpty()) add(aliases.joinToString(", "))
            if (keepAwake) add("CPU kept awake")
            if (keepAwake && !exempt) add("Battery saver may stop sessions; allow in the app")
        }.joinToString(" · ")
        return NotificationCompat.Builder(context, CHANNEL_RUNNING)
            .setSmallIcon(context.sessionHost.notificationIcon)
            .setContentTitle(title)
            .setContentText(details)
            .setStyle(NotificationCompat.BigTextStyle().bigText(details))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(openList(context))
            .addAction(0, "End all", serviceAction(context, SessionActions.endAll(context.packageName)))
            .addAction(
                0,
                if (keepAwake) "Let CPU sleep" else "Keep CPU awake",
                serviceAction(context, SessionActions.toggleWake(context.packageName)),
            )
            .build()
    }

    fun ended(context: Context, alias: String, exitCode: Int?): Notification {
        val code = exitCode?.toString() ?: "unknown"
        return NotificationCompat.Builder(context, CHANNEL_ENDED)
            .setSmallIcon(context.sessionHost.notificationIcon)
            .setContentTitle("$alias: session ended (exit $code)")
            .setContentIntent(openList(context))
            .setAutoCancel(true)
            .build()
    }

    private fun openList(context: Context): PendingIntent = PendingIntent.getActivity(
        context,
        0,
        context.sessionHost.openAppIntent()
            .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP),
        PendingIntent.FLAG_IMMUTABLE,
    )

    private fun serviceAction(context: Context, action: String): PendingIntent = PendingIntent.getService(
        context,
        action.hashCode(),
        Intent(context, SessionService::class.java).setAction(action),
        PendingIntent.FLAG_IMMUTABLE,
    )
}

internal fun runningTitle(aliases: List<String>): String = when (aliases.size) {
    0 -> "Stopping…"
    1 -> "1 session running"
    else -> "${aliases.size} sessions running"
}
