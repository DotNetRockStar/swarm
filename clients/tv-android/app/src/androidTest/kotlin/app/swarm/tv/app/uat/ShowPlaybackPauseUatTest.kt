package app.swarm.tv.app.uat

import android.view.KeyEvent
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onNodeWithTag
import app.swarm.tv.app.ui.UatTestTags
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Scenario 13: episode playback and the pause overlay, including the Next
 * Episode button — the counterpart to [MoviePlaybackPauseUatTest], which
 * asserts that same button is correctly absent for a movie.
 */
class ShowPlaybackPauseUatTest : UatTestBase() {

    @Test
    fun testEpisodePlaybackPauseAndNextEpisode() {
        openFilterRail()
        selectTagWithDpad(UatTestTags.FILTER_KIND_PREFIX + "SHOWS")
        waitForTag(UatTestTags.SHELF_SHOWS)
        val showTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.CARD_SHOW_PREFIX))
        selectTagWithDpad(showTag)
        waitForTag(UatTestTags.SEASON_SCREEN_SHOW_TITLE)

        val seasonTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.SEASON_CARD_PREFIX))
        selectTagWithDpad(seasonTag)
        composeTestRule.waitForTagPrefix(UatTestTags.EPISODE_ITEM_PREFIX)

        requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.EPISODE_ITEM_PREFIX))
        // SeasonScreen restores a real episode card's focus when the grid
        // opens. Activate that user-visible focus directly instead of
        // racing the screen's restoration by requesting a different card.
        composeTestRule.waitUntil(timeoutMillis = 5_000) {
            composeTestRule.focusedTagStartingWith(UatTestTags.EPISODE_ITEM_PREFIX) != null
        }
        requireNotNull(
            composeTestRule.focusedTagStartingWith(UatTestTags.EPISODE_ITEM_PREFIX),
        )
        // The grid has already restored focus to this exact card. Reissuing
        // RequestFocus here can overlap the TV Card's key-input setup on the
        // first frame; activate the established real focus directly.
        pressSelect() // episodes play immediately on D-pad Center
        device.waitForIdle(250)
        waitForTag(UatTestTags.PLAYER_SURFACE)
        focusTag(UatTestTags.PLAYER_SURFACE)

        Thread.sleep(30_000)

        pressSelect()
        waitForTag(UatTestTags.PAUSE_LABEL)
        composeTestRule.onNodeWithTag(UatTestTags.PAUSE_METADATA).assertIsDisplayed()
        assertTrue(
            "an episode's pause overlay should have a Next Episode button",
            composeTestRule.allTagsStartingWith(UatTestTags.PAUSE_NEXT_EPISODE_BUTTON).isNotEmpty(),
        )

        selectTagWithDpad(UatTestTags.PAUSE_RESUME_BUTTON)
        composeTestRule.waitUntil(timeoutMillis = 5_000) {
            composeTestRule.allTagsStartingWith(UatTestTags.PAUSE_LABEL).isEmpty()
        }

        device.pressKeyCode(KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE)
        waitForTag(UatTestTags.PAUSE_LABEL)
        selectTagWithDpad(UatTestTags.PAUSE_NEXT_EPISODE_BUTTON)
        // Next episode should start playing — pause overlay should
        // disappear again (a fresh playback session), then we can confirm
        // it's a real, playing session by pausing once more.
        composeTestRule.waitUntil(timeoutMillis = 10_000) {
            composeTestRule.allTagsStartingWith(UatTestTags.PAUSE_LABEL).isEmpty()
        }
        waitForTag(UatTestTags.PLAYER_SURFACE)
        focusTag(UatTestTags.PLAYER_SURFACE)
        Thread.sleep(2_000)
        pressSelect()
        waitForTag(UatTestTags.PAUSE_LABEL)

        pressBack()
        pressBack()
        waitForTag(UatTestTags.SHELF_SHOWS)
    }
}
