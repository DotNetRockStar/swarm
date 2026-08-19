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
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

sealed class UiState {
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
    /** Movies: Movies row -> here (detail before play) -> [Player]. */
    data class MovieDetail(val catalog: Catalog, val entry: MergedEntry) : UiState()
    /** Shows: Shows row -> here (grouped, replacing the old flat-episode shelf) -> [ShowSeasons]. */
    data class ShowShelf(val catalog: Catalog, val shows: List<ShowGroup>) : UiState()
    /** One show's seasons; [SeasonScreen] handles the season-list<->episode-grid sub-navigation locally. */
    data class ShowSeasons(val catalog: Catalog, val shows: List<ShowGroup>, val show: ShowGroup) : UiState()
    /** One episode's detail before play. */
    data class EpisodeDetail(val catalog: Catalog, val shows: List<ShowGroup>, val show: ShowGroup, val entry: MergedEntry) : UiState()
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
        val previous: Catalog,
        /** Which server negotiated [sessionId] — both needed to release its bandwidth reservation on exit, see [SwarmViewModel.releasePlaybackSession]. */
        val serverId: String,
        val sessionId: String,
    ) : UiState()
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
) : ViewModel() {
    private val _state = MutableStateFlow<UiState>(UiState.PasscodeEntry)
    private val logTag = "SwarmViewModel"
    val state: StateFlow<UiState> = _state.asStateFlow()

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

    private val proxy = PeerLoopbackProxy.start()
    private val catalogSession = CatalogSession(proxy)

    init {
        viewModelScope.launch { restoreSession() }
    }

    /**
     * Cold-start session resume: previously this app always began at
     * [UiState.PasscodeEntry] regardless of any prior registration — every
     * launch meant re-entering the STUN URL and a fresh passcode, since
     * nothing was ever read back from [tokenStore]/[connectionStore]. Now
     * that both are actually written on registration (see
     * [saveCurrentConnection]), restore from them here; leaves the app at
     * [UiState.PasscodeEntry] (unchanged behavior) for a genuinely first-
     * ever launch, a signed-out state, or if the saved swarm list is empty
     * (nothing a Dashboard could show).
     */
    private suspend fun restoreSession() {
        val token = tokenStore.load() ?: return
        val saved = connectionStore.get() ?: return
        if (saved.swarms.isEmpty() || saved.activeSwarmId == null) return
        accessToken = token
        deviceId = saved.deviceId
        swarmId = saved.activeSwarmId
        baseUrl = saved.baseUrl
        deviceName = saved.deviceName
        cachedSwarms = saved.swarms
        client = StunApiClient(saved.baseUrl)
        establishSignaling(saved.baseUrl, token, saved.deviceId)
        loadRoster()
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

    fun resync() {
        val current = _state.value
        Log.i(logTag, "resync() called, current state=${current::class.simpleName}")
        if (current !is UiState.Dashboard) return
        _state.value = current.copy(resyncing = true)
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
            val result = withContext(Dispatchers.IO) {
                catalogSession.refresh(current.devices, clientCertificate, clientKey)
            }
            Log.i(logTag, "browseCatalog() refresh done: entries=${result.entries.size} unreachable=${result.unreachable.size}")
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

    /** Movie/episode backdrop art for detail screens — best-effort, same gate as [artworkUrl]; a 404 (backdrop never scraped) just fails the image load silently, same as this app's existing artwork handling. */
    fun backdropUrl(entry: MergedEntry): String? {
        if (entry.entry.artworkEtag == null) return null
        return catalogSession.urlFor(entry.sources.first(), "/art/${entry.entry.entryKey}/backdrop")
    }

    /**
     * Negotiates a budgeted direct/HLS session on the first connected
     * source, resuming where a previous watch left off unless it was
     * already finished. Callable from the top-level [Catalog] screen or
     * from any of the hierarchical browse/detail screens built on top of
     * it (they all carry their originating [Catalog] — see [playEntry]).
     */
    fun play(entry: MergedEntry) {
        val catalog = when (val current = _state.value) {
            is UiState.Catalog -> current
            is UiState.ArtistAlbums -> current.catalog
            is UiState.MovieDetail -> current.catalog
            is UiState.EpisodeDetail -> current.catalog
            else -> return
        }
        playEntry(entry, catalog)
    }

    /** Finds and plays the episode after [UiState.Player.entry] — see [CatalogGrouping.nextEpisode]. No-op if there isn't one. */
    fun playNext() {
        val current = _state.value
        if (current !is UiState.Player) return
        val next = current.nextEntry ?: return
        releasePlaybackSession(current.previous, current.serverId, current.sessionId)
        playEntry(next, current.previous)
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
     * per-screen error UI to every new detail/list screen.
     */
    private fun playEntry(entry: MergedEntry, catalog: UiState.Catalog) {
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
                return@launch
            }
            val isHls = selection.mode == PlaybackMode.HLS
            val nextEntry = if (entry.entry.kind == MediaKind.EPISODE) {
                CatalogGrouping.nextEpisode(entry, CatalogGrouping.groupEpisodesByShowSeason(catalog.entries))
            } else {
                null
            }
            _state.value = UiState.Player(
                url = selection.url,
                title = entry.entry.scrapedTitle ?: entry.entry.title,
                fingerprint = fingerprint,
                resumePositionSecs = if (isHls) 0.0 else resumePositionSecs,
                positionOffsetSecs = if (isHls) resumePositionSecs else 0.0,
                maxBitrate = selection.maxBitrate,
                mediaDurationSecs = entry.entry.durationSecs,
                entry = entry,
                nextEntry = nextEntry,
                previous = catalog.copy(playbackError = null),
                serverId = serverId,
                sessionId = selection.sessionId,
            )
        }
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
        if (current !is UiState.Catalog) return
        _state.value = UiState.MovieDetail(current, entry)
    }

    fun backFromMovieDetail() {
        val current = _state.value
        if (current is UiState.MovieDetail) _state.value = current.catalog
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

    fun openEpisodeDetail(entry: MergedEntry) {
        val current = _state.value
        if (current !is UiState.ShowSeasons) return
        _state.value = UiState.EpisodeDetail(current.catalog, current.shows, current.show, entry)
    }

    fun backFromEpisodeDetail() {
        val current = _state.value
        if (current is UiState.EpisodeDetail) _state.value = UiState.ShowSeasons(current.catalog, current.shows, current.show)
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
            releasePlaybackSession(current.previous, current.serverId, current.sessionId)
            _state.value = current.previous
        }
    }

    fun backToDashboard() {
        val current = _state.value
        if (current is UiState.Catalog) _state.value = UiState.Dashboard(current.swarm, current.devices, allSwarms = cachedSwarms)
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
            _state.value = UiState.Error(e.message ?: "Could not load the swarm roster.")
        }
    }

    override fun onCleared() {
        signaling?.close()
        catalogSession.close()
        proxy.close()
    }
}
