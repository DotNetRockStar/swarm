/** Device-local movie/show watchlist, shared by every connected server. */
package app.swarm.tv.app.data

import android.content.Context
import app.swarm.tv.core.catalog.MergedEntry
import app.swarm.tv.core.catalog.ShowGroup
import java.util.Locale
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

private const val WATCHLIST_PREFS_NAME = "swarm_watchlist"
private const val KEY_ITEMS = "items"

object WatchlistKeys {
    /** A fingerprint is stable when the same movie is available from multiple servers. */
    fun movie(entry: MergedEntry): String = "movie:${entry.entry.fingerprint}"

    /** Shows are client-side groups, so their normalized canonical display title is their identity. */
    fun show(show: ShowGroup): String = "show:${show.show.trim().lowercase(Locale.ROOT)}"
}

class AndroidWatchlistStore(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences(WATCHLIST_PREFS_NAME, Context.MODE_PRIVATE)

    suspend fun loadAll(): Set<String> = withContext(Dispatchers.IO) {
        prefs.getStringSet(KEY_ITEMS, emptySet())?.toSet() ?: emptySet()
    }

    suspend fun setListed(key: String, listed: Boolean) = withContext(Dispatchers.IO) {
        val current = HashSet(prefs.getStringSet(KEY_ITEMS, emptySet()) ?: emptySet())
        if (listed) current.add(key) else current.remove(key)
        prefs.edit().putStringSet(KEY_ITEMS, current).apply()
    }
}
