package id.or.oo.pr

import android.os.Bundle
import android.util.Log
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.OnBackPressedCallback
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.sp
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import org.connectbot.terminal.SelectionController
import org.connectbot.terminal.TerminalEmulator
import org.connectbot.terminal.TerminalEmulatorFactory
import org.connectbot.terminal.Terminal
import id.or.oo.pr.engine.ProotLauncher
import java.io.File
import kotlin.concurrent.thread

class TerminalActivity : ComponentActivity() {

    companion object {
        private const val TAG = "PR"
    }

    private var session: ProotLauncher.Session? = null
    private var emulator: TerminalEmulator? = null
    private var readerThread: Thread? = null
    private var initialImeRequested = false
    private var initialImeShown = false
    private var backPressStarted = false
    private var selectionController: SelectionController? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val distroName = intent.getStringExtra("distro") ?: run {
            finish()
            return
        }

        val app = application as App
        val launcher = ProotLauncher(app)

        val em = TerminalEmulatorFactory.create(
            initialRows = 24,
            initialCols = 80,
            defaultForeground = Color.White,
            defaultBackground = Color(0xFF1a1a2e),
            onKeyboardInput = { data ->
                session?.write(data)
            },
            onResize = { dims ->
                Log.d(TAG, "Terminal resized to ${dims.rows}x${dims.columns}")
                session?.resize(dims.rows, dims.columns)
            }
        )
        emulator = em

        val sess = launcher.startSession(distroName)
        if (sess == null) {
            Log.e(TAG, "Failed to start session for $distroName")
            finish()
            return
        }
        session = sess

        readerThread = thread(name = "pty-reader") {
            val buf = ByteArray(8192)
            while (!sess.closed) {
                try {
                    val n = sess.read(buf)
                    if (n < 0) break
                    if (n > 0) {
                        em.writeInput(buf, 0, n)
                    }
                } catch (e: Exception) {
                    if (!sess.closed) Log.e(TAG, "PTY read error", e)
                    break
                }
            }
            Log.i(TAG, "PTY reader thread exited")
        }

        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                cleanup()
                finish()
            }
        })

        setContent {
            MaterialTheme {
                Surface(
                    modifier = Modifier.fillMaxSize(),
                    color = Color(0xFF1a1a2e)
                ) {
                    Terminal(
                        terminalEmulator = em,
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
        if (hasFocus && !initialImeRequested) {
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

    override fun onDestroy() {
        super.onDestroy()
        cleanup()
    }

    private fun cleanup() {
        session?.close()
        session = null
        readerThread?.interrupt()
        readerThread = null
    }
}
