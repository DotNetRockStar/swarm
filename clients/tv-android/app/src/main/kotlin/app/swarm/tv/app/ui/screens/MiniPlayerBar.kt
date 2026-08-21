/**
 * "Minimized to tray" — a compact control docked to the bottom-right of whatever
 * screen is currently showing (rendered as an overlay sibling in
 * [app.swarm.tv.app.MainActivity]'s `SwarmApp`, on top of the normal
 * browse/detail content, not instead of it) while a track keeps playing in
 * the background. Its root is only the size of the two visible controls —
 * never a full-screen overlay that can intercept focus from content behind
 * it. Selecting the artwork reopens [MusicPlayerScreen] for the same session;
 * the separate stop control ends playback without reopening anything.
 */
package app.swarm.tv.app.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmAccentHot
import app.swarm.tv.app.ui.theme.SwarmSurface
import app.swarm.tv.app.ui.theme.SwarmText
import app.swarm.tv.core.catalog.MergedEntry
import coil.compose.AsyncImage

@Composable
fun MiniPlayerBar(
    entry: MergedEntry,
    isPlaying: Boolean,
    artworkUrl: String?,
    onReopen: () -> Unit,
    onStop: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier = modifier,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Card(
            onClick = onReopen,
            modifier = Modifier.width(52.dp).height(52.dp),
            colors = CardDefaults.colors(containerColor = SwarmSurface),
        ) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                if (artworkUrl != null) {
                    AsyncImage(
                        model = artworkUrl,
                        contentDescription = "Reopen ${entry.entry.scrapedTitle ?: entry.entry.title}",
                        contentScale = ContentScale.Crop,
                        modifier = Modifier.fillMaxSize().clip(RoundedCornerShape(6.dp)),
                    )
                } else {
                    Text("♫", color = SwarmAccent, fontSize = 22.sp)
                }
                Text(
                    if (isPlaying) "▶" else "⏸",
                    color = SwarmText,
                    fontSize = 9.sp,
                    modifier = Modifier.align(Alignment.BottomEnd).padding(4.dp),
                )
            }
        }
        Card(
            onClick = onStop,
            modifier = Modifier.width(46.dp).height(46.dp),
            colors = CardDefaults.colors(
                containerColor = Color.White,
                focusedContainerColor = SwarmAccent,
                pressedContainerColor = SwarmAccentHot,
            ),
        ) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text("✕", color = Color(0xFF04263A), fontSize = 15.sp)
            }
        }
    }
}
