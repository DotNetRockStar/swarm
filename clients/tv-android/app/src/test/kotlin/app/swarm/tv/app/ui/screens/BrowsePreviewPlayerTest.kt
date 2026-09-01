package app.swarm.tv.app.ui.screens

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Test

class BrowsePreviewPlayerTest {
    @Test
    fun `card expands only when dwell and negotiation completed for same entry`() {
        assertEquals("movie-1", previewExpansionEntryKey("movie-1", "movie-1"))
        assertNull(previewExpansionEntryKey("movie-1", null))
        assertNull(previewExpansionEntryKey("movie-1", "movie-2"))
    }

    @Test
    fun `negotiated preview does not expand before dwell completes`() {
        assertNull(previewExpansionEntryKey(null, "movie-1"))
    }
}
