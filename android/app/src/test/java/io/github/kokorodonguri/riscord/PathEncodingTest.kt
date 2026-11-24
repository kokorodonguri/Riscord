package io.github.kokorodonguri.riscord

import org.junit.Assert.assertEquals
import org.junit.Test

/** デスクトップ（src/net/rest.rs の encode）と同じ結果になるか */
class PathEncodingTest {
    @Test fun matchesTheDesktopEncoding() {
        assertEquals("abc-_.~",encodePathSegment("abc-_.~"))
        assertEquals("name%3A123",encodePathSegment("name:123"))
        assertEquals("%2A%20%2B",encodePathSegment("* +"))
        assertEquals("%F0%9F%91%8D",encodePathSegment("👍"))
    }
}
