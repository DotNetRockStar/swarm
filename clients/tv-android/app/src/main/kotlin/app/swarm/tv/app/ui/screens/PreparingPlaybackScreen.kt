/**
 * The full-screen cover shown the instant a fresh play is requested from a
 * browse/detail screen (most visibly a "Continue Watching" tap), replacing
 * the frozen catalog while the server session is negotiated behind it (#122).
 *
 * It deliberately mirrors [PauseOverlay]'s dark-gradient-over-artwork look —
 * a "Continue Watching" start offers a focused Resume button here, so the
 * hand-off to the real paused player is seamless and the viewer never sees a
 * loading mascot before it. Pressing Resume before negotiation finishes just
 * starts playback the moment the stream is ready. A plain (non-paused) play
 * shows only a small preparing indicator. Mid-playback handoffs (next
 * episode, out-of-buffer seek) keep their plain video backdrop instead — see
 * [app.swarm.tv.app.data.UiState.PreparingPlayback].
 */
package app.swarm.tv.app.ui.screens

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
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.tv.material3.Button
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.swarm.tv.app.ui.UatTestTags
import app.swarm.tv.app.ui.components.swarmActionButtonColors
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmBackground
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmText
import coil.compose.AsyncImage

@Composable
fun PreparingPlaybackScreen(
    title: String,
    artworkUrl: String?,
    startPaused: Boolean,
    resumeRequested: Boolean,
    onResume: () -> Unit,
    onCancel: () -> Unit,
) {
    // Back abandons the negotiation and drops straight back to the screen the
    // play was started from, so a mistaken tap isn't a forced wait.
    BackHandler(onBack = onCancel)
    val resumeFocus = remember { FocusRequester() }
    val showResumeButton = startPaused && !resumeRequested
    LaunchedEffect(showResumeButton) {
        if (showResumeButton) runCatching { resumeFocus.requestFocus() }
    }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(SwarmBackground)
            .testTag(UatTestTags.PREPARING_PLAYBACK),
    ) {
        artworkUrl?.let { url ->
            AsyncImage(
                model = url,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxSize(),
            )
        }
        Box(
            Modifier.fillMaxSize().background(
                Brush.horizontalGradient(
                    listOf(
                        Color.Black.copy(alpha = 0.96f),
                        Color.Black.copy(alpha = 0.90f),
                        Color.Black.copy(alpha = 0.82f),
                    ),
                ),
            ),
        )
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(horizontal = 44.dp, vertical = 28.dp),
            verticalArrangement = Arrangement.Center,
        ) {
            Text(
                if (startPaused) "Ready when you are" else "Starting playback",
                color = SwarmAccent,
                fontSize = 13.sp,
                fontWeight = FontWeight.Bold,
            )
            Spacer(Modifier.height(6.dp))
            Text(
                title,
                color = SwarmText,
                fontSize = 28.sp,
                fontWeight = FontWeight.Black,
                maxLines = 2,
            )
            Spacer(Modifier.height(18.dp))
            if (showResumeButton) {
                Button(
                    onClick = onResume,
                    modifier = Modifier
                        .focusRequester(resumeFocus)
                        .testTag(UatTestTags.PREPARING_PLAYBACK_RESUME_BUTTON),
                    colors = swarmActionButtonColors(),
                ) {
                    Text("▶  Resume", color = Color(0xFF04263A), fontWeight = FontWeight.Bold)
                }
                Spacer(Modifier.height(12.dp))
                Text("Getting your stream ready…", color = SwarmMuted, fontSize = 13.sp)
            } else {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    CircularProgressIndicator(
                        color = SwarmAccent,
                        strokeWidth = 3.dp,
                        modifier = Modifier.width(22.dp).height(22.dp),
                    )
                    Spacer(Modifier.width(12.dp))
                    Text(
                        if (resumeRequested) "Starting…" else "Getting your stream ready…",
                        color = SwarmMuted,
                        fontSize = 14.sp,
                    )
                }
            }
            Spacer(Modifier.height(16.dp))
            Text("Press Back to cancel", color = SwarmMuted, fontSize = 12.sp)
        }
    }
}
