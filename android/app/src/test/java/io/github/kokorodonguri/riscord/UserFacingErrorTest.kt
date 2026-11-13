package io.github.kokorodonguri.riscord

import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import java.util.Locale

class UserFacingErrorTest {
    private val original=Locale.getDefault()
    @Before fun japanese() { Locale.setDefault(Locale.JAPAN) }
    @After fun restore() { Locale.setDefault(original) }
    @Test fun technicalDetailsAndServerTextAreNotExposed() {
        val text=userFacingError(IllegalStateException("DNS failure for https://example.test/private"))
        assertFalse(text.contains("example.test"))
        assertTrue(text.contains("再試行"))
        assertEquals("この操作を行う権限がありません。",userFacingError(IllegalStateException("Discord HTTP 403: Missing Access")))
    }
    @Test fun usefulLocalValidationAndRateLimitRemainActionable() {
        assertEquals("添付は10MBまでです",userFacingError(IllegalArgumentException("添付は10MBまでです")))
        assertEquals("混雑しています。5秒後に再試行してください。",userFacingError(IllegalStateException("混雑しています。5秒後に再試行してください。")))
    }
    @Test fun englishMessages() {
        Locale.setDefault(Locale.US)
        assertEquals("You don't have permission to do that.",userFacingError(IllegalStateException("Discord HTTP 403: Missing Access")))
        assertEquals("Attachments can be up to 10 MB",userFacingError(IllegalArgumentException("添付は10MBまでです")))
        assertEquals("Too busy. Try again in 5 seconds.",userFacingError(IllegalStateException("混雑しています。5秒後に再試行してください。")))
    }
}
