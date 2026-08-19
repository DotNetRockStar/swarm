/**
 * Registers a Coil [ImageLoader] with [ArtworkCache] wired in so the whole
 * app's artwork requests (every screen already just calls `AsyncImage`
 * against Coil's default singleton loader) get the configurable cache TTL
 * without touching each call site — [ImageLoaderFactory] is Coil's
 * supported hook for replacing that singleton, resolved once at process
 * start from the `Application` class named in AndroidManifest.xml.
 */
package app.swarm.tv.app

import android.app.Application
import app.swarm.tv.app.data.AndroidAppSettingsStore
import app.swarm.tv.app.ui.ArtworkCache
import coil.ImageLoader
import coil.ImageLoaderFactory
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

class SwarmApplication : Application(), ImageLoaderFactory {
    private val appScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val artworkCache = ArtworkCache()

    override fun onCreate() {
        super.onCreate()
        val settingsStore = AndroidAppSettingsStore(this)
        appScope.launch {
            settingsStore.observeArtworkCacheMinutes().collect { minutes -> artworkCache.ttlMinutes = minutes }
        }
    }

    override fun newImageLoader(): ImageLoader =
        ImageLoader.Builder(this)
            .components { add(artworkCache) }
            .build()
}
