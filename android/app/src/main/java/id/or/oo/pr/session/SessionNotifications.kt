package id.or.oo.pr.session

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationCompat
import id.or.oo.pr.MainActivity
import id.or.oo.pr.R

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
        val title = if (aliases.size == 1) "1 session running" else "${aliases.size} sessions running"
        val details = buildList {
            add(aliases.joinToString(", "))
            if (keepAwake) add("CPU kept awake")
            if (keepAwake && !exempt) add("Battery saver may stop sessions; allow in the app")
        }.joinToString(" · ")
        return NotificationCompat.Builder(context, CHANNEL_RUNNING)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(details)
            .setStyle(NotificationCompat.BigTextStyle().bigText(details))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(openList(context))
            .addAction(0, "End all", serviceAction(context, SessionService.ACTION_END_ALL))
            .addAction(
                0,
                if (keepAwake) "Let CPU sleep" else "Keep CPU awake",
                serviceAction(context, SessionService.ACTION_TOGGLE_WAKE),
            )
            .build()
    }

    fun ended(context: Context, alias: String, exitCode: Int?): Notification {
        val code = exitCode?.toString() ?: "unknown"
        return NotificationCompat.Builder(context, CHANNEL_ENDED)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("$alias: session ended (exit $code)")
            .setContentIntent(openList(context))
            .setAutoCancel(true)
            .build()
    }

    private fun openList(context: Context): PendingIntent = PendingIntent.getActivity(
        context,
        0,
        Intent(context, MainActivity::class.java)
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
