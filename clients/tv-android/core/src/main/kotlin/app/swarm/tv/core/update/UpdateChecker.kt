package app.swarm.tv.core.update

import app.swarm.tv.core.rest.SwarmJson
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.SerializationException
import kotlinx.serialization.decodeFromString
import okhttp3.OkHttpClient
import okhttp3.Request
import java.io.File
import java.io.IOException
import java.security.MessageDigest

/** Where the Fire TV client looks for its own updates. */
const val DEFAULT_TV_MANIFEST_URL =
    "https://github.com/SWARM-Media-Steaming/swarm/releases/latest/download/tv-latest.json"

sealed interface UpdateStatus {
    data object UpToDate : UpdateStatus
    data class Available(val manifest: UpdateManifest, val asset: UpdateAsset) : UpdateStatus
    data class Error(val message: String) : UpdateStatus
}

/**
 * Fetches [DEFAULT_TV_MANIFEST_URL], compares [UpdateManifest.versionCode] to
 * the running build, and (when newer) downloads + hashes the APK for this
 * device's ABI. Handing the verified file to the system installer is the
 * caller's job — that needs an Android `Context` and lives in the app module.
 */
class UpdateChecker(
    private val http: OkHttpClient = OkHttpClient(),
    private val manifestUrl: String = DEFAULT_TV_MANIFEST_URL,
) {
    suspend fun check(currentVersionCode: Long, supportedAbis: List<String>): UpdateStatus =
        withContext(Dispatchers.IO) {
            val manifest = try {
                val body = http.newCall(Request.Builder().url(manifestUrl).build()).execute().use { response ->
                    if (!response.isSuccessful) {
                        return@withContext UpdateStatus.Error("update manifest fetch failed: HTTP ${response.code}")
                    }
                    response.body?.string().orEmpty()
                }
                SwarmJson.decodeFromString<UpdateManifest>(body)
            } catch (error: IOException) {
                return@withContext UpdateStatus.Error("could not reach the update server: ${error.message ?: "network error"}")
            } catch (error: SerializationException) {
                return@withContext UpdateStatus.Error("update manifest was not readable: ${error.message ?: "bad JSON"}")
            }

            if (manifest.versionCode <= currentVersionCode) {
                return@withContext UpdateStatus.UpToDate
            }
            val asset = supportedAbis.firstNotNullOfOrNull { abi -> manifest.assets[abi] }
                ?: return@withContext UpdateStatus.Error(
                    "no update APK for this device (${supportedAbis.joinToString()})"
                )
            UpdateStatus.Available(manifest, asset)
        }

    /** Downloads [asset] into [target], verifying its SHA-256. Throws on any mismatch. */
    suspend fun download(asset: UpdateAsset, target: File): File = withContext(Dispatchers.IO) {
        target.parentFile?.mkdirs()
        val digest = MessageDigest.getInstance("SHA-256")
        http.newCall(Request.Builder().url(asset.url).build()).execute().use { response ->
            if (!response.isSuccessful) throw IOException("APK download failed: HTTP ${response.code}")
            val source = response.body?.byteStream() ?: throw IOException("APK download returned no body")
            target.outputStream().use { sink ->
                val buffer = ByteArray(64 * 1024)
                while (true) {
                    val read = source.read(buffer)
                    if (read < 0) break
                    digest.update(buffer, 0, read)
                    sink.write(buffer, 0, read)
                }
            }
        }
        val actual = digest.digest().joinToString("") { "%02x".format(it) }
        if (!actual.equals(asset.sha256, ignoreCase = true)) {
            target.delete()
            throw IOException("downloaded APK failed its checksum")
        }
        target
    }
}
