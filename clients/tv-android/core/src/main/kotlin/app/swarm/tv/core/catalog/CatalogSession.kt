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
import app.swarm.tv.core.capability.CapabilityProfile
import app.swarm.tv.core.peer.CatalogManifest
import app.swarm.tv.core.peer.PlaybackMode
import app.swarm.tv.core.peer.PlaybackPlan
import app.swarm.tv.core.peer.PlaybackPreferences
import app.swarm.tv.core.proxy.PeerLoopbackProxy
import app.swarm.tv.core.rest.DeviceType
import app.swarm.tv.core.rest.SwarmDevice
import app.swarm.tv.core.rest.SwarmJson
import app.swarm.tv.core.signal.SignalMessage
import app.swarm.tv.core.transport.PeerQuicClient
import app.swarm.tv.core.transport.connectToServer
import app.swarm.tv.core.transport.initiatePunchConnection
import java.net.InetSocketAddress
import java.io.IOException
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

/** Which method last got a connection to a given peer — see [CatalogSession]'s route-memory doc comment. */
enum class ConnectionRoute { DIRECT, PUNCH }

/**
 * `docs/PROTOCOL.md`'s route memory ("persist the last successful
 * candidate per peer and try it first next time — preference changes, the
 * trusted candidate set never widens"), scoped down to what actually costs
 * something to get wrong here: not *which* candidate within a punch
 * attempt (that negotiation is already fast — signaling round trips, not a
 * blocking timeout), but *whether to attempt the direct path at all*.
 * `connectToServer` blocks for a full `connectTimeout` (default 5s) before
 * failing when a peer's `peer_addr` isn't reachable, so remembering "punch
 * worked last time" and skipping straight to it is the one memory that's
 * actually worth the complexity right now.
 *
 * Deliberately in-memory only, not persisted to disk across app restarts
 * despite the protocol doc saying "persist" — this project has no existing
 * on-device storage pattern for this kind of cache yet (`TokenStore` is
 * for one secret, not a per-peer map), and the win that matters — not
 * re-trying a doomed direct connection *within one app session* while
 * browsing/switching back and forth between servers — is fully captured
 * without it. Revisit if cross-restart memory turns out to matter in
 * practice.
 */
private class RouteMemory {
    private val lastSuccessful = mutableMapOf<String, ConnectionRoute>()
    fun record(deviceId: String, route: ConnectionRoute) {
        lastSuccessful[deviceId] = route
    }
    fun preferred(deviceId: String): ConnectionRoute? = lastSuccessful[deviceId]
}

class CatalogSession(private val proxy: PeerLoopbackProxy) : AutoCloseable {
    private val connections = mutableMapOf<String, PeerQuicClient>()
    private val routeMemory = RouteMemory()

    /**
     * Set once a signaling session is available (typically right after
     * registration) to enable the hole-punch fallback; left null to stay
     * LAN-only (`peer_addr` direct connections work regardless).
     */
    var punchFallback: PunchFallback? = null

    data class Result(val entries: List<MergedEntry>, val unreachable: List<SwarmDevice>)
    data class PlaybackSelection(val url: String, val mode: PlaybackMode, val maxBitrate: Long)

    /** The URL to hand a media player for `peerPath` on `serverId` — only live once that server appeared connected in a [refresh]. */
    fun urlFor(serverId: String, peerPath: String): String = proxy.urlFor(serverId, peerPath)

    /**
     * Reserve this server's shared upload budget and obtain either a paced
     * direct-play path or a capability-pruned HLS master path. This request
     * goes over the authenticated peer connection; the rendezvous server is
     * not involved in playback policy.
     */
    @Throws(IOException::class)
    fun preparePlayback(
        serverId: String,
        entryKey: String,
        startPositionSecs: Long,
        capabilities: CapabilityProfile = CapabilityProfile.fireTvBaseline(),
    ): PlaybackSelection {
        val connection = connections[serverId] ?: throw IOException("server is no longer connected")
        val response = connection.request(
            path = "/play/$entryKey",
            playback = PlaybackPreferences(
                capabilities = capabilities,
                startPositionSecs = startPositionSecs,
                preferDirect = true,
            ),
        )
        val body = response.body.readBytes().decodeToString()
        if (response.header.status != 200) {
            throw IOException("server could not prepare playback (${response.header.status}): $body")
        }
        val plan = SwarmJson.decodeFromString<PlaybackPlan>(body)
        return PlaybackSelection(proxy.urlFor(serverId, plan.path), plan.mode, plan.maxBitrate)
    }

    suspend fun refresh(devices: List<SwarmDevice>, clientCertificate: X509Certificate, clientKey: PrivateKey): Result {
        val manifestsByServer = mutableMapOf<String, CatalogManifest>()
        val unreachable = mutableListOf<SwarmDevice>()

        for (device in devices.filter { it.deviceType != DeviceType.CLIENT }) {
            var connection = connectionFor(device, clientCertificate, clientKey)
            var manifest = connection?.let { fetchManifest(device.deviceId, it) }
            if (manifest == null && connection != null) {
                // fetchManifest already evicted the dead connection it was just
                // handed (peer restarted, or — confirmed live — a QUIC connection
                // that sat idle long enough to be dropped) — one retry gets a
                // fresh one in the same refresh, instead of leaving the device
                // "unreachable" until a second, separate Browse Library press.
                connection = connectionFor(device, clientCertificate, clientKey)
                manifest = connection?.let { fetchManifest(device.deviceId, it) }
            }
            if (manifest == null) unreachable += device else manifestsByServer[device.deviceId] = manifest
        }

        return Result(CatalogMerger.merge(manifestsByServer), unreachable)
    }

    private suspend fun connectionFor(device: SwarmDevice, clientCertificate: X509Certificate, clientKey: PrivateKey): PeerQuicClient? {
        connections[device.deviceId]?.let { return it }

        // Failures here fail open (device just ends up in Result.unreachable)
        // by design, but that previously made a real bug — a NoSuchMethodError
        // from an API-33-only call — indistinguishable from an ordinary
        // network timeout. Logging beats a second silent-failure debugging session.
        suspend fun tryDirect(): PeerQuicClient? =
            runCatching { connectToServer(device, clientCertificate, clientKey) }
                .onFailure { it.printStackTrace() }
                .getOrNull()
        suspend fun tryPunch(): PeerQuicClient? = punchFallback?.let { fallback ->
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
        }

        // Route memory only ever changes which method is tried *first* —
        // the trusted fingerprint being dialed never changes, so this
        // can't widen trust, only save the wasted wait on a connectToServer
        // attempt already known to time out for this peer.
        val preferPunch = routeMemory.preferred(device.deviceId) == ConnectionRoute.PUNCH
        val (connection, route) = if (preferPunch) {
            tryPunch()?.let { it to ConnectionRoute.PUNCH } ?: (tryDirect()?.let { it to ConnectionRoute.DIRECT })
        } else {
            tryDirect()?.let { it to ConnectionRoute.DIRECT } ?: (tryPunch()?.let { it to ConnectionRoute.PUNCH })
        } ?: return null

        routeMemory.record(device.deviceId, route)
        connections[device.deviceId] = connection
        proxy.register(device.deviceId, connection)
        return connection
    }

    private fun fetchManifest(serverId: String, connection: PeerQuicClient): CatalogManifest? {
        val manifest = runCatching {
            val response = connection.request("/catalog/manifest")
            SwarmJson.decodeFromString<CatalogManifest>(response.body.readBytes().decodeToString())
        }.onFailure { it.printStackTrace() }.getOrNull()
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
