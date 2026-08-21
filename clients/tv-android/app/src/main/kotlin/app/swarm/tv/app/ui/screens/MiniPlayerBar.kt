/**
 * "Minimized to tray" — a compact bar docked to the bottom of whatever
 * screen is currently showing (rendered as an overlay sibling in
 * [app.swarm.tv.app.MainActivity]'s `SwarmApp`, on top of the normal
 * browse/detail content, not instead of it) while a track keeps playing in
 * the background. Selecting the bar itself reopens [MusicPlayerScreen] for
 * the same session; the separate stop button ends playback without
 * reopening anything. Deliberately no play/pause here — this is a status
 * strip plus a way back in, not a second full transport control surface;
 * the real controls live on the full screen one D-pad press away.
 */
package app.swarm.tv.app.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.ButtonDefaults
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmSurface
import app.swarm.tv.app.ui.theme.SwarmSurfaceMuted
import app.swarm.tv.app.ui.theme.SwarmText
import app.swarm.tv.core.catalog.MergedEntry
import coil.compose.AsyncImage

@Composable
fun MiniPlayerBar(entry: MergedEntry, isPlaying: Boolean, artworkUrl: String?, onReopen: () -> Unit, onStop: () -> Unit) {
    Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 16.dp),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Card(
                onClick = onReopen,
                modifier = Modifier.weight(1f),
                colors = CardDefaults.colors(containerColor = SwarmSurface),
            ) {
                Row(modifier = Modifier.padding(10.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    if (artworkUrl != null) {
                        AsyncImage(
                            model = artworkUrl,
                            contentDescription = null,
                            contentScale = ContentScale.Crop,
                            modifier = Modifier.width(44.dp).aspectRatio(1f).clip(RoundedCornerShape(6.dp)),
                        )
                    }
                    Column(Modifier.weight(1f)) {
                        Text(
                            entry.entry.scrapedTitle ?: entry.entry.title,
                            color = SwarmText,
                            fontSize = 14.sp,
                            fontWeight = FontWeight.SemiBold,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        entry.entry.artist?.let {
                            Text(it, color = SwarmMuted, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                    Text(if (isPlaying) "▶ playing" else "⏸ paused", color = SwarmAccent, fontSize = 11.sp)
                }
            }
            Button(onClick = onStop, colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText)) {
                Text("Stop", fontSize = 13.sp)
            }
        }
    }
}
