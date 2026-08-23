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
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import androidx.tv.material3.Button
import app.swarm.tv.app.ui.components.swarmActionButtonColors
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmSurface
import app.swarm.tv.app.ui.theme.SwarmText
import app.swarm.tv.core.catalog.MergedEntry
import app.swarm.tv.core.catalog.SeasonGroup
import app.swarm.tv.core.catalog.ShowGroup

@Composable
fun SeasonScreen(
    show: ShowGroup,
    seasonArtworkUrl: (MergedEntry) -> String?,
    episodeArtworkUrl: (MergedEntry) -> String?,
    onPlayEpisode: (MergedEntry) -> Unit,
    onBack: () -> Unit,
    selectedSeason: SeasonGroup?,
    onSelectSeason: (SeasonGroup?) -> Unit,
    isWatchlisted: Boolean,
    onToggleWatchlist: () -> Unit,
) {
    BackHandler { if (selectedSeason != null) onSelectSeason(null) else onBack() }

    val season = selectedSeason
    if (season == null) {
        SeasonList(
            show,
            seasonArtworkUrl,
            isWatchlisted,
            onToggleWatchlist,
            onOpenSeason = onSelectSeason,
        )
    } else {
        EpisodeGrid(season, episodeArtworkUrl, onPlayEpisode)
    }
}

@Composable
private fun SeasonList(
    show: ShowGroup,
    seasonArtworkUrl: (MergedEntry) -> String?,
    isWatchlisted: Boolean,
    onToggleWatchlist: () -> Unit,
    onOpenSeason: (SeasonGroup) -> Unit,
) {
    val firstCardFocusRequester = remember { FocusRequester() }
    LaunchedEffect(show) { if (show.seasons.isNotEmpty()) firstCardFocusRequester.requestFocus() }

    LazyVerticalGrid(
        columns = GridCells.Fixed(5),
        modifier = Modifier.fillMaxSize().padding(40.dp),
        verticalArrangement = Arrangement.spacedBy(20.dp),
        horizontalArrangement = Arrangement.spacedBy(20.dp),
        // Room for tv-material3's focus-scale animation on edge cards — see CatalogScreen.kt.
        contentPadding = PaddingValues(12.dp),
    ) {
        item(
            key = "show-actions",
            span = { GridItemSpan(maxLineSpan) },
            contentType = "header",
        ) {
            Row(
                modifier = Modifier.fillMaxWidth().padding(bottom = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text(
                    show.show,
                    color = SwarmText,
                    fontSize = 22.sp,
                    fontWeight = FontWeight.Black,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f).padding(end = 16.dp),
                )
                Button(onClick = onToggleWatchlist, colors = swarmActionButtonColors()) {
                    Text(if (isWatchlisted) "✓ Watchlisted" else "+ Watchlist", fontSize = 13.sp)
                }
            }
        }
        itemsIndexed(
            items = show.seasons,
            key = { _, season -> season.season ?: -1 },
            contentType = { _, _ -> "season" },
        ) { index, season ->
            val focusModifier = if (index == 0) Modifier.focusRequester(firstCardFocusRequester) else Modifier
            val seasonArt = season.episodes.firstOrNull()?.let(seasonArtworkUrl)
            Card(
                onClick = { onOpenSeason(season) },
                colors = CardDefaults.colors(containerColor = SwarmSurface),
                scale = CardDefaults.scale(scale = 1f, focusedScale = 1f, pressedScale = 0.99f),
                modifier = focusModifier.fillMaxWidth(),
            ) {
                Column {
                    ArtworkImage(
                        label = seasonLabel(season.season),
                        placeholderType = "Show",
                        primaryUrl = seasonArt,
                        modifier = Modifier.fillMaxWidth().aspectRatio(2f / 3f).clip(RoundedCornerShape(4.dp)),
                    )
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

@Composable
private fun EpisodeGrid(
    season: SeasonGroup,
    episodeArtworkUrl: (MergedEntry) -> String?,
    onPlayEpisode: (MergedEntry) -> Unit,
) {
    val firstCardFocusRequester = remember { FocusRequester() }
    LaunchedEffect(season) { if (season.episodes.isNotEmpty()) firstCardFocusRequester.requestFocus() }

    LazyVerticalGrid(
        columns = GridCells.Fixed(4),
        modifier = Modifier.fillMaxSize().padding(40.dp),
        verticalArrangement = Arrangement.spacedBy(20.dp),
        horizontalArrangement = Arrangement.spacedBy(20.dp),
        // Room for tv-material3's focus-scale animation on edge cards — see CatalogScreen.kt.
        contentPadding = PaddingValues(12.dp),
    ) {
        itemsIndexed(
            items = season.episodes,
            key = { _, episode -> episode.entry.entryKey },
            contentType = { _, _ -> "episode" },
        ) { index, episode ->
            val focusModifier = if (index == 0) Modifier.focusRequester(firstCardFocusRequester) else Modifier
            Card(
                onClick = { onPlayEpisode(episode) },
                colors = CardDefaults.colors(containerColor = SwarmSurface),
                scale = CardDefaults.scale(scale = 1f, focusedScale = 1f, pressedScale = 0.99f),
                modifier = focusModifier.fillMaxWidth(),
            ) {
                Column {
                    ArtworkImage(
                        label = episode.entry.scrapedTitle ?: episode.entry.title,
                        placeholderType = "Show",
                        primaryUrl = episodeArtworkUrl(episode),
                        modifier = Modifier.fillMaxWidth().aspectRatio(16f / 9f).clip(RoundedCornerShape(4.dp)),
                    )
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
