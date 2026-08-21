/**
 * Full-screen "now playing" for a track — a real visual instead of the
 * plain black screen a video-oriented [PlayerScreen] leaves behind for
 * audio-only content (ExoPlayer's video renderer never fires for a track,
 * so [PlayerScreen] alone would just be a static black rectangle the whole
 * time). Real feedback from live use: cover art when the track has it,
 * falling back to the artist's photo, falling back to a generic pulsing
 * mark — always *something* to look at, full-bleed behind a scrim, with a
 * slow breathing scale animation while playing so the screen reads as
 * alive rather than frozen.
 *
 * Deliberately owns no [androidx.media3.exoplayer.ExoPlayer] itself — see
 * [app.swarm.tv.app.MainActivity]'s `SwarmApp` for why the player is
 * hoisted above this screen (it must survive [onMinimize] leaving this
 * composition entirely, unlike [PlayerScreen]'s own video player, which is
 * fine tied to its own screen's lifecycle since movies/episodes never
 * minimize). This screen is a pure function of already-observed playback
 * state plus callbacks, same as every other screen in this app.
 */
package app.swarm.tv.app.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.ButtonDefaults
import app.swarm.tv.R
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmAccentHot
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmSurfaceMuted
import app.swarm.tv.app.ui.theme.SwarmText
import app.swarm.tv.core.catalog.MergedEntry
import coil.compose.AsyncImage

@Composable
fun MusicPlayerScreen(
    entry: MergedEntry,
    nextTitle: String?,
    isPlaying: Boolean,
    isLoading: Boolean,
    shuffleEnabled: Boolean,
    isLiked: Boolean,
    artworkUrl: String?,
    artistPhotoUrl: String?,
    onTogglePlayPause: () -> Unit,
    onToggleShuffle: () -> Unit,
    onToggleLike: () -> Unit,
    onSkipNext: () -> Unit,
    onMinimize: () -> Unit,
) {
    // Minimize, not stop: this is the one screen in the app whose Back
    // press doesn't tear down what it's showing — see minimizePlayback's
    // doc comment. A user who presses Back expects to return to browsing,
    // which is exactly what minimizing does; a separate explicit "stop"
    // control (the mini-bar's own, once minimized) is where playback
    // actually ends.
    BackHandler(onBack = onMinimize)

    val playFocusRequester = remember { FocusRequester() }
    LaunchedEffect(entry) { playFocusRequester.requestFocus() }

    val visualUrl = artworkUrl ?: artistPhotoUrl

    val infiniteTransition = rememberInfiniteTransition(label = "now-playing-pulse")
    val pulseScale by infiniteTransition.animateFloat(
        initialValue = 1f,
        targetValue = if (isPlaying) 1.04f else 1f,
        animationSpec = infiniteRepeatable(animation = tween(2200), repeatMode = RepeatMode.Reverse),
        label = "now-playing-scale",
    )

    Box(modifier = Modifier.fillMaxSize().background(Color.Black)) {
        if (visualUrl != null) {
            AsyncImage(
                model = visualUrl,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxSize(),
            )
        }
        // Same full-bleed-plus-scrim treatment MovieDetailScreen already
        // uses, so text stays legible over art of any brightness.
        Box(
            modifier = Modifier.fillMaxSize().background(
                Brush.verticalGradient(listOf(Color.Black.copy(alpha = 0.35f), Color.Black.copy(alpha = 0.55f), Color.Black.copy(alpha = 0.92f))),
            ),
        )

        Column(
            modifier = Modifier.fillMaxSize().padding(horizontal = 56.dp, vertical = 36.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Bottom,
        ) {
            Box(modifier = Modifier.weight(1f), contentAlignment = Alignment.Center) {
                if (visualUrl != null) {
                    AsyncImage(
                        model = visualUrl,
                        contentDescription = null,
                        contentScale = ContentScale.Crop,
                        modifier = Modifier.width(280.dp).aspectRatio(1f).scale(pulseScale).clip(RoundedCornerShape(16.dp)),
                    )
                } else {
                    // No cover art, no artist photo — never a truly blank
                    // screen: the mascot at a large size plus the pulse
                    // animation is still "something alive to look at."
                    Image(
                        painter = painterResource(R.drawable.mascot),
                        contentDescription = null,
                        modifier = Modifier.width(220.dp).aspectRatio(1f).scale(pulseScale),
                    )
                }
            }

            Text(
                entry.entry.scrapedTitle ?: entry.entry.title,
                color = SwarmText,
                fontSize = 26.sp,
                fontWeight = FontWeight.Black,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            val subtitle = listOfNotNull(entry.entry.artist, entry.entry.album).joinToString("  •  ")
            if (subtitle.isNotEmpty()) {
                Spacer(Modifier.height(6.dp))
                Text(subtitle, color = SwarmMuted, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            if (nextTitle != null) {
                Spacer(Modifier.height(6.dp))
                Text("Up next: $nextTitle", color = SwarmMuted, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }

            Spacer(Modifier.height(20.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Button(
                    onClick = onToggleShuffle,
                    colors = ButtonDefaults.colors(
                        containerColor = if (shuffleEnabled) SwarmAccent else SwarmSurfaceMuted,
                        contentColor = if (shuffleEnabled) Color(0xFF04263A) else SwarmText,
                    ),
                ) {
                    Text(if (shuffleEnabled) "🔀 Shuffle on" else "🔀 Shuffle", fontSize = 13.sp)
                }
                Button(
                    onClick = onTogglePlayPause,
                    modifier = Modifier.focusRequester(playFocusRequester),
                    colors = ButtonDefaults.colors(containerColor = SwarmAccent, contentColor = Color(0xFF04263A)),
                ) {
                    Text(if (isLoading) "Buffering…" else if (isPlaying) "Pause" else "Play", fontWeight = FontWeight.Bold)
                }
                Button(onClick = onSkipNext, colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText)) {
                    Text("Skip ⏭", fontSize = 13.sp)
                }
                Button(
                    onClick = onToggleLike,
                    colors = ButtonDefaults.colors(
                        containerColor = if (isLiked) SwarmAccentHot else SwarmSurfaceMuted,
                        contentColor = if (isLiked) Color(0xFF3A0420) else SwarmText,
                    ),
                ) {
                    Text(if (isLiked) "♥ Liked" else "♡ Like", fontSize = 13.sp)
                }
                Button(onClick = onMinimize, colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText)) {
                    Text("Minimize ⌄", fontSize = 13.sp)
                }
            }
        }
    }
}
