/**
 * Plays [url] — a negotiated budgeted-direct or HLS URL from
 * [app.swarm.tv.core.catalog.CatalogSession.preparePlayback] — through a
 * standard Media3 `ExoPlayer`/`PlayerView`. ExoPlayer's own HTTP data
 * source needs nothing more exotic than what
 * [app.swarm.tv.core.proxy.PeerLoopbackProxy] already speaks (proven with a
 * plain OkHttp client in `PeerQuicClientInteropTest`), so this screen is
 * mechanical: point it at the local proxy URL like any other HTTP stream.
 *
 * Resume/watched state: seeks to [resumePositionSecs] on prepare, and
 * reports the final position back via [onPositionUpdate] when the screen
 * is disposed (back pressed, or navigated away from) — this screen itself
 * knows nothing about persistence, `SwarmViewModel` owns deciding what to
 * do with that report. Deliberately save-on-exit only, not a periodic
 * save while playing — simpler, and the only gap it leaves is losing the
 * position on a crash mid-playback rather than on a normal exit, which
 * isn't worth a repeating timer tied to Compose lifecycle for a first
 * version.
 *
 * Continue/autoplay: when [hasNext] is true and playback reaches
 * `Player.STATE_ENDED` naturally (not on back-press/manual exit — those
 * still go straight through [onBack] as before), a brief overlay offers
 * to play [nextTitle] next, auto-confirming after a few seconds unless
 * cancelled. Manual-exit resume/position-save is untouched either way —
 * [onDispose] always fires the same report regardless of which path led
 * to disposal.
 */
package app.swarm.tv.app.ui.screens

import android.media.audiofx.LoudnessEnhancer
import android.net.Uri
import android.content.res.ColorStateList
import android.view.ViewGroup
import android.widget.ImageButton
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.media3.common.C
import androidx.media3.common.ForwardingPlayer
import androidx.media3.common.MediaItem
import androidx.media3.common.MimeTypes
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.Timeline
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.HttpDataSource
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.upstream.DefaultBandwidthMeter
import androidx.media3.ui.PlayerView
import androidx.tv.material3.Button
import app.swarm.tv.app.ui.components.SwarmLoadingIndicator
import app.swarm.tv.app.ui.components.swarmActionButtonColors
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmAccentHot
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmText
import app.swarm.tv.core.peer.PlaybackMode
import app.swarm.tv.core.peer.SubtitleTrack
import kotlinx.coroutines.delay

private const val CONTINUE_COUNTDOWN_SECS = 8

// Real complaint from live use: even at max system/TV volume, some content isn't
// loud enough. LoudnessEnhancer processes the decoded PCM before it reaches the
// audio sink, so it can boost past what raising system volume alone can reach.
// 1000 millibels (10dB) is a noticeable default lift chosen to stay clear of the
// audible clipping/distortion louder passages start to show past ~15-20dB of gain.
private const val AUDIO_BOOST_MILLIBELS = 1000

@androidx.annotation.OptIn(UnstableApi::class)
@Composable
fun PlayerScreen(
    url: String,
    title: String,
    playbackMode: PlaybackMode,
    resumePositionSecs: Double,
    positionOffsetSecs: Double,
    mediaDurationSecs: Double?,
    maxBitrate: Long,
    subtitles: List<SubtitleTrack>,
    hasNext: Boolean,
    nextTitle: String?,
    onBack: () -> Unit,
    onPositionUpdate: (positionSecs: Double, durationSecs: Double) -> Unit,
    onContinue: () -> Unit,
    onSeekOutsideBuffer: (positionSecs: Double) -> Unit,
    onPlaybackSessionExpired: (positionSecs: Double, context: String?) -> Unit,
    onPlaybackRuntimeError: (message: String, context: String?) -> Unit,
) {
    BackHandler(onBack = onBack)
    val context = LocalContext.current
    var showContinuePrompt by remember(url) { mutableStateOf(false) }
    // Covers the gap between "screen opened" and "a frame is actually up" —
    // negotiation already succeeded by the time this screen exists, but the
    // player still has to buffer its first segment(s) over the peer proxy,
    // which visibly took long enough on real hardware to look broken with
    // nothing on screen but a black box. Keyed on url so autoplaying into
    // the next episode (a fresh ExoPlayer instance, same screen) shows it
    // again rather than staying permanently dismissed from the first play.
    var isLoading by remember(url) { mutableStateOf(true) }

    val player = remember(url, maxBitrate, subtitles) {
        // The HTTP URL is loopback, so Media3's network-type-based initial
        // estimate describes the TV's Wi-Fi rather than the media server's
        // constrained uplink. Start conservatively; segment transfer samples
        // will replace this estimate as playback proceeds.
        val initialBitrate = maxBitrate.coerceIn(250_000L, 2_000_000L)
        val bandwidthMeter = DefaultBandwidthMeter.Builder(context)
            .setInitialBitrateEstimate(initialBitrate)
            .build()
        ExoPlayer.Builder(context).setBandwidthMeter(bandwidthMeter).build().apply {
            // Direct-play containers may expose several embedded audio tracks.
            // Prefer English when it is tagged, while Media3 naturally falls
            // back to the container default when no English track exists.
            trackSelectionParameters = trackSelectionParameters
                .buildUpon()
                .setPreferredAudioLanguages("en", "eng")
                .build()
            val subtitleConfigurations = subtitles.map { track ->
                MediaItem.SubtitleConfiguration.Builder(Uri.parse(track.path))
                    .setMimeType(MimeTypes.TEXT_VTT)
                    .setLanguage(track.language)
                    .setLabel(track.label)
                    .setSelectionFlags(C.SELECTION_FLAG_DEFAULT)
                    .build()
            }
            setMediaItem(
                MediaItem.Builder()
                    .setUri(Uri.parse(url))
                    .setMediaId(title)
                    .setSubtitleConfigurations(subtitleConfigurations)
                    .build(),
            )
            if (resumePositionSecs > 0) {
                seekTo((resumePositionSecs * 1000).toLong())
            }
            playWhenReady = true
            prepare()
        }
    }
    // An EVENT HLS playlist grows as ffmpeg produces segments, so its native
    // Media3 timeline ends at the generated/buffered edge rather than at the
    // end of the movie. Present the catalog's probed full duration to the
    // controller. Seeks inside the generated window stay local; seeks beyond
    // it ask the ViewModel to replace this session with one transcoding from
    // the requested absolute position. Direct play uses the same full-length
    // display but continues to seek normally through HTTP Range requests.
    val controllerPlayer = remember(player, playbackMode, positionOffsetSecs, mediaDurationSecs) {
        FullDurationPlayer(
            delegate = player,
            fullDurationMs = mediaDurationSecs
                ?.takeIf { it.isFinite() && it > 0.0 }
                ?.let { (it * 1000.0).toLong() },
            positionOffsetMs = (positionOffsetSecs * 1000.0).toLong(),
            restartOutsideAvailableWindow = playbackMode == PlaybackMode.HLS,
            onRestartAt = { positionMs -> onSeekOutsideBuffer(positionMs / 1000.0) },
        )
    }
    val loudnessEnhancer = remember(player) {
        // player.audioSessionId is generated by ExoPlayer's audio sink at
        // construction time, so it's already valid here, before prepare()/playback
        // start. Guarded with runCatching: a small number of OEM audio HALs reject
        // LoudnessEnhancer for a given session, and that's not worth failing
        // playback over — it just means this specific device plays at unboosted
        // volume.
        runCatching {
            LoudnessEnhancer(player.audioSessionId).apply {
                setTargetGain(AUDIO_BOOST_MILLIBELS)
                enabled = true
            }
        }.getOrNull()
    }

    DisposableEffect(player) {
        val listener = object : Player.Listener {
            override fun onPlaybackStateChanged(playbackState: Int) {
                // Covers audio (tracks never render a video frame at all,
                // so onRenderedFirstFrame below would never fire for them).
                if (playbackState == Player.STATE_READY) {
                    isLoading = false
                }
                if (playbackState == Player.STATE_ENDED && hasNext) {
                    showContinuePrompt = true
                }
            }

            // Fires strictly before STATE_READY can be observed for video,
            // so relying on it too (whichever comes first) avoids a brief
            // dead frame between "ready" and pixels actually landing.
            override fun onRenderedFirstFrame() {
                isLoading = false
            }

            // Runtime failures after negotiation already succeeded (network
            // drop mid-stream, a decoder/codec error) — distinct from, and
            // not caught by, the negotiation-failure path in SwarmViewModel.
            // Playback-triage-worthy either way, so it gets the same report.
            override fun onPlayerError(error: PlaybackException) {
                val causes = generateSequence<Throwable>(error) { it.cause }
                    .take(6)
                    .joinToString(" -> ") { cause ->
                        val detail = cause.message?.takeIf { it.isNotBlank() }
                        if (detail == null) cause.javaClass.simpleName else "${cause.javaClass.simpleName}: $detail"
                    }
                val context = buildString {
                    append("error_code=").append(error.errorCode)
                    append("; error_code_name=").append(error.errorCodeName)
                    append("; position_ms=").append(player.currentPosition)
                    append("; buffered_position_ms=").append(player.bufferedPosition)
                    append("; duration_ms=").append(player.duration)
                    append("; playback_state=").append(player.playbackState)
                    append("; play_when_ready=").append(player.playWhenReady)
                    append("; causes=").append(causes)
                }

                // Playback URLs name a server-side session that is removed
                // after five idle minutes. A long pause therefore makes the
                // next HLS segment/direct-play Range request return 404 even
                // though the asset still exists. Preparing this same URL
                // again can never heal it: negotiate a fresh session at the
                // absolute playhead instead. Other 404s and all other player
                // failures retain the normal reporting path below.
                val responseCode = generateSequence<Throwable>(error) { it.cause }
                    .filterIsInstance<HttpDataSource.InvalidResponseCodeException>()
                    .firstOrNull()
                    ?.responseCode
                if (shouldRecoverExpiredPlaybackSession(error.errorCode, responseCode)) {
                    isLoading = true
                    val positionSecs = positionOffsetSecs + player.currentPosition.coerceAtLeast(0L) / 1000.0
                    onPlaybackSessionExpired(positionSecs, context)
                    return
                }

                isLoading = false
                onPlaybackRuntimeError(
                    "${error.message ?: "Playback failed"} (${error.errorCodeName})",
                    context,
                )
            }
        }
        player.addListener(listener)
        onDispose {
            player.removeListener(listener)
            val positionSecs = positionOffsetSecs + player.currentPosition / 1000.0
            val durationSecs = player.duration.takeIf { it != C.TIME_UNSET }?.let { positionOffsetSecs + it / 1000.0 } ?: 0.0
            onPositionUpdate(positionSecs, durationSecs)
            loudnessEnhancer?.release()
            player.release()
        }
    }

    // Letterbox/pillarbox areas around the video frame must be plain black,
    // not the app's own themed background (a dark blue) showing through —
    // this Box had no background of its own, so it inherited whatever was
    // behind it.
    Box(modifier = Modifier.fillMaxSize().background(Color.Black)) {
        AndroidView(
            modifier = Modifier.fillMaxSize(),
            factory = { ctx ->
                PlayerView(ctx).apply {
                    useController = true
                    applySwarmPlaybackControlColors(this)
                }
            },
            // Real bug, found live: autoplaying the next episode creates a
            // brand-new ExoPlayer (`remember(url, maxBitrate)` above keys on
            // the new url), but `factory` only ever runs once for a given
            // AndroidView call site — without this `update`, the on-screen
            // PlayerView stayed bound to the *previous*, already-released
            // player forever. Audio kept working regardless (ExoPlayer's
            // audio output doesn't need a bound view, only video rendering
            // does), which is exactly why sound played over a frozen frame
            // showing the old episode's final "ended" progress bar.
            update = { view -> view.player = controllerPlayer },
        )

        if (isLoading) {
            Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                SwarmLoadingIndicator(onBlackBackground = true)
            }
        }

        if (showContinuePrompt) {
            ContinueOverlay(
                nextTitle = nextTitle,
                onPlayNow = { showContinuePrompt = false; onContinue() },
                onCancel = { showContinuePrompt = false; onBack() },
            )
        }
    }
}

/** Only a missing negotiated stream is self-healable. A 404 reported under
 * another Media3 category, or a different bad HTTP status, needs the normal
 * error path instead of an automatic replay loop. */
internal fun shouldRecoverExpiredPlaybackSession(errorCode: Int, responseCode: Int?): Boolean =
    errorCode == PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS && responseCode == 404

/**
 * Gives Media3's stock TV controller an absolute, full-length timeline even
 * when the underlying HLS EVENT playlist currently contains only the first
 * few generated segments.
 */
@androidx.annotation.OptIn(UnstableApi::class)
private class FullDurationPlayer(
    delegate: Player,
    private val fullDurationMs: Long?,
    private val positionOffsetMs: Long,
    private val restartOutsideAvailableWindow: Boolean,
    private val onRestartAt: (Long) -> Unit,
) : ForwardingPlayer(delegate) {
    private var restartRequested = false

    override fun getDuration(): Long = fullDurationMs ?: super.getDuration()

    override fun getContentDuration(): Long = duration

    override fun getCurrentPosition(): Long = absolutePosition(super.getCurrentPosition())

    override fun getContentPosition(): Long = absolutePosition(super.getContentPosition())

    override fun getBufferedPosition(): Long = absolutePosition(super.getBufferedPosition())

    override fun getContentBufferedPosition(): Long = absolutePosition(super.getContentBufferedPosition())

    override fun getBufferedPercentage(): Int {
        val duration = fullDurationMs ?: return super.getBufferedPercentage()
        if (duration <= 0) return 0
        return ((bufferedPosition * 100L) / duration).coerceIn(0L, 100L).toInt()
    }

    override fun getCurrentTimeline(): Timeline {
        val timeline = super.getCurrentTimeline()
        val duration = fullDurationMs ?: return timeline
        return if (timeline.isEmpty) timeline else FullDurationTimeline(timeline, duration)
    }

    override fun isCurrentMediaItemSeekable(): Boolean = fullDurationMs != null || super.isCurrentMediaItemSeekable

    override fun isCurrentMediaItemDynamic(): Boolean = false

    override fun isCurrentMediaItemLive(): Boolean = false

    override fun seekTo(positionMs: Long) {
        seekTo(currentMediaItemIndex, positionMs)
    }

    override fun seekTo(mediaItemIndex: Int, positionMs: Long) {
        val target = clampToFullDuration(positionMs)
        val availableDuration = super.getDuration()
        val relativeTarget = target - positionOffsetMs
        val outsideAvailableWindow = shouldRestartHlsPlaybackForSeek(relativeTarget, availableDuration)

        if (restartOutsideAvailableWindow && outsideAvailableWindow) {
            if (!restartRequested) {
                restartRequested = true
                pause()
                onRestartAt(target)
            }
            return
        }
        super.seekTo(mediaItemIndex, relativeTarget.coerceAtLeast(0L))
    }

    override fun seekForward() {
        seekTo(currentPosition + seekForwardIncrement)
    }

    override fun seekBack() {
        seekTo(currentPosition - seekBackIncrement)
    }

    override fun seekToDefaultPosition() {
        seekTo(0L)
    }

    override fun seekToDefaultPosition(mediaItemIndex: Int) {
        seekTo(mediaItemIndex, 0L)
    }

    private fun absolutePosition(relativeMs: Long): Long =
        if (relativeMs == C.TIME_UNSET) relativeMs else positionOffsetMs + relativeMs

    private fun clampToFullDuration(positionMs: Long): Long =
        fullDurationMs?.let { positionMs.coerceIn(0L, it) } ?: positionMs.coerceAtLeast(0L)
}

internal fun shouldRestartHlsPlaybackForSeek(relativeTargetMs: Long, availableDurationMs: Long): Boolean =
    relativeTargetMs < 0L || availableDurationMs == C.TIME_UNSET || relativeTargetMs > availableDurationMs

/** Single-item timeline facade used by [FullDurationPlayer]. */
private class FullDurationTimeline(
    private val delegate: Timeline,
    fullDurationMs: Long,
) : Timeline() {
    private val fullDurationUs = fullDurationMs * 1000L

    override fun getWindowCount(): Int = delegate.windowCount

    override fun getWindow(windowIndex: Int, window: Window, defaultPositionProjectionUs: Long): Window =
        delegate.getWindow(windowIndex, window, defaultPositionProjectionUs).apply {
            durationUs = fullDurationUs
            defaultPositionUs = 0L
            positionInFirstPeriodUs = 0L
            isSeekable = true
            isDynamic = false
            liveConfiguration = null
        }

    override fun getPeriodCount(): Int = delegate.periodCount

    override fun getPeriod(periodIndex: Int, period: Period, setIds: Boolean): Period =
        delegate.getPeriod(periodIndex, period, setIds).apply {
            durationUs = fullDurationUs
            positionInWindowUs = 0L
        }

    override fun getIndexOfPeriod(uid: Any): Int = delegate.getIndexOfPeriod(uid)

    override fun getUidOfPeriod(periodIndex: Int): Any = delegate.getUidOfPeriod(periodIndex)
}

/** Media3 owns the movie/show transport UI, so tint its native buttons with
 * the same white -> cyan -> hot interaction sequence as Compose actions. */
private fun applySwarmPlaybackControlColors(root: ViewGroup) {
    val states = arrayOf(
        intArrayOf(android.R.attr.state_pressed),
        intArrayOf(android.R.attr.state_focused),
        intArrayOf(),
    )
    val colors = intArrayOf(SwarmAccentHot.toArgb(), SwarmAccent.toArgb(), Color.White.toArgb())
    for (index in 0 until root.childCount) {
        when (val child = root.getChildAt(index)) {
            is ImageButton -> child.imageTintList = ColorStateList(states, colors)
            is ViewGroup -> applySwarmPlaybackControlColors(child)
        }
    }
}

@Composable
private fun ContinueOverlay(nextTitle: String?, onPlayNow: () -> Unit, onCancel: () -> Unit) {
    var secondsLeft by remember { mutableStateOf(CONTINUE_COUNTDOWN_SECS) }
    LaunchedEffect(Unit) {
        while (secondsLeft > 0) {
            delay(1000)
            secondsLeft -= 1
        }
        onPlayNow()
    }
    val playNowFocusRequester = remember { FocusRequester() }
    LaunchedEffect(Unit) { playNowFocusRequester.requestFocus() }

    Box(modifier = Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.78f)), contentAlignment = Alignment.BottomEnd) {
        Column(modifier = Modifier.padding(40.dp).width(340.dp)) {
            Text("Up next", color = SwarmMuted, fontSize = 13.sp)
            Spacer(Modifier.height(4.dp))
            Text(nextTitle ?: "Next episode", color = SwarmText, fontSize = 18.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.height(16.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Button(
                    onClick = onPlayNow,
                    modifier = Modifier.focusRequester(playNowFocusRequester),
                    colors = swarmActionButtonColors(),
                ) {
                    Text("Play now ($secondsLeft)", color = Color(0xFF04263A), fontSize = 14.sp, fontWeight = FontWeight.Bold)
                }
                Button(onClick = onCancel, colors = swarmActionButtonColors()) {
                    Text("Cancel", fontSize = 14.sp)
                }
            }
        }
    }
}
