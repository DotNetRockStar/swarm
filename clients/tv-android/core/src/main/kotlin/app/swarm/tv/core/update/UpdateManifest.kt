package app.swarm.tv.core.update

import kotlinx.serialization.Serializable

/**
 * `tv-latest.json`, published on every `swarm` GitHub release next to the
 * split APKs. Snake_case on the wire via [app.swarm.tv.core.rest.SwarmJson],
 * matching how the release workflow renders it.
 */
@Serializable
data class UpdateManifest(
    val versionCode: Long,
    val versionName: String,
    val notes: String = "",
    val minSdkVersion: Int = 0,
    /** ABI (`arm64-v8a`, `armeabi-v7a`) -> the APK built for it. */
    val assets: Map<String, UpdateAsset> = emptyMap(),
)

@Serializable
data class UpdateAsset(
    val url: String,
    /** Lowercase hex SHA-256 of the APK; verified before the install prompt. */
    val sha256: String,
)
