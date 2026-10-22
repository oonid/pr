package id.or.oo.pr.session

internal class ServiceState {

    sealed interface Action {
        data class StartForeground(val running: List<String>, val awake: Boolean) : Action
        data class Notify(val running: List<String>, val awake: Boolean) : Action
        data class WakeLock(val on: Boolean) : Action
        data class Stop(val startId: Int) : Action
    }

    private var foreground = false
    private var lastStartId = 0

    fun onStartCommand(startId: Int, running: List<String>, awake: Boolean): List<Action> {
        lastStartId = startId
        foreground = true
        return if (running.isEmpty()) listOf(Action.StartForeground(running, awake), stop())
        else listOf(Action.StartForeground(running, awake))
    }

    fun onRunningChanged(running: List<String>, awake: Boolean): List<Action> = buildList {
        add(Action.WakeLock(awake && running.isNotEmpty()))
        if (running.isNotEmpty()) add(Action.Notify(running, awake))
        else if (foreground) add(stop())
    }

    private fun stop(): Action {
        foreground = false
        return Action.Stop(lastStartId)
    }
}
