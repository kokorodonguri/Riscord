package io.github.kokorodonguri.riscord

import org.junit.Assert.*
import org.junit.Test

class UserFacingErrorTest {
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
}
