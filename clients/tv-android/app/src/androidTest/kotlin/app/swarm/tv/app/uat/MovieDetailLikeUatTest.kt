package app.swarm.tv.app.uat

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import app.swarm.tv.app.ui.UatTestTags
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Scenarios 7, 8 — a random movie's detail screen renders every required
 * field, and the Like round-trips through the "Liked only" filter.
 *
 * Navigation between screens uses Compose's `performClick()` on the tagged
 * node rather than raw D-pad focus traversal: it invokes the same click
 * handler a real Select press would, and is the standard, robust way to
 * drive Compose UI tests — raw D-pad sequences are reserved for scenarios
 * that are explicitly testing D-pad behavior itself (playback seek, filter
 * expansion), per the other scenario classes.
 */
class MovieDetailLikeUatTest : UatTestBase() {

    private fun openFirstMovie(): String {
        waitForTag(UatTestTags.SHELF_MOVIES)
        val tag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.CARD_MOVIE_PREFIX)) {
            "no movie card found to open"
        }
        composeTestRule.onNodeWithTag(tag).performClick()
        waitForTag(UatTestTags.MOVIE_DETAIL_ARTWORK)
        return tag
    }

    /** Scenario 7: detail screen has box art, year, genres, cast, description, and all four action buttons. */
    @Test
    fun testMovieDetailFields() {
        openFirstMovie()
        composeTestRule.onNodeWithTag(UatTestTags.MOVIE_DETAIL_ARTWORK).assertIsDisplayed()
        composeTestRule.onNodeWithTag(UatTestTags.MOVIE_DETAIL_YEAR).assertIsDisplayed()
        composeTestRule.onNodeWithTag(UatTestTags.MOVIE_DETAIL_GENRES).assertIsDisplayed()
        composeTestRule.onNodeWithTag(UatTestTags.MOVIE_DETAIL_CAST).assertIsDisplayed()
        composeTestRule.onNodeWithTag(UatTestTags.MOVIE_DETAIL_DESCRIPTION).assertIsDisplayed()
        composeTestRule.onNodeWithTag(UatTestTags.MOVIE_DETAIL_PLAY_BUTTON).assertIsDisplayed()
        composeTestRule.onNodeWithTag(UatTestTags.MOVIE_DETAIL_LIKE_BUTTON).assertIsDisplayed()
        composeTestRule.onNodeWithTag(UatTestTags.MOVIE_DETAIL_WATCHLIST_BUTTON).assertIsDisplayed()
        composeTestRule.onNodeWithTag(UatTestTags.MOVIE_DETAIL_REPORT_PROBLEM_BUTTON).assertIsDisplayed()
    }

    /** Scenario 8: Like round-trips through real on-device state and the "Liked only" filter. */
    @Test
    fun testLikeRoundTripThroughFilter() {
        val movieTag = openFirstMovie()

        val before = runBlocking { likedEntriesStore.loadAll() }
        composeTestRule.onNodeWithTag(UatTestTags.MOVIE_DETAIL_LIKE_BUTTON).performClick()
        waitUntilStoreChanged(before.size + 1) { runBlocking { likedEntriesStore.loadAll() }.size }
        val afterLike = runBlocking { likedEntriesStore.loadAll() }
        assertTrue("liked-entries store should have grown by exactly one", afterLike.size == before.size + 1)

        pressBack()
        waitForTag(UatTestTags.SHELF_MOVIES)
        composeTestRule.onNodeWithTag(UatTestTags.FILTER_LIKED_ONLY).performClick()

        waitForTag(movieTag)
        composeTestRule.onNodeWithTag(movieTag).assertIsDisplayed()

        composeTestRule.onNodeWithTag(movieTag).performClick()
        waitForTag(UatTestTags.MOVIE_DETAIL_ARTWORK)
        composeTestRule.onNodeWithTag(UatTestTags.MOVIE_DETAIL_LIKE_BUTTON).performClick()
        waitUntilStoreChanged(before.size) { runBlocking { likedEntriesStore.loadAll() }.size }
        val afterUnlike = runBlocking { likedEntriesStore.loadAll() }
        assertTrue("liked-entries store should be back to its original size", afterUnlike.size == before.size)

        pressBack()
        waitForTag(UatTestTags.FILTER_RAIL)
        val stillFiltered = composeTestRule.allTagsStartingWith(movieTag)
        assertFalse("unliked movie should no longer appear under the Liked-only filter", stillFiltered.isNotEmpty())
    }

    private fun waitUntilStoreChanged(expectedSize: Int, timeoutMs: Long = 5_000, read: () -> Int) {
        composeTestRule.waitUntil(timeoutMillis = timeoutMs) { read() == expectedSize }
    }
}
