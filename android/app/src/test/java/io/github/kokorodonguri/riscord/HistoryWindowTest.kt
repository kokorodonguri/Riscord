package io.github.kokorodonguri.riscord

import org.junit.Assert.*
import org.junit.Test

class HistoryWindowTest {
    @Test fun loadingPagesInBothDirectionsRetainsTheRequestedPage() {
        val current = (51..550).map(Int::toString)
        val older = mergeHistoryWindow(current, (1..50).map(Int::toString), true, false,
            true, false, false, 500, 50) { it }
        assertEquals(500, older.items.size)
        assertEquals("1", older.items.first())
        assertEquals("500", older.items.last())
        assertTrue(older.hasNewer)
        val newer = mergeHistoryWindow(older.items, (501..550).map(Int::toString), false, true,
            older.hasOlder, older.hasNewer, false, 500, 50) { it }
        assertEquals(500, newer.items.size)
        assertEquals("51", newer.items.first())
        assertEquals("550", newer.items.last())
        assertTrue(newer.hasOlder)
    }

    @Test fun latestReloadDoesNotMergeADisconnectedHistoricalWindow() {
        val page = mergeHistoryWindow(listOf("1", "2"), listOf("100", "101"), false, false,
            true, true, false, 500, 50) { it }
        assertEquals(listOf("100", "101"), page.items)
        assertFalse(page.hasNewer)
    }

    @Test fun messageArrivingDuringNewerFetchKeepsNewerNavigationAvailable() {
        val page = mergeHistoryWindow(listOf("1", "2"), listOf("3"), false, true,
            false, true, true, 500, 50) { it }
        assertEquals(listOf("1", "2", "3"), page.items)
        assertTrue(page.hasNewer)
    }
}
