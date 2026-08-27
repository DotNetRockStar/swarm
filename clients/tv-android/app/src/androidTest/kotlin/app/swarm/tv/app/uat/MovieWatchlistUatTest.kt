package app.swarm.tv.app.uat

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onNodeWithTag
import app.swarm.tv.app.ui.UatTestTags
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Scenario 9: Watchlist add/remove round-trips through toasts, the Watchlist row, and real on-device state. */
class MovieWatchlistUatTest : UatTestBase() {

    @Test
    fun testWatchlistRoundTrip() {
        navigateDownUntilTag(UatTestTags.SHELF_MOVIES)
        val movieTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.CARD_MOVIE_PREFIX))
        selectTagWithDpad(movieTag)
        waitForTag(UatTestTags.MOVIE_DETAIL_ARTWORK)

        val before = runBlocking { watchlistStore.loadAll() }
        selectTagWithDpad(UatTestTags.MOVIE_DETAIL_WATCHLIST_BUTTON)
        waitForText("Added to Watchlist")
        composeTestRule.waitUntil(timeoutMillis = 5_000) {
            runBlocking { watchlistStore.loadAll() }.size == before.size + 1
        }

        pressBack()
        waitForTag(UatTestTags.SHELF_MOVIES)
        val watchlistRow = composeTestRule.allTagsStartingWith(UatTestTags.ROW_WATCHLIST)
        assertTrue("Watchlist row should be visible after adding an item", watchlistRow.isNotEmpty())

        selectTagWithDpad(movieTag)
        waitForTag(UatTestTags.MOVIE_DETAIL_ARTWORK)
        selectTagWithDpad(UatTestTags.MOVIE_DETAIL_WATCHLIST_BUTTON)
        waitForText("Removed from Watchlist")
        composeTestRule.waitUntil(timeoutMillis = 5_000) {
            runBlocking { watchlistStore.loadAll() }.size == before.size
        }

        pressBack()
        waitForTag(UatTestTags.SHELF_MOVIES)
        // The definitive check is the real on-device store (asserted via
        // waitUntil above, which already timed out the test if it never
        // returned to `before.size`). The Watchlist row itself only exists
        // at all while at least one item is on the watchlist, so if this
        // was the only watchlisted item, the row should now be gone too.
        assertTrue(
            "watchlist store should be back to its original size",
            runBlocking { watchlistStore.loadAll() }.size == before.size,
        )
        if (before.isEmpty()) {
            val rowNodes = composeTestRule.allTagsStartingWith(UatTestTags.ROW_WATCHLIST)
            assertFalse("Watchlist row should be gone once its only item was removed", rowNodes.isNotEmpty())
        }
    }
}
