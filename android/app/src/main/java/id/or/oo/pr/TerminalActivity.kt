package id.or.oo.pr

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import androidx.activity.ComponentActivity
import androidx.activity.OnBackPressedCallback
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import id.or.oo.pr.session.ManagedSession
import id.or.oo.pr.session.SessionManager
import id.or.oo.pr.session.SessionService
import org.connectbot.terminal.SelectionController
import org.connectbot.terminal.Terminal

class TerminalActivity : ComponentActivity() {

    private var session: ManagedSession? = null
    private val screen = ManagedSession.Screen { finish() }
    private var initialImeRequested = false
    private var initialImeShown = false
    private var backPressStarted = false
    private var selectionController: SelectionController? = null
    private val showLeavePrompt = mutableStateOf(false)

    private val notificationPermission =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            if (granted && (application as App).sessions.running.value.isNotEmpty()) {
                ContextCompat.startForegroundService(this, Intent(this, SessionService::class.java))
            }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val alias = intent.getStringExtra("distro") ?: run {
            finish()
            return
        }
        val sessions = (application as App).sessions

        when (val result = sessions.open(alias)) {
            is SessionManager.OpenResult.Failed -> {
                setContent { StartFailed(alias, result.reason) }
                onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
                    override fun handleOnBackPressed() = finish()
                })
                return
            }
            is SessionManager.OpenResult.Opened -> {
                session = result.session
                sessions.attach(result.session, screen)
            }
        }
        val current = session!!
        requestNotificationPermissionOnce()

        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                if (current.exit.value != null) finish() else showLeavePrompt.value = true
            }
        })

        setContent {
            val exit by current.exit.collectAsState()
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize(), color = Color(0xFF1a1a2e)) {
                    Box(modifier = Modifier.fillMaxSize()) {
                        Terminal(
                            terminalEmulator = current.emulator!!,
                            modifier = Modifier.fillMaxSize().systemBarsPadding(),
                            initialFontSize = 12.sp,
                            backgroundColor = Color(0xFF1a1a2e),
                            foregroundColor = Color.White,
                            keyboardEnabled = true,
                            showSoftKeyboard = true,
                            // The library moves focus to its Compose node first; re-target the IME after it.
                            onTerminalTap = { window.decorView.post { showKeyboard() } },
                            onSelectionControllerAvailable = { selectionController = it },
                        )
                        exit?.let { ended ->
                            Text(
                                text = "Session ended (exit ${ended.code ?: "unknown"})",
                                color = Color.White,
                                modifier = Modifier
                                    .align(Alignment.BottomCenter)
                                    .fillMaxWidth()
                                    .background(Color(0xCC000000))
                                    .navigationBarsPadding()
                                    .padding(12.dp),
                            )
                        }
                    }
                    if (showLeavePrompt.value) {
                        LeavePrompt(
                            onEnd = {
                                showLeavePrompt.value = false
                                sessions.end(alias)
                                finish()
                            },
                            onKeep = {
                                showLeavePrompt.value = false
                                finish()
                            },
                            onCancel = { showLeavePrompt.value = false },
                        )
                    }
                }
            }
        }
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (event.keyCode != KeyEvent.KEYCODE_BACK) return super.dispatchKeyEvent(event)
        when (event.action) {
            KeyEvent.ACTION_DOWN -> if (event.repeatCount == 0) backPressStarted = true
            KeyEvent.ACTION_UP -> {
                if (backPressStarted && !event.isCanceled) {
                    val selection = selectionController
                    if (selection != null && selection.isSelectionActive) selection.clearSelection()
                    else onBackPressedDispatcher.onBackPressed()
                }
                backPressStarted = false
            }
        }
        return true
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus && !initialImeRequested && session != null) {
            // Once per screen: with a hardware keyboard the IME never shows, and later focus gains
            // (returning from another app) must not force it open.
            initialImeRequested = true
            ensureInitialKeyboard(attempt = 0)
        }
    }

    // The terminal library asks for the IME once, 100 ms after composition. If the activity's open
    // transition has not finished, WindowManager drops that request (ImeTracker:
    // PHASE_WM_ABORT_SHOW_IME_POST_LAYOUT) and nothing asks again, so the keyboard never appears.
    // Re-ask from the focused window until the IME is actually visible once.
    private fun ensureInitialKeyboard(attempt: Int) {
        window.decorView.postDelayed({
            if (isFinishing || initialImeShown) return@postDelayed
            val imeVisible = ViewCompat.getRootWindowInsets(window.decorView)
                ?.isVisible(WindowInsetsCompat.Type.ime()) == true
            if (imeVisible) {
                initialImeShown = true
            } else if (attempt < 5) {
                showKeyboard()
                ensureInitialKeyboard(attempt + 1)
            }
        }, 300)
    }

    // The IME attaches to the library's input view (the one text editor in the window), not to
    // whatever Compose node holds focus.
    private fun showKeyboard() {
        val editor = findTextEditor(window.decorView) ?: return
        editor.requestFocus()
        WindowCompat.getInsetsController(window, editor).show(WindowInsetsCompat.Type.ime())
    }

    private fun findTextEditor(view: View): View? {
        if (view.onCheckIsTextEditor()) return view
        if (view is ViewGroup) {
            for (i in 0 until view.childCount) {
                findTextEditor(view.getChildAt(i))?.let { return it }
            }
        }
        return null
    }

    private fun requestNotificationPermissionOnce() {
        if (Build.VERSION.SDK_INT < 33) return
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) ==
            PackageManager.PERMISSION_GRANTED
        ) return
        val prefs = getSharedPreferences("pr_prefs", Context.MODE_PRIVATE)
        if (prefs.getBoolean("notifications_asked", false)) return
        prefs.edit().putBoolean("notifications_asked", true).apply()
        notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
    }

    override fun onDestroy() {
        session?.let { (application as App).sessions.detach(it, screen) }
        super.onDestroy()
    }
}

@androidx.compose.runtime.Composable
private fun LeavePrompt(onEnd: () -> Unit, onKeep: () -> Unit, onCancel: () -> Unit) {
    AlertDialog(
        onDismissRequest = onCancel,
        title = { Text("Leave the terminal?") },
        text = { Text("Keep running leaves the session working in the background. End session stops it and everything started in it.") },
        confirmButton = { TextButton(onClick = onKeep) { Text("Keep running") } },
        dismissButton = {
            Row {
                TextButton(onClick = onEnd) { Text("End session") }
                TextButton(onClick = onCancel) { Text("Cancel") }
            }
        },
    )
}

@androidx.compose.runtime.Composable
private fun StartFailed(alias: String, reason: String) {
    MaterialTheme {
        Surface(modifier = Modifier.fillMaxSize(), color = Color(0xFF1a1a2e)) {
            Box(modifier = Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
                Text("Couldn't start $alias: $reason", color = Color.White)
            }
        }
    }
}
