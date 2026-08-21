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
import app.swarm.tv.core.peer.ByteRange
import app.swarm.tv.core.peer.CatalogManifest
import app.swarm.tv.core.peer.ClientErrorReport
import app.swarm.tv.core.peer.LikeToggle
import app.swarm.tv.core.peer.PlaybackMode
import app.swarm.tv.core.peer.PlaybackPlan
import app.swarm.tv.core.peer.PlaybackPreferences
import app.swarm.tv.core.proxy.PeerLoopbackProxy
import app.swarm.tv.core.rest.DeviceType
import app.swarm.tv.core.rest.SwarmDevice
import app.swarm.tv.core.rest.SwarmJson
import app.swarm.tv.core.signal.SignalMessage
import app.swarm.tv.core.transport.PeerConnection
import app.swarm.tv.core.transport.PeerQuicClient
import app.swarm.tv.core.transport.PeerResponse
import app.swarm.tv.core.transport.connectToServer
import app.swarm.tv.core.transport.initiatePunchConnection
import java.net.InetSocketAddress
import java.io.IOException
import java.security.PrivateKey
import java.security.cert.X509Certificate
import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.coroutines.runBlocking
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
    fun forget(deviceId: String) {
        lastSuccessful.remove(deviceId)
    }
}

class CatalogSession(private val proxy: PeerLoopbackProxy) : AutoCloseable {
    private val connections = mutableMapOf<String, PeerQuicClient>()
    private val routeMemory = RouteMemory()
    /** Which devices already have a [ReconnectingConnection] registered with [proxy] — see [connectionFor]'s doc comment on why that registration must happen at most once per device, never repeated on a reconnect. */
    private val proxyRegisteredDevices = mutableSetOf<String>()

    /**
     * Set once a signaling session is available (typically right after
     * registration) to enable the hole-punch fallback; left null to stay
     * LAN-only (`peer_addr` direct connections work regardless).
     */
    var punchFallback: PunchFallback? = null

    data class Result(val entries: List<MergedEntry>, val unreachable: List<SwarmDevice>)
    data class PlaybackSelection(
        val url: String,
        val mode: PlaybackMode,
        val maxBitrate: Long,
        val sessionId: String,
        val lyrics: app.swarm.tv.core.peer.TrackLyrics? = null,
        val subtitles: List<app.swarm.tv.core.peer.SubtitleTrack> = emptyList(),
    )

    /** The URL to hand a media player for `peerPath` on `serverId` — only live once that server appeared connected in a [refresh]. */
    fun urlFor(serverId: String, peerPath: String): String = proxy.urlFor(serverId, peerPath)

    /** Drop a cached indirect route so the next connection uses a newly discovered LAN address first. */
    fun preferDirect(deviceId: String) {
        connections.remove(deviceId)?.let { runCatching { it.close() } }
        routeMemory.record(deviceId, ConnectionRoute.DIRECT)
    }

    /** Close this TV's connection to one server without changing any other device's swarm membership. */
    fun disconnect(deviceId: String) {
        connections.remove(deviceId)?.let { runCatching { it.close() } }
        routeMemory.forget(deviceId)
        if (proxyRegisteredDevices.remove(deviceId)) proxy.unregister(deviceId)
    }

    /**
     * Reserve this server's shared upload budget and obtain either a paced
     * direct-play path or a capability-pruned HLS master path. This request
     * goes over the authenticated peer connection; the rendezvous server is
     * not involved in playback policy.
     */
    @Throws(IOException::class)
    suspend fun preparePlayback(
        device: SwarmDevice,
        entryKey: String,
        startPositionSecs: Long,
        clientCertificate: X509Certificate,
        clientKey: PrivateKey,
        capabilities: CapabilityProfile = CapabilityProfile.fireTvBaseline(),
    ): PlaybackSelection {
        val preferences = PlaybackPreferences(
            capabilities = capabilities,
            startPositionSecs = startPositionSecs,
            preferDirect = true,
        )
        var connection = connectionFor(device, clientCertificate, clientKey)
            ?: throw IOException("server is no longer connected")
        val response = try {
            connection.request(path = "/play/$entryKey", playback = preferences)
        } catch (e: IOException) {
            // A cached connection can die between browsing and pressing play
            // — confirmed live: QUIC dropped it well under a minute after its
            // last request. Evict and retry once with a fresh connection,
            // same self-heal refresh()/fetchManifest already does for browse.
            // Only the raw connection is evicted here, never the proxy
            // registration itself — see ReconnectingConnection's doc comment.
            connections.remove(device.deviceId)
            connection = connectionFor(device, clientCertificate, clientKey)
                ?: throw IOException("server is no longer connected")
            connection.request(path = "/play/$entryKey", playback = preferences)
        }
        val body = response.body.readBytes().decodeToString()
        if (response.header.status != 200) {
            throw IOException("server could not prepare playback (${response.header.status}): $body")
        }
        val plan = SwarmJson.decodeFromString<PlaybackPlan>(body)
        return PlaybackSelection(
            proxy.urlFor(device.deviceId, plan.path),
            plan.mode,
            plan.maxBitrate,
            plan.sessionId,
            plan.lyrics,
            plan.subtitles.map { track -> track.copy(path = proxy.urlFor(device.deviceId, track.path)) },
        )
    }

    /**
     * Releases [sessionId]'s bandwidth reservation on [device] — call once
     * the player screen for it is torn down (back-press, or moving on to
     * the next entry), so a same-device retry doesn't get rejected with
     * "not enough upload bandwidth" for the rest of the server's idle
     * timeout. Best-effort and silent on failure: this is cleanup on the
     * way out, and the worst case if it doesn't land (dead connection, no
     * reachable server) is exactly today's behavior — the reservation
     * still expires on its own, just later.
     */
    suspend fun stopPlayback(device: SwarmDevice, sessionId: String, clientCertificate: X509Certificate, clientKey: PrivateKey) {
        var connection = connectionFor(device, clientCertificate, clientKey) ?: return
        fun stop(current: PeerQuicClient): Boolean = runCatching {
            val response = current.request(path = "/stop/$sessionId")
            response.body.readBytes()
            response.header.status == 200
        }.getOrDefault(false)

        if (stop(connection)) return

        // A track can end after several minutes with no control requests in
        // between, leaving the cached QUIC connection stale. Cleanup matters
        // most precisely then, so reconnect and retry once instead of silently
        // leaving the old transcode reservation until its idle timeout.
        connections.remove(device.deviceId)
        runCatching { connection.close() }
        connection = connectionFor(device, clientCertificate, clientKey) ?: return
        stop(connection)
    }

    /**
     * Sends [report] to [device] for triage on that server's own swarm page
     * — see `swarm_core::peer::ClientErrorReport`'s doc comment for why this
     * rides the authenticated peer connection rather than a separate HTTP
     * call. Best-effort and silent on failure, same posture as
     * [stopPlayback]: a device too unreachable to accept an error report is
     * itself unremarkable (it's very possibly *why* the original error
     * happened), and this must never itself become a second point of
     * failure in an error-handling path.
     */
    suspend fun reportError(device: SwarmDevice, report: ClientErrorReport, clientCertificate: X509Certificate, clientKey: PrivateKey) {
        val connection = connectionFor(device, clientCertificate, clientKey) ?: return
        runCatching { connection.request(path = "/errors/report", errorReport = report) }
    }

    /**
     * Sends a like/unlike toggle to [device] — see
     * `swarm_core::peer::LikeToggle`'s doc comment. Best-effort and silent
     * on failure, same posture as [reportError]/[stopPlayback]: the local
     * liked-state (see [app.swarm.tv.app.data.AndroidLikedEntriesStore])
     * already updated optimistically before this is called, so a dropped
     * request just means the server-side aggregate count/dashboard listing
     * lags until the next successful toggle — never blocks or reverts the
     * client's own UI.
     */
    suspend fun toggleLike(device: SwarmDevice, like: LikeToggle, clientCertificate: X509Certificate, clientKey: PrivateKey) {
        val connection = connectionFor(device, clientCertificate, clientKey) ?: return
        runCatching { connection.request(path = "/likes/toggle", like = like) }
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

    /** Lightweight authenticated reachability check used by the dashboard's Resync action. */
    suspend fun probe(device: SwarmDevice, clientCertificate: X509Certificate, clientKey: PrivateKey): Boolean {
        repeat(2) {
            val connection = connectionFor(device, clientCertificate, clientKey) ?: return@repeat
            val reachable = runCatching {
                val response = connection.request("/catalog/thumbprint")
                response.body.readBytes()
                response.header.status == 200
            }.getOrDefault(false)
            if (reachable) return true
            connections.remove(device.deviceId)
            runCatching { connection.close() }
        }
        return false
    }

    /**
     * Resolves (connecting fresh if necessary) the raw [PeerQuicClient] for
     * [device] — used directly by [preparePlayback]/[stopPlayback]/
     * [reportError]/[refresh], each of which already has its own
     * deliberate, narrowly-scoped retry-once-on-failure logic for the one
     * request it's making. [ReconnectingConnection] is the *other* consumer
     * of this method: registered once with [proxy] the first time a device
     * connects, it calls back in here on every failure so anything routed
     * through the loopback proxy (chiefly artwork — many concurrent,
     * unattended fetches with no caller left to notice and retry) gets the
     * same self-healing without each of *those* call sites needing to know
     * anything about reconnection.
     */
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
        // Registered once, ever, per device — a reconnect after this
        // (this same branch, later, once `connections[device.deviceId]` has
        // been cleared by a failure elsewhere) must not register a second
        // wrapper on top of the still-live first one; the wrapper always
        // looks up the *current* raw connection dynamically at request
        // time, so it never goes stale on its own.
        if (proxyRegisteredDevices.add(device.deviceId)) {
            proxy.register(device.deviceId, ReconnectingConnection(device, clientCertificate, clientKey))
        }
        return connection
    }

    /**
     * A [PeerConnection] that transparently reconnects through
     * [connectionFor] on failure instead of just failing — see
     * [connectionFor]'s doc comment for why this exists (artwork fetches
     * over the loopback proxy have no caller left to notice a dead
     * connection and retry the way [preparePlayback]/[refresh] do for
     * themselves). `runBlocking` is safe here: [PeerConnection.request] is
     * called from [PeerLoopbackProxy]'s own cached-thread-pool worker
     * threads, never the caller's main/UI thread.
     */
    private inner class ReconnectingConnection(
        private val device: SwarmDevice,
        private val clientCertificate: X509Certificate,
        private val clientKey: PrivateKey,
    ) : PeerConnection {
        override fun request(path: String, range: ByteRange?, ifNoneMatch: String?, playback: PlaybackPreferences?, errorReport: ClientErrorReport?, like: LikeToggle?): PeerResponse {
            val current = connections[device.deviceId]
                ?: runBlocking { connectionFor(device, clientCertificate, clientKey) }
                ?: throw IOException("server is no longer connected")
            return try {
                current.request(path, range, ifNoneMatch, playback, errorReport, like)
            } catch (e: IOException) {
                connections.remove(device.deviceId)
                val fresh = runBlocking { connectionFor(device, clientCertificate, clientKey) } ?: throw e
                fresh.request(path, range, ifNoneMatch, playback, errorReport, like)
            }
        }
    }

    private fun fetchManifest(serverId: String, connection: PeerQuicClient): CatalogManifest? {
        val manifest = runCatching {
            val response = connection.request("/catalog/manifest")
            SwarmJson.decodeFromString<CatalogManifest>(response.body.readBytes().decodeToString())
        }.onFailure { it.printStackTrace() }.getOrNull()
        if (manifest == null) {
            // Stale connection (peer restarted, network dropped) — drop the
            // raw connection so the next refresh (or a ReconnectingConnection
            // handling an artwork fetch in the meantime) reconnects. The
            // proxy registration itself stays — see ReconnectingConnection's
            // doc comment.
            connections.remove(serverId)
            runCatching { connection.close() }
        }
        return manifest
    }

    override fun close() {
        // Every device that ever got a ReconnectingConnection registered,
        // not just connections.keys — a device whose raw connection died
        // and was evicted (awaiting self-heal on next use) would otherwise
        // keep its wrapper registered with the proxy past this session's end.
        proxyRegisteredDevices.forEach(proxy::unregister)
        proxyRegisteredDevices.clear()
        connections.values.forEach { runCatching { it.close() } }
        connections.clear()
    }
}
