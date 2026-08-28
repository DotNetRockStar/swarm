package app.swarm.tv.app.uat

import app.swarm.tv.app.ui.UatTestTags
import app.swarm.tv.core.watch.WatchState
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.After
import org.junit.Before
import org.junit.Test

/** Real player completion behavior, accelerated only by the debug testing-mode seek hook. */
class EndOfMediaUatTest : UatTestBase() {

    private var watchStatesBefore: Map<String, WatchState> = emptyMap()

    @Before
    fun snapshotWatchState() {
        watchStatesBefore = runBlocking { watchStateStore.all() }
    }

    @After
    fun restoreWatchState() {
        restoreWatchStates(watchStatesBefore)
    }

    @Test
    fun testEpisodeContinuePlayNowAdvances() {
        openFirstEpisodePlayer()
        focusTag(UatTestTags.PLAYER_SURFACE)
        pressSelect()
        waitForTag(UatTestTags.PAUSE_LABEL)
        val originalTitle = composeTestRule.textUnderTag(UatTestTags.PAUSE_TITLE)
        selectTagWithDpad(UatTestTags.PAUSE_RESUME_BUTTON)

        withActivity { it.seekPlaybackNearEndForUat() }
        waitForTag(UatTestTags.CONTINUE_OVERLAY, timeoutMs = 20_000)
        selectTagWithDpad(UatTestTags.CONTINUE_PLAY_NOW_BUTTON)
        waitForTagGone(UatTestTags.CONTINUE_OVERLAY, timeoutMs = 15_000)
        waitForTag(UatTestTags.PLAYER_SURFACE, timeoutMs = 20_000)
        focusTag(UatTestTags.PLAYER_SURFACE)
        // The advanced-to episode is a real, freshly-negotiated file, not a
        // resume — give it more settle time than a simple pause/resume needs.
        Thread.sleep(3_000)
        pressSelect()
        waitForTag(UatTestTags.PAUSE_LABEL, timeoutMs = 10_000)
        val nextTitle = composeTestRule.textUnderTag(UatTestTags.PAUSE_TITLE)
        assertNotEquals("Play now should advance to the next episode", originalTitle, nextTitle)
    }

    @Test
    fun testEpisodeContinueCancelDoesNotAdvance() {
        val originalEpisodeTag = openFirstEpisodePlayer()
        focusTag(UatTestTags.PLAYER_SURFACE)
        pressSelect()
        waitForTag(UatTestTags.PAUSE_LABEL)
        selectTagWithDpad(UatTestTags.PAUSE_RESUME_BUTTON)

        withActivity { it.seekPlaybackNearEndForUat() }
        waitForTag(UatTestTags.CONTINUE_OVERLAY, timeoutMs = 20_000)
        selectTagWithDpad(UatTestTags.CONTINUE_CANCEL_BUTTON)
        waitForTagGone(UatTestTags.CONTINUE_OVERLAY)
        // Cancel at STATE_ENDED exits to the originating episode grid. It
        // must restore the episode just played rather than advancing focus.
        composeTestRule.waitForTagPrefix(UatTestTags.EPISODE_ITEM_PREFIX, timeoutMs = 15_000)
        composeTestRule.waitUntil(timeoutMillis = 5_000) {
            composeTestRule.focusedTagStartingWith(UatTestTags.EPISODE_ITEM_PREFIX) != null
        }
        assertEquals(originalEpisodeTag, composeTestRule.focusedTagStartingWith(UatTestTags.EPISODE_ITEM_PREFIX))
        assertTrue(composeTestRule.allTagsStartingWith(UatTestTags.PLAYER_SURFACE).isEmpty())
    }

    @Test
    fun testTrackCompletionAutomaticallyAdvances() {
        openFilterRail()
        selectTagWithDpad(UatTestTags.FILTER_KIND_PREFIX + "MUSIC")
        waitForTag(UatTestTags.SHELF_MUSIC)
        val artistTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.CARD_ARTIST_PREFIX))
        selectTagWithDpad(artistTag)
        composeTestRule.waitForTagPrefix(UatTestTags.ALBUM_CARD_PREFIX)
        val albumTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.ALBUM_CARD_PREFIX))
        selectTagWithDpad(albumTag)
        composeTestRule.waitForTagPrefix(UatTestTags.TRACK_ROW_PREFIX)
        val trackTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.TRACK_ROW_PREFIX))
        selectTagWithDpad(trackTag)
        waitForTag(UatTestTags.MUSIC_PLAYER_TITLE)
        val originalTitle = composeTestRule.textUnderTag(UatTestTags.MUSIC_PLAYER_TITLE)
        assertTrue("automatic-advance test needs a next track", composeTestRule.textUnderTag(UatTestTags.MUSIC_PLAYER_UP_NEXT).isNotBlank())

        withActivity { it.seekPlaybackNearEndForUat() }
        composeTestRule.waitUntil(timeoutMillis = 20_000) {
            runCatching { composeTestRule.textUnderTag(UatTestTags.MUSIC_PLAYER_TITLE) }
                .getOrNull()
                ?.let { it != originalTitle } == true
        }
    }

    private fun openFirstEpisodePlayer(): String {
        openFilterRail()
        selectTagWithDpad(UatTestTags.FILTER_KIND_PREFIX + "SHOWS")
        waitForTag(UatTestTags.SHELF_SHOWS)
        val showTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.CARD_SHOW_PREFIX))
        selectTagWithDpad(showTag)
        waitForTag(UatTestTags.SEASON_SCREEN_SHOW_TITLE)
        val seasonTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.SEASON_CARD_PREFIX))
        selectTagWithDpad(seasonTag)
        composeTestRule.waitForTagPrefix(UatTestTags.EPISODE_ITEM_PREFIX)
        composeTestRule.waitUntil(timeoutMillis = 5_000) {
            composeTestRule.focusedTagStartingWith(UatTestTags.EPISODE_ITEM_PREFIX) != null
        }
        val episodeTag = requireNotNull(composeTestRule.focusedTagStartingWith(UatTestTags.EPISODE_ITEM_PREFIX))
        pressSelect()
        waitForTag(UatTestTags.PLAYER_SURFACE, timeoutMs = 20_000)
        return episodeTag
    }
}
