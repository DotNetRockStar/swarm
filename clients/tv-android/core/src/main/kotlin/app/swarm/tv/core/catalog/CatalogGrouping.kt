/**
 * Groups a merged catalog into Netflix-style Artist->Album->Track and
 * Show->Season->Episode hierarchies, entirely client-side over data already
 * fetched by [CatalogMerger] — no server round trip, same "STUN/the server
 * never needs to know about this, it happens locally" philosophy as
 * [CatalogMerger]'s own doc comment. Movies need no grouping (each is its
 * own leaf), so there's no `MovieGroup` here.
 *
 * Entries missing a grouping field (artist/album/show/season) are bucketed
 * under an "Unknown ..." group rather than dropped, so a scrape gap or a
 * loose file never silently disappears from the library.
 */
package app.swarm.tv.core.catalog

import app.swarm.tv.core.peer.MediaKind

const val UNKNOWN_ARTIST = "Unknown Artist"
const val UNKNOWN_ALBUM = "Unknown Album"
const val UNKNOWN_SHOW = "Unknown Show"

data class AlbumGroup(val album: String, val tracks: List<MergedEntry>)
data class ArtistGroup(val artist: String, val albums: List<AlbumGroup>)

data class SeasonGroup(val season: Int?, val episodes: List<MergedEntry>)
data class ShowGroup(val show: String, val seasons: List<SeasonGroup>)

object CatalogGrouping {
    fun groupTracksByArtistAlbum(entries: List<MergedEntry>): List<ArtistGroup> {
        val byArtist = entries.filter { it.entry.kind == MediaKind.TRACK }.groupBy { artistNameOf(it) }
        return byArtist.entries
            .sortedWith(compareBy({ it.key == UNKNOWN_ARTIST }, { it.key.lowercase() }))
            .map { (artist, artistEntries) ->
                val byAlbum = artistEntries.groupBy { albumNameOf(it) }
                val albums = byAlbum.entries
                    .sortedWith(compareBy({ it.key == UNKNOWN_ALBUM }, { it.key.lowercase() }))
                    .map { (album, albumEntries) -> AlbumGroup(album, albumEntries.sortedWith(trackOrder)) }
                ArtistGroup(artist, albums)
            }
    }

    fun groupEpisodesByShowSeason(entries: List<MergedEntry>): List<ShowGroup> {
        val byShow = entries.filter { it.entry.kind == MediaKind.EPISODE }.groupBy { showNameOf(it) }
        return byShow.entries
            .sortedWith(compareBy({ it.key == UNKNOWN_SHOW }, { it.key.lowercase() }))
            .map { (show, showEntries) ->
                val bySeason = showEntries.groupBy { it.entry.season }
                val seasons = bySeason.entries
                    .sortedWith(compareBy({ it.key == null }, { it.key ?: Int.MAX_VALUE }))
                    .map { (season, seasonEntries) -> SeasonGroup(season, seasonEntries.sortedWith(episodeOrder)) }
                ShowGroup(show, seasons)
            }
    }

    /**
     * The episode after [current] in the same show: next episode in the
     * same season if one exists, else episode 1 of the next season, else
     * null (end of the show, or [current] isn't a known episode of a known
     * show in [shows] — e.g. it was removed from the catalog mid-playback).
     */
    fun nextEpisode(current: MergedEntry, shows: List<ShowGroup>): MergedEntry? {
        val show = shows.find { it.show == showNameOf(current) } ?: return null
        val seasonIndex = show.seasons.indexOfFirst { it.season == current.entry.season }
        if (seasonIndex == -1) return null
        val episodeIndex = show.seasons[seasonIndex].episodes.indexOfFirst { it.fingerprint == current.fingerprint }
        if (episodeIndex == -1) return null
        show.seasons[seasonIndex].episodes.getOrNull(episodeIndex + 1)?.let { return it }
        return show.seasons.getOrNull(seasonIndex + 1)?.episodes?.firstOrNull()
    }

    private val trackOrder = compareBy<MergedEntry>({ it.entry.trackNumber == null }, { it.entry.trackNumber ?: Int.MAX_VALUE }, { it.entry.title.lowercase() })
    private val episodeOrder = compareBy<MergedEntry>({ it.entry.episode == null }, { it.entry.episode ?: Int.MAX_VALUE }, { it.entry.title.lowercase() })

    private fun artistNameOf(e: MergedEntry) = e.entry.artist?.takeIf { it.isNotBlank() } ?: UNKNOWN_ARTIST
    private fun albumNameOf(e: MergedEntry) = e.entry.album?.takeIf { it.isNotBlank() } ?: UNKNOWN_ALBUM
    private fun showNameOf(e: MergedEntry) = e.entry.showTitle?.takeIf { it.isNotBlank() } ?: UNKNOWN_SHOW
}
