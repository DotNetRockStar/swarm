/**
 * One show: a list of its seasons, drilling into an episode grid on
 * selection — same local-state pattern as [AlbumScreen] (see its doc
 * comment for the rationale). Selecting an episode calls [onPlayEpisode]
 * directly — no separate detail screen in between, so getting to an
 * episode never takes more than season -> episode -> playing.
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
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
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
    onPlayEpisode: (MergedEntry) -> Unit,
    onBack: () -> Unit,
) {
    var selectedSeason by remember(show) { mutableStateOf<SeasonGroup?>(null) }
    BackHandler { if (selectedSeason != null) selectedSeason = null else onBack() }

    val season = selectedSeason
    if (season == null) {
        SeasonList(show, artworkUrl, onOpenSeason = { selectedSeason = it }, onBack = onBack)
    } else {
        EpisodeGrid(show, season, artworkUrl, onPlayEpisode, onBack = { selectedSeason = null })
    }
}

@Composable
private fun SeasonList(show: ShowGroup, artworkUrl: (MergedEntry) -> String?, onOpenSeason: (SeasonGroup) -> Unit, onBack: () -> Unit) {
    val firstCardFocusRequester = remember { FocusRequester() }
    LaunchedEffect(show) { if (show.seasons.isNotEmpty()) firstCardFocusRequester.requestFocus() }

    Column(modifier = Modifier.fillMaxSize().padding(40.dp)) {
        Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            Text(show.show, color = SwarmAccent, fontSize = 22.sp, fontWeight = FontWeight.Black)
            Button(onClick = onBack, colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText)) { Text("Back") }
        }
        Spacer(Modifier.height(24.dp))
        LazyVerticalGrid(
            columns = GridCells.Fixed(5),
            verticalArrangement = Arrangement.spacedBy(20.dp),
            horizontalArrangement = Arrangement.spacedBy(20.dp),
            // Room for tv-material3's focus-scale animation on edge cards — see CatalogScreen.kt.
            contentPadding = PaddingValues(12.dp),
        ) {
            itemsIndexed(show.seasons) { index, season ->
                val focusModifier = if (index == 0) Modifier.focusRequester(firstCardFocusRequester) else Modifier
                // No distinct per-season poster concept exists server-side
                // (TMDb TV scraping here is show-level only) — this falls
                // back to this season's own first episode's artwork, the
                // same representative-entry fallback the browse page's own
                // show card already uses (CatalogScreen.kt's ShowShelfRow),
                // just scoped to this one season instead of the whole show.
                val seasonArt = season.episodes.firstOrNull()?.let(artworkUrl)
                Card(onClick = { onOpenSeason(season) }, colors = CardDefaults.colors(containerColor = SwarmSurface), modifier = focusModifier.fillMaxWidth()) {
                    Column {
                        if (seasonArt != null) {
                            AsyncImage(
                                model = seasonArt,
                                contentDescription = null,
                                contentScale = ContentScale.Crop,
                                modifier = Modifier.fillMaxWidth().aspectRatio(2f / 3f).clip(RoundedCornerShape(4.dp)),
                            )
                        } else {
                            Box(
                                modifier = Modifier.fillMaxWidth().aspectRatio(2f / 3f).clip(RoundedCornerShape(4.dp)),
                                contentAlignment = Alignment.Center,
                            ) {
                                InitialBadge(seasonLabel(season.season))
                            }
                        }
                        Column(Modifier.padding(14.dp)) {
                            Text(seasonLabel(season.season), color = SwarmText, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
                            Spacer(Modifier.height(4.dp))
                            Text("${season.episodes.size} episode" + if (season.episodes.size == 1) "" else "s", color = SwarmMuted, fontSize = 11.sp)
                        }
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
    onPlayEpisode: (MergedEntry) -> Unit,
    onBack: () -> Unit,
) {
    val firstCardFocusRequester = remember { FocusRequester() }
    LaunchedEffect(season) { if (season.episodes.isNotEmpty()) firstCardFocusRequester.requestFocus() }

    Column(modifier = Modifier.fillMaxSize().padding(40.dp)) {
        Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            Text("${show.show} — " + seasonLabel(season.season), color = SwarmAccent, fontSize = 20.sp, fontWeight = FontWeight.Black)
            Button(onClick = onBack, colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText)) { Text("Back") }
        }
        Spacer(Modifier.height(24.dp))
        LazyVerticalGrid(
            columns = GridCells.Fixed(4),
            verticalArrangement = Arrangement.spacedBy(20.dp),
            horizontalArrangement = Arrangement.spacedBy(20.dp),
            // Room for tv-material3's focus-scale animation on edge cards — see CatalogScreen.kt.
            contentPadding = PaddingValues(12.dp),
        ) {
            itemsIndexed(season.episodes) { index, episode ->
                val focusModifier = if (index == 0) Modifier.focusRequester(firstCardFocusRequester) else Modifier
                Card(onClick = { onPlayEpisode(episode) }, colors = CardDefaults.colors(containerColor = SwarmSurface), modifier = focusModifier.fillMaxWidth()) {
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
                            Text(episode.entry.scrapedTitle ?: episode.entry.title, color = SwarmText, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, minLines = 2, maxLines = 2)
                        }
                    }
                }
            }
        }
    }
}

/**
 * Season 0 is the real-world Plex/Kodi/TheTVDB convention for a show's
 * bonus/extra content (featurettes, interviews, deleted scenes...) — a
 * single show-level bucket, not tied to any one real season. `null` means
 * classify() found no season signal at all, a different, unrelated case.
 */
private fun seasonLabel(season: Int?): String = when (season) {
    null -> "Unnumbered"
    0 -> "Specials"
    else -> "Season $season"
}
