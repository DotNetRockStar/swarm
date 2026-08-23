package app.swarm.tv.app.ui.screens

import androidx.media3.common.C
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class PlayerSeekTest {
    @Test
    fun `seek inside generated HLS window stays in current player`() {
        assertFalse(shouldRestartHlsPlaybackForSeek(relativeTargetMs = 45_000L, availableDurationMs = 60_000L))
    }

    @Test
    fun `seek beyond generated HLS window starts a session at requested position`() {
        assertTrue(shouldRestartHlsPlaybackForSeek(relativeTargetMs = 90_000L, availableDurationMs = 60_000L))
    }

    @Test
    fun `seek before current HLS session offset starts an earlier session`() {
        assertTrue(shouldRestartHlsPlaybackForSeek(relativeTargetMs = -1L, availableDurationMs = 60_000L))
    }

    @Test
    fun `unknown HLS window duration uses session restart`() {
        assertTrue(shouldRestartHlsPlaybackForSeek(relativeTargetMs = 45_000L, availableDurationMs = C.TIME_UNSET))
    }
}
