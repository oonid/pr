package id.or.oo.pr

import android.annotation.SuppressLint
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import id.or.oo.pr.session.isIgnoringBatteryOptimizations

@SuppressLint("BatteryLife")
@Composable
fun SessionBanners(app: App) {
    val context = LocalContext.current
    val keepAwake by app.sessions.keepCpuAwake.collectAsState()
    val backgroundError by app.sessions.backgroundError.collectAsState()
    val resumes = remember { mutableIntStateOf(0) }
    LifecycleResumeEffect(Unit) {
        resumes.intValue++
        onPauseOrDispose {}
    }
    val exempt = remember(resumes.intValue, keepAwake) { context.isIgnoringBatteryOptimizations() }

    Column {
        if (keepAwake && !exempt) {
            Banner(
                text = "Deep sleep will pause background sessions until battery optimization is turned off for this app.",
                action = "Allow",
                onAction = {
                    context.startActivity(
                        Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS)
                            .setData(Uri.parse("package:${context.packageName}"))
                    )
                },
            )
        }
        backgroundError?.let { reason ->
            Banner(text = "Background running unavailable: $reason", action = null, onAction = {})
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
