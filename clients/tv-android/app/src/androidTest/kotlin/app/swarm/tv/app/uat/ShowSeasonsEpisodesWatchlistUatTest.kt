package app.swarm.tv.app.uat

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import app.swarm.tv.app.ui.UatTestTags
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Scenario 14: show seasons/episodes, and the watchlist round-trip on a show. */
class ShowSeasonsEpisodesWatchlistUatTest : UatTestBase() {

    @Test
    fun testSeasonsEpisodesAndWatchlistRoundTrip() {
        waitForTag(UatTestTags.SHELF_SHOWS)
        val showTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.CARD_SHOW_PREFIX))
        composeTestRule.onNodeWithTag(showTag).performClick()
        waitForTag(UatTestTags.SEASON_SCREEN_SHOW_TITLE)

        val seasonTags = composeTestRule.allTagsStartingWith(UatTestTags.SEASON_CARD_PREFIX)
        assertTrue("expected at least one season", seasonTags.isNotEmpty())
        composeTestRule.onNodeWithTag(seasonTags.first()).assertIsDisplayed()
        // Season card text (number + episode count) is a single merged
        // block rather than separately tagged fields — assert it's present
        // and non-blank rather than parsing an exact format.
        assertFalse(
            "season card should show its number/episode count",
            composeTestRule.textUnderTag(seasonTags.first()).isBlank(),
        )

        val before = runBlocking { watchlistStore.loadAll() }
        composeTestRule.onNodeWithTag(UatTestTags.SEASON_SCREEN_WATCHLIST_BUTTON).performClick()
        composeTestRule.waitUntil(timeoutMillis = 5_000) {
            runBlocking { watchlistStore.loadAll() }.size == before.size + 1
        }

        pressBack()
        waitForTag(UatTestTags.SHELF_SHOWS)
        assertTrue(
            "Watchlist row should be visible after adding the show",
            composeTestRule.allTagsStartingWith(UatTestTags.ROW_WATCHLIST).isNotEmpty(),
        )

        composeTestRule.onNodeWithTag(showTag).performClick()
        waitForTag(UatTestTags.SEASON_SCREEN_SHOW_TITLE)
        composeTestRule.onNodeWithTag(UatTestTags.SEASON_SCREEN_WATCHLIST_BUTTON).performClick()
        composeTestRule.waitUntil(timeoutMillis = 5_000) {
            runBlocking { watchlistStore.loadAll() }.size == before.size
        }

        pressBack()
        waitForTag(UatTestTags.SHELF_SHOWS)
        assertTrue(
            "watchlist store should be back to its original size",
            runBlocking { watchlistStore.loadAll() }.size == before.size,
        )

        // Re-open the same show and check episode structure: at least one
        // episode, no duplicate entry keys, non-blank displayed text
        // (contains the episode name, per the scenario spec).
        composeTestRule.onNodeWithTag(showTag).performClick()
        waitForTag(UatTestTags.SEASON_SCREEN_SHOW_TITLE)
        composeTestRule.onNodeWithTag(seasonTags.first()).performClick()
        composeTestRule.waitForTagPrefix(UatTestTags.EPISODE_ITEM_PREFIX)

        val episodeTags = composeTestRule.allTagsStartingWith(UatTestTags.EPISODE_ITEM_PREFIX)
        assertTrue("expected at least one episode", episodeTags.isNotEmpty())
        assertEquals("episode entry keys should not repeat", episodeTags.size, episodeTags.distinct().size)
        for (tag in episodeTags) {
            assertFalse("episode item should display its number/name", composeTestRule.textUnderTag(tag).isBlank())
        }
    }
}
