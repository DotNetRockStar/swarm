/**
 * Resume/watched state storage for Android — implements `:core`'s
 * [WatchStateStore] using plain `SharedPreferences` (not the
 * `EncryptedSharedPreferences` `AndroidTokenStore` uses): a playback
 * position isn't a secret the way an access token is, so there's no
 * reason to pay Keystore encryption overhead for it. One JSON value per
 * fingerprint, reusing the same `SwarmJson` config every wire type in this
 * app already uses.
 */
package app.swarm.tv.app.data

import android.content.Context
import app.swarm.tv.core.rest.SwarmJson
import app.swarm.tv.core.watch.WatchState
import app.swarm.tv.core.watch.WatchStateStore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.decodeFromString
import kotlinx.serialization.encodeToString

private const val PREFS_NAME = "swarm_watch_state"

class AndroidWatchStateStore(context: Context) : WatchStateStore {
    private val prefs = context.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

    override suspend fun get(fingerprint: String): WatchState? = withContext(Dispatchers.IO) {
        prefs.getString(fingerprint, null)?.let { json ->
            runCatching { SwarmJson.decodeFromString<WatchState>(json) }.getOrNull()
        }
    }

    override suspend fun set(fingerprint: String, state: WatchState) = withContext(Dispatchers.IO) {
        prefs.edit().putString(fingerprint, SwarmJson.encodeToString(state)).apply()
    }

    override suspend fun clear(fingerprint: String) = withContext(Dispatchers.IO) {
        prefs.edit().remove(fingerprint).apply()
    }
}
