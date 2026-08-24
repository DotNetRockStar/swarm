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

import android.content.Context
import android.content.res.ColorStateList
import android.media.audiofx.LoudnessEnhancer
import android.net.Uri
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
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.media3.common.C
import androidx.media3.common.Format
import androidx.media3.common.ForwardingPlayer
import androidx.media3.common.MediaItem
import androidx.media3.common.MimeTypes
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.Timeline
import androidx.media3.common.Tracks
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.HttpDataSource
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.analytics.AnalyticsListener
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.exoplayer.source.LoadEventInfo
import androidx.media3.exoplayer.source.MediaLoadData
import androidx.media3.exoplayer.upstream.DefaultBandwidthMeter
import androidx.media3.exoplayer.upstream.DefaultLoadErrorHandlingPolicy
import androidx.media3.exoplayer.upstream.LoadErrorHandlingPolicy.LoadErrorInfo
import androidx.media3.ui.PlayerView
import androidx.tv.material3.Button
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import app.swarm.tv.app.data.episodeNumberLabel
import app.swarm.tv.app.data.pauseRecommendationTitle
import app.swarm.tv.app.data.PreparedEpisodePlayback
import app.swarm.tv.app.ui.components.SwarmLoadingIndicator
import app.swarm.tv.app.ui.components.swarmActionButtonColors
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmAccentHot
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmSurface
import app.swarm.tv.app.ui.theme.SwarmText
import app.swarm.tv.core.catalog.MergedEntry
import app.swarm.tv.core.catalog.displayTitle
import app.swarm.tv.core.peer.MediaKind
import app.swarm.tv.core.peer.PlaybackMode
import app.swarm.tv.core.peer.SubtitleTrack
import java.io.EOFException
import java.io.IOException
import java.net.ProtocolException
import java.net.SocketException
import java.net.SocketTimeoutException
import java.util.Locale
import kotlinx.coroutines.delay

private const val CONTINUE_COUNTDOWN_SECS = 8
private const val SERVER_OFFLINE_RETRY_DELAY_MS = 2_000L

// Real complaint from live use: even at max system/TV volume, some content isn't
// loud enough. LoudnessEnhancer processes the decoded PCM before it reaches the
// audio sink, so it can boost past what raising system volume alone can reach.
// 1000 millibels (10dB) is a noticeable default lift chosen to stay clear of the
// audible clipping/distortion louder passages start to show past ~15-20dB of gain.
private const val AUDIO_BOOST_MILLIBELS = 1000

private data class PlaybackPlayerConfig(
    val sessionId: String,
    val url: String,
    val title: String,
    val maxBitrate: Long,
    val subtitles: List<SubtitleTrack>,
    val resumePositionSecs: Double,
)

private fun PreparedEpisodePlayback.toPlayerConfig() = PlaybackPlayerConfig(
    sessionId = sessionId,
    url = url,
    title = title,
    maxBitrate = maxBitrate,
    subtitles = subtitles,
    resumePositionSecs = resumePositionSecs,
)

/** Media3 normally gives up after a small number of failed loads. A server
 * outage is different from a bad asset or decoder failure: keep requesting
 * the current Range/HLS segment so already-buffered media can play out and
 * playback can resume without user intervention when the route returns. */
private class ServerOfflineRetryPolicy : DefaultLoadErrorHandlingPolicy(Int.MAX_VALUE) {
    override fun getRetryDelayMsFor(loadErrorInfo: LoadErrorInfo): Long {
        val responseCode = httpResponseCode(loadErrorInfo.exception)
        return when {
            isServerOfflineLoadError(loadErrorInfo.exception) -> SERVER_OFFLINE_RETRY_DELAY_MS
            // A permanent HTTP response (especially the expired-session
            // 404 handled by onPlayerError) must not inherit the unlimited
            // transport retry count.
            responseCode != null -> C.TIME_UNSET
            else -> super.getRetryDelayMsFor(loadErrorInfo)
        }
    }
}

internal fun serverOfflineMediaSourceFactory(context: Context): DefaultMediaSourceFactory =
    DefaultMediaSourceFactory(context.applicationContext)
        .setLoadErrorHandlingPolicy(ServerOfflineRetryPolicy())

internal fun isServerOfflineHttpStatus(responseCode: Int): Boolean =
    responseCode == 500 || responseCode == 502 || responseCode == 503 || responseCode == 504

private fun httpResponseCode(error: IOException): Int? =
    generateSequence<Throwable>(error) { it.cause }
        .filterIsInstance<HttpDataSource.InvalidResponseCodeException>()
        .firstOrNull()
        ?.responseCode

/** Intentionally narrow: codec/parser failures and permanent 4xx asset
 * errors must still become terminal player errors instead of retry loops. */
internal fun isServerOfflineLoadError(error: IOException): Boolean =
    generateSequence<Throwable>(error) { it.cause }.any { cause ->
        when (cause) {
            is HttpDataSource.InvalidResponseCodeException -> isServerOfflineHttpStatus(cause.responseCode)
            is SocketException,
            is SocketTimeoutException,
            is EOFException,
            is ProtocolException,
            -> true
            else -> false
        }
    }

/** Owns the active video player plus at most one paused, buffering successor.
 * The pool lives across UiState.Player-to-UiState.Player recompositions, which
 * lets [activate] promote the exact preloaded ExoPlayer instead of throwing
 * away its buffer during the state handoff. */
@androidx.annotation.OptIn(UnstableApi::class)
private class VideoPlayerPool(context: Context) {
    private val appContext = context.applicationContext
    private var activeSessionId: String? = null
    private var activePlayer: ExoPlayer? = null
    private var preloadedSessionId: String? = null
    private var preloadedPlayer: ExoPlayer? = null

    fun activate(config: PlaybackPlayerConfig): ExoPlayer {
        if (activeSessionId == config.sessionId) return checkNotNull(activePlayer)

        var player = if (preloadedSessionId == config.sessionId) {
            preloadedSessionId = null
            preloadedPlayer.also { preloadedPlayer = null } ?: createPlayer(config, playWhenReady = true)
        } else {
            createPlayer(config, playWhenReady = true)
        }
        // A preload can fail while it has no UI listener. Recreate from the
        // still-valid negotiated URL on promotion so the active listener gets
        // a normal retry/error path instead of inheriting a silent terminal
        // player state.
        if (player.playerError != null) {
            player.release()
            player = createPlayer(config, playWhenReady = true)
        }
        activeSessionId = config.sessionId
        activePlayer = player
        player.playWhenReady = true
        return player
    }

    fun preload(config: PlaybackPlayerConfig) {
        if (activeSessionId == config.sessionId || preloadedSessionId == config.sessionId) return
        releasePreloaded()
        preloadedSessionId = config.sessionId
        preloadedPlayer = createPlayer(config, playWhenReady = false)
    }

    fun release(player: ExoPlayer) {
        if (activePlayer === player) {
            activePlayer = null
            activeSessionId = null
        }
        if (preloadedPlayer === player) {
            preloadedPlayer = null
            preloadedSessionId = null
        }
        player.release()
    }

    fun releasePreloaded() {
        preloadedPlayer?.release()
        preloadedPlayer = null
        preloadedSessionId = null
    }

    private fun createPlayer(config: PlaybackPlayerConfig, playWhenReady: Boolean): ExoPlayer {
        // The HTTP URL is loopback, so Media3's network-type-based initial
        // estimate describes the TV's Wi-Fi rather than the media server's
        // constrained uplink. Start conservatively; segment transfer samples
        // will replace this estimate as playback proceeds.
        val initialBitrate = config.maxBitrate.coerceIn(250_000L, 2_000_000L)
        val bandwidthMeter = DefaultBandwidthMeter.Builder(appContext)
            .setInitialBitrateEstimate(initialBitrate)
            .build()
        return ExoPlayer.Builder(appContext)
            .setBandwidthMeter(bandwidthMeter)
            .setMediaSourceFactory(serverOfflineMediaSourceFactory(appContext))
            .build()
            .apply {
            // Direct-play containers may expose several embedded audio tracks.
            // Prefer English when it is tagged, while Media3 naturally falls
            // back to the container default when no English track exists.
            trackSelectionParameters = trackSelectionParameters
                .buildUpon()
                .setPreferredAudioLanguages("en", "eng")
                .build()
            val subtitleConfigurations = config.subtitles.map { track ->
                MediaItem.SubtitleConfiguration.Builder(Uri.parse(track.path))
                    .setMimeType(MimeTypes.TEXT_VTT)
                    .setLanguage(track.language)
                    .setLabel(track.label)
                    .setSelectionFlags(C.SELECTION_FLAG_DEFAULT)
                    .build()
            }
            setMediaItem(
                MediaItem.Builder()
                    .setUri(Uri.parse(config.url))
                    .setMediaId(config.title)
                    .setSubtitleConfigurations(subtitleConfigurations)
                    .build(),
            )
            if (config.resumePositionSecs > 0) {
                seekTo((config.resumePositionSecs * 1000).toLong())
            }
            this.playWhenReady = playWhenReady
            prepare()
        }
    }
}

@androidx.annotation.OptIn(UnstableApi::class)
@Composable
fun PlayerScreen(
    sessionId: String,
    url: String,
    title: String,
    playbackMode: PlaybackMode,
    resumePositionSecs: Double,
    positionOffsetSecs: Double,
    mediaDurationSecs: Double?,
    maxBitrate: Long,
    subtitles: List<SubtitleTrack>,
    entry: MergedEntry,
    recommendations: List<MergedEntry>,
    artworkUrl: (MergedEntry) -> String?,
    hasNext: Boolean,
    nextTitle: String?,
    preloadedNext: PreparedEpisodePlayback?,
    onBack: () -> Unit,
    onPositionUpdate: (positionSecs: Double, durationSecs: Double) -> Unit,
    onContinue: () -> Unit,
    onPlayRecommendation: (MergedEntry) -> Unit,
    onPreloadNext: () -> Unit,
    onSeekOutsideBuffer: (positionSecs: Double) -> Unit,
    onPlaybackSessionExpired: (positionSecs: Double, context: String?) -> Unit,
    onServerOffline: (context: String?) -> Unit,
    onPlaybackRuntimeError: (message: String, context: String?) -> Unit,
) {
    BackHandler(onBack = onBack)
    val context = LocalContext.current
    var showContinuePrompt by remember(sessionId) { mutableStateOf(false) }
    // Covers the gap between "screen opened" and "a frame is actually up" —
    // negotiation already succeeded by the time this screen exists, but the
    // player still has to buffer its first segment(s) over the peer proxy,
    // which visibly took long enough on real hardware to look broken with
    // nothing on screen but a black box. Keyed on sessionId so autoplaying
    // into a preloaded next episode shows it again only when that player has
    // not already reached READY during the countdown.
    val playerPool = remember(context) { VideoPlayerPool(context) }
    val player = remember(playerPool, sessionId) {
        playerPool.activate(
            PlaybackPlayerConfig(
                sessionId = sessionId,
                url = url,
                title = title,
                maxBitrate = maxBitrate,
                subtitles = subtitles,
                resumePositionSecs = resumePositionSecs,
            ),
        )
    }
    var isLoading by remember(sessionId) { mutableStateOf(player.playbackState != Player.STATE_READY) }
    var serverOffline by remember(sessionId) { mutableStateOf(false) }
    var showPauseOverlay by remember(sessionId) { mutableStateOf(!player.playWhenReady) }
    var trackAvailability by remember(sessionId) {
        mutableStateOf(trackAvailability(player.currentTracks, subtitles))
    }

    // Negotiation finishes asynchronously during the Continue countdown.
    // Preparing with playWhenReady=false makes Media3 fetch and buffer the
    // next stream without advancing away from the ended episode. When the
    // ViewModel promotes this session, activate() returns this same player
    // instance instead of creating one with an empty buffer.
    LaunchedEffect(preloadedNext?.sessionId) {
        if (preloadedNext == null) {
            playerPool.releasePreloaded()
        } else {
            playerPool.preload(preloadedNext.toPlayerConfig())
        }
    }
    DisposableEffect(playerPool) {
        onDispose { playerPool.releasePreloaded() }
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
        val analyticsListener = object : AnalyticsListener {
            override fun onLoadError(
                eventTime: AnalyticsListener.EventTime,
                loadEventInfo: LoadEventInfo,
                mediaLoadData: MediaLoadData,
                error: IOException,
                wasCanceled: Boolean,
            ) {
                if (wasCanceled || !isServerOfflineLoadError(error)) return
                if (!serverOffline) {
                    serverOffline = true
                    onServerOffline(
                        "position_ms=${player.currentPosition}; buffered_position_ms=${player.bufferedPosition}; " +
                            "load_error=${error.javaClass.simpleName}: ${error.message.orEmpty()}",
                    )
                }
                // Let the buffered picture continue uninterrupted. The
                // loading overlay appears only once playback actually runs
                // out of buffered media and transitions to BUFFERING.
                if (player.playbackState == Player.STATE_BUFFERING) isLoading = true
            }

            override fun onLoadCompleted(
                eventTime: AnalyticsListener.EventTime,
                loadEventInfo: LoadEventInfo,
                mediaLoadData: MediaLoadData,
            ) {
                serverOffline = false
                if (player.playbackState == Player.STATE_READY) isLoading = false
            }
        }
        val listener = object : Player.Listener {
            override fun onPlaybackStateChanged(playbackState: Int) {
                // Covers audio (tracks never render a video frame at all,
                // so onRenderedFirstFrame below would never fire for them).
                if (playbackState == Player.STATE_READY) {
                    isLoading = false
                }
                if (playbackState == Player.STATE_BUFFERING && serverOffline) {
                    isLoading = true
                }
                if (playbackState == Player.STATE_ENDED && hasNext) {
                    showPauseOverlay = false
                    showContinuePrompt = true
                    onPreloadNext()
                }
            }

            override fun onPlayWhenReadyChanged(playWhenReady: Boolean, reason: Int) {
                showPauseOverlay = !playWhenReady && player.playbackState != Player.STATE_ENDED
            }

            override fun onTracksChanged(tracks: Tracks) {
                trackAvailability = trackAvailability(tracks, subtitles)
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
        player.addAnalyticsListener(analyticsListener)
        player.addListener(listener)
        onDispose {
            player.removeAnalyticsListener(analyticsListener)
            player.removeListener(listener)
            val positionSecs = positionOffsetSecs + player.currentPosition / 1000.0
            val durationSecs = player.duration.takeIf { it != C.TIME_UNSET }?.let { positionOffsetSecs + it / 1000.0 } ?: 0.0
            onPositionUpdate(positionSecs, durationSecs)
            loudnessEnhancer?.release()
            playerPool.release(player)
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
            // new active ExoPlayer (either freshly created or promoted from
            // the preload pool), but `factory` only ever runs once for a
            // given AndroidView call site — without this `update`, the
            // on-screen PlayerView stayed bound to the *previous*, already-
            // released player forever. Audio kept working regardless
            // (ExoPlayer's audio output doesn't need a bound view, only video
            // rendering does), which is exactly why sound played over a
            // frozen frame showing the old episode's final progress bar.
            update = { view ->
                view.player = controllerPlayer
                view.useController = !showPauseOverlay
                if (showPauseOverlay) view.hideController()
            },
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

        if (showPauseOverlay && !showContinuePrompt) {
            PauseOverlay(
                entry = entry,
                recommendations = recommendations,
                artworkUrl = artworkUrl,
                audioLanguages = trackAvailability.audioLanguages,
                subtitleLabels = trackAvailability.subtitles,
                onResume = player::play,
                onPlayRecommendation = onPlayRecommendation,
            )
        }
    }
}

private data class TrackAvailability(
    val audioLanguages: List<String>,
    val subtitles: List<String>,
)

private fun trackAvailability(tracks: Tracks, configuredSubtitles: List<SubtitleTrack>): TrackAvailability {
    val audioLanguages = mutableListOf<String>()
    val subtitleLabels = configuredSubtitles.mapTo(mutableListOf()) { track ->
        track.label.takeIf(String::isNotBlank) ?: languageDisplayName(track.language)
    }
    for (group in tracks.groups) {
        for (index in 0 until group.length) {
            val format = group.getTrackFormat(index)
            when (group.type) {
                C.TRACK_TYPE_AUDIO -> audioLanguages += audioTrackLabel(format)
                C.TRACK_TYPE_TEXT -> subtitleLabels += subtitleTrackLabel(format, index)
            }
        }
    }
    return TrackAvailability(
        audioLanguages = audioLanguages.distinctLabels(),
        subtitles = subtitleLabels.distinctLabels(),
    )
}

private fun audioTrackLabel(format: Format): String =
    format.language?.takeIf(String::isNotBlank)?.let(::languageDisplayName)
        ?: format.label?.takeIf(String::isNotBlank)
        ?: "Default audio"

private fun subtitleTrackLabel(format: Format, index: Int): String =
    format.label?.takeIf(String::isNotBlank)
        ?: format.language?.takeIf(String::isNotBlank)?.let(::languageDisplayName)
        ?: "Subtitle ${index + 1}"

private fun languageDisplayName(code: String): String {
    val normalized = code.trim().replace('_', '-')
    val locale = Locale.forLanguageTag(normalized)
    return locale.getDisplayLanguage(Locale.getDefault())
        .takeIf { it.isNotBlank() && !it.equals(normalized, ignoreCase = true) }
        ?: code.uppercase()
}

private fun List<String>.distinctLabels(): List<String> =
    distinctBy { it.trim().lowercase() }

@Composable
private fun PauseOverlay(
    entry: MergedEntry,
    recommendations: List<MergedEntry>,
    artworkUrl: (MergedEntry) -> String?,
    audioLanguages: List<String>,
    subtitleLabels: List<String>,
    onResume: () -> Unit,
    onPlayRecommendation: (MergedEntry) -> Unit,
) {
    val resumeFocusRequester = remember { FocusRequester() }
    LaunchedEffect(entry.fingerprint) { resumeFocusRequester.requestFocus() }
    val media = entry.entry
    val title = if (media.kind == MediaKind.EPISODE) pauseRecommendationTitle(entry) else media.displayTitle()
    val metadata = listOfNotNull(
        media.year?.toString(),
        media.durationSecs?.takeIf { it.isFinite() && it > 0.0 }?.let(::durationLabel),
        media.rating?.takeIf(String::isNotBlank)?.let { "Rated $it" },
        media.communityRating?.let { score ->
            val votes = media.communityRatingVotes?.takeIf { it > 0 }
                ?.let { String.format(Locale.US, " (%,d votes)", it) }
                .orEmpty()
            String.format(Locale.US, "★ %.1f/10", score) + votes
        },
        media.video?.height?.takeIf { it > 0 }?.let { "${it}p" },
    )

    Box(
        modifier = Modifier.fillMaxSize().background(
            Brush.horizontalGradient(
                listOf(Color.Black.copy(alpha = 0.96f), Color.Black.copy(alpha = 0.90f), Color.Black.copy(alpha = 0.82f)),
            ),
        ),
    ) {
        Column(modifier = Modifier.fillMaxSize().padding(horizontal = 44.dp, vertical = 28.dp)) {
            Row(modifier = Modifier.fillMaxWidth().weight(1f), horizontalArrangement = Arrangement.spacedBy(36.dp)) {
                Column(modifier = Modifier.weight(1f)) {
                    Text("Paused", color = SwarmAccent, fontSize = 13.sp, fontWeight = FontWeight.Bold)
                    Spacer(Modifier.height(5.dp))
                    Text(title, color = SwarmText, fontSize = 28.sp, fontWeight = FontWeight.Black, maxLines = 2)
                    episodeNumberLabel(entry)?.let { episodeLabel ->
                        Spacer(Modifier.height(4.dp))
                        Text(episodeLabel, color = SwarmText, fontSize = 15.sp, maxLines = 1)
                    }
                    if (metadata.isNotEmpty()) {
                        Spacer(Modifier.height(7.dp))
                        Text(metadata.joinToString("  •  "), color = SwarmMuted, fontSize = 13.sp, maxLines = 1)
                    }
                    if (media.genres.isNotEmpty()) {
                        Spacer(Modifier.height(6.dp))
                        Text(media.genres.take(4).joinToString("  •  "), color = SwarmAccent, fontSize = 12.sp, maxLines = 1)
                    }
                    if (media.cast.isNotEmpty()) {
                        Spacer(Modifier.height(6.dp))
                        Text(
                            "Cast: ${media.cast.take(5).joinToString { it.name }}",
                            color = SwarmMuted,
                            fontSize = 12.sp,
                            maxLines = 1,
                            overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
                        )
                    }
                    media.overview?.takeIf(String::isNotBlank)?.let { overview ->
                        Spacer(Modifier.height(10.dp))
                        Text(
                            overview,
                            color = SwarmMuted,
                            fontSize = 13.sp,
                            lineHeight = 18.sp,
                            maxLines = 3,
                            overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
                        )
                    }
                    Spacer(Modifier.height(12.dp))
                    Button(
                        onClick = onResume,
                        modifier = Modifier.focusRequester(resumeFocusRequester),
                        colors = swarmActionButtonColors(),
                    ) {
                        Text("▶  Resume", color = Color(0xFF04263A), fontWeight = FontWeight.Bold)
                    }
                }
                Column(modifier = Modifier.width(285.dp)) {
                    AvailabilityBlock(
                        heading = "Audio languages",
                        values = audioLanguages,
                        emptyLabel = "None reported",
                    )
                    Spacer(Modifier.height(18.dp))
                    AvailabilityBlock(
                        heading = "Available subtitles",
                        values = subtitleLabels,
                        emptyLabel = "None available",
                    )
                }
            }

            if (recommendations.isNotEmpty()) {
                Spacer(Modifier.height(12.dp))
                Text("More like this", color = SwarmText, fontSize = 17.sp, fontWeight = FontWeight.Bold)
                Spacer(Modifier.height(8.dp))
                LazyRow(
                    modifier = Modifier.fillMaxWidth().height(142.dp),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    items(recommendations, key = { it.fingerprint }) { recommendation ->
                        PauseRecommendationCard(
                            entry = recommendation,
                            artworkUrl = artworkUrl(recommendation),
                            onClick = { onPlayRecommendation(recommendation) },
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun AvailabilityBlock(heading: String, values: List<String>, emptyLabel: String) {
    Text(heading, color = SwarmMuted, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
    Spacer(Modifier.height(5.dp))
    Text(
        values.take(6).joinToString("  •  ").ifBlank { emptyLabel },
        color = SwarmText,
        fontSize = 13.sp,
        lineHeight = 18.sp,
        maxLines = 4,
        overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
    )
}

@Composable
private fun PauseRecommendationCard(entry: MergedEntry, artworkUrl: String?, onClick: () -> Unit) {
    Card(
        onClick = onClick,
        colors = CardDefaults.colors(containerColor = SwarmSurface),
        scale = CardDefaults.scale(scale = 1f, focusedScale = 1.06f, pressedScale = 0.98f),
        modifier = Modifier.width(102.dp),
    ) {
        Column {
            ArtworkImage(
                label = pauseRecommendationTitle(entry),
                placeholderType = if (entry.entry.kind == MediaKind.MOVIE) "Movie" else "Show",
                primaryUrl = artworkUrl,
                modifier = Modifier.fillMaxWidth().height(102.dp).clip(RoundedCornerShape(4.dp)),
            )
            Text(
                pauseRecommendationTitle(entry),
                color = SwarmText,
                fontSize = 10.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 2,
                overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
                modifier = Modifier.padding(horizontal = 6.dp, vertical = 4.dp),
            )
        }
    }
}

private fun durationLabel(durationSecs: Double): String {
    val totalMinutes = (durationSecs / 60.0).toInt().coerceAtLeast(1)
    val hours = totalMinutes / 60
    val minutes = totalMinutes % 60
    return if (hours == 0) "${minutes}m" else "${hours}h ${minutes}m"
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
