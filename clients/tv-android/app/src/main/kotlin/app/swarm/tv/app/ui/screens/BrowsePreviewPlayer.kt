package app.swarm.tv.app.ui.screens

import android.net.Uri
import android.util.Log
import android.view.LayoutInflater
import android.view.ViewGroup
import androidx.annotation.OptIn
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.upstream.DefaultBandwidthMeter
import androidx.media3.ui.PlayerView
import app.swarm.tv.R
import app.swarm.tv.app.data.BrowsePreview
import coil.compose.AsyncImage
import kotlinx.coroutines.delay

private const val PREVIEW_PLAY_TIME_MS = 30_000L

@Composable
internal fun PreviewLoadingIndicator(modifier: Modifier = Modifier) {
    Box(
        modifier = modifier.background(Color.Black).focusProperties { canFocus = false },
        contentAlignment = Alignment.Center,
    ) {
        AsyncImage(
            model = R.drawable.preview_loading_circle,
            contentDescription = "Loading preview",
            modifier = Modifier.size(56.dp),
        )
    }
}

/** Inline video preview with its original audio. `Player.stop()` cancels every media request after the
 * preview window; PlayerView's keep-content flag preserves the final decoded
 * frame instead of flashing back to artwork or black. */
@OptIn(UnstableApi::class)
@Composable
internal fun BrowsePreviewPlayer(
    preview: BrowsePreview,
    shouldPlay: Boolean,
    onFinished: (sessionId: String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    var renderedFirstFrame by remember(preview.sessionId) { mutableStateOf(false) }
    var finished by remember(preview.sessionId) { mutableStateOf(preview.released) }
    var failedBeforeFirstFrame by remember(preview.sessionId) { mutableStateOf(false) }
    val player = remember(preview.sessionId) {
        val bandwidthMeter = DefaultBandwidthMeter.Builder(context)
            .setInitialBitrateEstimate(preview.maxBitrate.coerceIn(250_000L, 2_000_000L))
            .build()
        ExoPlayer.Builder(context).setBandwidthMeter(bandwidthMeter).build().apply {
            // Keep direct/HLS preview selection aligned with full playback.
            // The server already maps English into transcoded previews; this
            // also covers any multi-track source exposed directly to Media3.
            trackSelectionParameters = trackSelectionParameters
                .buildUpon()
                .setPreferredAudioLanguages("en", "eng")
                .build()
            setMediaItem(MediaItem.fromUri(Uri.parse(preview.url)))
            if (preview.seekPositionSecs > 0L) seekTo(preview.seekPositionSecs * 1000L)
            // Prepare immediately so the first frame can buffer during the
            // second half of the focus dwell, but do not produce audio or
            // advance video until the card expands.
            playWhenReady = shouldPlay && !preview.released
            prepare()
        }
    }

    LaunchedEffect(player, shouldPlay, finished) {
        player.playWhenReady = shouldPlay && !finished
    }

    fun freezeAndRelease(reason: String) {
        if (finished) return
        finished = true
        player.pause()
        // Cancels buffering/the active HTTP request. PlayerView below keeps
        // the already-rendered frame on its surface after this reset.
        player.stop()
        Log.i("BrowsePreview", "Stopped ${preview.sessionId}: $reason")
        onFinished(preview.sessionId)
    }

    DisposableEffect(player) {
        val listener = object : Player.Listener {
            override fun onRenderedFirstFrame() {
                renderedFirstFrame = true
                Log.i("BrowsePreview", "First frame rendered for ${preview.sessionId}")
            }

            override fun onPlaybackStateChanged(playbackState: Int) {
                if (playbackState == Player.STATE_ENDED) freezeAndRelease("media ended")
            }

            override fun onPlayerError(error: androidx.media3.common.PlaybackException) {
                failedBeforeFirstFrame = !renderedFirstFrame
                Log.w("BrowsePreview", "Playback failed for ${preview.sessionId}", error)
                freezeAndRelease("playback error")
            }
        }
        player.addListener(listener)
        onDispose {
            player.removeListener(listener)
            player.release()
        }
    }

    // Buffering time does not consume the requested 30-second preview; the
    // clock begins only after the first frame is actually visible.
    LaunchedEffect(player, renderedFirstFrame, shouldPlay) {
        if (!renderedFirstFrame || !shouldPlay || finished) return@LaunchedEffect
        delay(PREVIEW_PLAY_TIME_MS)
        freezeAndRelease("30-second window complete")
    }

    // If playback failed before producing a frame, leave the transparent
    // overlay empty so the card's artwork remains visible instead of replacing
    // it with a black rectangle. A successfully rendered final frame remains.
    if (failedBeforeFirstFrame) return

    Box(modifier = modifier.background(Color.Black).focusProperties { canFocus = false }) {
        AndroidView(
            modifier = Modifier.fillMaxSize().focusProperties { canFocus = false },
            factory = { ctx ->
                // PlayerView's default SurfaceView is a separate Android
                // window layer and can sit behind the Compose card on Fire
                // TV, producing audio over a blank card. This layout opts
                // into TextureView so the video is composed and clipped with
                // the rest of the card.
                (LayoutInflater.from(ctx).inflate(R.layout.browse_preview_player, null) as PlayerView).apply {
                    isFocusable = false
                    isFocusableInTouchMode = false
                    descendantFocusability = ViewGroup.FOCUS_BLOCK_DESCENDANTS
                    this.player = player
                }
            },
            update = { it.player = player },
        )
        if (!renderedFirstFrame && !finished) {
            PreviewLoadingIndicator(Modifier.fillMaxSize())
        }
    }
}
