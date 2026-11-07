package id.or.oo.pr

import org.junit.Assert.assertEquals
import org.junit.Test

class NotificationAccessTest {

    @Test
    fun aPermissionNeverAskedForIsRequested() {
        assertEquals(NotificationFix.REQUEST, notificationFix(asked = false, showRationale = false))
    }

    @Test
    fun aPermissionDeniedOnceIsRequestedAgain() {
        assertEquals(NotificationFix.REQUEST, notificationFix(asked = true, showRationale = true))
    }

    @Test
    fun aPermissionTheSystemNoLongerPromptsForOpensTheSettings() {
        assertEquals(NotificationFix.OPEN_SETTINGS, notificationFix(asked = true, showRationale = false))
    }

    @Test
    fun theBannerShowsOnlyWhileSomethingRunsWithNotificationsOff() {
        assertEquals(true, showNotificationBanner(notificationsOn = false, running = true))
        assertEquals(false, showNotificationBanner(notificationsOn = true, running = true))
        assertEquals(false, showNotificationBanner(notificationsOn = false, running = false))
    }
}
