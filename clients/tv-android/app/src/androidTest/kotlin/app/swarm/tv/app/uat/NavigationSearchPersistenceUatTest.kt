package app.swarm.tv.app.uat

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performImeAction
import androidx.compose.ui.test.performTextReplacement
import app.swarm.tv.app.ui.UatTestTags
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** Remote focus, search/sort/filter composition, back-stack focus, and durable local state. */
class NavigationSearchPersistenceUatTest : UatTestBase() {

    @Test
    fun testPureRemoteNavigationAndFocusRestoration() {
        // No semantics RequestFocus in this test: start from the app's own
        // restored focus and traverse the real geometric TV focus graph.
        openFilterRail()
        pressDpadRight()
        val movieTag = navigateFocusUntilPrefix(UatTestTags.CARD_MOVIE_PREFIX, press = ::pressDpadDown)
        pressSelect()
        waitForTag(UatTestTags.MOVIE_DETAIL_PLAY_BUTTON)
        composeTestRule.waitUntil(timeoutMillis = 5_000) {
            composeTestRule.focusedTag() == UatTestTags.MOVIE_DETAIL_PLAY_BUTTON
        }

        pressBack()
        waitForTag(movieTag)
        composeTestRule.waitUntil(timeoutMillis = 5_000) {
            composeTestRule.focusedTag() == movieTag
        }

        pressSelect()
        waitForTag(UatTestTags.MOVIE_DETAIL_PLAY_BUTTON)
        composeTestRule.waitUntil(timeoutMillis = 5_000) {
            composeTestRule.focusedTag() == UatTestTags.MOVIE_DETAIL_PLAY_BUTTON
        }
        pressSelect()
        waitForTag(UatTestTags.PLAYER_SURFACE, timeoutMs = 15_000)
        // Back from the bare playback surface intentionally pauses first;
        // Back from the resulting pause overlay exits to detail.
        pressBack()
        waitForTag(UatTestTags.PAUSE_LABEL)
        pressBack()
        waitForTag(UatTestTags.MOVIE_DETAIL_ARTWORK)
        pressBack()
        waitForTag(movieTag)
        composeTestRule.waitUntil(timeoutMillis = 5_000) { composeTestRule.focusedTag() == movieTag }
    }

    @Test
    fun testSearchNoResultsClearSortAndCombinedFilters() {
        navigateDownUntilTag(UatTestTags.SHELF_MOVIES)
        val movieTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.CARD_MOVIE_PREFIX))
        val title = requireNotNull(composeTestRule.contentDescriptionsUnderTag(movieTag).firstOrNull())
            .removeSuffix(" artwork")
        val query = title.split(' ').firstOrNull { it.length >= 3 } ?: title

        // The TV field is deliberately read-only until a real D-pad Center
        // key-up opts into editing, preventing accidental IME popups while
        // viewers merely traverse the screen.
        selectTagWithDpad(UatTestTags.SEARCH_FIELD)
        composeTestRule.onNodeWithTag(UatTestTags.SEARCH_FIELD).performTextReplacement(query)
        composeTestRule.onNodeWithTag(UatTestTags.SEARCH_FIELD).performImeAction()
        waitForTag(movieTag)
        composeTestRule.onNodeWithTag(movieTag).assertIsDisplayed()

        selectTagWithDpad(UatTestTags.SEARCH_FIELD)
        composeTestRule.onNodeWithTag(UatTestTags.SEARCH_FIELD).performTextReplacement("zzzuatnomatchzzz")
        composeTestRule.onNodeWithTag(UatTestTags.SEARCH_FIELD).performImeAction()
        waitForTag(UatTestTags.SEARCH_NO_MATCHES)
        selectTagWithDpad(UatTestTags.SEARCH_CLEAR_BUTTON)
        // Clearing restores the full lazy catalog but does not force-scroll
        // the Movies shelf into the currently composed viewport.
        navigateDownUntilTag(UatTestTags.SHELF_MOVIES)

        openFilterRail()
        selectTagWithDpad(UatTestTags.FILTER_KIND_PREFIX + "MOVIES")
        waitForTag(UatTestTags.SHELF_MOVIES)
        assertTrue(composeTestRule.allTagsStartingWith(UatTestTags.CARD_SHOW_PREFIX).isEmpty())
        assertTrue(composeTestRule.allTagsStartingWith(UatTestTags.CARD_ARTIST_PREFIX).isEmpty())
        openFilterRail()
        val genreTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.FILTER_GENRE_PREFIX))
        selectTagWithDpad(genreTag)
        composeTestRule.waitUntil(timeoutMillis = 5_000) {
            composeTestRule.allTagsStartingWith(UatTestTags.CARD_MOVIE_PREFIX).isNotEmpty()
        }
        openFilterRail()
        selectTagWithDpad(genreTag)
        openFilterRail()
        selectTagWithDpad(UatTestTags.FILTER_KIND_PREFIX + "ALL")
        waitForTag(UatTestTags.SHELF_MOVIES)

        val firstMovie = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.CARD_MOVIE_PREFIX))
        focusTag(firstMovie)
        navigateFocusUntilPrefix(UatTestTags.BROWSE_ALL_MOVIES, timeoutMs = 15_000, press = ::pressDpadRight)
        pressSelect()
        composeTestRule.waitForTagPrefix(UatTestTags.GRID_MOVIE_PREFIX)
        val visibleGridTags = composeTestRule.allTagsStartingWith(UatTestTags.GRID_MOVIE_PREFIX)
        val visibleTitles = visibleGridTags.mapNotNull { tag ->
            composeTestRule.contentDescriptionsUnderTag(tag).firstOrNull()?.removeSuffix(" artwork")
        }
        assertTrue("Browse All should expose at least two visible movies", visibleTitles.size >= 2)
        assertEquals("Browse All movies should be alphabetical", visibleTitles.sortedBy(::sortKey), visibleTitles)
        pressBack()
        waitForTag(UatTestTags.SHELF_MOVIES)
    }

    @Test
    fun testLikeAndWatchlistPersistAcrossFreshActivity() {
        navigateDownUntilTag(UatTestTags.SHELF_MOVIES)
        val movieTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.CARD_MOVIE_PREFIX))
        selectTagWithDpad(movieTag)
        waitForTag(UatTestTags.MOVIE_DETAIL_ARTWORK)

        val likedBefore = runBlocking { likedEntriesStore.loadAll() }
        val watchlistBefore = runBlocking { watchlistStore.loadAll() }
        val initiallyLiked = composeTestRule.textUnderTag(UatTestTags.MOVIE_DETAIL_LIKE_BUTTON).contains("Liked")
        val initiallyWatchlisted = composeTestRule.textUnderTag(UatTestTags.MOVIE_DETAIL_WATCHLIST_BUTTON).contains("Watchlisted")
        if (!initiallyLiked) selectTagWithDpad(UatTestTags.MOVIE_DETAIL_LIKE_BUTTON)
        if (!initiallyWatchlisted) selectTagWithDpad(UatTestTags.MOVIE_DETAIL_WATCHLIST_BUTTON)
        composeTestRule.waitUntil(timeoutMillis = 5_000) {
            composeTestRule.textUnderTag(UatTestTags.MOVIE_DETAIL_LIKE_BUTTON).contains("Liked") &&
                composeTestRule.textUnderTag(UatTestTags.MOVIE_DETAIL_WATCHLIST_BUTTON).contains("Watchlisted")
        }

        restartActivityAndWaitForCatalog()
        navigateDownUntilTag(UatTestTags.SHELF_MOVIES)
        selectTagWithDpad(movieTag)
        waitForTag(UatTestTags.MOVIE_DETAIL_ARTWORK)
        assertTrue(composeTestRule.textUnderTag(UatTestTags.MOVIE_DETAIL_LIKE_BUTTON).contains("Liked"))
        assertTrue(composeTestRule.textUnderTag(UatTestTags.MOVIE_DETAIL_WATCHLIST_BUTTON).contains("Watchlisted"))

        if (!initiallyLiked) selectTagWithDpad(UatTestTags.MOVIE_DETAIL_LIKE_BUTTON)
        if (!initiallyWatchlisted) selectTagWithDpad(UatTestTags.MOVIE_DETAIL_WATCHLIST_BUTTON)
        composeTestRule.waitUntil(timeoutMillis = 5_000) {
            runBlocking { likedEntriesStore.loadAll() } == likedBefore &&
                runBlocking { watchlistStore.loadAll() } == watchlistBefore
        }
    }

    private fun sortKey(title: String): String {
        val trimmed = title.trim()
        return if (trimmed.startsWith("The ", ignoreCase = true)) trimmed.substring(4).trimStart().lowercase()
        else trimmed.lowercase()
    }
}
