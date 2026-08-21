package app.swarm.tv.app.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.swarm.tv.R
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmSurfaceMuted
import app.swarm.tv.core.catalog.AlbumGroup
import app.swarm.tv.core.catalog.ArtistGroup
import app.swarm.tv.core.catalog.MergedEntry
import coil.compose.AsyncImage

internal data class ArtistArtworkUrls(
    val artistPhoto: String?,
    val albumCoverFallback: String?,
)

/** Resolve an album cover from any track carrying artwork, rather than assuming track one has it. */
internal fun AlbumGroup.coverArtworkUrl(artworkUrl: (MergedEntry) -> String?): String? =
    tracks.firstNotNullOfOrNull(artworkUrl)

/**
 * Prefer an artist photo. If that best-effort route is absent or returns an
 * error, use one stable, pseudo-random album cover so cards do not change on
 * every recomposition while still giving different artists varied artwork.
 */
internal fun ArtistGroup.artworkUrls(
    artworkUrl: (MergedEntry) -> String?,
    artistPhotoUrl: (MergedEntry) -> String?,
): ArtistArtworkUrls {
    val tracks = albums.flatMap(AlbumGroup::tracks)
    val artistPhoto = tracks.firstNotNullOfOrNull(artistPhotoUrl)
    val covers = albums.mapNotNull { it.coverArtworkUrl(artworkUrl) }.distinct()
    val fallbackIndex = if (covers.isEmpty()) -1 else Math.floorMod(artist.lowercase().hashCode(), covers.size)
    return ArtistArtworkUrls(artistPhoto, covers.getOrNull(fallbackIndex))
}

/** Artwork with a network-error fallback and a branded placeholder that is always present underneath. */
@Composable
internal fun ArtworkImage(
    label: String,
    placeholderType: String,
    primaryUrl: String?,
    fallbackUrl: String? = null,
    modifier: Modifier = Modifier,
) {
    var useFallback by remember(primaryUrl, fallbackUrl) { mutableStateOf(primaryUrl == null) }
    val resolvedFallback = fallbackUrl?.takeUnless { it == primaryUrl }
    val model = if (useFallback) resolvedFallback else primaryUrl

    Box(
        modifier = modifier.background(
            Brush.linearGradient(listOf(SwarmSurfaceMuted, SwarmAccent.copy(alpha = 0.2f))),
        ),
        contentAlignment = Alignment.Center,
    ) {
        Image(
            painter = painterResource(R.drawable.mascot),
            contentDescription = null,
            modifier = Modifier.fillMaxWidth(0.48f).aspectRatio(1f),
        )
        Text(
            placeholderType.uppercase(),
            color = SwarmAccent,
            fontSize = 11.sp,
            fontWeight = FontWeight.Black,
            modifier = Modifier.align(Alignment.BottomCenter).padding(10.dp),
        )
        if (model != null) {
            AsyncImage(
                model = model,
                contentDescription = "$label artwork",
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxSize(),
                onError = {
                    if (!useFallback && resolvedFallback != null) useFallback = true
                },
            )
        }
    }
}
