package app.swarm.tv.app.data

import android.content.Context
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

private const val PREFS_NAME = "swarm_disconnected_servers"

/** Servers intentionally disconnected from this TV, scoped to each swarm. */
class AndroidDisconnectedServerStore(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

    suspend fun load(swarmId: String): Set<String> = withContext(Dispatchers.IO) {
        prefs.getStringSet(swarmId, emptySet())?.toSet() ?: emptySet()
    }

    suspend fun setDisconnected(swarmId: String, fingerprint: String, disconnected: Boolean) =
        withContext(Dispatchers.IO) {
            val current = HashSet(prefs.getStringSet(swarmId, emptySet()) ?: emptySet())
            if (disconnected) current.add(fingerprint) else current.remove(fingerprint)
            prefs.edit().putStringSet(swarmId, current).apply()
        }
}
