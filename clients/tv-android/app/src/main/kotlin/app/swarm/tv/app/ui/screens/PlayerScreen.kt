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

import android.net.Uri
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.upstream.DefaultBandwidthMeter
import androidx.media3.ui.PlayerView
import androidx.tv.material3.Button
import androidx.tv.material3.ButtonDefaults
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmSurfaceMuted
import app.swarm.tv.app.ui.theme.SwarmText
import kotlinx.coroutines.delay

private const val CONTINUE_COUNTDOWN_SECS = 8

@androidx.annotation.OptIn(UnstableApi::class)
@Composable
fun PlayerScreen(
    url: String,
    title: String,
    resumePositionSecs: Double,
    positionOffsetSecs: Double,
    maxBitrate: Long,
    hasNext: Boolean,
    nextTitle: String?,
    onBack: () -> Unit,
    onPositionUpdate: (positionSecs: Double, durationSecs: Double) -> Unit,
    onContinue: () -> Unit,
) {
    BackHandler(onBack = onBack)
    val context = LocalContext.current
    var showContinuePrompt by remember(url) { mutableStateOf(false) }

    val player = remember(url, maxBitrate) {
        // The HTTP URL is loopback, so Media3's network-type-based initial
        // estimate describes the TV's Wi-Fi rather than the media server's
        // constrained uplink. Start conservatively; segment transfer samples
        // will replace this estimate as playback proceeds.
        val initialBitrate = maxBitrate.coerceIn(250_000L, 2_000_000L)
        val bandwidthMeter = DefaultBandwidthMeter.Builder(context)
            .setInitialBitrateEstimate(initialBitrate)
            .build()
        ExoPlayer.Builder(context).setBandwidthMeter(bandwidthMeter).build().apply {
            setMediaItem(MediaItem.Builder().setUri(Uri.parse(url)).setMediaId(title).build())
            if (resumePositionSecs > 0) {
                seekTo((resumePositionSecs * 1000).toLong())
            }
            playWhenReady = true
            prepare()
        }
    }
    DisposableEffect(player) {
        val listener = object : Player.Listener {
            override fun onPlaybackStateChanged(playbackState: Int) {
                if (playbackState == Player.STATE_ENDED && hasNext) {
                    showContinuePrompt = true
                }
            }
        }
        player.addListener(listener)
        onDispose {
            player.removeListener(listener)
            val positionSecs = positionOffsetSecs + player.currentPosition / 1000.0
            val durationSecs = player.duration.takeIf { it != C.TIME_UNSET }?.let { positionOffsetSecs + it / 1000.0 } ?: 0.0
            onPositionUpdate(positionSecs, durationSecs)
            player.release()
        }
    }

    Box(modifier = Modifier.fillMaxSize()) {
        AndroidView(
            modifier = Modifier.fillMaxSize(),
            factory = { ctx ->
                PlayerView(ctx).apply {
                    this.player = player
                    useController = true
                }
            },
        )

        if (showContinuePrompt) {
            ContinueOverlay(
                nextTitle = nextTitle,
                onPlayNow = { showContinuePrompt = false; onContinue() },
                onCancel = { showContinuePrompt = false; onBack() },
            )
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
                    colors = ButtonDefaults.colors(containerColor = SwarmAccent, contentColor = Color(0xFF04263A)),
                ) {
                    Text("Play now ($secondsLeft)", color = Color(0xFF04263A), fontSize = 14.sp, fontWeight = FontWeight.Bold)
                }
                Button(onClick = onCancel, colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText)) {
                    Text("Cancel", color = SwarmText, fontSize = 14.sp)
                }
            }
        }
    }
}
