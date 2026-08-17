/**
 * Turns a swarm roster into a browsable merged catalog. For each server
 * device (`deviceType != CLIENT`), connects over QUIC — [connectToServer]
 * (direct, via the server's self-reported `peer_addr`) first, falling back
 * to [initiatePunchConnection] when [punchFallback] is configured and the
 * direct attempt fails, same best-first-with-fallback philosophy
 * `docs/PROTOCOL.md`'s source-selection section describes — registers the
 * live connection with [proxy] (so a later player can stream from it via
 * [PeerLoopbackProxy.urlFor]), fetches its `/catalog/manifest`, and merges
 * every server's manifest with [CatalogMerger]. A server that isn't
 * reachable by either path is reported in [Result.unreachable] rather than
 * failing the whole refresh: matches the fail-open-to-stale posture
 * `ServerCore` (Rust) already uses for its own roster sync, since one bad
 * peer should never block browsing the rest of the swarm.
 */
package app.swarm.tv.core.catalog

import app.swarm.tv.core.client.SignalingClient
import app.swarm.tv.core.peer.CatalogManifest
import app.swarm.tv.core.proxy.PeerLoopbackProxy
import app.swarm.tv.core.rest.DeviceType
import app.swarm.tv.core.rest.SwarmDevice
import app.swarm.tv.core.rest.SwarmJson
import app.swarm.tv.core.signal.SignalMessage
import app.swarm.tv.core.transport.PeerQuicClient
import app.swarm.tv.core.transport.connectToServer
import app.swarm.tv.core.transport.initiatePunchConnection
import java.net.InetSocketAddress
import java.security.PrivateKey
import java.security.cert.X509Certificate
import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.serialization.decodeFromString

/**
 * What [CatalogSession] needs to attempt a hole-punched connection when a
 * server's `peer_addr` isn't directly reachable (off-LAN). Bundled rather
 * than three loose parameters because they're only ever meaningful
 * together, and because [signalRx] is borrowed exclusively for the
 * duration of one punch attempt — see [initiatePunchConnection]'s doc
 * comment — so whoever constructs this is asserting nothing else is
 * reading this signaling session's inbound messages concurrently.
 */
class PunchFallback(
    val signaling: SignalingClient,
    val signalRx: ReceiveChannel<SignalMessage>,
    val reflectorAddr: InetSocketAddress,
    val ownFingerprint: String,
)

class CatalogSession(private val proxy: PeerLoopbackProxy) : AutoCloseable {
    private val connections = mutableMapOf<String, PeerQuicClient>()

    /**
     * Set once a signaling session is available (typically right after
     * registration) to enable the hole-punch fallback; left null to stay
     * LAN-only (`peer_addr` direct connections work regardless).
     */
    var punchFallback: PunchFallback? = null

    data class Result(val entries: List<MergedEntry>, val unreachable: List<SwarmDevice>)

    /** The URL to hand a media player for `peerPath` on `serverId` — only live once that server appeared connected in a [refresh]. */
    fun urlFor(serverId: String, peerPath: String): String = proxy.urlFor(serverId, peerPath)

    suspend fun refresh(devices: List<SwarmDevice>, clientCertificate: X509Certificate, clientKey: PrivateKey): Result {
        val manifestsByServer = mutableMapOf<String, CatalogManifest>()
        val unreachable = mutableListOf<SwarmDevice>()

        for (device in devices.filter { it.deviceType != DeviceType.CLIENT }) {
            val connection = connectionFor(device, clientCertificate, clientKey)
            val manifest = connection?.let { fetchManifest(device.deviceId, it) }
            if (manifest == null) unreachable += device else manifestsByServer[device.deviceId] = manifest
        }

        return Result(CatalogMerger.merge(manifestsByServer), unreachable)
    }

    private suspend fun connectionFor(device: SwarmDevice, clientCertificate: X509Certificate, clientKey: PrivateKey): PeerQuicClient? {
        connections[device.deviceId]?.let { return it }

        val direct = runCatching { connectToServer(device, clientCertificate, clientKey) }.getOrNull()
        val connection = direct ?: punchFallback?.let { fallback ->
            runCatching {
                initiatePunchConnection(
                    signaling = fallback.signaling,
                    signalRx = fallback.signalRx,
                    reflectorAddr = fallback.reflectorAddr,
                    peerDeviceId = device.deviceId,
                    ownFingerprint = fallback.ownFingerprint,
                    clientCertificate = clientCertificate,
                    clientKey = clientKey,
                    expectedFingerprint = device.certFingerprint,
                )
            }.getOrNull()
        } ?: return null

        connections[device.deviceId] = connection
        proxy.register(device.deviceId, connection)
        return connection
    }

    private fun fetchManifest(serverId: String, connection: PeerQuicClient): CatalogManifest? {
        val manifest = runCatching {
            val response = connection.request("/catalog/manifest")
            SwarmJson.decodeFromString<CatalogManifest>(response.body.readBytes().decodeToString())
        }.getOrNull()
        if (manifest == null) {
            // Stale connection (peer restarted, network dropped) — drop it so the next refresh reconnects.
            connections.remove(serverId)
            proxy.unregister(serverId)
            runCatching { connection.close() }
        }
        return manifest
    }

    override fun close() {
        connections.keys.forEach(proxy::unregister)
        connections.values.forEach { runCatching { it.close() } }
        connections.clear()
    }
}
