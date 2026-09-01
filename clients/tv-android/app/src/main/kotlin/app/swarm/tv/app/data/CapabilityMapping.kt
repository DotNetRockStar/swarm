/**
 * Pure helpers for turning probed decoder facts into a
 * [app.swarm.tv.core.capability.CapabilityProfile]. Kept free of any
 * `android.*` types so it is unit-testable without Robolectric — the Android
 * `MediaCodecList`/`Display` enumeration lives in [AndroidCapabilityProbe].
 */
package app.swarm.tv.app.data

import app.swarm.tv.core.capability.CapabilityProfile

/** A generous ceiling so a decoder that reports an absurd bitrate range does
 * not let the server pick a rendition no real link could carry. */
const val MAX_ADVERTISED_BITRATE_BPS = 100_000_000L

/**
 * Capability token the server matches against, e.g. `"hevc:main10"`,
 * `"h264:high"`, `"av1:main"`. The profile half matters — `main10` is what
 * tells the server a 10-bit / HDR source may be passed through untouched.
 * The level suffix is intentionally omitted: the server already bounds
 * playback by resolution and bitrate, and a device that advertises a codec
 * decodes it at the resolutions it also advertises.
 */
fun videoCodecToken(codec: String, tenBit: Boolean): String {
    val base = codec.trim().lowercase()
    val profile = when (base) {
        "h264", "avc" -> "high"
        "hevc", "h265" -> if (tenBit) "main10" else "main"
        "av1", "av01" -> if (tenBit) "main10" else "main"
        "vp9" -> if (tenBit) "profile2" else "profile0"
        else -> return base
    }
    val name = when (base) {
        "avc" -> "h264"
        "h265" -> "hevc"
        "av01" -> "av1"
        else -> base
    }
    return "$name:$profile"
}

/**
 * Fold the probed values together with the conservative baseline so a single
 * failed probe never *removes* a capability the baseline guarantees, and
 * never leaves a zeroed dimension. Video/audio codec lists are unioned;
 * numeric fields take the larger of probe vs. baseline; `hdr` is sticky-true.
 */
fun mergeWithBaseline(
    probedContainers: List<String>,
    probedVideoCodecs: List<String>,
    probedAudioCodecs: List<String>,
    probedMaxWidth: Int,
    probedMaxHeight: Int,
    probedMaxBitrate: Long,
    probedHdr: Boolean,
    baseline: CapabilityProfile = CapabilityProfile.fireTvBaseline(),
): CapabilityProfile {
    fun union(probe: List<String>, base: List<String>): List<String> =
        (base + probe).map { it.trim().lowercase() }.filter { it.isNotEmpty() }.distinct()

    return CapabilityProfile(
        containers = union(probedContainers, baseline.containers),
        videoCodecs = union(probedVideoCodecs, baseline.videoCodecs),
        audioCodecs = union(probedAudioCodecs, baseline.audioCodecs),
        maxWidth = maxOf(probedMaxWidth, baseline.maxWidth).coerceAtLeast(1),
        maxHeight = maxOf(probedMaxHeight, baseline.maxHeight).coerceAtLeast(1),
        maxBitrate = probedMaxBitrate
            .coerceIn(baseline.maxBitrate, MAX_ADVERTISED_BITRATE_BPS),
        hdr = probedHdr || baseline.hdr,
    )
}
