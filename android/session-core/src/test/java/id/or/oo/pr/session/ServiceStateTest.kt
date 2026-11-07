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
            state.onStartCommand(1, emptyList(), false, emptyList()),
        )
    }

    @Test
    fun aStartWithASessionStartsForegroundAndKeepsRunning() {
        assertEquals(listOf(StartForeground(listOf("a"), true)), state.onStartCommand(1, listOf("a"), true, emptyList()))
    }

    @Test
    fun nothingStopsBeforeTheFirstStartForeground() {
        val actions = state.onRunningChanged(emptyList(), false, emptyList())
        assertFalse(actions.any { it is Stop })
    }

    @Test
    fun aRunningSessionIsNotifiedEvenBeforeTheFirstStart() {
        assertTrue(Notify(listOf("a"), false) in state.onRunningChanged(listOf("a"), false, emptyList()))
    }

    @Test
    fun theRunningListEmptyingStopsWithTheLastStartId() {
        state.onStartCommand(1, listOf("a"), false, emptyList())
        state.onStartCommand(2, listOf("a"), false, emptyList())
        val actions = state.onRunningChanged(emptyList(), false, emptyList())
        assertEquals(listOf(Stop(2)), actions.filterIsInstance<Stop>())
        assertFalse(actions.any { it is Notify })
    }

    @Test
    fun anEmptyListAfterAStopDoesNotStopAgain() {
        state.onStartCommand(1, listOf("a"), false, emptyList())
        state.onRunningChanged(emptyList(), false, emptyList())
        assertFalse(state.onRunningChanged(emptyList(), false, emptyList()).any { it is Stop })
    }

    @Test
    fun aStartArrivingWhileStoppingKeepsTheServiceAndLaterStopsUseTheNewerId() {
        state.onStartCommand(1, listOf("a"), false, emptyList())
        assertEquals(listOf(Stop(1)), state.onRunningChanged(emptyList(), false, emptyList()).filterIsInstance<Stop>())
        assertEquals(listOf(StartForeground(listOf("b"), false)), state.onStartCommand(2, listOf("b"), false, emptyList()))
        assertEquals(listOf(Notify(listOf("b"), false)), state.onRunningChanged(listOf("b"), false, emptyList()).filter { it !is WakeLock })
        assertEquals(listOf(Stop(2)), state.onRunningChanged(emptyList(), false, emptyList()).filterIsInstance<Stop>())
    }

    @Test
    fun aStartWithNoSessionAfterAStopStopsWithTheNewerId() {
        state.onStartCommand(1, listOf("a"), false, emptyList())
        state.onRunningChanged(emptyList(), false, emptyList())
        assertEquals(
            listOf(StartForeground(emptyList(), false), Stop(2)),
            state.onStartCommand(2, emptyList(), false, emptyList()),
        )
    }

    @Test
    fun aConflatedEmptyListKeepsTheService() {
        state.onStartCommand(1, listOf("a"), false, emptyList())
        state.onRunningChanged(listOf("a"), false, emptyList())
        assertEquals(listOf(StartForeground(listOf("a"), false)), state.onStartCommand(2, listOf("a"), false, emptyList()))
        val actions = state.onRunningChanged(listOf("a"), false, emptyList())
        assertFalse(actions.any { it is Stop })
        assertTrue(Notify(listOf("a"), false) in actions)
    }

    @Test
    fun theWakeLockFollowsKeepAwakeWhileASessionRuns() {
        state.onStartCommand(1, listOf("a"), false, emptyList())
        assertEquals(WakeLock(false), state.onRunningChanged(listOf("a"), false, emptyList()).filterIsInstance<WakeLock>().single())
        assertEquals(WakeLock(true), state.onRunningChanged(listOf("a"), true, emptyList()).filterIsInstance<WakeLock>().single())
        assertEquals(WakeLock(false), state.onRunningChanged(emptyList(), true, emptyList()).filterIsInstance<WakeLock>().single())
    }

    @Test
    fun aStartWithOnlyATaskStartsForegroundAndKeepsRunning() {
        assertEquals(
            listOf(StartForeground(emptyList(), false, listOf("t"))),
            state.onStartCommand(1, emptyList(), false, listOf("t")),
        )
    }

    @Test
    fun aRunningTaskIsNotifiedAndHoldsTheWakeLock() {
        state.onStartCommand(1, emptyList(), false, listOf("t"))
        assertEquals(
            listOf(WakeLock(true), Notify(emptyList(), false, listOf("t"))),
            state.onRunningChanged(emptyList(), false, listOf("t")),
        )
    }

    @Test
    fun theLastTaskEndingWithNoSessionReleasesTheWakeLockAndStops() {
        state.onStartCommand(1, emptyList(), false, listOf("t"))
        assertEquals(listOf(WakeLock(false), Stop(1)), state.onRunningChanged(emptyList(), false, emptyList()))
    }

    @Test
    fun aTaskEndingWhileASessionRunsKeepsTheService() {
        state.onStartCommand(1, listOf("a"), false, listOf("t"))
        assertEquals(
            listOf(WakeLock(false), Notify(listOf("a"), false)),
            state.onRunningChanged(listOf("a"), false, emptyList()),
        )
    }

    @Test
    fun theWakeLockIsReleasedBeforeTheStop() {
        state.onStartCommand(1, listOf("a"), true, emptyList())
        state.onRunningChanged(listOf("a"), true, emptyList())
        assertEquals(listOf(WakeLock(false), Stop(1)), state.onRunningChanged(emptyList(), false, emptyList()))
    }
}
