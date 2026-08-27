package app.swarm.tv.app.uat

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onNodeWithTag
import app.swarm.tv.app.ui.UatTestTags
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Scenarios 1, 2, 3, 4, 5, 6, 16 — see the swarm-tv-uat-suite skill's
 * scenario catalog. Covers the browse/catalog surface: auto-open, that
 * every media kind loads with box art, the Continue Watching cap, and the
 * filter rail's controls.
 */
class BrowseCatalogUatTest : UatTestBase() {

    /** Scenario 1: browse opens automatically after pairing, no extra click needed. */
    @Test
    fun testBrowseAutoOpensWithoutClickingLibrary() {
        navigateDownUntilTag(UatTestTags.SHELF_MOVIES, timeoutMs = 20_000)
        composeTestRule.onNodeWithTag(UatTestTags.FILTER_RAIL).assertIsDisplayed()
    }

    /** Scenario 2: movies, shows, and music all load real entries. */
    @Test
    fun testMoviesShowsMusicLoad() {
        navigateDownUntilTag(UatTestTags.SHELF_MOVIES)
        assertTrue(
            "expected at least one movie card",
            composeTestRule.allTagsStartingWith(UatTestTags.CARD_MOVIE_PREFIX).isNotEmpty(),
        )
        navigateDownUntilTag(UatTestTags.SHELF_SHOWS)
        assertTrue(
            "expected at least one show card",
            composeTestRule.allTagsStartingWith(UatTestTags.CARD_SHOW_PREFIX).isNotEmpty(),
        )
        navigateDownUntilTag(UatTestTags.SHELF_MUSIC)
        assertTrue(
            "expected at least one music artist card",
            composeTestRule.allTagsStartingWith(UatTestTags.CARD_ARTIST_PREFIX).isNotEmpty(),
        )
    }

    /** Scenarios 3, 4, 5: box art renders for at least one movie, show, and artist. */
    @Test
    fun testBoxArtLoadsForMovieShowAndArtist() {
        navigateDownUntilTag(UatTestTags.SHELF_MOVIES)
        val movieTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.CARD_MOVIE_PREFIX))
        focusTag(movieTag)
        composeTestRule.onAllNodesWithTag(movieTag)[0].assertIsDisplayed()

        navigateDownUntilTagPrefix(UatTestTags.CARD_SHOW_PREFIX)
        val showTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.CARD_SHOW_PREFIX))
        focusTag(showTag)
        composeTestRule.onAllNodesWithTag(showTag)[0].assertIsDisplayed()

        navigateDownUntilTagPrefix(UatTestTags.CARD_ARTIST_PREFIX)
        val artistTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.CARD_ARTIST_PREFIX))
        focusTag(artistTag)
        composeTestRule.onAllNodesWithTag(artistTag)[0].assertIsDisplayed()
        // Note: this asserts the tile composable (which hosts the artwork
        // image) is laid out and on-screen — Compose semantics has no
        // "decoded bitmap arrived" signal for a Coil AsyncImage, so this is
        // the strongest assertion available without adding a load-state
        // semantics property to the app itself.
    }

    /** Scenario 6: Continue Watching never shows more than 6 items, when present at all. */
    @Test
    fun testContinueWatchingCapAtSix() {
        navigateDownUntilTag(UatTestTags.SHELF_MOVIES)
        val rowNodes = composeTestRule.allTagsStartingWith(UatTestTags.ROW_CONTINUE_WATCHING)
        if (rowNodes.isEmpty()) return // nothing to assert — no continue-watching assets yet
        val count = composeTestRule.countDescendantNodesWithTagPrefix(
            UatTestTags.ROW_CONTINUE_WATCHING,
            UatTestTags.CARD_QUICK_ACCESS_PREFIX,
        )
        assertTrue("Continue Watching should show at most 6 items, saw $count", count <= 6)
    }

    /** Scenario 16: the filter rail exposes All/Movies/Shows/Music, Liked-only, and at least one genre. */
    @Test
    fun testFilterBarMediaTypesAndGenre() {
        waitForTag(UatTestTags.FILTER_RAIL)
        openFilterRail()
        for (kind in listOf("ALL", "MOVIES", "SHOWS", "MUSIC")) {
            composeTestRule.onNodeWithTag(UatTestTags.FILTER_KIND_PREFIX + kind).assertIsDisplayed()
        }
        composeTestRule.onNodeWithTag(UatTestTags.FILTER_LIKED_ONLY).assertIsDisplayed()
        assertTrue(
            "expected at least one genre filter",
            composeTestRule.allTagsStartingWith(UatTestTags.FILTER_GENRE_PREFIX).isNotEmpty(),
        )
    }
}
