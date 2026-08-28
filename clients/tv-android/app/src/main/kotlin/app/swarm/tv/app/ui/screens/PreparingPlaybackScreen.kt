/**
 * The full-screen cover shown the instant a fresh play is requested from a
 * browse/detail screen (most visibly a "Continue Watching" tap), replacing
 * the frozen catalog while the server session is negotiated and the first
 * frames buffer (#122). Mid-playback handoffs (next episode, out-of-buffer
 * seek) keep their plain video backdrop instead — see
 * [app.swarm.tv.app.data.UiState.PreparingPlayback].
 */
package app.swarm.tv.app.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.swarm.tv.app.ui.UatTestTags
import app.swarm.tv.app.ui.components.SwarmLoadingIndicator
import app.swarm.tv.app.ui.theme.SwarmBackground
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmText
import coil.compose.AsyncImage

@Composable
fun PreparingPlaybackScreen(
    title: String,
    artworkUrl: String?,
    onCancel: () -> Unit,
) {
    // Back abandons the negotiation and drops straight back to the screen the
    // play was started from, so a mistaken tap isn't a forced wait.
    BackHandler(onBack = onCancel)
    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(SwarmBackground)
            .testTag(UatTestTags.PREPARING_PLAYBACK),
        contentAlignment = Alignment.Center,
    ) {
        artworkUrl?.let { url ->
            AsyncImage(
                model = url,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxSize(),
            )
            Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.7f)))
        }
        Column(
            modifier = Modifier.padding(48.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(
                title,
                color = SwarmText,
                fontSize = 26.sp,
                fontWeight = FontWeight.Black,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(22.dp))
            SwarmLoadingIndicator()
            Spacer(Modifier.height(16.dp))
            Text("Press Back to cancel", color = SwarmMuted, fontSize = 13.sp)
        }
    }
}
