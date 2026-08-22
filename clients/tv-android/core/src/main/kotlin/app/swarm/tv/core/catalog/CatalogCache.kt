package app.swarm.tv.core.catalog

import app.swarm.tv.core.peer.CatalogManifest

/**
 * Persistent, per-server catalog storage supplied by the platform layer.
 *
 * The core module deliberately does not know where Android stores files.
 * Keeping the cache behind this small interface also makes catalog refresh
 * policy independently testable. A cache failure is always non-fatal: the
 * network remains authoritative and callers may return null/drop writes.
 */
interface CatalogCache {
    suspend fun load(serverId: String): CatalogManifest?
    suspend fun store(serverId: String, manifest: CatalogManifest)
}
