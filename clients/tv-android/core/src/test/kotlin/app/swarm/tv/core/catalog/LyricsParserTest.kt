package app.swarm.tv.core.catalog

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Test

class LyricsParserTest {
    @Test
    fun parses_sorts_and_applies_offset_to_lrc_lines() {
        val result = parseSyncedLyrics(
            """
            [ar:Test Artist]
            [offset:+250]
            [00:03.5]Second
            [00:01.00][00:02.00]First twice
            """.trimIndent(),
        )

        assertEquals(
            listOf(
                TimedLyricLine(1_250, "First twice"),
                TimedLyricLine(2_250, "First twice"),
                TimedLyricLine(3_750, "Second"),
            ),
            result,
        )
        assertEquals(1, activeLyricIndex(result, 2_500))
    }
}
