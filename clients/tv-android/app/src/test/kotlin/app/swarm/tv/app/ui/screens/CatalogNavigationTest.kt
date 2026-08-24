package app.swarm.tv.app.ui.screens

import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class CatalogNavigationTest {
    @Test
    fun `back leaves rail while it is expanded`() {
        assertTrue(shouldLeaveFilterRailOnBack(expanded = true, hasFocus = true))
    }

    @Test
    fun `back leaves collapsed rail when focus remains inside it`() {
        assertTrue(shouldLeaveFilterRailOnBack(expanded = false, hasFocus = true))
    }

    @Test
    fun `back leaves catalog when focus is in browse content`() {
        assertFalse(shouldLeaveFilterRailOnBack(expanded = false, hasFocus = false))
    }
}
