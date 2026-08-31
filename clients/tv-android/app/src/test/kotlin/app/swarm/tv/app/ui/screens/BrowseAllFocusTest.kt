package app.swarm.tv.app.ui.screens

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Test

/**
 * [shelfRestoreIndex] resolves which position a top-level Movies/Shows/Music
 * row should restore D-pad focus to when the browse page is re-entered — a
 * remembered card, or (issue #159) the row's Browse All tile when the user
 * left via that tile.
 */
class BrowseAllFocusTest {
    private val items = (1..30).map { "key-$it" }
    private val keyOf: (String) -> String = { it }

    @Test
    fun `no remembered focus restores nothing`() {
        assertNull(shelfRestoreIndex(items, focusKey = null, genreFiltered = false, keyOf))
    }

    @Test
    fun `a remembered card restores its position`() {
        assertEquals(4, shelfRestoreIndex(items, focusKey = "key-5", genreFiltered = false, keyOf))
    }

    @Test
    fun `a remembered card that is gone restores nothing`() {
        assertNull(shelfRestoreIndex(items, focusKey = "key-999", genreFiltered = false, keyOf))
    }

    @Test
    fun `browse-all sentinel restores the tile position after the visible cap`() {
        // 30 items > MAX_SHELF_ITEMS (20), so the tile sits at index 20.
        assertEquals(20, shelfRestoreIndex(items, focusKey = BROWSE_ALL_TILE_FOCUS_KEY, genreFiltered = false, keyOf))
    }

    @Test
    fun `browse-all sentinel restores nothing when the row has no tile`() {
        val short = items.take(10)
        assertNull(shelfRestoreIndex(short, focusKey = BROWSE_ALL_TILE_FOCUS_KEY, genreFiltered = false, keyOf))
    }

    @Test
    fun `browse-all sentinel restores nothing in the genre-filtered grid which has no tile`() {
        assertNull(shelfRestoreIndex(items, focusKey = BROWSE_ALL_TILE_FOCUS_KEY, genreFiltered = true, keyOf))
    }
}
