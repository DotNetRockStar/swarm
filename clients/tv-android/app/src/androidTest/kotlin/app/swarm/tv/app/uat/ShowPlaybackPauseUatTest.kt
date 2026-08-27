package app.swarm.tv.app.uat

import android.view.KeyEvent
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
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
        waitForTag(UatTestTags.SHELF_SHOWS)
        val showTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.CARD_SHOW_PREFIX))
        composeTestRule.onNodeWithTag(showTag).performClick()
        waitForTag(UatTestTags.SEASON_SCREEN_SHOW_TITLE)

        val seasonTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.SEASON_CARD_PREFIX))
        composeTestRule.onNodeWithTag(seasonTag).performClick()
        composeTestRule.waitForTagPrefix(UatTestTags.EPISODE_ITEM_PREFIX)

        val episodeTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.EPISODE_ITEM_PREFIX))
        composeTestRule.onNodeWithTag(episodeTag).performClick() // episodes play immediately on click

        Thread.sleep(30_000)

        pressSelect()
        waitForTag(UatTestTags.PAUSE_LABEL)
        composeTestRule.onNodeWithTag(UatTestTags.PAUSE_METADATA).assertIsDisplayed()
        assertTrue(
            "an episode's pause overlay should have a Next Episode button",
            composeTestRule.allTagsStartingWith(UatTestTags.PAUSE_NEXT_EPISODE_BUTTON).isNotEmpty(),
        )

        composeTestRule.onNodeWithTag(UatTestTags.PAUSE_RESUME_BUTTON).performClick()
        composeTestRule.waitUntil(timeoutMillis = 5_000) {
            composeTestRule.allTagsStartingWith(UatTestTags.PAUSE_LABEL).isEmpty()
        }

        device.pressKeyCode(KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE)
        waitForTag(UatTestTags.PAUSE_LABEL)
        composeTestRule.onNodeWithTag(UatTestTags.PAUSE_NEXT_EPISODE_BUTTON).performClick()
        // Next episode should start playing — pause overlay should
        // disappear again (a fresh playback session), then we can confirm
        // it's a real, playing session by pausing once more.
        composeTestRule.waitUntil(timeoutMillis = 10_000) {
            composeTestRule.allTagsStartingWith(UatTestTags.PAUSE_LABEL).isEmpty()
        }
        Thread.sleep(2_000)
        pressSelect()
        waitForTag(UatTestTags.PAUSE_LABEL)

        pressBack()
        pressBack()
        waitForTag(UatTestTags.SHELF_SHOWS)
    }
}
