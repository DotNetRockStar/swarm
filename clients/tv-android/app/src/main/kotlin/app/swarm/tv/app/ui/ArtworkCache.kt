/**
 * Enforces a 30-day max-age on Coil's artwork cache.
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
import java.net.URI
import java.security.MessageDigest
import java.util.concurrent.ConcurrentHashMap
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

class ArtworkCache(context: Context) : Interceptor {
    private val lastFetchedAt = ConcurrentHashMap<String, Long>()
    private val refreshLocks = Array(64) { Mutex() }
    private val persistedFetchTimes = context.applicationContext.getSharedPreferences("swarm_artwork_fetch_times", Context.MODE_PRIVATE)

    override suspend fun intercept(chain: Interceptor.Chain): ImageResult {
        val key = artworkRequestCacheKey(chain.request.data)
        val keyedRequest = chain.request.newBuilder()
            .diskCacheKey(key)
            .build()
        val now = System.currentTimeMillis()
        val ttlMillis = DEFAULT_ARTWORK_CACHE_MINUTES * 60_000L
        val timestampKey = key.sha256()
        if (!isStale(key, timestampKey, now, ttlMillis)) {
            return chain.proceed(keyedRequest)
        }

        // Prefetch and the newly visible card can request the same URL at the
        // same time. Serialize only that cache key so one request fetches the
        // bytes and every waiter then reads the completed Coil cache entry.
        val refreshLock = refreshLocks[Math.floorMod(key.hashCode(), refreshLocks.size)]
        return refreshLock.withLock {
            val refreshNow = System.currentTimeMillis()
            if (!isStale(key, timestampKey, refreshNow, ttlMillis)) {
                return@withLock chain.proceed(keyedRequest)
            }
            val refreshRequest = keyedRequest.newBuilder()
                .memoryCachePolicy(CachePolicy.WRITE_ONLY)
                .diskCachePolicy(CachePolicy.WRITE_ONLY)
                .build()
            val result = chain.proceed(refreshRequest)
            // Persist only a successful forced refresh. Cache hits must not
            // turn this into a sliding expiration, and failed requests must
            // remain retryable.
            if (result is SuccessResult) {
                val fetchedAt = System.currentTimeMillis()
                lastFetchedAt[key] = fetchedAt
                persistedFetchTimes.edit().putLong(timestampKey, fetchedAt).apply()
            }
            result
        }
    }

    private fun isStale(key: String, timestampKey: String, now: Long, ttlMillis: Long): Boolean {
        val lastFetch = lastFetchedAt[key] ?: persistedFetchTimes.getLong(timestampKey, 0L).also {
            if (it > 0L) lastFetchedAt[key] = it
        }
        return lastFetch == 0L || now - lastFetch >= ttlMillis
    }

    private fun String.sha256(): String = MessageDigest.getInstance("SHA-256")
        .digest(toByteArray())
        .joinToString("") { "%02x".format(it) }
}

/**
 * Coil normally keys an HTTP image by its complete URL. SWARM's host is a
 * loopback proxy on a random port, so that default discards every disk-cache
 * entry after an app restart even though server, artwork path, version, and
 * requested width are unchanged. Strip only the transient loopback authority;
 * the server id remains in the path and version/size remain in the query.
 */
internal fun artworkRequestCacheKey(data: Any): String {
    val raw = data.toString()
    val uri = runCatching { URI(raw) }.getOrNull() ?: return raw
    val path = uri.rawPath ?: return raw
    if (uri.scheme != "http" || uri.host !in setOf("127.0.0.1", "localhost") || "/art/" !in path) {
        return raw
    }
    return buildString {
        append("swarm-artwork:")
        append(path)
        uri.rawQuery?.let { append('?').append(it) }
    }
}
