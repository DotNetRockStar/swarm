package app.swarm.tv.app.data

import android.content.Context
import android.util.AtomicFile
import app.swarm.tv.core.catalog.CatalogCache
import app.swarm.tv.core.peer.CatalogManifest
import app.swarm.tv.core.rest.SwarmJson
import java.io.File
import java.security.MessageDigest
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
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
    private val testingResidue = context.getSharedPreferences("testing_catalog_residue", Context.MODE_PRIVATE)

    // Reading and writing these snapshots is filesystem IO plus a full-catalog
    // JSON (de)serialisation whose cost scales with library size. Callers reach
    // this from Main-dispatched coroutines (catalog refresh, the live change
    // feed), so the work must not run on the calling thread — a large library
    // otherwise stalls the UI thread hard enough to drop remote input and trip
    // an ANR (#208).
    @OptIn(ExperimentalSerializationApi::class)
    override suspend fun load(serverId: String): CatalogManifest? = withContext(Dispatchers.IO) {
        val file = AtomicFile(cacheFile(serverId))
        if (!file.baseFile.exists()) return@withContext null
        try {
            file.openRead().buffered().use { SwarmJson.decodeFromStream<CatalogManifest>(it) }
        } catch (_: Exception) {
            // AtomicFile protects interrupted writes; this also self-heals a
            // logically corrupt or schema-incompatible old snapshot.
            file.delete()
            null
        }
    }

    @OptIn(ExperimentalSerializationApi::class)
    override suspend fun store(serverId: String, manifest: CatalogManifest) = withContext(Dispatchers.IO) {
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

    fun markTestingServer(serverId: String) {
        val names = testingResidue.getStringSet("cache_names", emptySet()).orEmpty().toMutableSet()
        names += cacheName(serverId)
        testingResidue.edit().putStringSet("cache_names", names).commit()
    }

    fun remove(serverId: String) {
        AtomicFile(cacheFile(serverId)).delete()
        val names = testingResidue.getStringSet("cache_names", emptySet()).orEmpty().toMutableSet()
        names -= cacheName(serverId)
        testingResidue.edit().putStringSet("cache_names", names).commit()
    }

    /** Removes derived data from a testing session that ended by process death. */
    fun clearTestingResidue() {
        testingResidue.getStringSet("cache_names", emptySet()).orEmpty().forEach { name ->
            AtomicFile(File(directory, "$name.json")).delete()
        }
        testingResidue.edit().clear().commit()
    }

    private fun cacheFile(serverId: String): File {
        return File(directory, "${cacheName(serverId)}.json")
    }

    private fun cacheName(serverId: String): String =
        MessageDigest.getInstance("SHA-256")
            .digest(serverId.toByteArray(Charsets.UTF_8))
            .joinToString("") { byte -> (byte.toInt() and 0xff).toString(16).padStart(2, '0') }
}
