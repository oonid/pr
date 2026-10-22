package id.or.oo.pr.session

import android.content.ComponentName
import android.content.ContextWrapper
import android.content.Intent
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CopyOnWriteArrayList

class ServiceForegroundControllerTest {

    private val undeclared = object : ContextWrapper(null) {
        override fun startForegroundService(service: Intent?): ComponentName? = null
    }

    private val ptys = CopyOnWriteArrayList<FakePty>()

    private val sessions = SessionManager(
        { alias, onEnded ->
            val pty = FakePty().also { ptys += it }
            ManagedSession(alias, pty, null, { _, _, _ -> }, onEnded, exitCodeWaitMs = 100)
        },
        ServiceForegroundController(undeclared),
    ) { _, _ -> }

    @After
    fun tearDown() {
        ptys.forEach { it.exit(0) }
    }

    @Test
    fun anUndeclaredServiceIsReportedAsTheBackgroundErrorAndTheSessionStillOpens() {
        val result = sessions.open("debian")

        assertTrue(result is SessionManager.OpenResult.Opened)
        assertEquals(listOf("debian"), sessions.running.value)
        assertEquals("SessionService is not declared in the app manifest", sessions.backgroundError.value)
    }
}
