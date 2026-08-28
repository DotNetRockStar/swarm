package app.swarm.tv.app.uat

import app.swarm.tv.app.ui.UatTestTags
import org.junit.Assert.assertNotEquals
import org.junit.Test

/** Opt-in transport-drop/recovery checks; never starts, stops, or mutates the media server. */
class ResilienceUatTest : UatTestBase() {

    @Test
    fun testCatalogTransportDropRecovers() {
        val before = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.TRANSPORT_RECOVERY_PREFIX))
        withActivity { it.dropAndRecoverTransportForUat() }
        composeTestRule.waitUntil(timeoutMillis = 50_000) {
            composeTestRule.firstTagStartingWith(UatTestTags.TRANSPORT_RECOVERY_PREFIX)?.let { it != before } == true
        }
        waitForTag(UatTestTags.FILTER_RAIL)
        // Recovery may return focus inside the expanded filter rail. Exit it
        // with the product's real D-pad Right gesture before traversing the
        // catalog, rather than walking every genre option with Down.
        openFilterRail()
        focusTag(UatTestTags.FILTER_KIND_PREFIX + "ALL")
        pressDpadRight()
        waitForTagGone(UatTestTags.FILTER_KIND_PREFIX + "ALL")
        navigateDownUntilTag(UatTestTags.SHELF_MOVIES)
    }

    @Test
    fun testPlaybackTransportDropCleansUpAndRecovers() {
        navigateDownUntilTag(UatTestTags.SHELF_MOVIES)
        val movieTag = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.CARD_MOVIE_PREFIX))
        selectTagWithDpad(movieTag)
        waitForTag(UatTestTags.MOVIE_DETAIL_PLAY_BUTTON)
        selectTagWithDpad(UatTestTags.MOVIE_DETAIL_PLAY_BUTTON)
        waitForTag(UatTestTags.PLAYER_SURFACE, timeoutMs = 15_000)
        val before = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.TRANSPORT_RECOVERY_PREFIX))

        withActivity { it.dropAndRecoverTransportForUat() }
        composeTestRule.waitUntil(timeoutMillis = 50_000) {
            composeTestRule.firstTagStartingWith(UatTestTags.TRANSPORT_RECOVERY_PREFIX)?.let { it != before } == true
        }
        navigateDownUntilTag(UatTestTags.SHELF_MOVIES)
        val after = requireNotNull(composeTestRule.firstTagStartingWith(UatTestTags.TRANSPORT_RECOVERY_PREFIX))
        assertNotEquals(before, after)

        selectTagWithDpad(movieTag)
        waitForTag(UatTestTags.MOVIE_DETAIL_PLAY_BUTTON)
        selectTagWithDpad(UatTestTags.MOVIE_DETAIL_PLAY_BUTTON)
        waitForTag(UatTestTags.PLAYER_SURFACE, timeoutMs = 15_000)
    }
}
