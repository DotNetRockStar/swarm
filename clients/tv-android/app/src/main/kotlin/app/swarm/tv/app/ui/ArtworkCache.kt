/**
 * Enforces a one-day max-age on Coil's artwork cache.
 * Coil's own [coil.disk.DiskCache]/[coil.memory.MemoryCache]
 * are pure LRU (size-bounded, not time-bounded), and the loopback proxy
 * artwork actually loads through ([app.swarm.tv.core.proxy.PeerLoopbackProxy])
 * sends no `Cache-Control` header for Coil's own HTTP-cache-header handling
 * to key off — so this is a small [Interceptor] instead: it tracks the
 * last time each request's URL was actually fetched, and once
 * [ttlMinutes] has elapsed for that URL, forces a real re-fetch
 * (`WRITE_ONLY` on both cache reads, so Coil still writes the fresh bytes
 * back into its normal caches for the next, now-fresh window) rather than
 * silently serving a scraped-over-and-replaced image forever.
 *
 * Fetch timestamps are persisted by a SHA-256 of the versioned artwork URL.
 * A cold app start can therefore reuse Coil's disk cache instead of forcing
 * one network fetch per image; changing `?v=<artwork version>` still produces
 * an immediate cache miss when the server replaces an image.
 */
package app.swarm.tv.app.ui

import android.content.Context
import app.swarm.tv.app.data.DEFAULT_ARTWORK_CACHE_MINUTES
import coil.intercept.Interceptor
import coil.request.CachePolicy
import coil.request.ImageResult
import coil.request.SuccessResult
import java.security.MessageDigest
import java.util.concurrent.ConcurrentHashMap

class ArtworkCache(context: Context) : Interceptor {
    private val lastFetchedAt = ConcurrentHashMap<String, Long>()
    private val persistedFetchTimes = context.applicationContext.getSharedPreferences("swarm_artwork_fetch_times", Context.MODE_PRIVATE)

    override suspend fun intercept(chain: Interceptor.Chain): ImageResult {
        val key = chain.request.data.toString()
        val now = System.currentTimeMillis()
        val ttlMillis = DEFAULT_ARTWORK_CACHE_MINUTES * 60_000L
        val timestampKey = key.sha256()
        val lastFetch = lastFetchedAt[key] ?: persistedFetchTimes.getLong(timestampKey, 0L).also {
            if (it > 0L) lastFetchedAt[key] = it
        }
        val stale = lastFetch == 0L || now - lastFetch >= ttlMillis

        val request = if (stale) {
            chain.request.newBuilder()
                .memoryCachePolicy(CachePolicy.WRITE_ONLY)
                .diskCachePolicy(CachePolicy.WRITE_ONLY)
                .build()
        } else {
            chain.request
        }
        val result = chain.proceed(request)
        // Persist only a successful forced refresh. Cache hits must not turn
        // this into a sliding expiration, and failed requests must remain
        // retryable. Persisting the timestamp fixes the former cold-start
        // behavior where every image skipped an otherwise-valid disk entry
        // once per process launch.
        if (stale && result is SuccessResult) {
            lastFetchedAt[key] = now
            persistedFetchTimes.edit().putLong(timestampKey, now).apply()
        }
        return result
    }

    private fun String.sha256(): String = MessageDigest.getInstance("SHA-256")
        .digest(toByteArray())
        .joinToString("") { "%02x".format(it) }
}
