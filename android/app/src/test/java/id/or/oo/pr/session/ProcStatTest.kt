package id.or.oo.pr.session

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ProcStatTest {

    @Test
    fun readsTheParentPidAfterTheCommandName() {
        assertEquals(1794, parentPid("1804 (sh) S 1794 1804 1794 34816 1804 4194560 900"))
    }

    @Test
    fun handlesSpacesAndParenthesesInTheCommandName() {
        assertEquals(7, parentPid("42 (a (b) c) R 7 42 42 0 -1 4194304"))
    }

    @Test
    fun rejectsAMalformedLine() {
        assertNull(parentPid("garbage"))
    }
}
