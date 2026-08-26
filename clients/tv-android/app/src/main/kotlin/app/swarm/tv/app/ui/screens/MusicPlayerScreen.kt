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

import android.view.KeyEvent
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
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
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
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import app.swarm.tv.R
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmText
import app.swarm.tv.app.ui.components.swarmActionButtonColors
import app.swarm.tv.core.catalog.MergedEntry
import app.swarm.tv.core.catalog.activeLyricIndex
import app.swarm.tv.core.catalog.parseSyncedLyrics
import app.swarm.tv.core.peer.TrackLyrics
import coil.compose.AsyncImage

/** Music has no embedded Media3 controller, so its full-screen surface owns
 * D-pad seek gestures in addition to the transport-button mapping shared with
 * video. Keeping this mapping pure makes Fire TV's emitted key codes testable
 * without a device or Compose instrumentation. */
internal fun musicPlaybackAction(keyCode: Int): RemotePlaybackAction? = when (keyCode) {
    KeyEvent.KEYCODE_DPAD_LEFT -> RemotePlaybackAction.SEEK_BACK
    KeyEvent.KEYCODE_DPAD_RIGHT -> RemotePlaybackAction.SEEK_FORWARD
    else -> remotePlaybackAction(keyCode)
}

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
    lyrics: TrackLyrics?,
    positionMs: Long,
    onTogglePlayPause: () -> Unit,
    onPlay: () -> Unit,
    onPause: () -> Unit,
    onSeekBack: () -> Unit,
    onSeekForward: () -> Unit,
    onToggleShuffle: () -> Unit,
    onToggleLike: () -> Unit,
    onSkipNext: () -> Unit,
    onMinimize: () -> Unit,
    onClose: () -> Unit,
) {
    // Minimize, not stop: this is the one screen in the app whose Back
    // press doesn't tear down what it's showing — see minimizePlayback's
    // doc comment. A user who presses Back expects to return to browsing,
    // which is exactly what minimizing does. The visible Close button is
    // the explicit way to end playback; a redundant Minimize button would
    // waste D-pad space when Back already owns that behavior.
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

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(Color.Black)
            .onPreviewKeyEvent { composeEvent ->
                val event = composeEvent.nativeKeyEvent
                val action = musicPlaybackAction(event.keyCode) ?: return@onPreviewKeyEvent false
                if (action == RemotePlaybackAction.SHOW_CONTROLS) return@onPreviewKeyEvent false
                if (event.action == KeyEvent.ACTION_DOWN) {
                    when (action) {
                        RemotePlaybackAction.TOGGLE_PLAY_PAUSE -> onTogglePlayPause()
                        RemotePlaybackAction.PLAY -> onPlay()
                        RemotePlaybackAction.PAUSE -> onPause()
                        RemotePlaybackAction.SEEK_FORWARD -> onSeekForward()
                        RemotePlaybackAction.SEEK_BACK -> onSeekBack()
                        RemotePlaybackAction.SHOW_CONTROLS -> Unit
                    }
                }
                // Match video playback's direct left/right gesture while
                // still allowing Compose to move focus between the visible
                // buttons. A held D-pad direction repeats ACTION_DOWN, so it
                // continues seeking in the same 60-second steps as video.
                if (event.keyCode == KeyEvent.KEYCODE_DPAD_LEFT || event.keyCode == KeyEvent.KEYCODE_DPAD_RIGHT) {
                    return@onPreviewKeyEvent false
                }
                // Consume key-up as well so a transport-key release cannot
                // activate whichever Compose button currently owns focus.
                true
            },
    ) {
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
            Row(modifier = Modifier.weight(1f).fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Box(modifier = Modifier.weight(1f).fillMaxHeight(), contentAlignment = Alignment.Center) {
                    if (visualUrl != null) {
                        AsyncImage(
                            model = visualUrl,
                            contentDescription = null,
                            contentScale = ContentScale.Crop,
                            modifier = Modifier.width(if (lyrics == null) 280.dp else 240.dp).aspectRatio(1f).scale(pulseScale).clip(RoundedCornerShape(16.dp)),
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
                if (lyrics != null) {
                    Spacer(Modifier.width(36.dp))
                    LyricsPanel(
                        lyrics = lyrics,
                        positionMs = positionMs,
                        modifier = Modifier.weight(1f).fillMaxHeight(),
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
                    colors = swarmActionButtonColors(),
                ) {
                    Text(if (shuffleEnabled) "🔀 Shuffle on" else "🔀 Shuffle", fontSize = 13.sp)
                }
                Button(onClick = onSeekBack, colors = swarmActionButtonColors()) {
                    Text("↶ 60s", fontSize = 13.sp)
                }
                Button(
                    onClick = onTogglePlayPause,
                    modifier = Modifier.focusRequester(playFocusRequester),
                    colors = swarmActionButtonColors(),
                ) {
                    Text(if (isLoading) "Buffering…" else if (isPlaying) "Pause" else "Play", fontWeight = FontWeight.Bold)
                }
                Button(onClick = onSeekForward, colors = swarmActionButtonColors()) {
                    Text("60s ↷", fontSize = 13.sp)
                }
                Button(onClick = onSkipNext, colors = swarmActionButtonColors()) {
                    Text("Skip ⏭", fontSize = 13.sp)
                }
                Button(
                    onClick = onToggleLike,
                    colors = swarmActionButtonColors(),
                ) {
                    Text(if (isLiked) "♥ Liked" else "♡ Like", fontSize = 13.sp)
                }
                Button(onClick = onClose, colors = swarmActionButtonColors()) {
                    Text("Close ✕", fontSize = 13.sp)
                }
            }
        }
    }
}

@Composable
private fun LyricsPanel(lyrics: TrackLyrics, positionMs: Long, modifier: Modifier = Modifier) {
    val timedLines = remember(lyrics.syncedLyrics) { lyrics.syncedLyrics?.let(::parseSyncedLyrics).orEmpty() }
    val plainLines = remember(lyrics.plainLyrics) {
        lyrics.plainLyrics?.lineSequence()?.filter(String::isNotBlank)?.toList().orEmpty()
    }
    val currentIndex = remember(timedLines, positionMs) { activeLyricIndex(timedLines, positionMs) }
    val listState = rememberLazyListState()

    LaunchedEffect(currentIndex) {
        if (currentIndex >= 0) {
            listState.animateScrollToItem((currentIndex - 2).coerceAtLeast(0))
        }
    }

    Column(
        modifier = modifier.clip(RoundedCornerShape(18.dp)).background(Color.Black.copy(alpha = 0.5f)).padding(22.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("Lyrics", color = SwarmText, fontSize = 20.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.width(10.dp))
            Text(
                buildString {
                    append("LRCLIB")
                    lyrics.language?.takeIf(String::isNotBlank)?.let { append("  •  ${it.uppercase()}") }
                },
                color = SwarmMuted,
                fontSize = 11.sp,
            )
        }
        Spacer(Modifier.height(16.dp))
        when {
            lyrics.instrumental -> {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    Text("Instrumental", color = SwarmMuted, fontSize = 21.sp, fontWeight = FontWeight.Medium)
                }
            }
            timedLines.isNotEmpty() -> {
                LazyColumn(
                    state = listState,
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                    modifier = Modifier.fillMaxSize(),
                ) {
                    itemsIndexed(timedLines, key = { index, line -> "${line.timeMs}-$index" }) { index, line ->
                        val active = index == currentIndex
                        Text(
                            line.text,
                            color = if (active) SwarmAccent else SwarmMuted.copy(alpha = if (index < currentIndex) 0.55f else 0.85f),
                            fontSize = if (active) 20.sp else 16.sp,
                            fontWeight = if (active) FontWeight.Bold else FontWeight.Normal,
                        )
                    }
                }
            }
            plainLines.isNotEmpty() -> {
                LazyColumn(verticalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.fillMaxSize()) {
                    items(plainLines.size) { index ->
                        Text(plainLines[index], color = SwarmText.copy(alpha = 0.88f), fontSize = 16.sp)
                    }
                }
            }
        }
    }
}
