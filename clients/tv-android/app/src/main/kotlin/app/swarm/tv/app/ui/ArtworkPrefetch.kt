package app.swarm.tv.app.ui

import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.platform.LocalContext
import coil.imageLoader
import coil.request.ImageRequest
import coil.size.Precision
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filterNotNull

private const val PREFETCH_AHEAD = 4

/** Warm only the next few shelf images; Coil's global limits keep this from becoming an unbounded burst. */
@Composable
fun PrefetchArtworkRow(state: LazyListState, urls: List<String?>) {
    val context = LocalContext.current
    val imageLoader = context.imageLoader
    LaunchedEffect(state, urls) {
        snapshotFlow { state.layoutInfo.visibleItemsInfo.lastOrNull()?.index }
            .filterNotNull()
            .distinctUntilChanged()
            .collect { lastVisible ->
                urls.drop(lastVisible + 1).take(PREFETCH_AHEAD).filterNotNull().forEach { url ->
                    imageLoader.enqueue(
                        ImageRequest.Builder(context)
                            .data(url)
                            .size(320, 480)
                            .precision(Precision.INEXACT)
                            .build(),
                    )
                }
            }
    }
}
