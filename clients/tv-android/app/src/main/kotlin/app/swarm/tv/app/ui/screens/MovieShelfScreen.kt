/**
 * Full grid of every movie in the catalog — reached from [CatalogScreen]'s
 * Movies row header ("Browse all"). Selecting a movie opens
 * [MovieDetailScreen].
 *
 * No title/Back header — the remote's own physical Back button (wired via
 * [BackHandler]) already does what an on-screen one would, same reasoning
 * [CatalogScreen] dropped its own header for.
 */
package app.swarm.tv.app.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.itemsIndexed
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmSurface
import app.swarm.tv.app.ui.UatTestTags
import app.swarm.tv.core.catalog.MergedEntry

@Composable
fun MovieShelfScreen(
    movies: List<MergedEntry>,
    artworkUrl: (MergedEntry) -> String?,
    onOpenMovie: (MergedEntry) -> Unit,
    onBack: () -> Unit,
    initialFocusKey: String? = null,
) {
    BackHandler(onBack = onBack)

    // Alphabetical, not the rating order the shelf row that opened this
    // screen sorts by — see [browseAllSortKey].
    val sortedMovies = remember(movies) {
        movies.sortedBy { browseAllSortKey(it.entry.scrapedTitle ?: it.entry.title) }
    }
    val firstCardFocusRequester = remember { FocusRequester() }
    val gridState = rememberLazyGridState()
    val focusIndex = remember(sortedMovies, initialFocusKey) {
        initialFocusKey?.let { key -> sortedMovies.indexOfFirst { it.entry.entryKey == key }.takeIf { it >= 0 } } ?: 0
    }
    LaunchedEffect(sortedMovies, focusIndex) {
        if (sortedMovies.isNotEmpty()) {
            gridState.scrollToItem(focusIndex)
            withFrameNanos {}
            firstCardFocusRequester.requestFocus()
        }
    }

    Column(modifier = Modifier.fillMaxSize().padding(horizontal = 40.dp)) {
        if (sortedMovies.isEmpty()) {
            Text("No movies in the catalog yet.", color = SwarmMuted, fontSize = 14.sp, modifier = Modifier.padding(top = 40.dp))
        } else {
            // top = 32.dp (not the flat 12.dp every other edge gets): without
            // it, the removed header's own real estate stops being the thing
            // that incidentally kept a focused top-row card's tv-material3
            // scale-up animation clear of the screen's top edge — real bug,
            // found live, same root cause and same fix already proven for
            // MovieRow's LazyRow in CatalogScreen.kt ("contentPadding, not
            // just the Column's own outer padding").
            LazyVerticalGrid(
                state = gridState,
                columns = GridCells.Fixed(5),
                verticalArrangement = Arrangement.spacedBy(20.dp),
                horizontalArrangement = Arrangement.spacedBy(20.dp),
                contentPadding = PaddingValues(start = 12.dp, end = 12.dp, top = 32.dp, bottom = 12.dp),
            ) {
                itemsIndexed(
                    items = sortedMovies,
                    key = { _, movie -> movie.entry.entryKey },
                    contentType = { _, _ -> "movie" },
                ) { index, movie ->
                    val focusModifier = if (index == focusIndex) Modifier.focusRequester(firstCardFocusRequester) else Modifier
                    Card(
                        onClick = { onOpenMovie(movie) },
                        colors = CardDefaults.colors(containerColor = SwarmSurface),
                        modifier = focusModifier.fillMaxWidth()
                            .testTag(UatTestTags.GRID_MOVIE_PREFIX + movie.entry.entryKey),
                    ) {
                        ArtworkImage(
                            label = movie.entry.scrapedTitle ?: movie.entry.title,
                            placeholderType = "Movie",
                            primaryUrl = artworkUrl(movie),
                            modifier = Modifier.fillMaxWidth().aspectRatio(2f / 3f).clip(RoundedCornerShape(4.dp)),
                        )
                    }
                }
            }
        }
    }
}
