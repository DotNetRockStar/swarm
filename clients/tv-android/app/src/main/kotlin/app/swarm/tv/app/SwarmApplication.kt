/**
 * Registers a Coil [ImageLoader] with [ArtworkCache] wired in so the whole
 * app's artwork requests (every screen already just calls `AsyncImage`
 * against Coil's default singleton loader) get the fixed one-day cache TTL
 * without touching each call site — [ImageLoaderFactory] is Coil's
 * supported hook for replacing that singleton, resolved once at process
 * start from the `Application` class named in AndroidManifest.xml.
 */
package app.swarm.tv.app

import android.app.Application
import app.swarm.tv.app.ui.ArtworkCache
import coil.ImageLoader
import coil.ImageLoaderFactory
import coil.decode.GifDecoder
import coil.decode.ImageDecoderDecoder
import coil.disk.DiskCache
import coil.memory.MemoryCache
import coil.size.Precision
import kotlinx.coroutines.Dispatchers
import okhttp3.Dispatcher
import okhttp3.OkHttpClient

class SwarmApplication : Application(), ImageLoaderFactory {
    private val artworkCache by lazy { ArtworkCache(this) }

    override fun newImageLoader(): ImageLoader =
        ImageLoader.Builder(this)
            .okHttpClient {
                OkHttpClient.Builder()
                    .dispatcher(
                        Dispatcher().apply {
                            maxRequests = 4
                            maxRequestsPerHost = 4
                        },
                    )
                    .build()
            }
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
            // TV hardware, where memory is shared with ExoPlayer, Compose,
            // and the OS. A fixed cap is more predictable than a large percentage on
            // Fire TV devices, and leaves headroom for Compose, Media3, and
            // the platform without immediately evicting visible card art.
            .memoryCache { MemoryCache.Builder(this).maxSizeBytes(48 * 1024 * 1024).build() }
            .diskCache {
                DiskCache.Builder()
                    .directory(cacheDir.resolve("coil_artwork_cache"))
                    .maxSizeBytes(200L * 1024 * 1024)
                    .build()
            }
            // TV sticks have few fast cores and a relatively small app
            // heap. Bound fetch/decode work so a newly composed shelf
            // cannot turn into a burst that competes with focus and frame
            // rendering, and decode close to (not above) the card's target.
            .fetcherDispatcher(Dispatchers.IO.limitedParallelism(4))
            .decoderDispatcher(Dispatchers.Default.limitedParallelism(2))
            .bitmapFactoryMaxParallelism(2)
            .precision(Precision.INEXACT)
            // Simultaneous fades across a shelf cost render frames. Fixed
            // placeholders retain card geometry without animating every
            // bitmap onto the screen.
            .crossfade(false)
            .eventListenerFactory { ArtworkEventListener(isDebuggable = applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE != 0) }
            .build()
}
