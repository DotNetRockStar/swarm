/**
 * Probes this device's real decoder and display capabilities so playback
 * negotiation advertises what the hardware can actually play, instead of the
 * conservative [CapabilityProfile.fireTvBaseline]. The server then only
 * transcodes what this device genuinely cannot decode — a 4K HEVC/HDR source
 * on a capable Fire TV is copied, not re-encoded.
 *
 * Modelled on [AndroidProblemReportDiagnostics]: every individual probe is
 * wrapped in `runCatching` and degrades to the baseline value for that field,
 * so a quirky HAL can never make negotiation worse than it is today. Pure
 * token/merge logic lives in [CapabilityMapping] for unit testing; the
 * `android.media`/`android.view` enumeration here is exercised only by the
 * instrumented suite.
 */
package app.swarm.tv.app.data

import android.content.Context
import android.hardware.display.DisplayManager
import android.media.MediaCodecInfo
import android.media.MediaCodecInfo.CodecProfileLevel
import android.media.MediaCodecList
import android.util.Log
import android.view.Display
import app.swarm.tv.core.capability.CapabilityProfile

private const val TAG = "CapabilityProbe"

private val VIDEO_MIME_TO_CODEC = linkedMapOf(
    "video/hevc" to "hevc",
    "video/av01" to "av1",
    "video/x-vnd.on2.vp9" to "vp9",
    "video/avc" to "h264",
)

private val AUDIO_MIME_TO_CODEC = linkedMapOf(
    "audio/mp4a-latm" to "aac",
    "audio/ac3" to "ac3",
    "audio/eac3" to "eac3",
    "audio/mpeg" to "mp3",
    "audio/opus" to "opus",
    "audio/flac" to "flac",
    "audio/vorbis" to "vorbis",
)

class AndroidCapabilityProbe(context: Context) {
    private val appContext = context.applicationContext

    /**
     * Runs on an IO dispatcher (enumerating codecs touches the media HAL).
     * Always returns a usable profile.
     */
    fun probe(): CapabilityProfile {
        val decoderMimes = runCatching { decoderMimeTypes() }
            .onFailure { Log.w(TAG, "codec enumeration failed", it) }
            .getOrDefault(emptySet())

        val videoCodecs = mutableListOf<String>()
        var decoderMaxWidth = 0
        var decoderMaxHeight = 0
        var maxBitrate = 0L

        val codecList = MediaCodecList(MediaCodecList.REGULAR_CODECS)
        for ((mime, canonical) in VIDEO_MIME_TO_CODEC) {
            if (mime !in decoderMimes) continue
            runCatching {
                val caps = videoCapabilitiesFor(codecList, mime) ?: return@runCatching
                val video = caps.videoCapabilities ?: return@runCatching
                videoCodecs += videoCodecToken(canonical, supportsTenBit(mime, caps))
                decoderMaxWidth = maxOf(decoderMaxWidth, video.supportedWidths.upper)
                decoderMaxHeight = maxOf(decoderMaxHeight, video.supportedHeights.upper)
                maxBitrate = maxOf(maxBitrate, video.bitrateRange.upper.toLong())
            }.onFailure { Log.w(TAG, "video capability probe failed for $mime", it) }
        }

        val audioCodecs = AUDIO_MIME_TO_CODEC.entries
            .filter { it.key in decoderMimes }
            .map { it.value }

        val display = runCatching { primaryDisplay() }.getOrNull()
        val (panelWidth, panelHeight) = display?.let { runCatching { panelResolution(it) }.getOrNull() }
            ?: (0 to 0)
        val hdr = display?.let { runCatching { displaySupportsHdr(it) }.getOrDefault(false) } ?: false

        // A decoder can report a surface larger than the panel; there is no
        // point advertising 4K to the server when the screen is 1080p.
        val maxWidth = clampToPanel(decoderMaxWidth, panelWidth)
        val maxHeight = clampToPanel(decoderMaxHeight, panelHeight)

        return mergeWithBaseline(
            probedContainers = listOf("mp4", "hls", "mkv"),
            probedVideoCodecs = videoCodecs,
            probedAudioCodecs = audioCodecs,
            probedMaxWidth = maxWidth,
            probedMaxHeight = maxHeight,
            probedMaxBitrate = maxBitrate,
            probedHdr = hdr,
        )
    }

    private fun decoderMimeTypes(): Set<String> =
        MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos
            .asSequence()
            .filterNot { it.isEncoder }
            .flatMap { it.supportedTypes.asSequence() }
            .map { it.lowercase() }
            .toSet()

    private fun videoCapabilitiesFor(
        list: MediaCodecList,
        mime: String,
    ): MediaCodecInfo.CodecCapabilities? =
        list.codecInfos
            .asSequence()
            .filterNot { it.isEncoder }
            .filter { info -> info.supportedTypes.any { it.equals(mime, ignoreCase = true) } }
            .mapNotNull { runCatching { it.getCapabilitiesForType(mime) }.getOrNull() }
            .firstOrNull()

    private fun supportsTenBit(mime: String, caps: MediaCodecInfo.CodecCapabilities): Boolean {
        val tenBitProfiles: Set<Int> = when {
            mime.endsWith("hevc") -> setOf(
                CodecProfileLevel.HEVCProfileMain10,
                CodecProfileLevel.HEVCProfileMain10HDR10,
                CodecProfileLevel.HEVCProfileMain10HDR10Plus,
            )
            mime.endsWith("av01") -> setOf(
                CodecProfileLevel.AV1ProfileMain10,
                CodecProfileLevel.AV1ProfileMain10HDR10,
                CodecProfileLevel.AV1ProfileMain10HDR10Plus,
            )
            mime.endsWith("vp9") -> setOf(
                CodecProfileLevel.VP9Profile2,
                CodecProfileLevel.VP9Profile3,
                CodecProfileLevel.VP9Profile2HDR,
                CodecProfileLevel.VP9Profile3HDR,
            )
            mime.endsWith("avc") -> setOf(CodecProfileLevel.AVCProfileHigh10)
            else -> emptySet()
        }
        return caps.profileLevels.orEmpty().any { it.profile in tenBitProfiles }
    }

    private fun primaryDisplay(): Display? =
        (appContext.getSystemService(Context.DISPLAY_SERVICE) as? DisplayManager)
            ?.getDisplay(Display.DEFAULT_DISPLAY)

    private fun panelResolution(display: Display): Pair<Int, Int> {
        val mode = display.mode ?: return 0 to 0
        val width = maxOf(mode.physicalWidth, mode.physicalHeight)
        val height = minOf(mode.physicalWidth, mode.physicalHeight)
        return width to height
    }

    private fun displaySupportsHdr(display: Display): Boolean {
        val types = display.hdrCapabilities?.supportedHdrTypes ?: return false
        return types.isNotEmpty()
    }

    private fun clampToPanel(decoderMax: Int, panel: Int): Int = when {
        decoderMax <= 0 -> 0
        panel <= 0 -> decoderMax
        else -> minOf(decoderMax, panel)
    }
}
