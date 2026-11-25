package io.github.kokorodonguri.riscord

import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

class SessionTokenGateTest {
    @Test fun logoutWaitsForInFlightSaveAndRejectsLateSave() {
        val gate = SessionTokenGate()
        val attempt = gate.next()
        val writing = CountDownLatch(1)
        val finishWrite = CountDownLatch(1)
        var stored: String? = null
        val writer = thread {
            gate.ifCurrent(attempt) {
                writing.countDown()
                check(finishWrite.await(5, TimeUnit.SECONDS))
                stored = "old"
            }
        }
        assertTrue(writing.await(5, TimeUnit.SECONDS))
        val logout = thread { gate.invalidateAndClear { stored = null } }
        finishWrite.countDown()
        writer.join(5000)
        logout.join(5000)
        assertFalse(writer.isAlive)
        assertFalse(logout.isAlive)
        assertNull(stored)
        assertFalse(gate.ifCurrent(attempt) { stored = "old" })
        assertNull(stored)
        val next = gate.next()
        assertTrue(gate.ifCurrent(next) { stored = "new" })
        assertEquals("new", stored)
    }
}
