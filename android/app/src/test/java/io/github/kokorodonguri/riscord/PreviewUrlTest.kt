package io.github.kokorodonguri.riscord

import org.junit.Assert.*
import org.junit.Test

class PreviewUrlTest {
    private val url="https://cdn.discordapp.com/attachments/1/2/a.png?ex=1"
    @Test fun largeImagesAreFetchedAtScreenWidth() {
        assertEquals("https://media.discordapp.net/attachments/1/2/a.png?ex=1&width=1080&height=810",
            previewUrl("https://media.discordapp.net/attachments/1/2/a.png?ex=1",url,4000,3000,1080))
    }
    @Test fun smallImagesAreNotUpscaled() {
        assertEquals("https://media.discordapp.net/a.jpg?width=200&height=100",previewUrl("https://media.discordapp.net/a.jpg",url,200,100,1080))
    }
    @Test fun gifsAndUnknownSizesKeepTheProxyAsIs() {
        assertEquals("https://media.discordapp.net/a.GIF?ex=1",previewUrl("https://media.discordapp.net/a.GIF?ex=1",url,4000,3000,1080))
        assertEquals("https://media.discordapp.net/a.png",previewUrl("https://media.discordapp.net/a.png",url,0,0,1080))
    }
    @Test fun animatedWebpKeepsItsAnimation() {
        assertEquals("https://media.discordapp.net/a.webp?width=540&height=540&animated=true",previewUrl("https://media.discordapp.net/a.webp",url,2000,2000,540))
    }
    @Test fun missingProxyFallsBackToTheOriginal() {
        assertEquals(url,previewUrl(null,url,4000,3000,1080))
        assertEquals(url,previewUrl("https://example.com/a.png",url,4000,3000,1080))
    }
}
