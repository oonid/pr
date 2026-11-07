package id.or.oo.pr

enum class NotificationFix { REQUEST, OPEN_SETTINGS }

const val PREFS_NAME = "pr_prefs"
const val NOTIFICATIONS_ASKED = "notifications_asked"

fun notificationFix(asked: Boolean, showRationale: Boolean): NotificationFix =
    if (!asked || showRationale) NotificationFix.REQUEST else NotificationFix.OPEN_SETTINGS

fun showNotificationBanner(notificationsOn: Boolean, running: Boolean): Boolean = running && !notificationsOn
