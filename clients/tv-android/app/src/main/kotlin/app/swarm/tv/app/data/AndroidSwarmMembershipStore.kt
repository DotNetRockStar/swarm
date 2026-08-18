/**
 * Persists which swarms this device currently belongs to. Plain (not
 * encrypted) `SharedPreferences` like [AndroidWatchStateStore] — swarm
 * id/name pairs aren't a secret the way an access token is. One JSON array
 * under a single key, since the whole list is always read/written together
 * (unlike [AndroidWatchStateStore]'s per-fingerprint keys).
 *
 * [SwarmViewModel] keeps its own in-memory copy for the running session and
 * only writes through here — this app doesn't restore a session on cold
 * start at all yet (it always begins at `UiState.PasscodeEntry`, same as its
 * `swarmId`/`accessToken` fields), so nothing reads this back today. It
 * exists so that gap can be closed later without a storage-format change.
 */
package app.swarm.tv.app.data

import android.content.Context
import app.swarm.tv.core.rest.SwarmJson
import app.swarm.tv.core.rest.SwarmSummary
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.decodeFromString
import kotlinx.serialization.encodeToString

private const val PREFS_NAME = "swarm_memberships"
private const val KEY_SWARMS = "swarms"

class AndroidSwarmMembershipStore(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

    suspend fun get(): List<SwarmSummary> = withContext(Dispatchers.IO) {
        prefs.getString(KEY_SWARMS, null)?.let { json ->
            runCatching { SwarmJson.decodeFromString<List<SwarmSummary>>(json) }.getOrNull()
        } ?: emptyList()
    }

    suspend fun set(swarms: List<SwarmSummary>) = withContext(Dispatchers.IO) {
        prefs.edit().putString(KEY_SWARMS, SwarmJson.encodeToString(swarms)).apply()
    }
}
