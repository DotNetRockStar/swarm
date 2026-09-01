package app.swarm.tv.app.data

import app.swarm.tv.core.capability.CapabilityProfile
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class CapabilityMappingTest {
    @Test
    fun `video tokens carry the profile the server needs to gate 10-bit passthrough`() {
        assertEquals("h264:high", videoCodecToken("h264", tenBit = false))
        assertEquals("hevc:main", videoCodecToken("hevc", tenBit = false))
        assertEquals("hevc:main10", videoCodecToken("hevc", tenBit = true))
        assertEquals("av1:main10", videoCodecToken("av01", tenBit = true))
        assertEquals("vp9:profile0", videoCodecToken("vp9", tenBit = false))
    }

    @Test
    fun `merge unions codecs, grows dimensions, and never drops a baseline capability`() {
        val merged = mergeWithBaseline(
            probedContainers = listOf("mkv"),
            probedVideoCodecs = listOf("hevc:main10"),
            probedAudioCodecs = listOf("eac3"),
            probedMaxWidth = 3840,
            probedMaxHeight = 2160,
            probedMaxBitrate = 60_000_000,
            probedHdr = true,
        )
        // Baseline codecs survive, probed ones are added.
        assertTrue(merged.videoCodecs.contains("h264:high@4.2"))
        assertTrue(merged.videoCodecs.contains("hevc:main10"))
        assertTrue(merged.audioCodecs.containsAll(listOf("aac", "ac3", "eac3")))
        assertTrue(merged.containers.contains("mkv"))
        assertEquals(3840, merged.maxWidth)
        assertEquals(2160, merged.maxHeight)
        assertEquals(60_000_000, merged.maxBitrate)
        assertTrue(merged.hdr)
    }

    @Test
    fun `a fully failed probe degrades to exactly the baseline`() {
        val merged = mergeWithBaseline(
            probedContainers = emptyList(),
            probedVideoCodecs = emptyList(),
            probedAudioCodecs = emptyList(),
            probedMaxWidth = 0,
            probedMaxHeight = 0,
            probedMaxBitrate = 0,
            probedHdr = false,
        )
        val baseline = CapabilityProfile.fireTvBaseline()
        assertEquals(baseline.maxWidth, merged.maxWidth)
        assertEquals(baseline.maxHeight, merged.maxHeight)
        assertEquals(baseline.maxBitrate, merged.maxBitrate)
        assertEquals(baseline.hdr, merged.hdr)
        assertTrue(merged.videoCodecs.containsAll(baseline.videoCodecs.map { it.lowercase() }))
    }

    @Test
    fun `an absurd decoder bitrate is clamped to a sane ceiling`() {
        val merged = mergeWithBaseline(
            probedContainers = emptyList(),
            probedVideoCodecs = emptyList(),
            probedAudioCodecs = emptyList(),
            probedMaxWidth = 0,
            probedMaxHeight = 0,
            probedMaxBitrate = Long.MAX_VALUE,
            probedHdr = false,
        )
        assertEquals(MAX_ADVERTISED_BITRATE_BPS, merged.maxBitrate)
    }
}
