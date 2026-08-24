package app.swarm.tv.core.catalog

import app.swarm.tv.core.peer.ByteRange
import app.swarm.tv.core.peer.CatalogEntry
import app.swarm.tv.core.peer.CatalogManifest
import app.swarm.tv.core.peer.ClientErrorReport
import app.swarm.tv.core.peer.LikeToggle
import app.swarm.tv.core.peer.MediaKind
import app.swarm.tv.core.peer.PeerResponseHeader
import app.swarm.tv.core.peer.PlaybackPreferences
import app.swarm.tv.core.proxy.PeerLoopbackProxy
import app.swarm.tv.core.rest.DeviceType
import app.swarm.tv.core.rest.SwarmDevice
import app.swarm.tv.core.rest.SwarmJson
import app.swarm.tv.core.transport.PeerConnection
import app.swarm.tv.core.transport.PeerResponse
import app.swarm.tv.core.transport.TestIdentity
import java.io.ByteArrayInputStream
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.encodeToString
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
}
