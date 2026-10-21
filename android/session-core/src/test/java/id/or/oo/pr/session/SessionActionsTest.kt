package id.or.oo.pr.session

import org.junit.Assert.assertEquals
import org.junit.Test

class SessionActionsTest {

    @Test
    fun prKeepsItsActionNames() {
        assertEquals("id.or.oo.pr.action.END_ALL", SessionActions.endAll("id.or.oo.pr"))
        assertEquals("id.or.oo.pr.action.TOGGLE_WAKE", SessionActions.toggleWake("id.or.oo.pr"))
    }

    @Test
    fun anotherAppGetsActionsUnderItsOwnPackage() {
        assertEquals("org.example.gateway.action.END_ALL", SessionActions.endAll("org.example.gateway"))
        assertEquals("org.example.gateway.action.TOGGLE_WAKE", SessionActions.toggleWake("org.example.gateway"))
    }
}
