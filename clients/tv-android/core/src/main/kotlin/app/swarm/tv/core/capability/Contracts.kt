/**
 * Client playback capability profile, hand-mirrored from
 * `swarm-core::capability` (Rust) — sent once per signaling session so a
 * server can decide direct play vs. transcode and prune the ABR ladder.
 */
package app.swarm.tv.core.capability

import kotlinx.serialization.Serializable

@Serializable
data class CapabilityProfile(
    val containers: List<String>,
    val videoCodecs: List<String>,
    val audioCodecs: List<String>,
    val maxWidth: Int,
    val maxHeight: Int,
    val maxBitrate: Long,
    val hdr: Boolean = false,
) {
    companion object {
        /** Baseline every Fire OS 6+ device satisfies — matches `CapabilityProfile::fire_tv_baseline()` (Rust). */
        fun fireTvBaseline() = CapabilityProfile(
            containers = listOf("mp4", "hls"),
            videoCodecs = listOf("h264:high@4.2"),
            audioCodecs = listOf("aac", "ac3", "mp3"),
            maxWidth = 1920,
            maxHeight = 1080,
            maxBitrate = 12_000_000,
            hdr = false,
        )
    }
}
