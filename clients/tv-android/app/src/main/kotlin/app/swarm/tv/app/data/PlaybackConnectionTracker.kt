package app.swarm.tv.app.data

import java.util.concurrent.ConcurrentHashMap

/** Correlates a transport reconnect with the active playback session that
 * actually surfaced an offline notification. Thread-safe because Media3 and
 * the loopback proxy report the two transitions from different threads. */
internal class PlaybackConnectionTracker {
    private val offlineSessions = ConcurrentHashMap<String, String>()

    fun markOffline(serverId: String, sessionId: String) {
        offlineSessions[serverId] = sessionId
    }

    fun markRestored(
        serverId: String,
        activeServerId: String?,
        activeSessionId: String?,
    ): Boolean {
        val offlineSessionId = offlineSessions.remove(serverId) ?: return false
        return serverId == activeServerId && offlineSessionId == activeSessionId
    }
}
