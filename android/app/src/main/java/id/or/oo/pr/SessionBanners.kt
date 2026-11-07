package id.or.oo.pr

import android.Manifest
import android.app.Activity
import android.annotation.SuppressLint
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LifecycleResumeEffect
import id.or.oo.pr.session.SessionService
import id.or.oo.pr.session.isIgnoringBatteryOptimizations

@SuppressLint("BatteryLife")
@Composable
fun SessionBanners(app: App) {
    val context = LocalContext.current
    val running by app.sessions.running.collectAsState()
    val keepAwake by app.sessions.keepCpuAwake.collectAsState()
    val backgroundError by app.sessions.backgroundError.collectAsState()
    val tasks by app.sessions.tasks.collectAsState()
    val busy = running.isNotEmpty() || tasks.isNotEmpty()
    val notificationLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted && (app.sessions.running.value.isNotEmpty() || app.sessions.tasks.value.isNotEmpty())) {
            ContextCompat.startForegroundService(context, Intent(context, SessionService::class.java))
        }
    }
    val resumes = remember { mutableIntStateOf(0) }
    val notificationsGranted = remember { booleanArrayOf(context.canPostNotifications()) }
    val exemptGranted = remember { booleanArrayOf(context.isIgnoringBatteryOptimizations()) }
    LifecycleResumeEffect(Unit) {
        resumes.intValue++
        val granted = context.canPostNotifications()
        val nowExempt = context.isIgnoringBatteryOptimizations()
        val newlyGranted = (granted && !notificationsGranted[0]) || (nowExempt && !exemptGranted[0])
        if (newlyGranted && (app.sessions.running.value.isNotEmpty() || app.sessions.tasks.value.isNotEmpty())) {
            ContextCompat.startForegroundService(context, Intent(context, SessionService::class.java))
        }
        notificationsGranted[0] = granted
        exemptGranted[0] = nowExempt
        onPauseOrDispose {}
    }
    val exempt = remember(resumes.intValue, keepAwake) { context.isIgnoringBatteryOptimizations() }
    val notificationsOn = remember(resumes.intValue, busy) { context.canPostNotifications() }

    Column {
        if (running.isNotEmpty() && backgroundError == null) {
            Card(modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp)) {
                Row(
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        "Keep CPU awake",
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = Modifier.weight(1f),
                    )
                    Switch(checked = keepAwake, onCheckedChange = { app.sessions.setKeepCpuAwake(it) })
                }
            }
        }
        if (keepAwake && !exempt) {
            Banner(
                text = "Some phones' battery savers can still stop background sessions. Allow PR to run unrestricted.",
                action = "Allow",
                onAction = {
                    try {
                        context.startActivity(
                            Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS)
                                .setData(Uri.parse("package:${context.packageName}"))
                        )
                    } catch (e: ActivityNotFoundException) {
                        try {
                            context.startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))
                        } catch (missing: ActivityNotFoundException) {
                        }
                    }
                },
            )
        }
        if (showNotificationBanner(notificationsOn, busy)) {
            Banner(
                text = "Notifications are off, so a background session has no notification to end it or keep the CPU awake.",
                action = "Allow",
                onAction = { context.allowNotifications { notificationLauncher.launch(Manifest.permission.POST_NOTIFICATIONS) } },
            )
        }
        backgroundError?.let { reason ->
            Banner(
                text = "Background running unavailable: $reason",
                action = "Retry",
                onAction = { app.sessions.retryBackground() },
            )
        }
    }
}

private fun Context.canPostNotifications() = NotificationManagerCompat.from(this).areNotificationsEnabled()

private fun Context.allowNotifications(request: () -> Unit) {
    if (Build.VERSION.SDK_INT >= 33) {
        val prefs = getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val rationale = (this as? Activity)
            ?.shouldShowRequestPermissionRationale(Manifest.permission.POST_NOTIFICATIONS) ?: false
        if (notificationFix(prefs.getBoolean(NOTIFICATIONS_ASKED, false), rationale) == NotificationFix.REQUEST) {
            prefs.edit().putBoolean(NOTIFICATIONS_ASKED, true).apply()
            request()
            return
        }
    }
    try {
        startActivity(
            Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, packageName)
        )
    } catch (e: ActivityNotFoundException) {
        try {
            startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).setData(Uri.parse("package:$packageName")))
        } catch (missing: ActivityNotFoundException) {
        }
    }
}

@Composable
private fun Banner(text: String, action: String?, onAction: () -> Unit) {
    Card(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.errorContainer),
    ) {
        Column(modifier = Modifier.padding(12.dp)) {
            Text(text, style = MaterialTheme.typography.bodyMedium)
            if (action != null) TextButton(onClick = onAction) { Text(action) }
        }
    }
}
