package app.swarm.tv.app.data

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Test

class PreviewSelectionTest {
    @Test
    fun `movie preview starts between twenty and thirty five percent`() {
        val twoHourMovie = 2.0 * 60.0 * 60.0

        assertEquals(1_440L, previewStartSeconds(twoHourMovie, 0.20))
        assertEquals(2_520L, previewStartSeconds(twoHourMovie, 0.35))
    }

    @Test
    fun `preview percentage is bounded and missing runtime starts safely at beginning`() {
        assertEquals(1_200L, previewStartSeconds(6_000.0, 0.01))
        assertEquals(2_100L, previewStartSeconds(6_000.0, 0.99))
        assertEquals(0L, previewStartSeconds(null, 0.25))
    }

    @Test
    fun `short video start leaves room for complete preview`() {
        assertEquals(10L, previewStartSeconds(40.0, 0.35))
    }
}
