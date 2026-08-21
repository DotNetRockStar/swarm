/**
 * Exercises [PeerLoopbackProxy] through a real HTTP client (OkHttp) over
 * real loopback TCP — validating the actual wire protocol (status line,
 * headers, body framing), not just internal method calls — against a fake
 * [PeerConnection] so this stays fast and independent of any live QUIC
 * connection. The real cross-implementation QUIC path is covered instead by
 * `PeerQuicClientInteropTest`; this file is purely about the HTTP<->
 * PeerRequest translation.
 */
package app.swarm.tv.core.proxy

import app.swarm.tv.core.peer.ByteRange
import app.swarm.tv.core.peer.ClientErrorReport
import app.swarm.tv.core.peer.ContentRange
import app.swarm.tv.core.peer.LikeToggle
import app.swarm.tv.core.peer.PeerResponseHeader
import app.swarm.tv.core.peer.PlaybackPreferences
import app.swarm.tv.core.transport.PeerConnection
import app.swarm.tv.core.transport.PeerResponse
import java.io.ByteArrayInputStream
import okhttp3.OkHttpClient
import okhttp3.Request
import org.junit.jupiter.api.AfterEach
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.BeforeEach
import org.junit.jupiter.api.Test

private class FakePeerConnection(
    private val respond: (path: String, range: ByteRange?, ifNoneMatch: String?) -> PeerResponse,
) : PeerConnection {
    var lastPath: String? = null
    var lastRange: ByteRange? = null
    var lastIfNoneMatch: String? = null

    override fun request(
        path: String,
        range: ByteRange?,
        ifNoneMatch: String?,
        playback: PlaybackPreferences?,
        errorReport: ClientErrorReport?,
        like: LikeToggle?,
    ): PeerResponse {
        lastPath = path
        lastRange = range
        lastIfNoneMatch = ifNoneMatch
        return respond(path, range, ifNoneMatch)
    }
}

private fun bodyResponse(status: Int, bytes: ByteArray, contentType: String = "application/octet-stream", contentRange: ContentRange? = null, etag: String? = null) =
    PeerResponse(
        PeerResponseHeader(status = status, len = bytes.size.toLong(), contentType = contentType, contentRange = contentRange, etag = etag),
        ByteArrayInputStream(bytes),
    )

class PeerLoopbackProxyTest {
    private lateinit var proxy: PeerLoopbackProxy
    private val http = OkHttpClient()

    @BeforeEach
    fun setUp() {
        proxy = PeerLoopbackProxy.start()
    }

    @AfterEach
    fun tearDown() {
        proxy.close()
    }

    @Test
    fun `full body request returns 200 with correct headers and bytes`() {
        val payload = "hello swarm".toByteArray()
        val fake = FakePeerConnection { _, _, _ -> bodyResponse(200, payload, contentType = "video/x-matroska", etag = "fp123") }
        proxy.register("srv1", fake)

        val response = http.newCall(Request.Builder().url(proxy.urlFor("srv1", "/media/abc")).build()).execute()
        response.use {
            assertEquals(200, it.code)
            assertEquals("video/x-matroska", it.header("Content-Type"))
            assertEquals("11", it.header("Content-Length"))
            assertEquals("\"fp123\"", it.header("ETag"))
            assertEquals("bytes", it.header("Accept-Ranges"))
            assertEquals("hello swarm", it.body!!.string())
        }
        assertEquals("/media/abc", fake.lastPath)
        assertNull(fake.lastRange)
    }

    @Test
    fun `range header is parsed and forwarded as FromTo`() {
        val fake = FakePeerConnection { _, _, _ ->
            bodyResponse(206, "partial".toByteArray(), contentRange = ContentRange(100, 199, 1000))
        }
        proxy.register("srv1", fake)

        val response = http.newCall(
            Request.Builder().url(proxy.urlFor("srv1", "/media/abc")).header("Range", "bytes=100-199").build(),
        ).execute()
        response.use {
            assertEquals(206, it.code)
            assertEquals("bytes 100-199/1000", it.header("Content-Range"))
        }
        assertEquals(ByteRange.FromTo(100, 199), fake.lastRange)
    }

    @Test
    fun `open-ended range has a null end`() {
        val fake = FakePeerConnection { _, _, _ -> bodyResponse(206, ByteArray(0), contentRange = ContentRange(500, 999, 1000)) }
        proxy.register("srv1", fake)

        http.newCall(Request.Builder().url(proxy.urlFor("srv1", "/media/abc")).header("Range", "bytes=500-").build())
            .execute().use { assertEquals(206, it.code) }
        assertEquals(ByteRange.FromTo(500, null), fake.lastRange)
    }

    @Test
    fun `suffix range is parsed`() {
        val fake = FakePeerConnection { _, _, _ -> bodyResponse(206, ByteArray(0), contentRange = ContentRange(950, 999, 1000)) }
        proxy.register("srv1", fake)

        http.newCall(Request.Builder().url(proxy.urlFor("srv1", "/media/abc")).header("Range", "bytes=-50").build())
            .execute().use { assertEquals(206, it.code) }
        assertEquals(ByteRange.Suffix(50), fake.lastRange)
    }

    @Test
    fun `if-none-match header is forwarded without quotes`() {
        val fake = FakePeerConnection { _, _, _ -> bodyResponse(304, ByteArray(0)) }
        proxy.register("srv1", fake)

        http.newCall(Request.Builder().url(proxy.urlFor("srv1", "/art/abc/poster")).header("If-None-Match", "\"v3\"").build())
            .execute().use { assertEquals(304, it.code) }
        assertEquals("v3", fake.lastIfNoneMatch)
    }

    @Test
    fun `playlist query string is preserved on the peer request`() {
        val fake = FakePeerConnection { _, _, _ -> bodyResponse(200, "#EXTM3U".toByteArray()) }
        proxy.register("srv1", fake)

        http.newCall(Request.Builder().url(proxy.urlFor("srv1", "/hls/session/master.m3u8?token=abc")).build())
            .execute().use { assertEquals(200, it.code) }
        assertEquals("/hls/session/master.m3u8?token=abc", fake.lastPath)
    }

    @Test
    fun `unknown server id is 404 without touching any connection`() {
        var called = false
        proxy.register("srv1", FakePeerConnection { _, _, _ -> called = true; bodyResponse(200, ByteArray(0)) })

        http.newCall(Request.Builder().url(proxy.urlFor("does-not-exist", "/media/abc")).build())
            .execute().use { assertEquals(404, it.code) }
        assert(!called)
    }

    @Test
    fun `unregistering a server makes it 404 again`() {
        proxy.register("srv1", FakePeerConnection { _, _, _ -> bodyResponse(200, ByteArray(0)) })
        proxy.unregister("srv1")

        http.newCall(Request.Builder().url(proxy.urlFor("srv1", "/media/abc")).build())
            .execute().use { assertEquals(404, it.code) }
    }

    @Test
    fun `a peer connection failure becomes 500 not a hung or crashed proxy`() {
        proxy.register("srv1", FakePeerConnection { _, _, _ -> throw java.io.IOException("connection reset") })

        http.newCall(Request.Builder().url(proxy.urlFor("srv1", "/media/abc")).build())
            .execute().use { assertEquals(500, it.code) }

        // The proxy itself must still be usable for the next request.
        proxy.register("srv2", FakePeerConnection { _, _, _ -> bodyResponse(200, "ok".toByteArray()) })
        http.newCall(Request.Builder().url(proxy.urlFor("srv2", "/media/abc")).build())
            .execute().use { assertEquals(200, it.code) }
    }

    @Test
    fun `each proxy instance gets its own ephemeral port`() {
        PeerLoopbackProxy.start().use { other ->
            assert(other.port != proxy.port)
        }
    }
}
