/**
 * Cross-language wire-compatibility checks for the peer protocol, same
 * discipline as `rest/ContractsTest.kt` — fixtures captured from real
 * `serde_json` output. [ByteRange] gets the most scrutiny here since it's
 * hand-serialized (see [ByteRangeSerializer]'s doc comment for why).
 */
package app.swarm.tv.core.peer

import app.swarm.tv.core.rest.SwarmJson
import kotlinx.serialization.decodeFromString
import kotlinx.serialization.encodeToString
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Test

class ContractsTest {

    @Test
    fun `kotlin decodes a real serde-produced catalog manifest`() {
        val json = """{"thumbprint":"${"ff".repeat(32)}","entries":[{"entry_key":"030fe19c72f2665e6efd018a",""" +
            """"fingerprint":"704ac5a4284267953aab77855e0e32aa","kind":"movie","title":"Inception",""" +
            """"size":4700000000,"duration_secs":8880.0,"scraped_title":"Inception (2010)","genres":["Sci-Fi"],""" +
            """"video":{"codec":"h264","width":1920,"height":1080,"level":"4.1","bitrate":8000000},""" +
            """"audio":{"codec":"aac","channels":6},"artwork_etag":"v1"}],"removed":[]}"""
        val manifest = SwarmJson.decodeFromString<CatalogManifest>(json)
        assertEquals(1, manifest.entries.size)
        val entry = manifest.entries[0]
        assertEquals(MediaKind.MOVIE, entry.kind)
        assertEquals("Inception", entry.title)
        assertEquals(4_700_000_000L, entry.size)
        assertEquals(8880.0, entry.durationSecs)
        assertEquals("Inception (2010)", entry.scrapedTitle)
        assertEquals(listOf("Sci-Fi"), entry.genres)
        assertEquals(VideoStreamInfo("h264", 1920, 1080, "4.1", 8_000_000L), entry.video)
        // audio.bitrate was omitted by serde (skip_serializing_if) — must
        // still decode cleanly to the Kotlin default of null.
        assertEquals(AudioStreamInfo("aac", 6, null), entry.audio)
        assertNull(entry.showTitle)
        assertNull(entry.artist)
    }

    @Test
    fun `byte range from_to matches serde's externally tagged shape`() {
        val expected = """{"path":"/media/030fe19c72f2665e6efd018a","range":{"from_to":{"start":1024,"end":null}}}"""
        val request = PeerRequest(path = "/media/030fe19c72f2665e6efd018a", range = ByteRange.FromTo(start = 1024, end = null))
        assertEquals(expected, SwarmJson.encodeToString(request))

        val decoded = SwarmJson.decodeFromString<PeerRequest>(expected)
        assertEquals(request, decoded)
    }

    @Test
    fun `byte range suffix matches serde's externally tagged shape`() {
        val expected = """{"path":"/media/x","range":{"suffix":{"last":500}}}"""
        val request = PeerRequest(path = "/media/x", range = ByteRange.Suffix(last = 500))
        assertEquals(expected, SwarmJson.encodeToString(request))
        assertEquals(request, SwarmJson.decodeFromString<PeerRequest>(expected))
    }

    @Test
    fun `byte range from_to with an explicit end decodes correctly`() {
        val decoded = SwarmJson.decodeFromString<ByteRange>("""{"from_to":{"start":100,"end":199}}""")
        assertEquals(ByteRange.FromTo(100, 199), decoded)
    }

    @Test
    fun `kotlin decodes a real serde-produced peer response header`() {
        val json = """{"status":206,"len":1024,"content_type":"video/x-matroska",""" +
            """"content_range":{"start":0,"end":1023,"total":4700000000}}"""
        val header = SwarmJson.decodeFromString<PeerResponseHeader>(json)
        assertEquals(206, header.status)
        assertEquals(1024L, header.len)
        assertEquals("video/x-matroska", header.contentType)
        assertEquals(ContentRange(0, 1023, 4_700_000_000L), header.contentRange)
        assertNull(header.etag)
    }
}
