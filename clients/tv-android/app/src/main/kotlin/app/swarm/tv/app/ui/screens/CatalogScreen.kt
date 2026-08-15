/**
 * The merged multi-server catalog — the payoff of `peer_addr` self-report
 * plus [app.swarm.tv.core.catalog.CatalogSession]: every server in the
 * swarm that's currently dialable is connected to directly, and their
 * libraries appear as one browsable list grouped by kind. Selecting an
 * entry hands its roster-device-id + entry key to [PlayerScreen] via
 * [app.swarm.tv.core.catalog.CatalogSession.urlFor].
 */
package app.swarm.tv.app.ui.screens

import androidx.activity.compose.BackHandler
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
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontWeight
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
import app.swarm.tv.core.peer.MediaKind
import app.swarm.tv.core.rest.SwarmDevice
import app.swarm.tv.core.rest.SwarmSummary
import coil.compose.AsyncImage

private val KIND_ROWS = listOf(MediaKind.MOVIE to "Movies", MediaKind.EPISODE to "Shows", MediaKind.TRACK to "Music")

@Composable
fun CatalogScreen(
    swarm: SwarmSummary,
    entries: List<MergedEntry>,
    loading: Boolean,
    unreachable: List<SwarmDevice>,
    artworkUrl: (MergedEntry) -> String?,
    onPlay: (MergedEntry) -> Unit,
    onBack: () -> Unit,
) {
    BackHandler(onBack = onBack)

    Column(modifier = Modifier.fillMaxSize().padding(40.dp)) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column {
                Text("SWARM", color = SwarmAccent, fontSize = 22.sp, fontWeight = FontWeight.Black)
                Text("${swarm.name} — library", color = SwarmText, fontSize = 16.sp)
            }
            Button(onClick = onBack, colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText)) {
                Text("Back")
            }
        }

        if (unreachable.isNotEmpty()) {
            Spacer(Modifier.height(12.dp))
            Text(
                "${unreachable.size} server(s) not reachable yet: ${unreachable.joinToString { it.name }}",
                color = SwarmMuted,
                fontSize = 12.sp,
            )
        }
        Spacer(Modifier.height(24.dp))

        when {
            loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(color = SwarmAccent)
            }
            entries.isEmpty() -> Text("Nothing in the catalog yet.", color = SwarmMuted, fontSize = 14.sp)
            else -> LazyColumn(verticalArrangement = Arrangement.spacedBy(32.dp)) {
                for ((kind, label) in KIND_ROWS) {
                    val row = entries.filter { it.entry.kind == kind }
                    if (row.isNotEmpty()) {
                        item { CatalogRow(label, row, artworkUrl, onPlay) }
                    }
                }
            }
        }
    }
}

@Composable
private fun CatalogRow(label: String, entries: List<MergedEntry>, artworkUrl: (MergedEntry) -> String?, onPlay: (MergedEntry) -> Unit) {
    Column {
        Text(label, color = SwarmMuted, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
        Spacer(Modifier.height(10.dp))
        LazyRow(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            items(entries) { entry -> CatalogCard(entry, artworkUrl(entry), onClick = { onPlay(entry) }) }
        }
    }
}

@Composable
private fun CatalogCard(merged: MergedEntry, artworkUrl: String?, onClick: () -> Unit) {
    Card(onClick = onClick, colors = CardDefaults.colors(containerColor = SwarmSurface), modifier = Modifier.width(160.dp)) {
        Column {
            if (artworkUrl != null) {
                AsyncImage(
                    model = artworkUrl,
                    contentDescription = null,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier.fillMaxWidth().aspectRatio(2f / 3f).clip(RoundedCornerShape(4.dp)),
                )
            }
            Column(modifier = Modifier.padding(14.dp)) {
                Text(
                    merged.entry.scrapedTitle ?: merged.entry.title,
                    color = SwarmText,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 2,
                )
                if (merged.sources.size > 1) {
                    Spacer(Modifier.height(6.dp))
                    Text("${merged.sources.size} sources", color = SwarmAccent, fontSize = 11.sp)
                }
            }
        }
    }
}
