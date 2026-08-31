package app.swarm.tv.app.uat

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onNodeWithTag
import app.swarm.tv.app.ui.UatTestTags
import org.junit.Assert.assertEquals
import org.junit.Test

/** Settings -> STUN activation -> cancellation, driven through real TV UI. */
class AddServerFromSettingsUatTest : UatTestBase() {

    @Test
    fun testAddServerFromSettingsShowsStunCodeAndReturnsToSettings() {
        navigateUpUntilTag(UatTestTags.OPEN_SWARM_BUTTON)
        selectTagWithDpad(UatTestTags.OPEN_SWARM_BUTTON)
        waitForTag(UatTestTags.DASHBOARD_SETTINGS_BUTTON)
        selectTagWithDpad(UatTestTags.DASHBOARD_SETTINGS_BUTTON)

        waitForTag(UatTestTags.SETTINGS_ADD_SERVER_BUTTON)
        composeTestRule.onNodeWithTag(UatTestTags.SETTINGS_ADD_SERVER_BUTTON).assertIsDisplayed()
        selectTagWithDpad(UatTestTags.SETTINGS_ADD_SERVER_BUTTON)

        waitForTag(UatTestTags.ACTIVATION_CODE, timeoutMs = 45_000)
        val digits = composeTestRule.textUnderTag(UatTestTags.ACTIVATION_CODE).filter(Char::isDigit)
        assertEquals("STUN activation must show an eight-digit approval code", 8, digits.length)

        selectTagWithDpad(UatTestTags.ACTIVATION_CANCEL_BUTTON)
        waitForTag(UatTestTags.SETTINGS_ADD_SERVER_BUTTON)
        composeTestRule.onNodeWithTag(UatTestTags.SETTINGS_ADD_SERVER_BUTTON).assertIsDisplayed()
    }
}
