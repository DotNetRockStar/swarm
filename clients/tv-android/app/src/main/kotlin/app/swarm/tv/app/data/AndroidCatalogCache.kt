package app.swarm.tv.app.data

import android.content.Context
import android.util.AtomicFile
import app.swarm.tv.core.catalog.CatalogCache
import app.swarm.tv.core.peer.CatalogManifest
import app.swarm.tv.core.rest.SwarmJson
import java.io.File
import java.security.MessageDigest
import kotlinx.serialization.ExperimentalSerializationApi
import kotlinx.serialization.json.decodeFromStream
import kotlinx.serialization.json.encodeToStream

/**
 * Atomic, persistent catalog snapshots. These are derived cache files, not
 * user data: a corrupt/incomplete snapshot is discarded and rebuilt from
 * its authenticated media server on the next refresh.
 */
class AndroidCatalogCache(context: Context) : CatalogCache {
    private val directory = File(context.filesDir, "catalog_cache")

    @OptIn(ExperimentalSerializationApi::class)
    override suspend fun load(serverId: String): CatalogManifest? {
        val file = AtomicFile(cacheFile(serverId))
        if (!file.baseFile.exists()) return null
        return try {
            file.openRead().buffered().use { SwarmJson.decodeFromStream<CatalogManifest>(it) }
        } catch (_: Exception) {
            // AtomicFile protects interrupted writes; this also self-heals a
            // logically corrupt or schema-incompatible old snapshot.
            file.delete()
            null
        }
    }

    @OptIn(ExperimentalSerializationApi::class)
    override suspend fun store(serverId: String, manifest: CatalogManifest) {
        directory.mkdirs()
        val file = AtomicFile(cacheFile(serverId))
        val output = file.startWrite()
        try {
            SwarmJson.encodeToStream(manifest, output)
            file.finishWrite(output)
        } catch (error: Exception) {
            file.failWrite(output)
            throw error
        }
    }

    private fun cacheFile(serverId: String): File {
        val safeName = MessageDigest.getInstance("SHA-256")
            .digest(serverId.toByteArray(Charsets.UTF_8))
            .joinToString("") { byte -> (byte.toInt() and 0xff).toString(16).padStart(2, '0') }
        return File(directory, "$safeName.json")
    }
}
