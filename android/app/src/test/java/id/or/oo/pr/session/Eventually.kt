package id.or.oo.pr.session

import org.junit.Assert.fail

fun eventually(timeoutMs: Long = 3_000, condition: () -> Boolean) {
    val deadline = System.currentTimeMillis() + timeoutMs
    while (!condition()) {
        if (System.currentTimeMillis() > deadline) fail("condition not met within $timeoutMs ms")
        Thread.sleep(10)
    }
}
