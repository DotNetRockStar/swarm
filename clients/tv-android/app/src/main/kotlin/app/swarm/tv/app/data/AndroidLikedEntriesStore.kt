/**
 * Local "did I like this" state — a flat set of liked content fingerprints
 * (not entry_keys: a [app.swarm.tv.core.catalog.MergedEntry] can have a
 * different entry_key per source server for what's still the same asset,
 * while its fingerprint is stable across all of them). Plain
 * `SharedPreferences`, not Room: same reasoning as [AndroidWatchStateStore]
 * — a no-relations key-membership set has no reason to carry Room's setup
 * for a single flat table, see the `tv-client-database` skill's "what
 * deliberately stays OUT of this database" section.
 *
 * This is deliberately the *only* source of truth the UI reads for "is this
 * liked" — the server's own `entry_likes` table (aggregate `like_count`,
 * dashboard visibility) is written to best-effort via
 * [app.swarm.tv.core.catalog.CatalogSession.toggleLike] but never read back
 * for this device's own heart-icon state, so the UI never waits on a round
 * trip or flickers if that call fails. [SwarmViewModel] loads the full set
 * once via [loadAll] and keeps its own in-memory copy from then on (mirrors
 * how `cachedSwarms` already mirrors `AndroidConnectionStore`) rather than
 * hitting this store synchronously from Compose on every card render.
 */
package app.swarm.tv.app.data

import android.content.Context
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

private const val PREFS_NAME = "swarm_liked_entries"
private const val KEY_FINGERPRINTS = "fingerprints"

class AndroidLikedEntriesStore(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

    suspend fun loadAll(): Set<String> = withContext(Dispatchers.IO) {
        prefs.getStringSet(KEY_FINGERPRINTS, emptySet())?.toSet() ?: emptySet()
    }

    suspend fun setLiked(fingerprint: String, liked: Boolean) = withContext(Dispatchers.IO) {
        // getStringSet's returned Set must be treated as immutable (its own
        // contract) and mutated via a fresh copy — mutating it in place can
        // silently corrupt the underlying SharedPreferences file.
        val current = HashSet(prefs.getStringSet(KEY_FINGERPRINTS, emptySet()) ?: emptySet())
        if (liked) current.add(fingerprint) else current.remove(fingerprint)
        prefs.edit().putStringSet(KEY_FINGERPRINTS, current).apply()
    }
}
