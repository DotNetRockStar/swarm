/** Full grid of every artist in the catalog — reached from [CatalogScreen]'s Music row header ("Browse all"), a fuller alternative to that row's horizontal preview. Selecting an artist opens [AlbumScreen]. */
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
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
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

@Composable
fun ArtistShelfScreen(artists: List<ArtistGroup>, onOpenArtist: (ArtistGroup) -> Unit, onBack: () -> Unit) {
    BackHandler(onBack = onBack)

    val firstCardFocusRequester = remember { FocusRequester() }
    LaunchedEffect(artists) { if (artists.isNotEmpty()) firstCardFocusRequester.requestFocus() }

    Column(modifier = Modifier.fillMaxSize().padding(40.dp)) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("Music — ${artists.size} artist" + if (artists.size == 1) "" else "s", color = SwarmAccent, fontSize = 22.sp, fontWeight = FontWeight.Black)
            Button(onClick = onBack, colors = ButtonDefaults.colors(containerColor = SwarmSurfaceMuted, contentColor = SwarmText)) {
                Text("Back")
            }
        }
        Spacer(Modifier.height(24.dp))

        if (artists.isEmpty()) {
            Text("No music in the catalog yet.", color = SwarmMuted, fontSize = 14.sp)
        } else {
            LazyVerticalGrid(
                columns = GridCells.Fixed(5),
                verticalArrangement = Arrangement.spacedBy(20.dp),
                horizontalArrangement = Arrangement.spacedBy(20.dp),
            ) {
                itemsIndexed(artists) { index, artist ->
                    val focusModifier = if (index == 0) Modifier.focusRequester(firstCardFocusRequester) else Modifier
                    Card(
                        onClick = { onOpenArtist(artist) },
                        colors = CardDefaults.colors(containerColor = SwarmSurface),
                        modifier = focusModifier.fillMaxWidth(),
                    ) {
                        Column(Modifier.padding(16.dp)) {
                            Box(
                                modifier = Modifier.fillMaxWidth().aspectRatio(1f).clip(RoundedCornerShape(8.dp)).background(SwarmSurfaceMuted),
                                contentAlignment = Alignment.Center,
                            ) {
                                Text(
                                    artist.artist.trim().firstOrNull()?.uppercaseChar()?.toString() ?: "?",
                                    color = SwarmAccent,
                                    fontSize = 30.sp,
                                    fontWeight = FontWeight.Black,
                                )
                            }
                            Spacer(Modifier.height(10.dp))
                            Text(artist.artist, color = SwarmText, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, maxLines = 2)
                            Text("${artist.albums.size} album" + if (artist.albums.size == 1) "" else "s", color = SwarmMuted, fontSize = 11.sp)
                        }
                    }
                }
            }
        }
    }
}
