/**
 * Enforces a configurable max-age on Coil's artwork cache — item 3 of the
 * client-updates request: "cache configurable in the app but default it to
 * 5 minutes." Coil's own [coil.disk.DiskCache]/[coil.memory.MemoryCache]
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
 * The bookkeeping map is in-memory only, not persisted — a cold app start
 * treats everything as stale once, which is harmless (one extra fetch per
 * image) and simpler than trying to make cache-age survive process death.
 */
package app.swarm.tv.app.ui

import app.swarm.tv.app.data.DEFAULT_ARTWORK_CACHE_MINUTES
import coil.intercept.Interceptor
import coil.request.CachePolicy
import coil.request.ImageResult
import java.util.concurrent.ConcurrentHashMap

class ArtworkCache : Interceptor {
    /** Live-updated by [app.swarm.tv.app.SwarmApplication] as the config-page setting changes — no app restart needed for a new value to take effect. */
    @Volatile var ttlMinutes: Int = DEFAULT_ARTWORK_CACHE_MINUTES

    private val lastFetchedAt = ConcurrentHashMap<String, Long>()

    override suspend fun intercept(chain: Interceptor.Chain): ImageResult {
        val key = chain.request.data.toString()
        val now = System.currentTimeMillis()
        val ttlMillis = ttlMinutes.coerceAtLeast(0) * 60_000L
        val stale = lastFetchedAt[key]?.let { now - it >= ttlMillis } ?: true

        val request = if (stale) {
            chain.request.newBuilder()
                .memoryCachePolicy(CachePolicy.WRITE_ONLY)
                .diskCachePolicy(CachePolicy.WRITE_ONLY)
                .build()
        } else {
            chain.request
        }
        val result = chain.proceed(request)
        lastFetchedAt[key] = now
        return result
    }
}
