package id.or.oo.pr.session

import id.or.oo.pr.session.ServiceState.Action.Notify
import id.or.oo.pr.session.ServiceState.Action.StartForeground
import id.or.oo.pr.session.ServiceState.Action.Stop
import id.or.oo.pr.session.ServiceState.Action.WakeLock
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ServiceStateTest {

    private val state = ServiceState()

    @Test
    fun aStartWithNoSessionStartsForegroundThenStopsWithItsId() {
        assertEquals(
            listOf(StartForeground(emptyList(), false), Stop(1)),
            state.onStartCommand(1, emptyList(), false),
        )
    }

    @Test
    fun aStartWithASessionStartsForegroundAndKeepsRunning() {
        assertEquals(listOf(StartForeground(listOf("a"), true)), state.onStartCommand(1, listOf("a"), true))
    }

    @Test
    fun nothingStopsBeforeTheFirstStartForeground() {
        val actions = state.onRunningChanged(emptyList(), false)
        assertFalse(actions.any { it is Stop })
    }

    @Test
    fun aRunningSessionIsNotifiedEvenBeforeTheFirstStart() {
        assertTrue(Notify(listOf("a"), false) in state.onRunningChanged(listOf("a"), false))
    }

    @Test
    fun theRunningListEmptyingStopsWithTheLastStartId() {
        state.onStartCommand(1, listOf("a"), false)
        state.onStartCommand(2, listOf("a"), false)
        val actions = state.onRunningChanged(emptyList(), false)
        assertEquals(listOf(Stop(2)), actions.filterIsInstance<Stop>())
        assertFalse(actions.any { it is Notify })
    }

    @Test
    fun anEmptyListAfterAStopDoesNotStopAgain() {
        state.onStartCommand(1, listOf("a"), false)
        state.onRunningChanged(emptyList(), false)
        assertFalse(state.onRunningChanged(emptyList(), false).any { it is Stop })
    }

    @Test
    fun aStartArrivingWhileStoppingKeepsTheServiceAndLaterStopsUseTheNewerId() {
        state.onStartCommand(1, listOf("a"), false)
        assertEquals(listOf(Stop(1)), state.onRunningChanged(emptyList(), false).filterIsInstance<Stop>())
        assertEquals(listOf(StartForeground(listOf("b"), false)), state.onStartCommand(2, listOf("b"), false))
        assertEquals(listOf(Notify(listOf("b"), false)), state.onRunningChanged(listOf("b"), false).filter { it !is WakeLock })
        assertEquals(listOf(Stop(2)), state.onRunningChanged(emptyList(), false).filterIsInstance<Stop>())
    }

    @Test
    fun aStartWithNoSessionAfterAStopStopsWithTheNewerId() {
        state.onStartCommand(1, listOf("a"), false)
        state.onRunningChanged(emptyList(), false)
        assertEquals(
            listOf(StartForeground(emptyList(), false), Stop(2)),
            state.onStartCommand(2, emptyList(), false),
        )
    }

    @Test
    fun aConflatedEmptyListKeepsTheService() {
        state.onStartCommand(1, listOf("a"), false)
        state.onRunningChanged(listOf("a"), false)
        assertEquals(listOf(StartForeground(listOf("a"), false)), state.onStartCommand(2, listOf("a"), false))
        val actions = state.onRunningChanged(listOf("a"), false)
        assertFalse(actions.any { it is Stop })
        assertTrue(Notify(listOf("a"), false) in actions)
    }

    @Test
    fun theWakeLockFollowsKeepAwakeWhileASessionRuns() {
        state.onStartCommand(1, listOf("a"), false)
        assertEquals(WakeLock(false), state.onRunningChanged(listOf("a"), false).filterIsInstance<WakeLock>().single())
        assertEquals(WakeLock(true), state.onRunningChanged(listOf("a"), true).filterIsInstance<WakeLock>().single())
        assertEquals(WakeLock(false), state.onRunningChanged(emptyList(), true).filterIsInstance<WakeLock>().single())
    }

    @Test
    fun theWakeLockIsReleasedBeforeTheStop() {
        state.onStartCommand(1, listOf("a"), true)
        state.onRunningChanged(listOf("a"), true)
        assertEquals(listOf(WakeLock(false), Stop(1)), state.onRunningChanged(emptyList(), false))
    }
}
