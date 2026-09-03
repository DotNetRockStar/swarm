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
    private fun testServerDevice() = SwarmDevice(
        deviceId = "server-1",
        name = "Media server",
        deviceType = DeviceType.SERVER,
        certFingerprint = "ab".repeat(32),
        online = true,
        metadata = mapOf("peer_addr" to "192.168.1.2:8544"),
    )

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
    fun `refresh recovers when a credit-starved connection self-bounds its request instead of closing`() = runBlocking {
        // Regression coverage for #140: at the end of a long movie the server
        // stopped issuing QUIC stream credit, so kwik's createStream parked
        // every request forever and closing the connection could not release
        // it (unlike the #100 stall above, which close() unblocks). The
        // transport now self-bounds stream creation, turning that into a
        // prompt IOException — modeled here by a connection whose request()
        // fails on its own timer regardless of close() — and refresh()'s
        // existing retry loop then reconnects and completes.
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
        val starved = CreditStarvedConnection(selfBoundMs = 150L)
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
                if (connectionAttempts.incrementAndGet() == 1) starved else healthy
            },
            manifestFetchTimeoutMs = 2_000L,
        ).use { session ->
            val result = session.refresh(listOf(device), identity.certificate, identity.privateKey)

            assertTrue(result.unreachable.isEmpty())
            assertEquals(listOf("Recovered"), result.entries.map { it.entry.title })
            assertTrue(connectionAttempts.get() >= 2)
            assertTrue(starved.wasClosed())
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
    fun `preview preparation uses its short timeout and retries on a fresh connection`() = runBlocking {
        val stalling = StallingCatalogConnection()
        val healthy = PlaybackConnection("preview-session")
        val connectionAttempts = AtomicInteger()
        val proxy = PeerLoopbackProxy.start()
        val identity = TestIdentity.generate()
        val device = testServerDevice()

        CatalogSession(
            proxy,
            directConnector = { _, _, _ ->
                if (connectionAttempts.incrementAndGet() == 1) stalling else healthy
            },
            playbackPreparationTimeoutMs = 10_000L,
            previewPreparationTimeoutMs = 100L,
        ).use { session ->
            val selection = session.preparePlayback(
                device,
                "0123456789abcdef01234567",
                0,
                identity.certificate,
                identity.privateKey,
                preview = true,
            )

            assertEquals("preview-session", selection.sessionId)
            assertEquals(2, connectionAttempts.get())
            assertTrue(stalling.wasClosed())
        }
        proxy.close()
    }

    @Test
    fun `stalled playback stop is bounded and closes the connection`() = runBlocking {
        val stalling = StallingCatalogConnection()
        val connectionAttempts = AtomicInteger()
        val proxy = PeerLoopbackProxy.start()
        val identity = TestIdentity.generate()
        val device = testServerDevice()

        CatalogSession(
            proxy,
            directConnector = { _, _, _ ->
                if (connectionAttempts.incrementAndGet() == 1) stalling else null
            },
            playbackStopTimeoutMs = 100L,
        ).use { session ->
            session.stopPlayback(device, "stalled-session", identity.certificate, identity.privateKey)
        }

        assertTrue(stalling.wasClosed())
        assertEquals(2, connectionAttempts.get())
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

    @Test
    fun `pollChanges applies a server delta onto the cached manifest without a full refetch`() = runBlocking {
        val baseline = CatalogManifest(
            thumbprint = "catalog-v1",
            entries = listOf(
                CatalogEntry("movie-1", "fp-1", MediaKind.MOVIE, "Alpha", 1_024),
                CatalogEntry("movie-2", "fp-2", MediaKind.MOVIE, "Beta", 1_024),
            ),
        )
        val delta = CatalogManifest(
            thumbprint = "catalog-v2",
            entries = listOf(
                CatalogEntry("movie-2", "fp-2", MediaKind.MOVIE, "Beta (updated)", 1_024),
                CatalogEntry("movie-3", "fp-3", MediaKind.MOVIE, "Gamma", 1_024),
            ),
            removed = listOf("movie-1"),
        )
        val connection = ChangeFeedConnection(baseline, delta)
        val proxy = PeerLoopbackProxy.start()
        val identity = TestIdentity.generate()
        val device = testServerDevice()

        CatalogSession(proxy, directConnector = { _, _, _ -> connection }).use { session ->
            session.refresh(listOf(device), identity.certificate, identity.privateKey)

            val poll = session.pollChanges(device, listOf(device), identity.certificate, identity.privateKey)

            assertTrue(poll.supported)
            assertEquals(
                listOf("Beta (updated)", "Gamma"),
                poll.entries?.map { it.entry.title }?.sorted(),
            )
        }
        proxy.close()
    }

    @Test
    fun `pollChanges persists and re-merges off the caller's thread`() = runBlocking {
        // The change feed is pumped from a Main-dispatched coroutine. Applying
        // the delta, re-serialising the whole manifest to disk and re-merging
        // every server used to run inline on that thread, which stalled the UI
        // long enough on a large library to drop remote input and ANR the app
        // on a memory-pressured Fire TV (#208).
        val baseline = CatalogManifest(
            thumbprint = "catalog-v1",
            entries = listOf(CatalogEntry("movie-1", "fp-1", MediaKind.MOVIE, "Alpha", 1_024)),
        )
        val delta = CatalogManifest(
            thumbprint = "catalog-v2",
            entries = listOf(CatalogEntry("movie-2", "fp-2", MediaKind.MOVIE, "Beta", 1_024)),
        )
        val connection = ChangeFeedConnection(baseline, delta)
        val proxy = PeerLoopbackProxy.start()
        val identity = TestIdentity.generate()
        val device = testServerDevice()
        val callerThread = Thread.currentThread()
        val storeThread = AtomicReference<Thread?>()
        val cache = object : CatalogCache {
            override suspend fun load(serverId: String): CatalogManifest? = null
            override suspend fun store(serverId: String, manifest: CatalogManifest) {
                storeThread.set(Thread.currentThread())
            }
        }

        CatalogSession(proxy, catalogCache = cache, directConnector = { _, _, _ -> connection }).use { session ->
            session.refresh(listOf(device), identity.certificate, identity.privateKey)
            storeThread.set(null)

            val poll = session.pollChanges(device, listOf(device), identity.certificate, identity.privateKey)

            assertEquals(listOf("Alpha", "Beta"), poll.entries?.map { it.entry.title }?.sorted())
        }
        proxy.close()

        val recorded = storeThread.get()
        assertTrue(recorded != null, "pollChanges never persisted the merged manifest")
        assertTrue(recorded !== callerThread, "catalog persist/merge must run off the caller's thread (#208)")
    }

    @Test
    fun `pollChanges reports unsupported when the server has no change feed`() = runBlocking {
        val baseline = CatalogManifest(
            thumbprint = "catalog-v1",
            entries = listOf(CatalogEntry("movie-1", "fp-1", MediaKind.MOVIE, "Alpha", 1_024)),
        )
        val connection = ChangeFeedConnection(baseline, delta = null)
        val proxy = PeerLoopbackProxy.start()
        val identity = TestIdentity.generate()
        val device = testServerDevice()

        CatalogSession(proxy, directConnector = { _, _, _ -> connection }).use { session ->
            session.refresh(listOf(device), identity.certificate, identity.privateKey)

            val poll = session.pollChanges(device, listOf(device), identity.certificate, identity.privateKey)

            assertEquals(false, poll.supported)
            assertEquals(null, poll.entries)
        }
        proxy.close()
    }

    /** Serves the initial catalog like [CatalogConnection], then answers the
     * `/catalog/changes` long-poll with [delta] (or 404 everywhere when
     * [delta] is null, modelling a server built before the change feed). */
    private class ChangeFeedConnection(
        private val baseline: CatalogManifest,
        private val delta: CatalogManifest?,
    ) : PeerConnection {
        override fun request(
            path: String,
            range: ByteRange?,
            ifNoneMatch: String?,
            playback: PlaybackPreferences?,
            errorReport: ClientErrorReport?,
            like: LikeToggle?,
        ): PeerResponse {
            val request = path.substringBefore('?')
            val body = when (request) {
                "/catalog/thumbprint" ->
                    """{"thumbprint":"${baseline.thumbprint}","entry_count":${baseline.entries.size}}"""
                "/catalog/manifest.gz" -> return response(404, ByteArray(0))
                "/catalog/manifest" -> SwarmJson.encodeToString(baseline)
                "/catalog/changes.gz" -> return response(404, ByteArray(0))
                "/catalog/changes" ->
                    delta?.let { SwarmJson.encodeToString(it) } ?: return response(404, ByteArray(0))
                else -> error("unexpected catalog request: $path")
            }.toByteArray()
            return response(200, body)
        }

        private fun response(status: Int, body: ByteArray) = PeerResponse(
            PeerResponseHeader(status = status, len = body.size.toLong()),
            ByteArrayInputStream(body),
        )
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

    /** Models the fixed transport under #140's credit starvation: request()
     * blocks briefly and then fails on its own bound, whether or not [close]
     * is ever called (kwik's parked createStream ignores connection close). */
    private class CreditStarvedConnection(private val selfBoundMs: Long) : PeerConnection, AutoCloseable {
        private val closed = java.util.concurrent.atomic.AtomicBoolean(false)

        override fun request(
            path: String,
            range: ByteRange?,
            ifNoneMatch: String?,
            playback: PlaybackPreferences?,
            errorReport: ClientErrorReport?,
            like: LikeToggle?,
        ): PeerResponse {
            Thread.sleep(selfBoundMs)
            throw IOException("peer stream creation stalled; aborted after ${selfBoundMs}ms (peer issued no stream credit)")
        }

        override fun close() {
            closed.set(true)
        }

        fun wasClosed(): Boolean = closed.get()
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

    @Test
    fun `preparePlayback sends the probed capability profile, not the baseline`() = runBlocking {
        val capturing = CapturingPlaybackConnection("session-1")
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
        val probed = app.swarm.tv.core.capability.CapabilityProfile(
            containers = listOf("mp4", "hls", "mkv"),
            videoCodecs = listOf("h264:high", "hevc:main10"),
            audioCodecs = listOf("aac", "ac3", "eac3"),
            maxWidth = 3840,
            maxHeight = 2160,
            maxBitrate = 60_000_000,
            hdr = true,
        )

        CatalogSession(
            proxy,
            directConnector = { _, _, _ -> capturing },
            playbackPreparationTimeoutMs = 100L,
        ).use { session ->
            session.preparePlayback(
                device,
                "0123456789abcdef01234567",
                0,
                identity.certificate,
                identity.privateKey,
                capabilities = probed,
            )
        }
        proxy.close()

        assertEquals(probed, capturing.received?.capabilities)
    }

    private class CapturingPlaybackConnection(private val sessionId: String) : PeerConnection {
        @Volatile
        var received: PlaybackPreferences? = null

        override fun request(
            path: String,
            range: ByteRange?,
            ifNoneMatch: String?,
            playback: PlaybackPreferences?,
            errorReport: ClientErrorReport?,
            like: LikeToggle?,
        ): PeerResponse {
            received = playback
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
