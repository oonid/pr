package id.or.oo.pr.session

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotSame
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicInteger
import kotlin.concurrent.thread

class SessionManagerTest {

    private val ptys = ConcurrentHashMap<String, FakePty>()
    private val created = AtomicInteger()
    private val foregroundCalls = CopyOnWriteArrayList<String>()
    private val endedNotices = CopyOnWriteArrayList<Pair<String, Int?>>()
    private var failNext = false
    private var foregroundFails = false

    private val factory = SessionManager.SessionFactory { alias, onEnded ->
        if (failNext) throw IllegalStateException("pr-cli not found")
        created.incrementAndGet()
        val pty = FakePty()
        ptys[alias] = pty
        ManagedSession(alias, pty, null, { _, _, _ -> }, onEnded, endGraceMs = 100, exitCodeWaitMs = 100)
    }

    private val foreground = object : SessionManager.ForegroundController {
        override fun start() {
            if (foregroundFails) throw IllegalStateException("not allowed")
            foregroundCalls += "start"
        }

        override fun stop() {
            foregroundCalls += "stop"
        }
    }

    private val manager = SessionManager(factory, foreground) { alias, code -> endedNotices += alias to code }

    private fun open(alias: String) =
        (manager.open(alias) as SessionManager.OpenResult.Opened).session

    @Test
    fun openingARunningContainerReturnsTheSameSession() {
        val first = open("alpine")
        val second = open("alpine")
        assertSame(first, second)
        assertEquals(1, created.get())
        assertEquals(listOf("alpine"), manager.running.value)
    }

    @Test
    fun foregroundStartsWithTheFirstSessionAndStopsWithTheLast() {
        open("alpine")
        open("ubuntu")
        assertEquals(listOf("start"), foregroundCalls)
        manager.end("alpine")
        eventually { manager.running.value == listOf("ubuntu") }
        assertEquals(listOf("start"), foregroundCalls)
        ptys.getValue("ubuntu").exit(0)
        eventually { manager.running.value.isEmpty() }
        assertEquals(listOf("start", "stop"), foregroundCalls)
    }

    @Test
    fun keepCpuAwakeResetsWhenTheLastSessionEnds() {
        open("alpine")
        manager.keepCpuAwake.value = true
        manager.endAll()
        eventually { manager.running.value.isEmpty() }
        assertEquals(false, manager.keepCpuAwake.value)
    }

    @Test
    fun programExitWithNoScreenPostsTheEndedNotice() {
        open("alpine")
        ptys.getValue("alpine").exit(1)
        eventually { endedNotices.isNotEmpty() }
        assertEquals(listOf("alpine" to 1), endedNotices)
    }

    @Test
    fun endingASessionPostsNoEndedNotice() {
        open("alpine")
        manager.end("alpine")
        eventually { manager.running.value.isEmpty() }
        Thread.sleep(50)
        assertTrue(endedNotices.isEmpty())
    }

    @Test
    fun anEndedSessionStaysWhileAScreenShowsIt() {
        val session = open("alpine")
        val screen = ManagedSession.Screen {}
        manager.attach(session, screen)
        ptys.getValue("alpine").exit(2)
        eventually { session.exit.value != null }
        assertTrue(endedNotices.isEmpty())
        assertTrue(manager.running.value.isEmpty())
        manager.detach(session, screen)
        val next = open("alpine")
        assertNotSame(session, next)
        assertEquals(2, created.get())
    }

    @Test
    fun attachingANewScreenClosesThePreviousOne() {
        val session = open("alpine")
        val closed = CountDownLatch(1)
        manager.attach(session, ManagedSession.Screen { closed.countDown() })
        manager.attach(session, ManagedSession.Screen {})
        assertEquals(0L, closed.count)
    }

    @Test
    fun aFailedStartIsReportedAndStartsNothing() {
        failNext = true
        val result = manager.open("alpine")
        assertEquals(SessionManager.OpenResult.Failed("pr-cli not found"), result)
        assertTrue(manager.running.value.isEmpty())
        assertTrue(foregroundCalls.isEmpty())
    }

    @Test
    fun aForegroundFailureIsReportedAndTheSessionStillOpens() {
        foregroundFails = true
        open("alpine")
        assertEquals(listOf("alpine"), manager.running.value)
        assertEquals("not allowed", manager.backgroundError.value)
    }

    @Test
    fun concurrentOpensCreateOneSession() {
        val start = CountDownLatch(1)
        val opened = CopyOnWriteArrayList<ManagedSession>()
        val threads = (1..8).map {
            thread {
                start.await()
                opened += open("alpine")
            }
        }
        start.countDown()
        threads.forEach { it.join() }
        assertEquals(1, created.get())
        assertEquals(1, opened.toSet().size)
    }

    @Test
    fun endAllEndsEverySession() {
        open("alpine")
        open("ubuntu")
        manager.endAll()
        eventually { manager.running.value.isEmpty() }
        assertTrue(ptys.values.all { it.eofSent })
    }

    @Test
    fun backgroundErrorIsNullWhenTheServiceStarts() {
        open("alpine")
        assertNull(manager.backgroundError.value)
    }
}
