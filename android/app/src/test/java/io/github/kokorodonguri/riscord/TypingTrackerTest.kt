package io.github.kokorodonguri.riscord

import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import java.util.Locale

class TypingTrackerTest {
    private val original=Locale.getDefault()
    @Before fun japanese() { Locale.setDefault(Locale.JAPAN) }
    @After fun restore() { Locale.setDefault(original) }
    private var time=0L
    private val tracker=TypingTracker {time}
    @Test fun repeatedEventsExtendTheSamePersonsDeadline() {
        tracker.start("a","A");time=8000;tracker.start("a","A")
        time=10000;assertEquals("A が入力中…",tracker.label())
        time=18000;assertNull(tracker.label());assertNull(tracker.nextExpiry())
    }
    @Test fun onePersonsMessageDoesNotClearOtherPeople() {
        tracker.start("a","A");tracker.start("b","B")
        assertEquals("A、B が入力中…",tracker.label())
        tracker.remove("a");assertEquals("B が入力中…",tracker.label())
    }
    @Test fun peopleExpireIndependently() {
        tracker.start("a","A");time=2000;tracker.start("b","B")
        assertEquals(8000L,tracker.nextExpiry())
        time=10000;assertEquals("B が入力中…",tracker.label())
        time=12000;assertNull(tracker.label())
    }
    @Test fun changingConversationClearsIncomingButKeepsSendThrottle() {
        tracker.start("a","A");assertTrue(tracker.reserveSend("one"))
        tracker.clear();assertNull(tracker.label());assertFalse(tracker.reserveSend("one"))
        assertTrue(tracker.reserveSend("two"))
    }
    @Test fun sendsAreThrottledPerChannelAndResetOnLogout() {
        assertTrue(tracker.reserveSend("one"));time=7999;assertFalse(tracker.reserveSend("one"))
        time=8000;assertTrue(tracker.reserveSend("one"));tracker.reset();assertTrue(tracker.reserveSend("one"))
    }
    @Test fun crowdedIndicatorIsShortAndUsersAreDeduplicated() {
        tracker.start("a","A");tracker.start("a","A");tracker.start("b","B");tracker.start("c","C")
        assertEquals("3人が入力中…",tracker.label())
    }
    @Test fun englishLabels() {
        Locale.setDefault(Locale.US)
        tracker.start("a","A");assertEquals("A is typing…",tracker.label())
        tracker.start("b","B");assertEquals("A and B are typing…",tracker.label())
        tracker.start("c","C");assertEquals("3 people are typing…",tracker.label())
    }
    @Test fun chineseLabels() {
        Locale.setDefault(Locale.SIMPLIFIED_CHINESE)
        tracker.start("a","A");assertEquals("A 正在输入…",tracker.label())
        tracker.start("b","B");assertEquals("A、B 正在输入…",tracker.label())
        tracker.start("c","C");assertEquals("3 人正在输入…",tracker.label())
    }
}
