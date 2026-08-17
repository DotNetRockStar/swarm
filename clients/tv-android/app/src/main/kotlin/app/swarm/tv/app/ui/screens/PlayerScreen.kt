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
 */
package app.swarm.tv.app.ui.screens

import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.viewinterop.AndroidView
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.upstream.DefaultBandwidthMeter
import androidx.media3.ui.PlayerView

@androidx.annotation.OptIn(UnstableApi::class)
@Composable
fun PlayerScreen(
    url: String,
    title: String,
    resumePositionSecs: Double,
    positionOffsetSecs: Double,
    maxBitrate: Long,
    onBack: () -> Unit,
    onPositionUpdate: (positionSecs: Double, durationSecs: Double) -> Unit,
) {
    BackHandler(onBack = onBack)
    val context = LocalContext.current

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
        onDispose {
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
    }
}
