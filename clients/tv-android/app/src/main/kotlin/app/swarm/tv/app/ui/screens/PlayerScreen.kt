/**
 * Plays [url] — an `http://127.0.0.1:<port>/<serverId>/media/<entryKey>`
 * URL from [app.swarm.tv.core.catalog.CatalogSession.urlFor] — through a
 * standard Media3 `ExoPlayer`/`PlayerView`. ExoPlayer's own HTTP data
 * source needs nothing more exotic than what
 * [app.swarm.tv.core.proxy.PeerLoopbackProxy] already speaks (proven with a
 * plain OkHttp client in `PeerQuicClientInteropTest`), so this screen is
 * mechanical: point it at the local proxy URL like any other HTTP stream.
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
import androidx.media3.common.MediaItem
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.ui.PlayerView

@Composable
fun PlayerScreen(url: String, title: String, onBack: () -> Unit) {
    BackHandler(onBack = onBack)
    val context = LocalContext.current

    val player = remember(url) {
        ExoPlayer.Builder(context).build().apply {
            setMediaItem(MediaItem.Builder().setUri(Uri.parse(url)).setMediaId(title).build())
            playWhenReady = true
            prepare()
        }
    }
    DisposableEffect(player) {
        onDispose { player.release() }
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
