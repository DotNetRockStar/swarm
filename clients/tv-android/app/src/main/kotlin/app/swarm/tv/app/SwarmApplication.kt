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
import coil.decode.GifDecoder
import coil.decode.ImageDecoderDecoder
import coil.disk.DiskCache
import coil.memory.MemoryCache
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
            .components {
                add(artworkCache)
                // ImageDecoderDecoder needs API 28 (Android 9); Fire TV
                // devices as old as the 1st-gen stick sit below that, so
                // GifDecoder covers them as the fallback the newer decoder
                // doesn't run on. Only the player's loading indicator uses
                // an animated GIF today, but this makes any future one work
                // app-wide without another wiring step.
                if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.P) {
                    add(ImageDecoderDecoder.Factory())
                } else {
                    add(GifDecoder.Factory())
                }
            }
            // Real complaint from live use: scrolling a card off-screen and
            // back made its artwork visibly blank-then-repaint, as if it
            // were being re-fetched every time rather than cached. Compose's
            // LazyRow/LazyVerticalGrid genuinely does dispose an off-screen
            // card's AsyncImage and recreate it on the way back — that part
            // is normal and expected — so avoiding a real re-fetch on that
            // recreation depends entirely on Coil's own caches actually
            // holding the bytes. Leaving both caches at Coil's
            // platform-computed defaults (a percentage of *current*
            // available memory/storage) is a real risk specifically on Fire
            // TV hardware, where the low-end Stick models run with as little
            // as ~1-1.5GB total RAM shared with ExoPlayer/Compose/the OS —
            // explicit, generous floors here mean artwork survives both a
            // Lazy-list scroll (memory cache) and a cold navigation back to
            // a previously-browsed screen or a fresh app launch within the
            // TTL window (disk cache — unaffected by memory pressure at
            // all), instead of silently shrinking under whatever memory
            // happens to be free at the moment.
            .memoryCache { MemoryCache.Builder(this).maxSizePercent(0.3).build() }
            .diskCache {
                DiskCache.Builder()
                    .directory(cacheDir.resolve("coil_artwork_cache"))
                    .maxSizeBytes(200L * 1024 * 1024)
                    .build()
            }
            // Smooths over the rare genuine re-fetch (first view of an
            // image, or a real TTL expiry) so even that case reads as a
            // quick fade-in rather than a jarring blank flash.
            .crossfade(true)
            .build()
}
