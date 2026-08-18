/**
 * The merged multi-server catalog — the payoff of `peer_addr` self-report
 * plus [app.swarm.tv.core.catalog.CatalogSession]: every server in the
 * swarm that's currently dialable is connected to directly, and their
 * libraries appear as one browsable list grouped by kind. Movies stay a
 * flat shelf (each is its own leaf); Shows and Music are grouped
 * client-side ([app.swarm.tv.core.catalog.CatalogGrouping]) into Show and
 * Artist shelves — clicking a card in either goes straight one level
 * deeper ([SeasonScreen]/[AlbumScreen]), and the row's own header opens a
 * fuller grid ([ShowShelfScreen]/[ArtistShelfScreen]) for browsing many at
 * once.
 */
package app.swarm.tv.app.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
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
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
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
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmSurface
import app.swarm.tv.app.ui.theme.SwarmSurfaceMuted
import app.swarm.tv.app.ui.theme.SwarmText
import app.swarm.tv.core.catalog.ArtistGroup
import app.swarm.tv.core.catalog.CatalogGrouping
import app.swarm.tv.core.catalog.MergedEntry
import app.swarm.tv.core.catalog.ShowGroup
import app.swarm.tv.core.peer.MediaKind
import app.swarm.tv.core.rest.SwarmDevice
import app.swarm.tv.core.rest.SwarmSummary
import coil.compose.AsyncImage

@Composable
fun CatalogScreen(
    swarm: SwarmSummary,
    entries: List<MergedEntry>,
    loading: Boolean,
    unreachable: List<SwarmDevice>,
    playbackError: String?,
    artworkUrl: (MergedEntry) -> String?,
    onOpenMovie: (MergedEntry) -> Unit,
    onOpenArtistShelf: () -> Unit,
    onOpenArtist: (ArtistGroup) -> Unit,
    onOpenShowShelf: () -> Unit,
    onOpenShow: (ShowGroup) -> Unit,
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
        if (playbackError != null) {
            Spacer(Modifier.height(12.dp))
            Text(playbackError, color = SwarmAccent, fontSize = 12.sp)
        }
        Spacer(Modifier.height(24.dp))

        when {
            loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(color = SwarmAccent)
            }
            entries.isEmpty() -> Text("Nothing in the catalog yet.", color = SwarmMuted, fontSize = 14.sp)
            else -> {
                val movies = remember(entries) { entries.filter { it.entry.kind == MediaKind.MOVIE } }
                val shows = remember(entries) { CatalogGrouping.groupEpisodesByShowSeason(entries) }
                val artists = remember(entries) { CatalogGrouping.groupTracksByArtistAlbum(entries) }

                // Reached via a UiState swap (see SwarmDashboardScreen's doc
                // comment for the same fix) — nothing else ever requests
                // D-pad focus here, so without this the first card is
                // visible but silently unreachable by remote.
                val firstCardFocusRequester = remember { FocusRequester() }
                LaunchedEffect(entries) { firstCardFocusRequester.requestFocus() }
                val firstSection = when {
                    movies.isNotEmpty() -> "movies"
                    shows.isNotEmpty() -> "shows"
                    artists.isNotEmpty() -> "music"
                    else -> null
                }

                LazyColumn(verticalArrangement = Arrangement.spacedBy(32.dp)) {
                    if (movies.isNotEmpty()) {
                        item {
                            MovieRow(movies, artworkUrl, onOpenMovie, if (firstSection == "movies") firstCardFocusRequester else null)
                        }
                    }
                    if (shows.isNotEmpty()) {
                        item {
                            ShowShelfRow(shows, artworkUrl, onOpenShowShelf, onOpenShow, if (firstSection == "shows") firstCardFocusRequester else null)
                        }
                    }
                    if (artists.isNotEmpty()) {
                        item {
                            ArtistShelfRow(artists, onOpenArtistShelf, onOpenArtist, if (firstSection == "music") firstCardFocusRequester else null)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun ShelfHeader(label: String, onOpenAll: () -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(label, color = SwarmMuted, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
        Button(onClick = onOpenAll, colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmAccent)) {
            Text("Browse all", fontSize = 12.sp)
        }
    }
}

@Composable
private fun MovieRow(
    movies: List<MergedEntry>,
    artworkUrl: (MergedEntry) -> String?,
    onOpenMovie: (MergedEntry) -> Unit,
    firstCardFocusRequester: FocusRequester?,
) {
    Column {
        Text("Movies", color = SwarmMuted, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
        Spacer(Modifier.height(10.dp))
        LazyRow(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            itemsIndexed(movies) { index, entry ->
                CatalogCard(entry, artworkUrl(entry), onClick = { onOpenMovie(entry) }, focusRequester = if (index == 0) firstCardFocusRequester else null)
            }
        }
    }
}

@Composable
private fun ShowShelfRow(
    shows: List<ShowGroup>,
    artworkUrl: (MergedEntry) -> String?,
    onOpenShowShelf: () -> Unit,
    onOpenShow: (ShowGroup) -> Unit,
    firstCardFocusRequester: FocusRequester?,
) {
    Column {
        ShelfHeader("Shows", onOpenShowShelf)
        Spacer(Modifier.height(10.dp))
        LazyRow(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            itemsIndexed(shows) { index, show ->
                val representative = show.seasons.firstOrNull()?.episodes?.firstOrNull()
                GroupCard(
                    title = show.show,
                    subtitle = "${show.seasons.size} season" + if (show.seasons.size == 1) "" else "s",
                    artworkUrl = representative?.let(artworkUrl),
                    onClick = { onOpenShow(show) },
                    focusRequester = if (index == 0) firstCardFocusRequester else null,
                )
            }
        }
    }
}

@Composable
private fun ArtistShelfRow(
    artists: List<ArtistGroup>,
    onOpenArtistShelf: () -> Unit,
    onOpenArtist: (ArtistGroup) -> Unit,
    firstCardFocusRequester: FocusRequester?,
) {
    Column {
        ShelfHeader("Music", onOpenArtistShelf)
        Spacer(Modifier.height(10.dp))
        LazyRow(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            itemsIndexed(artists) { index, artist ->
                val albumCount = artist.albums.size
                GroupCard(
                    title = artist.artist,
                    subtitle = "$albumCount album" + if (albumCount == 1) "" else "s",
                    artworkUrl = null,
                    onClick = { onOpenArtist(artist) },
                    focusRequester = if (index == 0) firstCardFocusRequester else null,
                )
            }
        }
    }
}

@Composable
private fun CatalogCard(merged: MergedEntry, artworkUrl: String?, onClick: () -> Unit, focusRequester: FocusRequester?) {
    val focusModifier = if (focusRequester != null) Modifier.focusRequester(focusRequester) else Modifier
    Card(
        onClick = onClick,
        colors = CardDefaults.colors(containerColor = SwarmSurface),
        modifier = focusModifier.width(160.dp),
    ) {
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

/** A Show or Artist shelf card — no per-group artwork field exists server-side, so a group falls back to a representative entry's art (Show) or a plain initial (Artist, per the plan's explicit fallback allowance). */
@Composable
private fun GroupCard(title: String, subtitle: String, artworkUrl: String?, onClick: () -> Unit, focusRequester: FocusRequester?) {
    val focusModifier = if (focusRequester != null) Modifier.focusRequester(focusRequester) else Modifier
    Card(
        onClick = onClick,
        colors = CardDefaults.colors(containerColor = SwarmSurface),
        modifier = focusModifier.width(160.dp),
    ) {
        Column {
            if (artworkUrl != null) {
                AsyncImage(
                    model = artworkUrl,
                    contentDescription = null,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier.fillMaxWidth().aspectRatio(2f / 3f).clip(RoundedCornerShape(4.dp)),
                )
            } else {
                Box(
                    modifier = Modifier.fillMaxWidth().aspectRatio(2f / 3f).clip(RoundedCornerShape(4.dp)),
                    contentAlignment = Alignment.Center,
                ) {
                    InitialBadge(title)
                }
            }
            Column(modifier = Modifier.padding(14.dp)) {
                Text(title, color = SwarmText, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, maxLines = 2)
                Spacer(Modifier.height(6.dp))
                Text(subtitle, color = SwarmMuted, fontSize = 11.sp)
            }
        }
    }
}

@Composable
internal fun InitialBadge(title: String) {
    Box(
        modifier = Modifier.size(56.dp).clip(RoundedCornerShape(28.dp)).background(SwarmSurfaceMuted),
        contentAlignment = Alignment.Center,
    ) {
        Text(title.trim().firstOrNull()?.uppercaseChar()?.toString() ?: "?", color = SwarmAccent, fontSize = 22.sp, fontWeight = FontWeight.Black)
    }
}
