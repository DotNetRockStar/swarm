package app.swarm.tv.app.data

import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class PlaybackConnectionTrackerTest {
    @Test
    fun `restored active offline session produces one notification`() {
        val tracker = PlaybackConnectionTracker()
        tracker.markOffline("server-1", "session-1")

        assertTrue(tracker.markRestored("server-1", "server-1", "session-1"))
        assertFalse(tracker.markRestored("server-1", "server-1", "session-1"))
    }

    @Test
    fun `background and stale reconnects stay silent`() {
        val tracker = PlaybackConnectionTracker()

        assertFalse(tracker.markRestored("server-1", "server-1", "session-1"))
        tracker.markOffline("server-1", "old-session")
        assertFalse(tracker.markRestored("server-1", "server-1", "new-session"))
    }
}
