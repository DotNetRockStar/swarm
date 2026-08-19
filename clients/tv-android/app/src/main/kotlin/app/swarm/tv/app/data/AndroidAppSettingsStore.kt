/**
 * App-level settings that aren't tied to any one connection/swarm — today
 * just the artwork cache TTL (see [app.swarm.tv.app.ui.ArtworkCache]),
 * Room-backed via [app.swarm.tv.app.data.db.AppSettingsEntity].
 */
package app.swarm.tv.app.data

import android.content.Context
import app.swarm.tv.app.data.db.AppDatabase
import app.swarm.tv.app.data.db.AppSettingsEntity
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.withContext

const val DEFAULT_ARTWORK_CACHE_MINUTES = 5

class AndroidAppSettingsStore(context: Context) {
    private val dao = AppDatabase.getInstance(context).appSettingsDao()

    suspend fun getArtworkCacheMinutes(): Int = withContext(Dispatchers.IO) {
        dao.get()?.artworkCacheMinutes ?: DEFAULT_ARTWORK_CACHE_MINUTES
    }

    fun observeArtworkCacheMinutes() = dao.observe().map { it?.artworkCacheMinutes ?: DEFAULT_ARTWORK_CACHE_MINUTES }

    suspend fun setArtworkCacheMinutes(minutes: Int) = withContext(Dispatchers.IO) {
        dao.upsert(AppSettingsEntity(artworkCacheMinutes = minutes))
    }
}
