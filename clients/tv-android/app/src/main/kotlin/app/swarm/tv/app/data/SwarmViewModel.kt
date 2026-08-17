package app.swarm.tv.app.data

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import app.swarm.tv.core.catalog.CatalogSession
import app.swarm.tv.core.catalog.MergedEntry
import app.swarm.tv.core.catalog.PunchFallback
import app.swarm.tv.core.client.SignalingClient
import app.swarm.tv.core.client.StunApiClient
import app.swarm.tv.core.client.StunClientError
import app.swarm.tv.core.peer.MediaKind
import app.swarm.tv.core.proxy.PeerLoopbackProxy
import app.swarm.tv.core.rest.DeviceRegistration
import app.swarm.tv.core.rest.DeviceType
import app.swarm.tv.core.rest.SwarmDevice
import app.swarm.tv.core.rest.SwarmSummary
import app.swarm.tv.core.token.TokenStore
import java.net.InetAddress
import java.net.InetSocketAddress
import java.security.PrivateKey
import java.security.cert.X509Certificate
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

sealed class UiState {
    data object PasscodeEntry : UiState()
    data object Registering : UiState()
    data class Dashboard(val swarm: SwarmSummary, val devices: List<SwarmDevice>, val resyncing: Boolean = false) : UiState()
    data class Catalog(
        val swarm: SwarmSummary,
        val devices: List<SwarmDevice>,
        val entries: List<MergedEntry> = emptyList(),
        val loading: Boolean = true,
        val unreachable: List<SwarmDevice> = emptyList(),
    ) : UiState()
    data class Player(val url: String, val title: String, val previous: Catalog) : UiState()
    data class Error(val message: String) : UiState()
}

/**
 * Onboarding, swarm-dashboard, merged-catalog, and playback state.
 * Registration is fully wired (real network calls against a real STUN
 * server, real encrypted token storage, a real device identity
 * fingerprint). Right after registration, also opens a [SignalingClient]
 * session and wires it into [catalogSession] as a [PunchFallback] —
 * best-effort, same as `ServerCore`'s `establish_signaling` (Rust): a
 * device with no working signaling session still reaches LAN servers via
 * `peer_addr` fine, it just can't fall back to a hole-punched connection
 * for one that isn't directly reachable. [browseCatalog] connects to every
 * server in the roster (direct first, punched fallback second — see
 * `CatalogSession`) and merges their catalogs; [play] streams the chosen
 * entry through the same session's loopback proxy. `clientCertificate`/
 * `clientKey` are this device's own identity — `AndroidDeviceIdentity`'s
 * `AndroidKeyStore`-backed cert/key on the shipped app, an in-memory one in
 * tests — the mTLS credential a peer's `RosterClientVerifier` checks
 * against the swarm roster it already trusts.
 */
class SwarmViewModel(
    private val tokenStore: TokenStore,
    private val machineId: String,
    private val certFingerprint: String,
    private val clientCertificate: X509Certificate,
    private val clientKey: PrivateKey,
) : ViewModel() {
    private val _state = MutableStateFlow<UiState>(UiState.PasscodeEntry)
    val state: StateFlow<UiState> = _state.asStateFlow()

    private var client: StunApiClient? = null
    private var accessToken: String? = null
    private var swarmId: String? = null
    private var signaling: SignalingClient? = null

    private val proxy = PeerLoopbackProxy.start()
    private val catalogSession = CatalogSession(proxy)

    fun submitPasscode(baseUrl: String, code: String, deviceName: String) {
        val trimmedBaseUrl = baseUrl.trim()
        val trimmedCode = code.trim()
        if (trimmedBaseUrl.isEmpty() || trimmedCode.length != 8) {
            _state.value = UiState.Error("Enter the STUN server URL and the 8-digit join code.")
            return
        }
        viewModelScope.launch {
            _state.value = UiState.Registering
            val api = StunApiClient(trimmedBaseUrl)
            try {
                val response = api.registerDevice(
                    trimmedCode,
                    DeviceRegistration(
                        name = deviceName.ifBlank { "Fire TV" },
                        deviceType = DeviceType.CLIENT,
                        machineId = machineId,
                        certFingerprint = certFingerprint,
                        platform = "android-tv",
                        appVersion = "0.1.0",
                    ),
                )
                tokenStore.save(response.accessToken)
                client = api
                accessToken = response.accessToken
                swarmId = response.swarm.id
                establishSignaling(trimmedBaseUrl, response.accessToken, response.deviceId)
                loadRoster()
            } catch (e: StunClientError) {
                _state.value = UiState.Error(e.message ?: "Could not join that swarm.")
            }
        }
    }

    fun resync() {
        val current = _state.value
        if (current !is UiState.Dashboard) return
        _state.value = current.copy(resyncing = true)
        viewModelScope.launch { loadRoster() }
    }

    fun dismissError() {
        _state.value = UiState.PasscodeEntry
    }

    /** Connects to every reachable server in the roster and merges their catalogs — see [CatalogSession]. */
    fun browseCatalog() {
        val current = _state.value
        if (current !is UiState.Dashboard) return
        _state.value = UiState.Catalog(current.swarm, current.devices, loading = true)
        viewModelScope.launch {
            // CatalogSession.refresh blocks on real network I/O (connect + one
            // request per server) — never run it on the Main dispatcher.
            val result = withContext(Dispatchers.IO) {
                catalogSession.refresh(current.devices, clientCertificate, clientKey)
            }
            val stateNow = _state.value
            if (stateNow is UiState.Catalog) {
                _state.value = stateNow.copy(entries = result.entries, loading = false, unreachable = result.unreachable)
            }
        }
    }

    /**
     * The proxy URL for [entry]'s artwork, or null if it never scraped any
     * (`artworkEtag == null` — no point spending a request finding that
     * out). Movies/episodes use the `poster` kind, tracks `cover` — the
     * scraper (`swarm-media`) always writes exactly one of those per kind
     * of entry, per `docs/PROTOCOL.md`'s artwork section.
     */
    fun artworkUrl(entry: MergedEntry): String? {
        if (entry.entry.artworkEtag == null) return null
        val kind = if (entry.entry.kind == MediaKind.TRACK) "cover" else "poster"
        return catalogSession.urlFor(entry.sources.first(), "/art/${entry.entry.entryKey}/$kind")
    }

    /** Streams [entry] from whichever of its sources [CatalogSession] already holds a connection to. */
    fun play(entry: MergedEntry) {
        val current = _state.value
        if (current !is UiState.Catalog) return
        val serverId = entry.sources.first()
        val url = catalogSession.urlFor(serverId, "/media/${entry.entry.entryKey}")
        _state.value = UiState.Player(url, entry.entry.scrapedTitle ?: entry.entry.title, current)
    }

    fun stopPlayback() {
        val current = _state.value
        if (current is UiState.Player) _state.value = current.previous
    }

    fun backToDashboard() {
        val current = _state.value
        if (current is UiState.Catalog) _state.value = UiState.Dashboard(current.swarm, current.devices)
    }

    /**
     * Opens a signaling session and, if that succeeds, resolves the
     * reflector's address and wires [catalogSession]'s [PunchFallback].
     * Best-effort and never fatal — see the class doc.
     */
    private suspend fun establishSignaling(baseUrl: String, accessToken: String, deviceId: String) {
        val (signalingClient, signalRx) = try {
            SignalingClient.connect(baseUrl, accessToken, deviceId)
        } catch (e: Exception) {
            return
        }
        val reflectorAddr = resolveReflectorAddr(baseUrl, signalingClient.reflectorPorts)
        if (reflectorAddr == null) {
            signalingClient.close()
            return
        }
        signaling = signalingClient
        catalogSession.punchFallback = PunchFallback(signalingClient, signalRx, reflectorAddr, certFingerprint)
    }

    /** The reflector runs inside the STUN server process, so its address is the STUN host plus whichever port `hello_ack` advertised as live. */
    private suspend fun resolveReflectorAddr(baseUrl: String, reflectorPorts: List<Int>): InetSocketAddress? {
        val port = reflectorPorts.firstOrNull() ?: return null
        val host = baseUrl.removePrefix("https://").removePrefix("http://").substringBefore('/').substringBefore(':')
        return withContext(Dispatchers.IO) {
            runCatching { InetSocketAddress(InetAddress.getByName(host), port) }.getOrNull()
        }
    }

    private suspend fun loadRoster() {
        val api = client ?: return
        val token = accessToken ?: return
        val id = swarmId ?: return
        try {
            val roster = api.swarmDevices(token, id)
            _state.value = UiState.Dashboard(roster.swarm, roster.devices)
        } catch (e: StunClientError) {
            _state.value = UiState.Error(e.message ?: "Could not load the swarm roster.")
        }
    }

    override fun onCleared() {
        signaling?.close()
        catalogSession.close()
        proxy.close()
    }
}
