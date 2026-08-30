package app.swarm.tv.core.catalog

import app.swarm.tv.core.peer.ByteRange
import app.swarm.tv.core.peer.CatalogEntry
import app.swarm.tv.core.peer.CatalogManifest
import app.swarm.tv.core.peer.ClientErrorReport
import app.swarm.tv.core.peer.LikeToggle
import app.swarm.tv.core.peer.MediaKind
import app.swarm.tv.core.peer.PeerResponseHeader
import app.swarm.tv.core.peer.PlaybackMode
import app.swarm.tv.core.peer.PlaybackPlan
import app.swarm.tv.core.peer.PlaybackPreferences
import app.swarm.tv.core.proxy.PeerLoopbackProxy
import app.swarm.tv.core.rest.DeviceType
import app.swarm.tv.core.rest.SwarmDevice
import app.swarm.tv.core.rest.SwarmJson
import app.swarm.tv.core.transport.PeerConnection
import app.swarm.tv.core.transport.PeerResponse
import app.swarm.tv.core.transport.TestIdentity
import java.io.ByteArrayInputStream
import java.io.IOException
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.encodeToString
import okhttp3.OkHttpClient
import okhttp3.Request
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class CatalogSessionTest {
    @Test
    fun `refresh retries when the first connection cannot be established`() = runBlocking {
        val manifest = CatalogManifest(
            thumbprint = "catalog-v1",
            entries = listOf(
                CatalogEntry(
                    entryKey = "movie-1",
                    fingerprint = "media-fingerprint",
                    kind = MediaKind.MOVIE,
                    title = "First Connection",
                    size = 1_024,
                ),
            ),
        )
        val connection = CatalogConnection(manifest)
        var connectionAttempts = 0
        val proxy = PeerLoopbackProxy.start()
        val identity = TestIdentity.generate()
        val device = SwarmDevice(
            deviceId = "server-1",
            name = "Media server",
            deviceType = DeviceType.SERVER,
            certFingerprint = "ab".repeat(32),
            online = true,
            metadata = mapOf("peer_addr" to "192.168.1.2:8544"),
        )

        CatalogSession(proxy, directConnector = { _, _, _ ->
            connectionAttempts += 1
            if (connectionAttempts == 1) null else connection
        }).use { session ->
            val result = session.refresh(listOf(device), identity.certificate, identity.privateKey)

            assertEquals(2, connectionAttempts)
            assertTrue(result.unreachable.isEmpty())
            assertEquals(listOf("First Connection"), result.entries.map { it.entry.title })
        }
        proxy.close()
    }

    @Test
    fun `refresh survives two consecutive connection failures on a server that just approved LAN pairing`() = runBlocking {
        // Regression coverage for #47: two rapid, back-to-back retries were
        // not enough on a real device — a server that has just approved a
        // new LAN pairing can flake on this TV's first couple of connection
        // attempts before it recovers. refresh() now allows a third attempt.
        val manifest = CatalogManifest(
            thumbprint = "catalog-v1",
            entries = listOf(
                CatalogEntry(
                    entryKey = "movie-1",
                    fingerprint = "media-fingerprint",
                    kind = MediaKind.MOVIE,
                    title = "First Connection",
                    size = 1_024,
                ),
            ),
        )
        val connection = CatalogConnection(manifest)
        var connectionAttempts = 0
        val proxy = PeerLoopbackProxy.start()
        val identity = TestIdentity.generate()
        val device = SwarmDevice(
            deviceId = "server-1",
            name = "Media server",
            deviceType = DeviceType.SERVER,
            certFingerprint = "ab".repeat(32),
            online = true,
            metadata = mapOf("peer_addr" to "192.168.1.2:8544"),
        )

        CatalogSession(proxy, directConnector = { _, _, _ ->
            connectionAttempts += 1
            if (connectionAttempts <= 2) null else connection
        }).use { session ->
            val result = session.refresh(listOf(device), identity.certificate, identity.privateKey)

            assertEquals(3, connectionAttempts)
            assertTrue(result.unreachable.isEmpty())
            assertEquals(listOf("First Connection"), result.entries.map { it.entry.title })
        }
        proxy.close()
    }

    @Test
    fun `refresh abandons a stalled manifest fetch and recovers on the next attempt`() = runBlocking {
        // Regression coverage for #100: the initial Browse load failed with a
        // hard "loading timed out" while an immediate manual retry succeeded.
        // A manifest fetch that stalls on a live connection used to consume
        // the whole outer catalog budget; refresh() now bounds each attempt
        // so its own retry loop reconnects and completes.
        val manifest = CatalogManifest(
            thumbprint = "catalog-v1",
            entries = listOf(
                CatalogEntry(
                    entryKey = "movie-1",
                    fingerprint = "media-fingerprint",
                    kind = MediaKind.MOVIE,
                    title = "Recovered",
                    size = 1_024,
                ),
            ),
        )
        val stalling = StallingCatalogConnection()
        val healthy = CatalogConnection(manifest)
        val connectionAttempts = AtomicInteger()
        val proxy = PeerLoopbackProxy.start()
        val identity = TestIdentity.generate()
        val device = SwarmDevice(
            deviceId = "server-1",
            name = "Media server",
            deviceType = DeviceType.SERVER,
            certFingerprint = "ab".repeat(32),
            online = true,
            metadata = mapOf("peer_addr" to "192.168.1.2:8544"),
        )

        CatalogSession(
            proxy,
            directConnector = { _, _, _ ->
                if (connectionAttempts.incrementAndGet() == 1) stalling else healthy
            },
            manifestFetchTimeoutMs = 200L,
        ).use { session ->
            val result = session.refresh(listOf(device), identity.certificate, identity.privateKey)

            assertTrue(result.unreachable.isEmpty())
            assertEquals(listOf("Recovered"), result.entries.map { it.entry.title })
            assertTrue(connectionAttempts.get() >= 2)
            assertTrue(stalling.wasClosed())
        }
        proxy.close()
    }

    @Test
    fun `playback preparation abandons a stalled response and retries on a fresh connection`() = runBlocking {
        val stalling = StallingCatalogConnection()
        val healthy = PlaybackConnection("replacement-session")
        val connectionAttempts = AtomicInteger()
        val proxy = PeerLoopbackProxy.start()
        val identity = TestIdentity.generate()
        val device = SwarmDevice(
            deviceId = "server-1",
            name = "Media server",
            deviceType = DeviceType.SERVER,
            certFingerprint = "ab".repeat(32),
            online = true,
            metadata = mapOf("peer_addr" to "192.168.1.2:8544"),
        )

        CatalogSession(
            proxy,
            directConnector = { _, _, _ ->
                if (connectionAttempts.incrementAndGet() == 1) stalling else healthy
            },
            playbackPreparationTimeoutMs = 100L,
        ).use { session ->
            val selection = session.preparePlayback(
                device,
                "0123456789abcdef01234567",
                0,
                identity.certificate,
                identity.privateKey,
            )

            assertEquals("replacement-session", selection.sessionId)
            assertEquals(2, connectionAttempts.get())
            assertTrue(stalling.wasClosed())
        }
        proxy.close()
    }

    @Test
    fun `interrupted playback reconnect becomes 503 then reconnects on a later request`() = runBlocking {
        val manifest = CatalogManifest(thumbprint = "catalog-v1", entries = emptyList())
        val connection = InterruptingCatalogConnection(manifest)
        val replacement = MediaConnection("resumed")
        val connectionAttempts = AtomicInteger()
        val allowReconnect = AtomicBoolean()
        val restoredServer = AtomicReference<String>()
        val proxy = PeerLoopbackProxy.start()
        val identity = TestIdentity.generate()
        val device = SwarmDevice(
            deviceId = "server-1",
            name = "Media server",
            deviceType = DeviceType.SERVER,
            certFingerprint = "ab".repeat(32),
            online = true,
            metadata = mapOf("peer_addr" to "192.168.1.2:8544"),
        )

        CatalogSession(
            proxy,
            directConnector = { _, _, _ ->
                val attempt = connectionAttempts.incrementAndGet()
                when {
                    attempt == 1 -> connection
                    allowReconnect.get() -> replacement
                    else -> null
                }
            },
            onConnectionRestored = restoredServer::set,
        ).use { session ->
            val result = session.refresh(listOf(device), identity.certificate, identity.privateKey)
            assertTrue(result.unreachable.isEmpty())
            assertEquals(null, restoredServer.get())

            // Mirrors the Fire TV crash: kwik reported the dead connection
            // and left the proxy worker interrupted immediately before its
            // runBlocking reconnect attempt.
            connection.interruptOnRequest = true
            OkHttpClient().newCall(
                Request.Builder().url(proxy.urlFor(device.deviceId, "/media/song")).build(),
            ).execute().use { response -> assertEquals(503, response.code) }
            assertEquals(null, restoredServer.get())

            // Media3 retries this loopback URL after its two-second outage
            // delay. Once the server is available, the durable proxy route
            // must establish a new raw QUIC connection and serve the song.
            allowReconnect.set(true)
            OkHttpClient().newCall(
                Request.Builder().url(proxy.urlFor(device.deviceId, "/media/song")).build(),
            ).execute().use { response ->
                assertEquals(200, response.code)
                assertEquals("resumed", response.body?.string())
            }
            assertTrue(connectionAttempts.get() >= 2)
            assertEquals(device.deviceId, restoredServer.get())
        }
        proxy.close()
    }

    private class CatalogConnection(private val manifest: CatalogManifest) : PeerConnection {
        override fun request(
            path: String,
            range: ByteRange?,
            ifNoneMatch: String?,
            playback: PlaybackPreferences?,
            errorReport: ClientErrorReport?,
            like: LikeToggle?,
        ): PeerResponse {
            val body = when (path) {
                "/catalog/thumbprint" -> """{"thumbprint":"${manifest.thumbprint}","entry_count":${manifest.entries.size}}"""
                "/catalog/manifest.gz" -> return response(404, ByteArray(0))
                "/catalog/manifest" -> SwarmJson.encodeToString(manifest)
                else -> error("unexpected catalog request: $path")
            }.toByteArray()
            return response(200, body)
        }

        private fun response(status: Int, body: ByteArray) = PeerResponse(
            PeerResponseHeader(status = status, len = body.size.toLong()),
            ByteArrayInputStream(body),
        )
    }

    private class InterruptingCatalogConnection(private val manifest: CatalogManifest) : PeerConnection {
        var interruptOnRequest = false

        override fun request(
            path: String,
            range: ByteRange?,
            ifNoneMatch: String?,
            playback: PlaybackPreferences?,
            errorReport: ClientErrorReport?,
            like: LikeToggle?,
        ): PeerResponse {
            if (interruptOnRequest) {
                Thread.currentThread().interrupt()
                throw IOException("connection closed")
            }
            val body = when (path) {
                "/catalog/thumbprint" -> """{"thumbprint":"${manifest.thumbprint}","entry_count":${manifest.entries.size}}"""
                "/catalog/manifest.gz" -> return PeerResponse(
                    PeerResponseHeader(status = 404, len = 0),
                    ByteArrayInputStream(ByteArray(0)),
                )
                "/catalog/manifest" -> SwarmJson.encodeToString(manifest)
                else -> error("unexpected catalog request: $path")
            }.toByteArray()
            return PeerResponse(
                PeerResponseHeader(status = 200, len = body.size.toLong()),
                ByteArrayInputStream(body),
            )
        }
    }

    /** Blocks every request until [close] is called, then reports the
     * connection as dropped — models a peer that accepts the QUIC handshake
     * but never answers the catalog request. */
    private class StallingCatalogConnection : PeerConnection, AutoCloseable {
        private val gate = Object()
        private var closed = false

        override fun request(
            path: String,
            range: ByteRange?,
            ifNoneMatch: String?,
            playback: PlaybackPreferences?,
            errorReport: ClientErrorReport?,
            like: LikeToggle?,
        ): PeerResponse {
            synchronized(gate) {
                while (!closed) gate.wait()
            }
            throw IOException("connection closed")
        }

        override fun close() {
            synchronized(gate) {
                closed = true
                gate.notifyAll()
            }
        }

        fun wasClosed(): Boolean = synchronized(gate) { closed }
    }

    private class MediaConnection(private val content: String) : PeerConnection {
        override fun request(
            path: String,
            range: ByteRange?,
            ifNoneMatch: String?,
            playback: PlaybackPreferences?,
            errorReport: ClientErrorReport?,
            like: LikeToggle?,
        ): PeerResponse {
            require(path == "/media/song") { "unexpected media request: $path" }
            val body = content.toByteArray()
            return PeerResponse(
                PeerResponseHeader(status = 200, len = body.size.toLong()),
                ByteArrayInputStream(body),
            )
        }
    }

    private class PlaybackConnection(private val sessionId: String) : PeerConnection {
        override fun request(
            path: String,
            range: ByteRange?,
            ifNoneMatch: String?,
            playback: PlaybackPreferences?,
            errorReport: ClientErrorReport?,
            like: LikeToggle?,
        ): PeerResponse {
            require(path.startsWith("/play/")) { "unexpected playback request: $path" }
            val body = SwarmJson.encodeToString(
                PlaybackPlan(
                    mode = PlaybackMode.HLS,
                    path = "/hls/$sessionId/master.m3u8",
                    maxBitrate = 8_192_000,
                    sessionId = sessionId,
                ),
            ).toByteArray()
            return PeerResponse(
                PeerResponseHeader(status = 200, len = body.size.toLong()),
                ByteArrayInputStream(body),
            )
        }
    }
}
