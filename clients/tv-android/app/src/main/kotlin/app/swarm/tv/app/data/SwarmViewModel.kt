package app.swarm.tv.app.data

import android.util.Log
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import app.swarm.tv.core.catalog.ArtistGroup
import app.swarm.tv.core.catalog.CatalogGrouping
import app.swarm.tv.core.catalog.CatalogSession
import app.swarm.tv.core.catalog.MergedEntry
import app.swarm.tv.core.catalog.PunchFallback
import app.swarm.tv.core.catalog.ShowGroup
import app.swarm.tv.core.client.SignalingClient
import app.swarm.tv.core.client.StunApiClient
import app.swarm.tv.core.client.StunClientError
import app.swarm.tv.core.peer.ClientErrorReport
import app.swarm.tv.core.peer.LikeToggle
import app.swarm.tv.core.peer.MediaKind
import app.swarm.tv.core.peer.PlaybackMode
import app.swarm.tv.core.proxy.PeerLoopbackProxy
import app.swarm.tv.core.rest.DeviceRegistration
import app.swarm.tv.core.rest.DeviceType
import app.swarm.tv.core.rest.SwarmDevice
import app.swarm.tv.core.rest.SwarmSummary
import app.swarm.tv.core.token.TokenStore
import app.swarm.tv.core.watch.WatchState
import app.swarm.tv.core.watch.WatchStateStore
import java.net.InetAddress
import java.net.InetSocketAddress
import java.security.PrivateKey
import java.security.cert.X509Certificate
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

sealed class UiState {
    /**
     * The real initial state — shown only until [SwarmViewModel.restoreSession]
     * decides whether there's a saved session to resume. Real bug, found
     * live: this used to start straight at [PasscodeEntry] and only switch
     * away from it once a *successful* restore completed, so a real device
     * with a fully saved session still showed the "enter STUN URL/passcode"
     * screen for however long establishSignaling()+loadRoster()'s network
     * round trips took (a few real seconds) before snapping to Dashboard —
     * indistinguishable from a genuine first-run/signed-out device the
     * whole time it was showing. [Loading] is what's on screen during that
     * gap instead; [PasscodeEntry] is now reached only once restoration has
     * actually concluded there's nothing to resume.
     */
    data object Loading : UiState()
    data object PasscodeEntry : UiState()
    data object Registering : UiState()
    data class Dashboard(
        val swarm: SwarmSummary,
        val devices: List<SwarmDevice>,
        val resyncing: Boolean = false,
        val allSwarms: List<SwarmSummary> = emptyList(),
    ) : UiState()
    /** [activeSwarmId] is null only once every swarm has been left — the device is still registered, just not a member of anything yet. */
    data class Settings(
        val allSwarms: List<SwarmSummary>,
        val activeSwarmId: String?,
        val baseUrl: String,
        val deviceName: String,
        val artworkCacheMinutes: Int,
        val busy: Boolean = false,
        val error: String? = null,
        /** Every distinct genre currently in the last-browsed catalog — backs Kid Mode's genre allow-list picker. Empty until [browseCatalog] has actually run once (Settings is only ever reached from [Dashboard], before that) — the picker degrades to "no genre restriction available yet" rather than erroring. */
        val availableGenres: List<String> = emptyList(),
    ) : UiState()
    data class Catalog(
        val swarm: SwarmSummary,
        val devices: List<SwarmDevice>,
        val entries: List<MergedEntry> = emptyList(),
        val loading: Boolean = true,
        val unreachable: List<SwarmDevice> = emptyList(),
        val playbackError: String? = null,
    ) : UiState()
    /** Music: Music row -> here (grouped, replacing the old flat-track shelf) -> [ArtistAlbums]. */
    data class ArtistShelf(val catalog: Catalog, val artists: List<ArtistGroup>) : UiState()
    /** One artist's albums; [AlbumScreen] handles the album-grid<->track-list sub-navigation locally. */
    data class ArtistAlbums(val catalog: Catalog, val artists: List<ArtistGroup>, val artist: ArtistGroup) : UiState()
    /** Movies: Movies row -> here (all movies, "Browse all") -> [MovieDetail]. */
    data class MovieShelf(val catalog: Catalog, val movies: List<MergedEntry>) : UiState()
    /** Movies: Movies row or [MovieShelf] -> here (detail before play) -> [Player]. [previous] is whichever of those it was opened from, so Back returns to the right one — same reasoning as [Player.previous]. */
    data class MovieDetail(val previous: UiState, val entry: MergedEntry) : UiState()
    /** Shows: Shows row -> here (grouped, replacing the old flat-episode shelf) -> [ShowSeasons]. */
    data class ShowShelf(val catalog: Catalog, val shows: List<ShowGroup>) : UiState()
    /** One show's seasons; [SeasonScreen] handles the season-list<->episode-grid sub-navigation locally. */
    data class ShowSeasons(val catalog: Catalog, val shows: List<ShowGroup>, val show: ShowGroup) : UiState()
    data class Player(
        val url: String,
        val title: String,
        val fingerprint: String,
        val resumePositionSecs: Double,
        val positionOffsetSecs: Double,
        val maxBitrate: Long,
        val mediaDurationSecs: Double?,
        /** The entry actually playing — needed by [nextEpisode]-driven Continue/autoplay. */
        val entry: MergedEntry,
        /** Precomputed at negotiation time: the next episode if [entry] is an Episode and one follows it, else null. */
        val nextEntry: MergedEntry?,
        /**
         * The exact screen [play] was called from — [Catalog] if played
         * from the flat shelf, or the originating [MovieDetail]/
         * [ArtistAlbums]/[ShowSeasons] otherwise, so Back returns to where
         * the user actually was instead of always the top-level catalog.
         * Real bug, found live: this used to be typed as plain [Catalog]
         * (every `play` call site unwrapped its own nested state down to
         * just the embedded catalog before storing it here), so leaving
         * playback from three levels deep in Show/Artist browsing always
         * dropped the user back at the flat browse page. See
         * [embeddedCatalog] for how the pieces that still need a plain
         * [Catalog] (device lookup, next-episode lookup) get it back out.
         */
        val previous: UiState,
        /** Which server negotiated [sessionId] — both needed to release its bandwidth reservation on exit, see [SwarmViewModel.releasePlaybackSession]. */
        val serverId: String,
        val sessionId: String,
    ) : UiState()
    data class Error(val message: String) : UiState()
}

/** The [UiState.Catalog] embedded in any screen built on top of it — every browse/detail state carries one. */
private fun UiState.embeddedCatalog(): UiState.Catalog? = when (this) {
    is UiState.Catalog -> this
    is UiState.ArtistAlbums -> catalog
    is UiState.MovieShelf -> catalog
    is UiState.MovieDetail -> previous.embeddedCatalog()
    is UiState.ShowSeasons -> catalog
    else -> null
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
 * against the swarm roster it already trusts. [watchStateStore] backs
 * resume/watched state, keyed by each entry's cross-server fingerprint —
 * see [play] and [savePlaybackPosition].
 */
class SwarmViewModel(
    private val tokenStore: TokenStore,
    private val machineId: String,
    private val certFingerprint: String,
    private val clientCertificate: X509Certificate,
    private val clientKey: PrivateKey,
    private val watchStateStore: WatchStateStore,
    private val connectionStore: AndroidConnectionStore,
    private val settingsStore: AndroidAppSettingsStore,
    private val likedEntriesStore: AndroidLikedEntriesStore,
    private val kidModeStore: AndroidKidModeStore,
    private val lanDiscovery: LanDiscoveryManager,
    private val lanConnectionStore: AndroidLanConnectionStore,
) : ViewModel() {
    private val _state = MutableStateFlow<UiState>(UiState.Loading)
    private val logTag = "SwarmViewModel"
    val state: StateFlow<UiState> = _state.asStateFlow()

    val lanServers: StateFlow<List<LanServer>> = lanDiscovery.servers
    private val _lanPairingBusy = MutableStateFlow(false)
    val lanPairingBusy: StateFlow<Boolean> = _lanPairingBusy.asStateFlow()
    private val _lanError = MutableStateFlow<String?>(null)
    val lanError: StateFlow<String?> = _lanError.asStateFlow()

    /** In-memory mirror of [likedEntriesStore], loaded once in [init] — see that store's own doc comment for why the UI never reads through it directly. */
    private val _likedFingerprints = MutableStateFlow<Set<String>>(emptySet())
    val likedFingerprints: StateFlow<Set<String>> = _likedFingerprints.asStateFlow()

    /** Loaded once in [init], refreshed on every [enableKidMode]/[updateKidModeRules]/[disableKidMode] — [browseCatalog] filters through this every time it applies a fresh manifest, see that function. */
    private val _kidModeSettings = MutableStateFlow<KidModeSettings?>(null)
    val kidModeSettings: StateFlow<KidModeSettings?> = _kidModeSettings.asStateFlow()

    /** The last successfully-browsed catalog's genres — see [UiState.Settings.availableGenres]'s doc comment for why this can't just be read off [UiState.Settings] itself. */
    private var lastKnownGenres: List<String> = emptyList()

    /** Music-only "keep playing but pick something else" mode — see [CatalogGrouping.nextTrack] and [toggleShuffle]. */
    private val _shuffleEnabled = MutableStateFlow(false)
    val shuffleEnabled: StateFlow<Boolean> = _shuffleEnabled.asStateFlow()

    /** Non-null exactly when a track is playing in the background after [minimizePlayback] — see [activePlayerSession]. */
    private val _minimizedPlayer = MutableStateFlow<UiState.Player?>(null)
    val minimizedPlayer: StateFlow<UiState.Player?> = _minimizedPlayer.asStateFlow()

    private var client: StunApiClient? = null
    private var accessToken: String? = null
    private var deviceId: String? = null
    private var swarmId: String? = null
    private var signaling: SignalingClient? = null
    /** In-memory for the running session, same as [swarmId]/[accessToken] — see [AndroidConnectionStore]'s doc comment. */
    private var cachedSwarms: List<SwarmSummary> = emptyList()
    /** Set on a successful registration or a restored session; re-sent to [connectionStore] on every edit from the config page. */
    private var baseUrl: String? = null
    private var deviceName: String? = null
    private var localSession = false
    private var activeLocalServer: LanServer? = null

    private val proxy = PeerLoopbackProxy.start()
    private val catalogSession = CatalogSession(proxy)

    init {
        lanDiscovery.start()
        viewModelScope.launch { restoreSession() }
        viewModelScope.launch {
            lanDiscovery.servers.collect { discovered -> refreshActiveLocalServer(discovered) }
        }
        viewModelScope.launch { _likedFingerprints.value = likedEntriesStore.loadAll() }
        viewModelScope.launch { _kidModeSettings.value = kidModeStore.get() }
    }

    /**
     * Cold-start session resume, run once from [init] while [UiState.Loading]
     * is showing. Falls back to [UiState.PasscodeEntry] for a genuinely
     * first-ever launch only after trying the two persisted connection modes
     * in priority order: a complete STUN session first, then the most recently
     * authenticated LAN server. Both resume directly to [Dashboard]; the
     * landing screen appears only when neither exists.
     */
    private suspend fun restoreSession() {
        val token = tokenStore.load()
        val saved = token?.let { connectionStore.get() }
        val savedActiveSwarm = saved?.activeSwarmId?.let { activeId -> saved.swarms.find { it.id == activeId } }
        if (token != null && saved != null && savedActiveSwarm != null) {
            localSession = false
            activeLocalServer = null
            accessToken = token
            deviceId = saved.deviceId
            swarmId = saved.activeSwarmId
            baseUrl = saved.baseUrl
            deviceName = saved.deviceName
            cachedSwarms = saved.swarms
            client = StunApiClient(saved.baseUrl)
            _state.value = UiState.Dashboard(savedActiveSwarm, emptyList(), resyncing = true, allSwarms = saved.swarms)
            establishSignaling(saved.baseUrl, token, saved.deviceId)
            loadRoster()
            return
        }

        val local = lanConnectionStore.mostRecent()
        if (local != null) {
            showLocalDashboard(local.server, local.deviceName)
            return
        }
        _state.value = UiState.PasscodeEntry
    }

    fun submitPasscode(baseUrl: String, code: String, deviceName: String) {
        val trimmedBaseUrl = baseUrl.trim()
        val trimmedCode = code.trim()
        if (trimmedBaseUrl.isEmpty() || trimmedCode.length != 8) {
            _state.value = UiState.Error("Enter the STUN server URL and the 8-digit join code.")
            return
        }
        val trimmedDeviceName = deviceName.ifBlank { "Fire TV" }
        viewModelScope.launch {
            _state.value = UiState.Registering
            val api = StunApiClient(trimmedBaseUrl)
            try {
                val response = api.registerDevice(
                    trimmedCode,
                    DeviceRegistration(
                        name = trimmedDeviceName,
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
                deviceId = response.deviceId
                swarmId = response.swarm.id
                this@SwarmViewModel.baseUrl = trimmedBaseUrl
                this@SwarmViewModel.deviceName = trimmedDeviceName
                cachedSwarms = listOf(response.swarm)
                viewModelScope.launch { connectionStore.saveNewConnection(trimmedBaseUrl, trimmedDeviceName, response.deviceId, response.swarm) }
                establishSignaling(trimmedBaseUrl, response.accessToken, response.deviceId)
                loadRoster()
            } catch (e: StunClientError) {
                _state.value = UiState.Error(e.message ?: "Could not join that swarm.")
            }
        }
    }

    /**
     * Connects directly to a discovered server. This succeeds without a
     * code when the server already trusts this client's certificate; a new
     * client is rejected by the existing mutual-TLS verifier and can then
     * use [pairLanServer] once to establish that trust.
     */
    fun connectLanServer(server: LanServer, deviceName: String) {
        viewModelScope.launch { connectLanServerNow(server, deviceName) }
    }

    fun pairLanServer(server: LanServer, code: String, deviceName: String) {
        val trimmedCode = code.trim()
        if (trimmedCode.length != 6) {
            _lanError.value = "Enter the 6-digit LAN pairing code shown by the media server."
            return
        }
        val trimmedName = deviceName.ifBlank { "Fire TV" }
        viewModelScope.launch {
            _lanPairingBusy.value = true
            _lanError.value = null
            val paired = lanDiscovery.pair(server, trimmedCode, trimmedName, certFingerprint)
            if (paired.isFailure) {
                _lanPairingBusy.value = false
                _lanError.value = paired.exceptionOrNull()?.message ?: "Could not pair with the media server."
                return@launch
            }
            connectLanServerNow(server, trimmedName)
        }
    }

    private suspend fun connectLanServerNow(server: LanServer, clientName: String) {
        _lanPairingBusy.value = true
        _lanError.value = null
        val name = clientName.ifBlank { "Fire TV" }
        val device = server.asSwarmDevice()
        val swarm = SwarmSummary("lan", "Local network")
        val result = withTimeoutOrNull(15_000) {
            withContext(Dispatchers.IO) {
                catalogSession.refresh(listOf(device), clientCertificate, clientKey)
            }
        }
        _lanPairingBusy.value = false
        if (result == null || result.unreachable.isNotEmpty()) {
            _lanError.value = "Could not connect securely. If this is the first connection, open LAN pairing on the media server and enter its code."
            return
        }
        localSession = true
        activeLocalServer = server
        deviceId = deviceId ?: "lan-client-${certFingerprint.take(16)}"
        deviceName = name
        lastKnownGenres = result.entries.flatMap { it.entry.genres }.distinct().sortedWith(String.CASE_INSENSITIVE_ORDER)
        lanConnectionStore.save(server, name)
        _state.value = UiState.Dashboard(swarm = swarm, devices = listOf(device))
    }

    private fun showLocalDashboard(server: LanServer, clientName: String) {
        localSession = true
        activeLocalServer = server
        deviceId = deviceId ?: "lan-client-${certFingerprint.take(16)}"
        deviceName = clientName
        _state.value = UiState.Dashboard(
            swarm = SwarmSummary("lan", "Local network"),
            devices = listOf(server.asSwarmDevice()),
        )
    }

    /** Refresh cached address data when the persisted server is rediscovered after DHCP or network changes. */
    private fun refreshActiveLocalServer(discovered: List<LanServer>) {
        if (!localSession) return
        val current = activeLocalServer ?: return
        val refreshed = discovered.firstOrNull { it.certFingerprint == current.certFingerprint } ?: return
        activeLocalServer = refreshed
        val stateNow = _state.value
        if (stateNow is UiState.Dashboard && stateNow.swarm.id == "lan") {
            _state.value = stateNow.copy(devices = listOf(refreshed.asSwarmDevice()))
        }
        viewModelScope.launch { lanConnectionStore.save(refreshed, deviceName ?: "Fire TV") }
    }

    fun resync() {
        val current = _state.value
        Log.i(logTag, "resync() called, current state=${current::class.simpleName}")
        if (current !is UiState.Dashboard) return
        _state.value = current.copy(resyncing = true)
        if (localSession) {
            val server = activeLocalServer ?: run {
                _state.value = current.copy(resyncing = false)
                return
            }
            viewModelScope.launch {
                val reachable = withTimeoutOrNull(10_000) {
                    withContext(Dispatchers.IO) {
                        catalogSession.probe(server.asSwarmDevice(), clientCertificate, clientKey)
                    }
                } ?: false
                val stateNow = _state.value
                if (stateNow is UiState.Dashboard && stateNow.swarm.id == "lan") {
                    _state.value = stateNow.copy(
                        devices = listOf(server.asSwarmDevice().copy(online = reachable)),
                        resyncing = false,
                    )
                }
            }
            return
        }
        viewModelScope.launch { loadRoster() }
    }

    fun dismissError() {
        _state.value = UiState.PasscodeEntry
    }

    fun openSettings() {
        val current = _state.value
        if (current !is UiState.Dashboard) return
        viewModelScope.launch {
            val artworkCacheMinutes = settingsStore.getArtworkCacheMinutes()
            _state.value = UiState.Settings(
                allSwarms = current.allSwarms,
                activeSwarmId = current.swarm.id,
                baseUrl = baseUrl.orEmpty(),
                deviceName = deviceName.orEmpty(),
                artworkCacheMinutes = artworkCacheMinutes,
                availableGenres = lastKnownGenres,
            )
        }
    }

    /** Config-page edit: where this device connects next — see [AndroidConnectionStore.updateBaseUrl]. */
    fun updateBaseUrl(newBaseUrl: String) {
        val current = _state.value
        if (current !is UiState.Settings) return
        val trimmed = newBaseUrl.trim()
        if (trimmed.isEmpty()) {
            _state.value = current.copy(error = "Enter a STUN server URL.")
            return
        }
        baseUrl = trimmed
        _state.value = current.copy(baseUrl = trimmed, error = null)
        viewModelScope.launch { connectionStore.updateBaseUrl(trimmed) }
    }

    /** Config-page edit: the locally-remembered device label — see [AndroidConnectionStore.updateDeviceName] for why this never renames the device on the server. */
    fun updateDeviceName(newName: String) {
        val current = _state.value
        if (current !is UiState.Settings) return
        val trimmed = newName.ifBlank { "Fire TV" }
        deviceName = trimmed
        _state.value = current.copy(deviceName = trimmed, error = null)
        viewModelScope.launch { connectionStore.updateDeviceName(trimmed) }
    }

    /** Config-page edit: how long Coil trusts a cached artwork image before re-fetching — see [app.swarm.tv.app.ui.ArtworkCache]. */
    fun updateArtworkCacheMinutes(minutes: Int) {
        val current = _state.value
        if (current !is UiState.Settings) return
        val clamped = minutes.coerceIn(0, 1440)
        _state.value = current.copy(artworkCacheMinutes = clamped, error = null)
        viewModelScope.launch { settingsStore.setArtworkCacheMinutes(clamped) }
    }

    /** Redeems an additional join code against this device's existing STUN session — same server, a new swarm on it. */
    fun joinAdditionalSwarm(code: String) {
        val current = _state.value
        if (current !is UiState.Settings) return
        val api = client ?: return
        val token = accessToken ?: return
        val trimmedCode = code.trim()
        if (trimmedCode.length != 8) {
            _state.value = current.copy(error = "Enter the 8-digit join code.")
            return
        }
        _state.value = current.copy(busy = true, error = null)
        viewModelScope.launch {
            try {
                val joined = withContext(Dispatchers.IO) { api.joinSwarm(token, trimmedCode) }
                cachedSwarms = (cachedSwarms + joined).distinctBy { it.id }
                connectionStore.updateSwarms(cachedSwarms, swarmId)
                val stateNow = _state.value
                if (stateNow is UiState.Settings) {
                    _state.value = stateNow.copy(allSwarms = cachedSwarms, busy = false)
                }
            } catch (e: StunClientError) {
                val stateNow = _state.value
                if (stateNow is UiState.Settings) {
                    _state.value = stateNow.copy(busy = false, error = e.message ?: "Could not join that swarm.")
                }
            }
        }
    }

    /**
     * Leaves one swarm, keeping this device's registration and its other
     * memberships intact. If the swarm left was the active one, the first
     * remaining swarm becomes active; if none remain, [Settings.activeSwarmId]
     * goes null — the device stays registered, just in zero swarms, and
     * [backFromSettings] falls back to [UiState.PasscodeEntry] in that case
     * rather than a broken Dashboard with nothing to show.
     */
    fun leaveSwarm(swarmIdToLeave: String) {
        val current = _state.value
        if (current !is UiState.Settings) return
        val api = client ?: return
        val token = accessToken ?: return
        val device = deviceId ?: return
        _state.value = current.copy(busy = true, error = null)
        viewModelScope.launch {
            try {
                withContext(Dispatchers.IO) { api.leaveSwarm(token, swarmIdToLeave, device) }
                cachedSwarms = cachedSwarms.filterNot { it.id == swarmIdToLeave }
                val newActiveId = if (current.activeSwarmId == swarmIdToLeave) cachedSwarms.firstOrNull()?.id else current.activeSwarmId
                swarmId = newActiveId
                connectionStore.updateSwarms(cachedSwarms, newActiveId)
                val stateNow = _state.value
                if (stateNow is UiState.Settings) {
                    _state.value = stateNow.copy(allSwarms = cachedSwarms, activeSwarmId = newActiveId, busy = false)
                }
            } catch (e: StunClientError) {
                val stateNow = _state.value
                if (stateNow is UiState.Settings) {
                    _state.value = stateNow.copy(busy = false, error = e.message ?: "Could not leave that swarm.")
                }
            }
        }
    }

    fun switchActiveSwarm(newSwarmId: String) {
        val current = _state.value
        if (current !is UiState.Settings) return
        swarmId = newSwarmId
        _state.value = current.copy(activeSwarmId = newSwarmId)
        viewModelScope.launch { connectionStore.setActiveSwarm(newSwarmId) }
    }

    fun backFromSettings() {
        val current = _state.value
        if (current !is UiState.Settings) return
        if (current.activeSwarmId == null) {
            _state.value = UiState.PasscodeEntry
            return
        }
        viewModelScope.launch { loadRoster() }
    }

    /** Connects to every reachable server in the roster and merges their catalogs — see [CatalogSession]. */
    fun browseCatalog() {
        val current = _state.value
        Log.i(logTag, "browseCatalog() called, current state=${current::class.simpleName}")
        if (current !is UiState.Dashboard) return
        _state.value = UiState.Catalog(current.swarm, current.devices, loading = true)
        viewModelScope.launch {
            // CatalogSession.refresh blocks on real network I/O (connect + one
            // request per server) — never run it on the Main dispatcher.
            //
            // Real bug, found live: refresh() itself fails open per-device
            // (a dead peer just lands in Result.unreachable), but nothing
            // ever bounded the *whole* call — PeerQuicClient.request has a
            // connect timeout but no read timeout on the response body, so
            // a manifest fetch that stalls partway through (or is just
            // very slow — a real library can now be thousands of entries,
            // far past anything tested on real hardware before) hung here
            // forever with `loading` never flipping back to false: an
            // infinite spinner with no visible error. `withTimeoutOrNull`
            // turns that into the same "server(s) not reachable" state the
            // UI already renders for a normal per-device failure, rather
            // than a wholly new error path. 30s is a starting estimate,
            // not measured against a real large-catalog transfer — revisit
            // once real hardware timing is known.
            val result = withTimeoutOrNull(30_000) {
                withContext(Dispatchers.IO) {
                    catalogSession.refresh(current.devices, clientCertificate, clientKey)
                }
            }
            if (result == null) {
                Log.w(logTag, "browseCatalog() timed out waiting on ${current.devices.size} device(s)")
            } else {
                Log.i(logTag, "browseCatalog() refresh done: entries=${result.entries.size} unreachable=${result.unreachable.size}")
            }
            val stateNow = _state.value
            if (stateNow is UiState.Catalog) {
                _state.value = if (result != null) {
                    // lastKnownGenres is derived from the *full*, unfiltered
                    // manifest (not what's about to be kept below) — the
                    // Kid Mode rules editor needs to offer every genre in
                    // the real library to restrict, including ones a
                    // currently-active restriction is already hiding, or a
                    // parent could never widen an existing restriction back
                    // out again.
                    lastKnownGenres = result.entries.flatMap { it.entry.genres }.distinct().sortedWith(String.CASE_INSENSITIVE_ORDER)
                    stateNow.copy(entries = applyKidModeFilter(result.entries), loading = false, unreachable = result.unreachable)
                } else {
                    // Same "which devices were actually dialed" filter refresh()
                    // itself applies, so the count shown matches what a
                    // completed (non-timed-out) refresh would have reported.
                    stateNow.copy(loading = false, unreachable = current.devices.filter { it.deviceType != DeviceType.CLIENT })
                }
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

    /** Movie/episode backdrop art for detail screens — best-effort, same gate as [artworkUrl]; a 404 (backdrop never scraped) just fails the image load silently, same as this app's existing artwork handling. */
    fun backdropUrl(entry: MergedEntry): String? {
        if (entry.entry.artworkEtag == null) return null
        return catalogSession.urlFor(entry.sources.first(), "/art/${entry.entry.entryKey}/backdrop")
    }

    /** Track-only fallback visual for the music player when no cover art was scraped — an artist photo reads better as "something to look at" than a blank/placeholder square. Same best-effort gate as [artworkUrl]; a 404 just fails the image load silently. */
    fun artistPhotoUrl(entry: MergedEntry): String? {
        if (entry.entry.artworkEtag == null) return null
        return catalogSession.urlFor(entry.sources.first(), "/art/${entry.entry.entryKey}/artist")
    }

    /**
     * Negotiates a budgeted direct/HLS session on the first connected
     * source, resuming where a previous watch left off unless it was
     * already finished. Callable from the top-level [Catalog] screen or
     * from any of the hierarchical browse/detail screens built on top of
     * it — the exact current screen (not just its embedded [Catalog]) is
     * what Back returns to, see [UiState.Player.previous].
     */
    fun play(entry: MergedEntry) {
        val current = _state.value
        val catalog = current.embeddedCatalog() ?: return
        playEntry(entry, catalog, previousScreen = current)
    }

    /**
     * Finds and plays whatever comes after the *currently active* session's
     * entry — [UiState.Player.entry] if the full player screen is showing,
     * or [minimizedPlayer]'s if music is playing in the background while
     * the user browses elsewhere (see [activePlayerSession]). No-op if
     * neither is active or there's no next entry ([UiState.Player.nextEntry],
     * from [CatalogGrouping.nextEpisode]/[CatalogGrouping.nextTrack]).
     */
    fun playNext() {
        val current = activePlayerSession() ?: return
        val next = current.nextEntry ?: return
        val catalog = current.previous.embeddedCatalog() ?: return
        val wasMinimized = _minimizedPlayer.value != null
        releasePlaybackSession(catalog, current.serverId, current.sessionId)
        // Carries the same `previous` forward (not just `catalog`) so Back
        // after auto-playing into a second, third, ... episode/track still
        // returns to the screen the user actually started browsing from,
        // not somewhere reset to the flat catalog. keepMinimized preserves
        // "still in the background" across the track change instead of
        // popping the full screen back up just because playback advanced.
        playEntry(next, catalog, previousScreen = current.previous, keepMinimized = wasMinimized)
    }

    /** Whichever [UiState.Player] is actually live right now — the full screen's own state, or a session still playing in the background after [minimizePlayback]. At most one is ever non-null. */
    private fun activePlayerSession(): UiState.Player? = (_state.value as? UiState.Player) ?: _minimizedPlayer.value

    /** ExoPlayer's own `STATE_ENDED` callback for a track (see the hoisted player in [app.swarm.tv.app.MainActivity]'s `SwarmApp`) — advances to [UiState.Player.nextEntry] if there is one, same as pressing "Play now" would on the episode Continue prompt, but immediate and with no prompt (an unbroken "keep playing" queue is the expected music-player behavior, not a per-track confirmation). Does nothing when there's no next track, same as this app's existing end-of-content behavior for episodes: the player just sits at the ended state until the user backs out. */
    fun onTrackPlaybackEnded() {
        if (activePlayerSession()?.nextEntry != null) playNext()
    }

    /** Music only — movies/episodes have no shuffle concept and never call this. Flips the flag and, if a track is currently active (full-screen or minimized), immediately recomputes its `nextEntry` under the new mode so the "what's up next" the UI reflects updates right away rather than only on the *next* track change. */
    fun toggleShuffle() {
        _shuffleEnabled.value = !_shuffleEnabled.value
        val current = activePlayerSession() ?: return
        if (current.entry.entry.kind != MediaKind.TRACK) return
        val catalog = current.previous.embeddedCatalog() ?: return
        val next = CatalogGrouping.nextTrack(current.entry, CatalogGrouping.groupTracksByArtistAlbum(catalog.entries), _shuffleEnabled.value)
        val updated = current.copy(nextEntry = next)
        if (_minimizedPlayer.value != null) _minimizedPlayer.value = updated else _state.value = updated
    }

    /**
     * "Minimize to tray": leaves the full music player screen for whatever
     * screen it was opened from (same navigation [stopPlayback] would land
     * on), but — unlike [stopPlayback] — keeps the session alive in
     * [minimizedPlayer] instead of releasing it, so the hoisted player in
     * `SwarmApp` keeps playing in the background while the user browses.
     * No-op for anything that isn't a track — movies/episodes have no
     * minimize button in the first place, but this guards the ViewModel
     * method too rather than trusting the UI alone.
     */
    fun minimizePlayback() {
        val current = _state.value
        if (current !is UiState.Player || current.entry.entry.kind != MediaKind.TRACK) return
        _minimizedPlayer.value = current
        _state.value = current.previous
    }

    /** Re-enters the full music player screen for whatever's playing in the background — same session, same ExoPlayer instance (keyed on sessionId in `SwarmApp`), no re-negotiation. */
    fun restoreMinimizedPlayback() {
        val minimized = _minimizedPlayer.value ?: return
        _minimizedPlayer.value = null
        _state.value = minimized
    }

    /** Stops a minimized session entirely (the mini-bar's own stop control) without returning to the full player screen first. */
    fun stopMinimizedPlayback() {
        val minimized = _minimizedPlayer.value ?: return
        _minimizedPlayer.value = null
        minimized.previous.embeddedCatalog()?.let { releasePlaybackSession(it, minimized.serverId, minimized.sessionId) }
    }

    /** Best-effort, fire-and-forget release of a just-finished player's server-side bandwidth reservation — see [CatalogSession.stopPlayback]. */
    private fun releasePlaybackSession(catalog: UiState.Catalog, serverId: String, sessionId: String) {
        val device = catalog.devices.find { it.deviceId == serverId } ?: return
        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    catalogSession.stopPlayback(device, sessionId, clientCertificate, clientKey)
                }
            }.onFailure { Log.w(logTag, "failed to release playback session $sessionId", it) }
        }
    }

    /**
     * Best-effort, fire-and-forget report of a client-observed error back to
     * [device] — see [CatalogSession.reportError]'s doc comment for why
     * failures here are swallowed rather than surfaced. Reports the error to
     * the specific server the failure is about (rather than every reachable
     * server) since that's the one whose swarm page a human would actually
     * be looking at to triage it.
     */
    private fun reportClientError(device: SwarmDevice, message: String, entryKey: String? = null, assetTitle: String? = null, kind: String? = null) {
        val id = deviceId ?: return
        val name = deviceName ?: "Fire TV"
        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    catalogSession.reportError(
                        device,
                        ClientErrorReport(
                            deviceId = id,
                            deviceName = name,
                            entryKey = entryKey,
                            assetTitle = assetTitle,
                            kind = kind,
                            message = message,
                            occurredAtMs = System.currentTimeMillis(),
                        ),
                        clientCertificate,
                        clientKey,
                    )
                }
            }.onFailure { Log.w(logTag, "failed to report client error to ${device.deviceId}", it) }
        }
    }

    /**
     * The actual negotiation, shared by [play] and [playNext]. A failed
     * negotiation always surfaces on the top-level [catalog] screen (same
     * recovery behavior this had before hierarchical browsing existed,
     * just now reachable from more places that navigate through it) —
     * pressing play three levels deep in Artist/Album/Episode browsing and
     * having it fail will visibly return to the flat catalog with an error
     * shown, rather than staying on the nested screen. A known rough edge,
     * not a silent one: acceptable for now since it can't lose any state
     * (browsing this deep is cheap to redo) and matches this codebase's
     * existing single-error-surface convention rather than adding
     * per-screen error UI to every new detail/list screen. [previousScreen]
     * is what a *successful* negotiation's Back button returns to instead —
     * see [UiState.Player.previous].
     */
    private fun playEntry(entry: MergedEntry, catalog: UiState.Catalog, previousScreen: UiState, keepMinimized: Boolean = false) {
        val serverId = entry.sources.first()
        val device = catalog.devices.find { it.deviceId == serverId }
        val fingerprint = entry.entry.fingerprint
        viewModelScope.launch {
            val resumePositionSecs = watchStateStore.get(fingerprint)?.takeUnless { it.watched }?.positionSecs ?: 0.0
            val selection = runCatching {
                requireNotNull(device) { "server no longer in the swarm roster" }
                withContext(Dispatchers.IO) {
                    catalogSession.preparePlayback(
                        device,
                        entry.entry.entryKey,
                        resumePositionSecs.toLong(),
                        clientCertificate,
                        clientKey,
                    )
                }
            }.getOrElse { error ->
                Log.e(logTag, "playback negotiation failed", error)
                _state.value = catalog.copy(playbackError = error.message ?: "Could not prepare playback.")
                if (device != null) {
                    reportClientError(
                        device = device,
                        message = error.message ?: "Could not prepare playback.",
                        entryKey = entry.entry.entryKey,
                        assetTitle = entry.entry.scrapedTitle ?: entry.entry.title,
                        kind = entry.entry.kind.name.lowercase(),
                    )
                }
                return@launch
            }
            val isHls = selection.mode == PlaybackMode.HLS
            val nextEntry = when (entry.entry.kind) {
                MediaKind.EPISODE -> CatalogGrouping.nextEpisode(entry, CatalogGrouping.groupEpisodesByShowSeason(catalog.entries))
                MediaKind.TRACK -> CatalogGrouping.nextTrack(entry, CatalogGrouping.groupTracksByArtistAlbum(catalog.entries), _shuffleEnabled.value)
                MediaKind.MOVIE -> null
            }
            // A stale error only ever lives on a flat Catalog screen (the
            // only state playbackError is ever set on — see this
            // function's own failure branch above) — clear it there so it
            // doesn't reappear on Back after a *successful* play; nothing
            // to clear on the other, richer previousScreen types.
            val cleanedPrevious = if (previousScreen is UiState.Catalog) previousScreen.copy(playbackError = null) else previousScreen
            val playerState = UiState.Player(
                url = selection.url,
                title = entry.entry.scrapedTitle ?: entry.entry.title,
                fingerprint = fingerprint,
                resumePositionSecs = if (isHls) 0.0 else resumePositionSecs,
                positionOffsetSecs = if (isHls) resumePositionSecs else 0.0,
                maxBitrate = selection.maxBitrate,
                mediaDurationSecs = entry.entry.durationSecs,
                entry = entry,
                nextEntry = nextEntry,
                previous = cleanedPrevious,
                serverId = serverId,
                sessionId = selection.sessionId,
            )
            // keepMinimized: an autoplay-to-next-track that started while
            // the mini-bar (not the full screen) was showing stays in the
            // background instead of popping the full player back up just
            // because the track changed underneath it — see [playNext].
            if (keepMinimized) _minimizedPlayer.value = playerState else _state.value = playerState
        }
    }

    /**
     * Reports an ExoPlayer runtime failure (network drop mid-playback, a
     * codec/decoder error, and the like) that happened *after* negotiation
     * already succeeded — distinct from [playEntry]'s own failure branch,
     * which only ever covers the "couldn't even start" case. [PlayerScreen]
     * calls this from its `Player.Listener.onPlayerError`; a no-op if the
     * current screen isn't actually a [UiState.Player] by the time it fires
     * (can race a fast Back-press) or its originating server can no longer
     * be resolved.
     */
    fun reportPlaybackRuntimeError(message: String) {
        val current = _state.value
        if (current !is UiState.Player) return
        val device = current.previous.embeddedCatalog()?.devices?.find { it.deviceId == current.serverId } ?: return
        reportClientError(
            device = device,
            message = message,
            entryKey = current.entry.entry.entryKey,
            assetTitle = current.entry.entry.scrapedTitle ?: current.entry.entry.title,
            kind = current.entry.entry.kind.name.lowercase(),
        )
    }

    /**
     * User-initiated, from a "Report a problem" button on an asset's detail
     * page — distinct from [reportPlaybackRuntimeError] (an automatic report
     * ExoPlayer's own error callback fires) in that this fires whether or
     * not anything actually broke visibly: a user might notice wrong
     * artwork, a mislabeled title, or audio out of sync, none of which
     * throws a [androidx.media3.common.PlaybackException]. Lands in the
     * same place either way — the server's swarm page "Client errors"
     * panel — since triage doesn't care which path found the problem.
     */
    fun reportAssetProblem(entry: MergedEntry) {
        val catalog = _state.value.embeddedCatalog() ?: return
        val device = catalog.devices.find { it.deviceId == entry.sources.first() } ?: return
        reportClientError(
            device = device,
            message = "User reported a problem with this asset from its detail page.",
            entryKey = entry.entry.entryKey,
            assetTitle = entry.entry.scrapedTitle ?: entry.entry.title,
            kind = entry.entry.kind.name.lowercase(),
        )
    }

    /**
     * Toggles [entry]'s liked state — optimistic: [_likedFingerprints] and
     * [likedEntriesStore] both update immediately (instant heart-icon
     * feedback, no round trip on the critical path), then the toggle fires
     * at the first device in [MergedEntry.sources] (same "pick a
     * representative source" pattern [reportAssetProblem] already uses)
     * best-effort — see [CatalogSession.toggleLike]'s doc comment for why a
     * dropped request here never needs to revert the local UI.
     */
    fun toggleLike(entry: MergedEntry) {
        val fingerprint = entry.entry.fingerprint
        val liked = fingerprint !in _likedFingerprints.value
        _likedFingerprints.value = if (liked) _likedFingerprints.value + fingerprint else _likedFingerprints.value - fingerprint
        viewModelScope.launch { likedEntriesStore.setLiked(fingerprint, liked) }

        val catalog = _state.value.embeddedCatalog() ?: return
        val device = catalog.devices.find { it.deviceId == entry.sources.first() } ?: return
        val id = deviceId ?: return
        val name = deviceName ?: "Fire TV"
        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    catalogSession.toggleLike(
                        device,
                        LikeToggle(deviceId = id, deviceName = name, entryKey = entry.entry.entryKey, liked = liked),
                        clientCertificate,
                        clientKey,
                    )
                }
            }.onFailure { Log.w(logTag, "failed to toggle like for ${entry.entry.entryKey}", it) }
        }
    }

    // --- Kid Mode ---

    /** True if [entry] passes the currently-active Kid Mode rules (or Kid Mode isn't on at all) — the one predicate every entry point into [applyKidModeFilter] shares. */
    private fun kidModeAllows(entry: MergedEntry, settings: KidModeSettings): Boolean {
        val e = entry.entry
        if (e.kind !in settings.allowedKinds) return false
        if (settings.allowedGenres != null && e.genres.none { it in settings.allowedGenres }) return false
        return when (e.kind) {
            MediaKind.MOVIE -> RatingScale.isAllowed(e.rating, settings.maxMovieRating, RatingScale.MOVIE_ORDER)
            MediaKind.EPISODE -> RatingScale.isAllowed(e.rating, settings.maxTvRating, RatingScale.TV_ORDER)
            MediaKind.TRACK -> true
        }
    }

    /** No-op (returns [entries] unchanged) when Kid Mode isn't enabled — the single chokepoint [browseCatalog] filters every fresh manifest through, so restricted content can't surface via search, genre shelves, or any "Browse all" grid just because one screen forgot to re-check. */
    private fun applyKidModeFilter(entries: List<MergedEntry>): List<MergedEntry> {
        val settings = _kidModeSettings.value?.takeIf { it.enabled } ?: return entries
        return entries.filter { kidModeAllows(it, settings) }
    }

    /** Turns Kid Mode on for the first time (or fully replaces the PIN + rules of an already-on one) — see [AndroidKidModeStore.enable]. Immediately re-filters the currently-browsed catalog, if any, so the new restriction takes effect without waiting for the next [browseCatalog]. */
    fun enableKidMode(pin: String, allowedKinds: Set<MediaKind>, allowedGenres: Set<String>?, maxMovieRating: String?, maxTvRating: String?) {
        viewModelScope.launch {
            kidModeStore.enable(pin, allowedKinds, allowedGenres, maxMovieRating, maxTvRating)
            _kidModeSettings.value = kidModeStore.get()
            reapplyKidModeToCurrentCatalog()
        }
    }

    /** Edits the content rules on an already-enabled Kid Mode without touching its PIN — see [AndroidKidModeStore.updateRules]. */
    fun updateKidModeRules(allowedKinds: Set<MediaKind>, allowedGenres: Set<String>?, maxMovieRating: String?, maxTvRating: String?) {
        viewModelScope.launch {
            kidModeStore.updateRules(allowedKinds, allowedGenres, maxMovieRating, maxTvRating)
            _kidModeSettings.value = kidModeStore.get()
            reapplyKidModeToCurrentCatalog()
        }
    }

    fun disableKidMode() {
        viewModelScope.launch {
            kidModeStore.disable()
            _kidModeSettings.value = null
            reapplyKidModeToCurrentCatalog()
        }
    }

    /**
     * Re-applies the Kid Mode filter to whatever's *currently* shown in
     * [UiState.Catalog.entries] — the full unfiltered manifest isn't kept
     * around separately, so this can only re-filter what's already there,
     * not restore something already filtered out. Correct for tightening a
     * restriction (strictly removes entries) but a newly-*widened* one
     * (raising a max rating, adding a genre back) only fully reflects
     * everything again after the next manual resync/re-browse. Acceptable
     * trade-off: Kid Mode is managed from the Settings screen, reached from
     * Dashboard, not mid-browse, so the common case is "browse fresh right
     * after," not "watch this list retroactively grow while already
     * looking at it."
     */
    private fun reapplyKidModeToCurrentCatalog() {
        val current = _state.value
        if (current is UiState.Catalog) _state.value = current.copy(entries = applyKidModeFilter(current.entries))
    }

    // --- Hierarchical browsing: Music (Artist -> Album -> Track) ---

    fun openArtistShelf() {
        val current = _state.value
        if (current !is UiState.Catalog) return
        _state.value = UiState.ArtistShelf(current, CatalogGrouping.groupTracksByArtistAlbum(current.entries))
    }

    fun openArtistAlbums(artist: ArtistGroup) {
        val (catalog, artists) = when (val current = _state.value) {
            is UiState.Catalog -> current to CatalogGrouping.groupTracksByArtistAlbum(current.entries)
            is UiState.ArtistShelf -> current.catalog to current.artists
            else -> return
        }
        _state.value = UiState.ArtistAlbums(catalog, artists, artist)
    }

    fun backFromArtistShelf() {
        val current = _state.value
        if (current is UiState.ArtistShelf) _state.value = current.catalog
    }

    fun backFromArtistAlbums() {
        val current = _state.value
        if (current is UiState.ArtistAlbums) _state.value = UiState.ArtistShelf(current.catalog, current.artists)
    }

    // --- Hierarchical browsing: Movies (flat, just a detail step before play) ---

    fun openMovieDetail(entry: MergedEntry) {
        val current = _state.value
        if (current !is UiState.Catalog && current !is UiState.MovieShelf) return
        _state.value = UiState.MovieDetail(current, entry)
    }

    fun backFromMovieDetail() {
        val current = _state.value
        if (current is UiState.MovieDetail) _state.value = current.previous
    }

    fun openMovieShelf() {
        val current = _state.value
        if (current !is UiState.Catalog) return
        _state.value = UiState.MovieShelf(current, current.entries.filter { it.entry.kind == MediaKind.MOVIE })
    }

    fun backFromMovieShelf() {
        val current = _state.value
        if (current is UiState.MovieShelf) _state.value = current.catalog
    }

    // --- Hierarchical browsing: Shows (Show -> Season -> Episode) ---

    fun openShowShelf() {
        val current = _state.value
        if (current !is UiState.Catalog) return
        _state.value = UiState.ShowShelf(current, CatalogGrouping.groupEpisodesByShowSeason(current.entries))
    }

    fun openShowSeasons(show: ShowGroup) {
        val (catalog, shows) = when (val current = _state.value) {
            is UiState.Catalog -> current to CatalogGrouping.groupEpisodesByShowSeason(current.entries)
            is UiState.ShowShelf -> current.catalog to current.shows
            else -> return
        }
        _state.value = UiState.ShowSeasons(catalog, shows, show)
    }

    fun backFromShowShelf() {
        val current = _state.value
        if (current is UiState.ShowShelf) _state.value = current.catalog
    }

    fun backFromShowSeasons() {
        val current = _state.value
        if (current is UiState.ShowSeasons) _state.value = UiState.ShowShelf(current.catalog, current.shows)
    }

    /** Called when [PlayerScreen] is disposed with where playback ended up — "watched" is a simple near-the-end heuristic, not a precise "credits rolled" signal. */
    fun savePlaybackPosition(fingerprint: String, positionSecs: Double, durationSecs: Double) {
        viewModelScope.launch {
            val watched = durationSecs > 0 && positionSecs / durationSecs > 0.9
            watchStateStore.set(fingerprint, WatchState(positionSecs, durationSecs, watched, System.currentTimeMillis()))
        }
    }

    fun stopPlayback() {
        val current = _state.value
        if (current is UiState.Player) {
            current.previous.embeddedCatalog()?.let { releasePlaybackSession(it, current.serverId, current.sessionId) }
            _state.value = current.previous
        }
    }

    fun backToDashboard() {
        val current = _state.value
        if (current is UiState.Catalog) {
            if (localSession) {
                _state.value = UiState.Dashboard(current.swarm, current.devices)
            } else {
                _state.value = UiState.Dashboard(current.swarm, current.devices, allSwarms = cachedSwarms)
            }
        }
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
            _state.value = UiState.Dashboard(roster.swarm, roster.devices, allSwarms = cachedSwarms)
        } catch (e: StunClientError) {
            val current = _state.value
            if (current is UiState.Dashboard) {
                Log.w(logTag, "could not refresh the saved STUN roster", e)
                _state.value = current.copy(
                    devices = current.devices.map { it.copy(online = false) },
                    resyncing = false,
                )
            } else {
                _state.value = UiState.Error(e.message ?: "Could not load the swarm roster.")
            }
        }
    }

    override fun onCleared() {
        lanDiscovery.close()
        signaling?.close()
        catalogSession.close()
        proxy.close()
    }
}
