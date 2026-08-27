/**
 * One artist: a grid of their albums, drilling into a track list on
 * selection — kept as one screen with local `selectedAlbum` state (not a
 * separate [app.swarm.tv.app.data.UiState] level) since both views only
 * ever need data already in hand from [app.swarm.tv.core.catalog.ArtistGroup],
 * no new negotiation or catalog fetch either way. `BackHandler` pops the
 * album selection first, then falls through to [onBack] (up to the artist
 * shelf) once at the top of this screen.
 */
package app.swarm.tv.app.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.BorderStroke
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.itemsIndexed
import androidx.compose.foundation.lazy.itemsIndexed as columnItemsIndexed
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
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Border
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import app.swarm.tv.app.ui.UatTestTags
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmAccentHot
import app.swarm.tv.app.ui.theme.SwarmBorder
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmSurface
import app.swarm.tv.app.ui.theme.SwarmSurfaceMuted
import app.swarm.tv.app.ui.theme.SwarmText
import app.swarm.tv.core.catalog.AlbumGroup
import app.swarm.tv.core.catalog.ArtistGroup
import app.swarm.tv.core.catalog.MergedEntry

@Composable
fun AlbumScreen(
    artist: ArtistGroup,
    artworkUrl: (MergedEntry) -> String?,
    onPlay: (MergedEntry) -> Unit,
    onBack: () -> Unit,
) {
    var selectedAlbum by remember(artist) { mutableStateOf<AlbumGroup?>(null) }
    BackHandler { if (selectedAlbum != null) selectedAlbum = null else onBack() }

    val album = selectedAlbum
    if (album == null) {
        AlbumGrid(artist, artworkUrl, onOpenAlbum = { selectedAlbum = it })
    } else {
        TrackList(artist, album, artworkUrl, onPlay)
    }
}

@Composable
private fun AlbumGrid(
    artist: ArtistGroup,
    artworkUrl: (MergedEntry) -> String?,
    onOpenAlbum: (AlbumGroup) -> Unit,
) {
    val firstCardFocusRequester = remember { FocusRequester() }
    LaunchedEffect(artist) { if (artist.albums.isNotEmpty()) firstCardFocusRequester.requestFocus() }

    Column(modifier = Modifier.fillMaxSize().padding(horizontal = 40.dp)) {
        LazyVerticalGrid(
            columns = GridCells.Fixed(5),
            verticalArrangement = Arrangement.spacedBy(20.dp),
            horizontalArrangement = Arrangement.spacedBy(20.dp),
            contentPadding = PaddingValues(start = 12.dp, end = 12.dp, top = 32.dp, bottom = 12.dp),
        ) {
            itemsIndexed(
                items = artist.albums,
                key = { _, album -> album.album },
                contentType = { _, _ -> "album" },
            ) { index, album ->
                val focusModifier = if (index == 0) Modifier.focusRequester(firstCardFocusRequester) else Modifier
                val cover = remember(album, artworkUrl) { album.coverArtworkUrl(artworkUrl) }
                Card(
                    onClick = { onOpenAlbum(album) },
                    colors = CardDefaults.colors(containerColor = SwarmSurface),
                    modifier = focusModifier.fillMaxWidth().testTag(UatTestTags.ALBUM_CARD_PREFIX + album.album),
                ) {
                    Column {
                        ArtworkImage(
                            label = album.album,
                            placeholderType = "Album",
                            primaryUrl = cover,
                            modifier = Modifier.fillMaxWidth().aspectRatio(1f).clip(RoundedCornerShape(8.dp)),
                        )
                        Column(Modifier.padding(14.dp)) {
                            Text(album.album, color = SwarmText, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, minLines = 2, maxLines = 2)
                            Spacer(Modifier.height(6.dp))
                            Text("${album.tracks.size} track" + if (album.tracks.size == 1) "" else "s", color = SwarmMuted, fontSize = 11.sp)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun TrackList(
    artist: ArtistGroup,
    album: AlbumGroup,
    artworkUrl: (MergedEntry) -> String?,
    onPlay: (MergedEntry) -> Unit,
) {
    val firstRowFocusRequester = remember { FocusRequester() }
    LaunchedEffect(album) { if (album.tracks.isNotEmpty()) firstRowFocusRequester.requestFocus() }
    val cover = album.coverArtworkUrl(artworkUrl)

    Column(modifier = Modifier.fillMaxSize().padding(40.dp)) {
        Column {
            Text(album.album, color = SwarmAccent, fontSize = 22.sp, fontWeight = FontWeight.Black)
            Text(artist.artist, color = SwarmMuted, fontSize = 14.sp)
        }
        Spacer(Modifier.height(18.dp))
        Row {
            ArtworkImage(
                label = album.album,
                placeholderType = "Album",
                primaryUrl = cover,
                modifier = Modifier.width(180.dp).aspectRatio(1f).clip(RoundedCornerShape(8.dp)),
            )
            Spacer(Modifier.width(24.dp))
            LazyColumn(modifier = Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                columnItemsIndexed(
                    items = album.tracks,
                    key = { _, track -> track.entry.entryKey },
                    contentType = { _, _ -> "track" },
                ) { index, track ->
                    val focusModifier = if (index == 0) Modifier.focusRequester(firstRowFocusRequester) else Modifier
                    TrackRow(track, focusModifier, onClick = { onPlay(track) })
                }
            }
        }
    }
}

@Composable
private fun TrackRow(track: MergedEntry, focusModifier: Modifier, onClick: () -> Unit) {
    val rowShape = RoundedCornerShape(8.dp)
    Card(
        onClick = onClick,
        colors = CardDefaults.colors(containerColor = SwarmSurface, focusedContainerColor = SwarmSurfaceMuted),
        scale = CardDefaults.scale(scale = 1f, focusedScale = 1f, pressedScale = 0.99f),
        border = CardDefaults.border(
            border = Border(BorderStroke(1.dp, SwarmBorder), shape = rowShape),
            focusedBorder = Border(BorderStroke(2.dp, SwarmAccent), shape = rowShape),
            pressedBorder = Border(BorderStroke(2.dp, SwarmAccentHot), shape = rowShape),
        ),
        shape = CardDefaults.shape(rowShape, rowShape, rowShape),
        modifier = focusModifier.fillMaxWidth().testTag(UatTestTags.TRACK_ROW_PREFIX + track.entry.entryKey),
    ) {
        Row(modifier = Modifier.fillMaxWidth().padding(14.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            Text(track.entry.trackNumber?.toString() ?: "–", color = SwarmMuted, fontSize = 14.sp, modifier = Modifier.width(24.dp))
            Text(track.entry.scrapedTitle ?: track.entry.title, color = SwarmText, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, modifier = Modifier.weight(1f))
            track.entry.durationSecs?.let { secs ->
                val minutes = (secs / 60).toInt()
                val seconds = (secs % 60).toInt()
                Text("%d:%02d".format(minutes, seconds), color = SwarmMuted, fontSize = 12.sp)
            }
        }
    }
}
