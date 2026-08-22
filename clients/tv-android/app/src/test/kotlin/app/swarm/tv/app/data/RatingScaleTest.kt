package app.swarm.tv.app.data

import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class RatingScaleTest {
    @Test
    fun `PG-13 limit includes lower movie ratings and excludes higher ones`() {
        assertTrue(RatingScale.isAllowed("G", "PG-13", RatingScale.MOVIE_ORDER))
        assertTrue(RatingScale.isAllowed("PG", "PG-13", RatingScale.MOVIE_ORDER))
        assertTrue(RatingScale.isAllowed("PG-13", "PG-13", RatingScale.MOVIE_ORDER))
        assertFalse(RatingScale.isAllowed("R", "PG-13", RatingScale.MOVIE_ORDER))
        assertFalse(RatingScale.isAllowed("NC-17", "PG-13", RatingScale.MOVIE_ORDER))
    }

    @Test
    fun `active limit hides missing and unknown ratings`() {
        assertFalse(RatingScale.isAllowed(null, "PG-13", RatingScale.MOVIE_ORDER))
        assertFalse(RatingScale.isAllowed("NR", "PG-13", RatingScale.MOVIE_ORDER))
    }

    @Test
    fun `no limit allows every rating state`() {
        assertTrue(RatingScale.isAllowed("R", null, RatingScale.MOVIE_ORDER))
        assertTrue(RatingScale.isAllowed(null, null, RatingScale.MOVIE_ORDER))
    }
}
