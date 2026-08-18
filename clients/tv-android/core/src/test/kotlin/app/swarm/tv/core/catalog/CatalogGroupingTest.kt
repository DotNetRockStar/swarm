package app.swarm.tv.core.catalog

import app.swarm.tv.core.peer.CatalogEntry
import app.swarm.tv.core.peer.MediaKind
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class CatalogGroupingTest {

    private fun track(fp: String, artist: String?, album: String?, trackNumber: Int?, title: String = fp) = MergedEntry(
        fingerprint = fp,
        sources = listOf("server-a"),
        entry = CatalogEntry(entryKey = fp, fingerprint = fp, kind = MediaKind.TRACK, title = title, size = 1000, artist = artist, album = album, trackNumber = trackNumber),
    )

    private fun episode(fp: String, show: String?, season: Int?, episode: Int?, title: String = fp) = MergedEntry(
        fingerprint = fp,
        sources = listOf("server-a"),
        entry = CatalogEntry(entryKey = fp, fingerprint = fp, kind = MediaKind.EPISODE, title = title, size = 1000, showTitle = show, season = season, episode = episode),
    )

    @Test
    fun `tracks group by artist then album, sorted by track number`() {
        val entries = listOf(
            track("t3", "Air", "Moon Safari", 3),
            track("t1", "Air", "Moon Safari", 1),
            track("t2", "Air", "Moon Safari", 2),
            track("b1", "Air", "Talkie Walkie", 1),
        )
        val artists = CatalogGrouping.groupTracksByArtistAlbum(entries)
        assertEquals(1, artists.size)
        assertEquals("Air", artists[0].artist)
        assertEquals(listOf("Moon Safari", "Talkie Walkie"), artists[0].albums.map { it.album })
        assertEquals(listOf("t1", "t2", "t3"), artists[0].albums[0].tracks.map { it.fingerprint })
    }

    @Test
    fun `artist with multiple albums groups both under one artist`() {
        val entries = listOf(track("a1", "Boards of Canada", "Music Has the Right to Children", 1), track("a2", "Boards of Canada", "Geogaddi", 1))
        val artists = CatalogGrouping.groupTracksByArtistAlbum(entries)
        assertEquals(1, artists.size)
        assertEquals(2, artists[0].albums.size)
    }

    @Test
    fun `tracks missing artist or album bucket under Unknown without being dropped`() {
        val entries = listOf(track("x1", null, null, null), track("x2", "Air", null, 1))
        val artists = CatalogGrouping.groupTracksByArtistAlbum(entries)
        val total = artists.sumOf { it.albums.sumOf { a -> a.tracks.size } }
        assertEquals(2, total)
        assertTrue(artists.any { it.artist == UNKNOWN_ARTIST })
        assertTrue(artists.first { it.artist == "Air" }.albums.any { it.album == UNKNOWN_ALBUM })
    }

    @Test
    fun `episodes group by show then season, sorted by episode number, multi-season ordering correct`() {
        val entries = listOf(
            episode("s2e2", "Dexter", 2, 2),
            episode("s1e2", "Dexter", 1, 2),
            episode("s1e1", "Dexter", 1, 1),
            episode("s2e1", "Dexter", 2, 1),
        )
        val shows = CatalogGrouping.groupEpisodesByShowSeason(entries)
        assertEquals(1, shows.size)
        assertEquals(listOf(1, 2), shows[0].seasons.map { it.season })
        assertEquals(listOf("s1e1", "s1e2"), shows[0].seasons[0].episodes.map { it.fingerprint })
        assertEquals(listOf("s2e1", "s2e2"), shows[0].seasons[1].episodes.map { it.fingerprint })
    }

    @Test
    fun `episode missing show or season does not crash and lands in fallback bucket`() {
        val entries = listOf(episode("u1", null, null, null), episode("u2", "Dexter", null, 3))
        val shows = CatalogGrouping.groupEpisodesByShowSeason(entries)
        val total = shows.sumOf { it.seasons.sumOf { s -> s.episodes.size } }
        assertEquals(2, total)
        assertTrue(shows.any { it.show == UNKNOWN_SHOW })
        assertTrue(shows.first { it.show == "Dexter" }.seasons.any { it.season == null })
    }

    @Test
    fun `nextEpisode returns the following episode in the same season`() {
        val entries = listOf(episode("s1e1", "Dexter", 1, 1), episode("s1e2", "Dexter", 1, 2), episode("s1e3", "Dexter", 1, 3))
        val shows = CatalogGrouping.groupEpisodesByShowSeason(entries)
        val next = CatalogGrouping.nextEpisode(entries[0], shows)
        assertEquals("s1e2", next?.fingerprint)
    }

    @Test
    fun `nextEpisode crosses a season boundary to episode 1 of the next season`() {
        val entries = listOf(episode("s1e1", "Dexter", 1, 1), episode("s1e2", "Dexter", 1, 2), episode("s2e1", "Dexter", 2, 1))
        val shows = CatalogGrouping.groupEpisodesByShowSeason(entries)
        val last = entries.first { it.fingerprint == "s1e2" }
        val next = CatalogGrouping.nextEpisode(last, shows)
        assertEquals("s2e1", next?.fingerprint)
    }

    @Test
    fun `nextEpisode returns null at the true end of a show`() {
        val entries = listOf(episode("s1e1", "Dexter", 1, 1), episode("s1e2", "Dexter", 1, 2))
        val shows = CatalogGrouping.groupEpisodesByShowSeason(entries)
        val last = entries.first { it.fingerprint == "s1e2" }
        assertNull(CatalogGrouping.nextEpisode(last, shows))
    }

    @Test
    fun `nextEpisode returns null when the entry is not found in the show groups`() {
        val entries = listOf(episode("s1e1", "Dexter", 1, 1))
        val shows = CatalogGrouping.groupEpisodesByShowSeason(entries)
        val stranger = episode("gone", "Some Other Show", 1, 1)
        assertNull(CatalogGrouping.nextEpisode(stranger, shows))
    }

    @Test
    fun `empty input produces empty groupings without crashing`() {
        assertTrue(CatalogGrouping.groupTracksByArtistAlbum(emptyList()).isEmpty())
        assertTrue(CatalogGrouping.groupEpisodesByShowSeason(emptyList()).isEmpty())
        assertNull(CatalogGrouping.nextEpisode(episode("x", "Y", 1, 1), emptyList()))
    }
}
