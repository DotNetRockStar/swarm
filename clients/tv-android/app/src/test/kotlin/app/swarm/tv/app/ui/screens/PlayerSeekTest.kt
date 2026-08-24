package app.swarm.tv.app.ui.screens

import androidx.media3.common.C
import androidx.media3.common.PlaybackException
import java.io.EOFException
import java.io.IOException
import java.net.SocketException
import org.junit.jupiter.api.Assertions.assertEquals
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

    @Test
    fun `expired playback session 404 is recovered`() {
        assertTrue(
            shouldRecoverExpiredPlaybackSession(
                errorCode = PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS,
                responseCode = 404,
            ),
        )
    }

    @Test
    fun `other HTTP failures are not treated as expired playback sessions`() {
        assertFalse(
            shouldRecoverExpiredPlaybackSession(
                errorCode = PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS,
                responseCode = 500,
            ),
        )
        assertFalse(
            shouldRecoverExpiredPlaybackSession(
                errorCode = PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_FAILED,
                responseCode = 404,
            ),
        )
    }

    @Test
    fun `server failure statuses are retried but permanent asset errors are not`() {
        assertTrue(isServerOfflineHttpStatus(500))
        assertTrue(isServerOfflineHttpStatus(503))
        assertFalse(isServerOfflineHttpStatus(404))
        assertFalse(isServerOfflineHttpStatus(416))
    }

    @Test
    fun `interrupted transport chains are recognized as server outages`() {
        assertTrue(isServerOfflineLoadError(EOFException("truncated response")))
        assertTrue(isServerOfflineLoadError(IOException("load failed", SocketException("connection reset"))))
        assertFalse(isServerOfflineLoadError(IOException("malformed media")))
    }

    @Test
    fun `dropping to a lower HLS rendition is reported as a downgrade`() {
        assertEquals(true, playbackQualityChange(previousHeight = 1080, newHeight = 480))
    }

    @Test
    fun `rising back to a higher HLS rendition is reported as an upgrade`() {
        assertEquals(false, playbackQualityChange(previousHeight = 480, newHeight = 1080))
    }

    @Test
    fun `the first format selected for a session is not a change`() {
        assertEquals(null, playbackQualityChange(previousHeight = null, newHeight = 1080))
    }

    @Test
    fun `reselecting the same rendition height is not a change`() {
        assertEquals(null, playbackQualityChange(previousHeight = 720, newHeight = 720))
    }
}
