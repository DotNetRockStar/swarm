package app.swarm.tv.app.data

import app.swarm.tv.core.catalog.MergedEntry
import app.swarm.tv.core.peer.CatalogEntry
import app.swarm.tv.core.peer.MediaKind
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class PlayerPauseInfoTest {
    @Test
    fun `recommendations contain movies and one representative per other show`() {
        val current = episode("current", "Current Show", season = 1, episode = 2, genres = listOf("Drama"))
        val sameShow = episode("same-show", "Current Show", season = 1, episode = 3, genres = listOf("Drama"))
        val similarShowFirst = episode("similar-show-1", "Similar Show", season = 1, episode = 1, genres = listOf("Drama"))
        val similarShowSecond = episode("similar-show-2", "Similar Show", season = 1, episode = 2, genres = listOf("Drama"))
        val similarMovie = movie("similar-movie", "Similar Movie", listOf("Drama"))
        val otherMovie = movie("other-movie", "Other Movie", listOf("Comedy"))

        val result = pauseRecommendations(
            current,
            listOf(current, sameShow, similarShowSecond, similarShowFirst, otherMovie, similarMovie),
        )

        assertEquals("Similar Show", pauseRecommendationTitle(result.first()))
        assertEquals(1, result.count { pauseRecommendationTitle(it) == "Similar Show" })
        assertTrue(result.any { it.fingerprint == "similar-movie" })
        assertTrue(result.any { it.fingerprint == "other-movie" })
        assertFalse(result.any { it.fingerprint == "same-show" })
    }

    @Test
    fun `recommendations are capped at ten`() {
        val current = movie("current", "Current", listOf("Drama"))
        val candidates = (1..14).map { movie("movie-$it", "Movie $it", listOf("Drama")) }

        assertEquals(10, pauseRecommendations(current, listOf(current) + candidates).size)
    }

    @Test
    fun `episode number label includes season episode and title`() {
        val entry = episode("episode", "Show", season = 3, episode = 7, episodeTitle = "The Return")

        assertEquals("Season 3  •  Episode 7  •  The Return", episodeNumberLabel(entry))
    }

    private fun movie(fingerprint: String, title: String, genres: List<String>) = merged(
        CatalogEntry(
            entryKey = fingerprint,
            fingerprint = fingerprint,
            kind = MediaKind.MOVIE,
            title = title,
            size = 1,
            scrapedTitle = title,
            genres = genres,
        ),
    )

    private fun episode(
        fingerprint: String,
        show: String,
        season: Int,
        episode: Int,
        genres: List<String> = emptyList(),
        episodeTitle: String = "Episode $episode",
    ) = merged(
        CatalogEntry(
            entryKey = fingerprint,
            fingerprint = fingerprint,
            kind = MediaKind.EPISODE,
            title = episodeTitle,
            size = 1,
            showTitle = show,
            scrapedTitle = show,
            season = season,
            episode = episode,
            episodeTitle = episodeTitle,
            genres = genres,
        ),
    )

    private fun merged(entry: CatalogEntry) = MergedEntry(entry.fingerprint, listOf("server"), entry)
}
