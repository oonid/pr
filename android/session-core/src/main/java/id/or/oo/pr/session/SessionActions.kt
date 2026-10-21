package id.or.oo.pr.session

object SessionActions {
    fun endAll(packageName: String): String = "$packageName.action.END_ALL"

    fun toggleWake(packageName: String): String = "$packageName.action.TOGGLE_WAKE"
}
