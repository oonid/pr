package id.or.oo.pr.session

import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.ServiceCompat
import id.or.oo.pr.App
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.launch

class SessionService : Service() {

    companion object {
        const val ACTION_END_ALL = "id.or.oo.pr.action.END_ALL"
        const val ACTION_TOGGLE_WAKE = "id.or.oo.pr.action.TOGGLE_WAKE"
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var wakeLock: PowerManager.WakeLock? = null
    private var foreground = false
    private var lastStartId = 0
    private val sessions: SessionManager get() = (application as App).sessions

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        SessionNotifications.createChannels(this)
        scope.launch {
            combine(sessions.running, sessions.keepCpuAwake) { running, awake -> running to awake }
                .collect { (running, awake) ->
                    setWakeLock(awake && running.isNotEmpty())
                    if (running.isNotEmpty()) {
                        getSystemService(NotificationManager::class.java)
                            .notify(SessionNotifications.RUNNING_ID, notification(running, awake))
                    } else if (foreground) {
                        stopRunning(lastStartId)
                    }
                }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        lastStartId = startId
        val running = sessions.running.value
        ServiceCompat.startForeground(
            this,
            SessionNotifications.RUNNING_ID,
            notification(running, sessions.keepCpuAwake.value),
            if (Build.VERSION.SDK_INT >= 34) ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else 0,
        )
        foreground = true
        when (intent?.action) {
            ACTION_END_ALL -> sessions.endAll()
            ACTION_TOGGLE_WAKE -> sessions.setKeepCpuAwake(!sessions.keepCpuAwake.value)
        }
        if (sessions.running.value.isEmpty()) stopRunning(startId)
        return START_NOT_STICKY
    }

    private fun stopRunning(startId: Int) {
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        foreground = false
        stopSelfResult(startId)
    }

    override fun onDestroy() {
        scope.cancel()
        setWakeLock(false)
        super.onDestroy()
    }

    private fun notification(running: List<String>, awake: Boolean) =
        SessionNotifications.running(this, running, awake, isIgnoringBatteryOptimizations())

    private fun setWakeLock(on: Boolean) {
        val held = wakeLock
        if (on && held == null) {
            wakeLock = getSystemService(PowerManager::class.java)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "pr:sessions")
                .apply { acquire() }
        } else if (!on && held != null) {
            if (held.isHeld) held.release()
            wakeLock = null
        }
    }
}
