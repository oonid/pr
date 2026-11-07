package id.or.oo.pr.session

internal class ServiceState {

    sealed interface Action {
        data class StartForeground(
            val running: List<String>,
            val awake: Boolean,
            val tasks: List<String> = emptyList(),
        ) : Action
        data class Notify(val running: List<String>, val awake: Boolean, val tasks: List<String> = emptyList()) : Action
        data class WakeLock(val on: Boolean) : Action
        data class Stop(val startId: Int) : Action
    }

    private var foreground = false
    private var lastStartId = 0

    fun onStartCommand(
        startId: Int,
        running: List<String>,
        awake: Boolean,
        tasks: List<String>,
    ): List<Action> {
        lastStartId = startId
        foreground = true
        val start = Action.StartForeground(running, awake, tasks)
        return if (running.isEmpty() && tasks.isEmpty()) listOf(start, stop()) else listOf(start)
    }

    fun onRunningChanged(
        running: List<String>,
        awake: Boolean,
        tasks: List<String>,
    ): List<Action> = buildList {
        add(Action.WakeLock((awake && running.isNotEmpty()) || tasks.isNotEmpty()))
        if (running.isNotEmpty() || tasks.isNotEmpty()) add(Action.Notify(running, awake, tasks))
        else if (foreground) add(stop())
    }

    private fun stop(): Action {
        foreground = false
        return Action.Stop(lastStartId)
    }
}
