package app.swarm.tv.core.catalog

/** One timestamped LRC line, relative to the beginning of the source track. */
data class TimedLyricLine(val timeMs: Long, val text: String)

private val timestamp = Regex("\\[(\\d{1,3}):(\\d{2})(?:[.:](\\d{1,3}))?]")
private val offset = Regex("(?im)^\\[offset:([+-]?\\d+)]$")

/** Parse standard and enhanced-LRC timestamp lines; metadata tags are ignored. */
fun parseSyncedLyrics(lrc: String): List<TimedLyricLine> {
    val offsetMs = offset.find(lrc)?.groupValues?.get(1)?.toLongOrNull() ?: 0L
    return lrc.lineSequence()
        .flatMap { rawLine ->
            val matches = timestamp.findAll(rawLine).toList()
            if (matches.isEmpty()) return@flatMap emptySequence()
            val text = rawLine.substring(matches.last().range.last + 1).trim()
            if (text.isEmpty()) return@flatMap emptySequence()
            matches.asSequence().map { match ->
                val minutes = match.groupValues[1].toLong()
                val seconds = match.groupValues[2].toLong()
                val fraction = match.groupValues[3]
                val fractionMs = when (fraction.length) {
                    1 -> fraction.toLong() * 100L
                    2 -> fraction.toLong() * 10L
                    3 -> fraction.toLong()
                    else -> 0L
                }
                TimedLyricLine(
                    timeMs = (minutes * 60_000L + seconds * 1_000L + fractionMs + offsetMs).coerceAtLeast(0L),
                    text = text,
                )
            }
        }
        .sortedBy(TimedLyricLine::timeMs)
        .toList()
}

fun activeLyricIndex(lines: List<TimedLyricLine>, positionMs: Long): Int =
    lines.indexOfLast { it.timeMs <= positionMs }
