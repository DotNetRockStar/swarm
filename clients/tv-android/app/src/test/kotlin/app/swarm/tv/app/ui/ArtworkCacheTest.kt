package app.swarm.tv.app.ui

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNotEquals
import org.junit.jupiter.api.Test

class ArtworkCacheTest {
    @Test
    fun `loopback port does not change artwork cache key`() {
        val first = artworkRequestCacheKey(
            "http://127.0.0.1:41001/server-1/art/entry-1/poster?v=v7&w=320",
        )
        val restarted = artworkRequestCacheKey(
            "http://127.0.0.1:52999/server-1/art/entry-1/poster?v=v7&w=320",
        )

        assertEquals(first, restarted)
        assertEquals("swarm-artwork:/server-1/art/entry-1/poster?v=v7&w=320", first)
    }

    @Test
    fun `artwork version width and server remain part of cache key`() {
        val base = artworkRequestCacheKey(
            "http://127.0.0.1:41001/server-1/art/entry-1/poster?v=v7&w=320",
        )

        assertNotEquals(base, artworkRequestCacheKey("http://127.0.0.1:41001/server-1/art/entry-1/poster?v=v8&w=320"))
        assertNotEquals(base, artworkRequestCacheKey("http://127.0.0.1:41001/server-1/art/entry-1/poster?v=v7&w=640"))
        assertNotEquals(base, artworkRequestCacheKey("http://127.0.0.1:41001/server-2/art/entry-1/poster?v=v7&w=320"))
    }

    @Test
    fun `non artwork URLs retain their complete identity`() {
        val media = "http://127.0.0.1:41001/server-1/media/entry-1"
        val remote = "https://example.com/art/entry-1/poster?v=v7"

        assertEquals(media, artworkRequestCacheKey(media))
        assertEquals(remote, artworkRequestCacheKey(remote))
    }
}
