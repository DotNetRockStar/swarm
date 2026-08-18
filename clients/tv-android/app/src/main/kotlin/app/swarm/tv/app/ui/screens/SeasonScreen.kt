/**
 * One show: a list of its seasons, drilling into an episode grid on
 * selection — same local-state pattern as [AlbumScreen] (see its doc
 * comment for the rationale). Selecting an episode calls [onOpenEpisode]
 * (a real [app.swarm.tv.app.data.UiState] transition, since episode detail
 * needs Play + Continue/next-episode wiring, unlike a track).
 */
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
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
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
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmSurface
import app.swarm.tv.app.ui.theme.SwarmSurfaceMuted
import app.swarm.tv.app.ui.theme.SwarmText
import app.swarm.tv.core.catalog.MergedEntry
import app.swarm.tv.core.catalog.SeasonGroup
import app.swarm.tv.core.catalog.ShowGroup
import coil.compose.AsyncImage

@Composable
fun SeasonScreen(
    show: ShowGroup,
    artworkUrl: (MergedEntry) -> String?,
    onOpenEpisode: (MergedEntry) -> Unit,
    onBack: () -> Unit,
) {
    var selectedSeason by remember(show) { mutableStateOf<SeasonGroup?>(null) }
    BackHandler { if (selectedSeason != null) selectedSeason = null else onBack() }

    val season = selectedSeason
    if (season == null) {
        SeasonList(show, onOpenSeason = { selectedSeason = it }, onBack = onBack)
    } else {
        EpisodeGrid(show, season, artworkUrl, onOpenEpisode, onBack = { selectedSeason = null })
    }
}

@Composable
private fun SeasonList(show: ShowGroup, onOpenSeason: (SeasonGroup) -> Unit, onBack: () -> Unit) {
    val firstRowFocusRequester = remember { FocusRequester() }
    LaunchedEffect(show) { if (show.seasons.isNotEmpty()) firstRowFocusRequester.requestFocus() }

    Column(modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(40.dp)) {
        Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            Text(show.show, color = SwarmAccent, fontSize = 22.sp, fontWeight = FontWeight.Black)
            Button(onClick = onBack, colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText)) { Text("Back") }
        }
        Spacer(Modifier.height(24.dp))
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            for ((index, season) in show.seasons.withIndex()) {
                val focusModifier = if (index == 0) Modifier.focusRequester(firstRowFocusRequester) else Modifier
                Card(onClick = { onOpenSeason(season) }, colors = CardDefaults.colors(containerColor = SwarmSurface), modifier = focusModifier.fillMaxWidth()) {
                    Row(modifier = Modifier.fillMaxWidth().padding(16.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                        Text(season.season?.let { "Season $it" } ?: "Unnumbered episodes", color = SwarmText, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                        Text("${season.episodes.size} episode" + if (season.episodes.size == 1) "" else "s", color = SwarmMuted, fontSize = 13.sp)
                    }
                }
            }
        }
    }
}

@Composable
private fun EpisodeGrid(
    show: ShowGroup,
    season: SeasonGroup,
    artworkUrl: (MergedEntry) -> String?,
    onOpenEpisode: (MergedEntry) -> Unit,
    onBack: () -> Unit,
) {
    val firstCardFocusRequester = remember { FocusRequester() }
    LaunchedEffect(season) { if (season.episodes.isNotEmpty()) firstCardFocusRequester.requestFocus() }

    Column(modifier = Modifier.fillMaxSize().padding(40.dp)) {
        Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            Text("${show.show} — " + (season.season?.let { "Season $it" } ?: "Unnumbered"), color = SwarmAccent, fontSize = 20.sp, fontWeight = FontWeight.Black)
            Button(onClick = onBack, colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText)) { Text("Back") }
        }
        Spacer(Modifier.height(24.dp))
        LazyVerticalGrid(columns = GridCells.Fixed(4), verticalArrangement = Arrangement.spacedBy(20.dp), horizontalArrangement = Arrangement.spacedBy(20.dp)) {
            itemsIndexed(season.episodes) { index, episode ->
                val focusModifier = if (index == 0) Modifier.focusRequester(firstCardFocusRequester) else Modifier
                Card(onClick = { onOpenEpisode(episode) }, colors = CardDefaults.colors(containerColor = SwarmSurface), modifier = focusModifier.fillMaxWidth()) {
                    Column {
                        artworkUrl(episode)?.let { url ->
                            AsyncImage(
                                model = url,
                                contentDescription = null,
                                contentScale = ContentScale.Crop,
                                modifier = Modifier.fillMaxWidth().aspectRatio(16f / 9f).clip(RoundedCornerShape(4.dp)),
                            )
                        }
                        Column(Modifier.padding(14.dp)) {
                            Text(
                                episode.entry.episode?.let { "Episode $it" } ?: "Episode",
                                color = SwarmMuted,
                                fontSize = 11.sp,
                            )
                            Text(episode.entry.scrapedTitle ?: episode.entry.title, color = SwarmText, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, maxLines = 2)
                        }
                    }
                }
            }
        }
    }
}
