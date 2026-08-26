/**
 * Full grid of every show in the catalog — reached from [CatalogScreen]'s
 * Shows row header ("Browse all"). Selecting a show opens [SeasonScreen].
 *
 * No title/Back header — see [MovieShelfScreen]'s identical doc comment.
 */
package app.swarm.tv.app.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.PaddingValues
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
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmSurface
import app.swarm.tv.core.catalog.MergedEntry
import app.swarm.tv.core.catalog.ShowGroup

@Composable
fun ShowShelfScreen(
    shows: List<ShowGroup>,
    artworkUrl: (MergedEntry) -> String?,
    onOpenShow: (ShowGroup) -> Unit,
    onBack: () -> Unit,
    initialFocusKey: String? = null,
) {
    BackHandler(onBack = onBack)

    // Alphabetical, not the rating order the shelf row that opened this
    // screen sorts by — see [browseAllSortKey].
    val sortedShows = remember(shows) { shows.sortedBy { browseAllSortKey(it.show) } }
    val firstCardFocusRequester = remember { FocusRequester() }
    val gridState = rememberLazyGridState()
    val focusIndex = remember(sortedShows, initialFocusKey) {
        initialFocusKey?.let { key -> sortedShows.indexOfFirst { it.show == key }.takeIf { it >= 0 } } ?: 0
    }
    LaunchedEffect(sortedShows, focusIndex) {
        if (sortedShows.isNotEmpty()) {
            gridState.scrollToItem(focusIndex)
            withFrameNanos {}
            firstCardFocusRequester.requestFocus()
        }
    }

    Column(modifier = Modifier.fillMaxSize().padding(horizontal = 40.dp)) {
        if (sortedShows.isEmpty()) {
            Text("No shows in the catalog yet.", color = SwarmMuted, fontSize = 14.sp, modifier = Modifier.padding(top = 40.dp))
        } else {
            // top = 32.dp — see MovieShelfScreen's identical comment on why.
            LazyVerticalGrid(
                state = gridState,
                columns = GridCells.Fixed(5),
                verticalArrangement = Arrangement.spacedBy(20.dp),
                horizontalArrangement = Arrangement.spacedBy(20.dp),
                contentPadding = PaddingValues(start = 12.dp, end = 12.dp, top = 32.dp, bottom = 12.dp),
            ) {
                itemsIndexed(
                    items = sortedShows,
                    key = { _, show -> show.show },
                    contentType = { _, _ -> "show" },
                ) { index, show ->
                    val representative = show.seasons.firstOrNull()?.episodes?.firstOrNull()
                    val focusModifier = if (index == focusIndex) Modifier.focusRequester(firstCardFocusRequester) else Modifier
                    Card(onClick = { onOpenShow(show) }, colors = CardDefaults.colors(containerColor = SwarmSurface), modifier = focusModifier.fillMaxWidth()) {
                        ArtworkImage(
                            label = show.show,
                            placeholderType = "Show",
                            primaryUrl = representative?.let(artworkUrl),
                            modifier = Modifier.fillMaxWidth().aspectRatio(2f / 3f).clip(RoundedCornerShape(4.dp)),
                        )
                    }
                }
            }
        }
    }
}
