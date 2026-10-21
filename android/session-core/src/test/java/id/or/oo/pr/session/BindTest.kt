package id.or.oo.pr.session

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class BindTest {

    private fun rejected(hostPath: String, guestPath: String): String =
        assertThrows(IllegalArgumentException::class.java) { Bind(hostPath, guestPath) }.message!!

    @Test
    fun aBlankHostPathIsRejected() {
        assertEquals("bind host path must be absolute: \" \"", rejected(" ", "/srv"))
    }

    @Test
    fun aBlankGuestPathIsRejected() {
        assertEquals("bind guest path must be absolute: \"\"", rejected("/sdcard/srv", ""))
    }

    @Test
    fun aRelativeHostPathIsRejected() {
        assertEquals("bind host path must be absolute: \"files/home\"", rejected("files/home", "/srv"))
    }

    @Test
    fun aRelativeGuestPathIsRejected() {
        assertEquals("bind guest path must be absolute: \"srv\"", rejected("/sdcard/srv", "srv"))
    }

    @Test
    fun aHostPathWithAColonIsRejected() {
        assertEquals(
            "bind host path must not contain ':': \"/sdcard/a:b\"",
            rejected("/sdcard/a:b", "/srv"),
        )
    }

    @Test
    fun absolutePathsWithoutAColonInTheHostPathAreAccepted() {
        assertEquals("/root/a:b", Bind("/sdcard/srv", "/root/a:b").guestPath)
    }
}
