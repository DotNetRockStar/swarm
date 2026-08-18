/** Movie detail: backdrop, poster, year/genres, cast (first ~10, TMDb billing order), and a Play button — reached from [CatalogScreen]'s Movies row. */
package app.swarm.tv.app.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.ButtonDefaults
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmSurfaceMuted
import app.swarm.tv.app.ui.theme.SwarmText
import app.swarm.tv.core.catalog.MergedEntry
import coil.compose.AsyncImage

@Composable
fun MovieDetailScreen(
    entry: MergedEntry,
    artworkUrl: (MergedEntry) -> String?,
    backdropUrl: (MergedEntry) -> String?,
    onPlay: (MergedEntry) -> Unit,
    onBack: () -> Unit,
) {
    BackHandler(onBack = onBack)
    val playFocusRequester = remember { FocusRequester() }
    LaunchedEffect(entry) { playFocusRequester.requestFocus() }

    Column(modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(40.dp)) {
        Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            Text(entry.entry.scrapedTitle ?: entry.entry.title, color = SwarmAccent, fontSize = 24.sp, fontWeight = FontWeight.Black, modifier = Modifier.weight(1f))
            Button(onClick = onBack, colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText)) { Text("Back") }
        }
        Spacer(Modifier.height(20.dp))

        backdropUrl(entry)?.let { url ->
            AsyncImage(
                model = url,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxWidth().height(280.dp).clip(RoundedCornerShape(12.dp)),
            )
            Spacer(Modifier.height(24.dp))
        }

        Row {
            artworkUrl(entry)?.let { url ->
                AsyncImage(
                    model = url,
                    contentDescription = null,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier.width(200.dp).aspectRatio(2f / 3f).clip(RoundedCornerShape(8.dp)),
                )
                Spacer(Modifier.width(28.dp))
            }
            Column(Modifier.weight(1f)) {
                val meta = listOfNotNull(entry.entry.year?.toString(), entry.entry.genres.takeIf { it.isNotEmpty() }?.joinToString(", "))
                if (meta.isNotEmpty()) {
                    Text(meta.joinToString("  •  "), color = SwarmMuted, fontSize = 14.sp)
                    Spacer(Modifier.height(16.dp))
                }
                Button(
                    onClick = { onPlay(entry) },
                    modifier = Modifier.focusRequester(playFocusRequester),
                    colors = ButtonDefaults.colors(containerColor = SwarmAccent, contentColor = androidx.compose.ui.graphics.Color(0xFF04263A)),
                ) {
                    Text("Play", fontWeight = FontWeight.Bold)
                }
                if (entry.entry.cast.isNotEmpty()) {
                    Spacer(Modifier.height(24.dp))
                    Text("Cast", color = SwarmMuted, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                    Spacer(Modifier.height(8.dp))
                    for (member in entry.entry.cast.take(10)) {
                        val label = member.character?.let { "${member.name} as $it" } ?: member.name
                        Text(label, color = SwarmText, fontSize = 13.sp)
                    }
                }
            }
        }
    }
}
