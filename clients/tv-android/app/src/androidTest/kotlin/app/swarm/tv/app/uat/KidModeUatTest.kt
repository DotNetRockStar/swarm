package app.swarm.tv.app.uat

import app.swarm.tv.app.ui.UatTestTags
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertTrue
import org.junit.Test

/** PIN setup/gating, filtering, restart persistence, rule editing, and cleanup. */
class KidModeUatTest : UatTestBase() {

    @Test
    fun testKidModePinFilteringPersistenceAndDisable() {
        val before = runBlocking { kidModeStore.get() }
        try {
            // Normalize the scenario through a debug-only, active-test-mode hook.
            withActivity { it.disableKidModeForUat() }
            composeTestRule.waitUntil(timeoutMillis = 5_000) {
                runBlocking { kidModeStore.get() }?.enabled != true
            }

            openFamilySettings()
            selectTagWithDpad(UatTestTags.KID_MODE_MANAGE_BUTTON)
            enterPin('1')
            // PIN-stage transitions are deliberately delayed so the fourth
            // digit paints. Do not send confirmation digits into the still-
            // full first-stage keypad during that transition.
            composeTestRule.waitUntil(timeoutMillis = 5_000) {
                runCatching { composeTestRule.textUnderTag(UatTestTags.KID_MODE_PIN_PROMPT) }
                    .getOrNull() == "Confirm the PIN"
            }
            enterPin('1')
            waitForTag(UatTestTags.KID_MODE_KIND_MUSIC)
            selectTagWithDpad(UatTestTags.KID_MODE_KIND_MUSIC)
            selectTagWithDpad(UatTestTags.KID_MODE_SAVE_BUTTON)
            composeTestRule.waitUntil(timeoutMillis = 5_000) {
                composeTestRule.textUnderTag(UatTestTags.KID_MODE_STATUS).startsWith("On")
            }

            restartActivityAndWaitForCatalog()
            openFilterRail()
            selectTagWithDpad(UatTestTags.FILTER_KIND_PREFIX + "MUSIC")
            waitForTag(UatTestTags.SEARCH_NO_MATCHES)

            // The persisted PIN must reject a wrong value after restart.
            openFamilySettings()
            selectTagWithDpad(UatTestTags.KID_MODE_MANAGE_BUTTON)
            enterPin('2')
            waitForTag(UatTestTags.KID_MODE_PIN_ERROR)
            assertTrue(composeTestRule.textUnderTag(UatTestTags.KID_MODE_PIN_ERROR).contains("Wrong PIN"))
            enterPin('1')
            waitForTag(UatTestTags.KID_MODE_DISABLE_BUTTON)
            selectTagWithDpad(UatTestTags.KID_MODE_DISABLE_BUTTON)
            composeTestRule.waitUntil(timeoutMillis = 5_000) {
                runBlocking { kidModeStore.get() }?.enabled != true
            }

            restartActivityAndWaitForCatalog()
            openFilterRail()
            selectTagWithDpad(UatTestTags.FILTER_KIND_PREFIX + "MUSIC")
            waitForTag(UatTestTags.SHELF_MUSIC)
        } finally {
            runBlocking { kidModeStore.restoreSnapshotForTesting(before) }
        }
    }

    private fun enterPin(digit: Char) {
        repeat(4) {
            selectTagWithDpad(UatTestTags.NUMBER_PAD_KEY_PREFIX + digit)
        }
    }

    private fun openFamilySettings() {
        if (composeTestRule.allTagsStartingWith(UatTestTags.OPEN_SWARM_BUTTON).isEmpty()) {
            pressBack()
            waitForTag(UatTestTags.FILTER_RAIL)
        }
        navigateUpUntilTag(UatTestTags.OPEN_SWARM_BUTTON)
        selectTagWithDpad(UatTestTags.OPEN_SWARM_BUTTON)
        waitForTag(UatTestTags.DASHBOARD_SETTINGS_BUTTON)
        selectTagWithDpad(UatTestTags.DASHBOARD_SETTINGS_BUTTON)
        waitForTag(UatTestTags.FAMILY_TAB_BUTTON)
        selectTagWithDpad(UatTestTags.FAMILY_TAB_BUTTON)
        waitForTag(UatTestTags.KID_MODE_STATUS)
    }
}
