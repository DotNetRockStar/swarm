package app.swarm.tv.core.transport

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class KwikStreamReadTimeoutTest {
    @Test
    fun `kwik stream reads use a finite timeout`() {
        assertTrue(configureKwikStreamReadTimeout(12_345L))

        val input = Class.forName("tech.kwik.core.stream.StreamInputStreamImpl")
        val timeout = input.getDeclaredField("waitForNextFrameTimeout").apply { isAccessible = true }
        assertEquals(12_345L, timeout.getLong(null))

        // Restore the production value so test ordering cannot affect QUIC
        // interop tests sharing this JVM.
        assertTrue(configureKwikStreamReadTimeout())
    }

    @Test
    fun `invalid timeout is rejected without touching kwik`() {
        assertFalse(configureKwikStreamReadTimeout(0L))
    }
}
