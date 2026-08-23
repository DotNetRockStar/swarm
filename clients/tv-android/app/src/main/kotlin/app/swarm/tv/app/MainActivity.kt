package app.swarm.tv.app

import app.swarm.tv.BuildConfig
import android.app.Activity
import android.net.Uri
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer
import app.swarm.tv.app.data.AndroidCatalogCache
import app.swarm.tv.app.data.AndroidConnectionStore
import app.swarm.tv.app.data.AndroidDeviceIdentity
import app.swarm.tv.app.data.AndroidDisconnectedServerStore
import app.swarm.tv.app.data.AndroidKidModeStore
import app.swarm.tv.app.data.AndroidLanConnectionStore
import app.swarm.tv.app.data.AndroidLikedEntriesStore
import app.swarm.tv.app.data.AndroidTokenStore
import app.swarm.tv.app.data.KidModeSettings
import app.swarm.tv.app.data.LanDiscoveryManager
import app.swarm.tv.app.data.LanPairingActivation
import app.swarm.tv.app.data.LanServer
import app.swarm.tv.app.data.AndroidWatchStateStore
import app.swarm.tv.app.data.AndroidWatchlistStore
import app.swarm.tv.app.data.WatchlistKeys
import app.swarm.tv.app.data.BrowsePreview
import app.swarm.tv.app.data.SwarmViewModel
import app.swarm.tv.app.data.UiState
import app.swarm.tv.app.data.androidMachineId
import app.swarm.tv.app.data.resolveDeviceName
import app.swarm.tv.app.ui.components.SwarmLoadingIndicator
import app.swarm.tv.app.ui.components.ClientToastHost
import app.swarm.tv.app.ui.components.rememberClientToastHostState
import app.swarm.tv.app.ui.screens.AlbumScreen
import app.swarm.tv.app.ui.screens.ArtistShelfScreen
import app.swarm.tv.app.ui.screens.CatalogScreen
import app.swarm.tv.app.ui.screens.CatalogBrowseState
import app.swarm.tv.app.ui.screens.ExitConfirmOverlay
import app.swarm.tv.app.ui.screens.MiniPlayerBar
import app.swarm.tv.app.ui.screens.MovieDetailScreen
import app.swarm.tv.app.ui.screens.MovieShelfScreen
import app.swarm.tv.app.ui.screens.MusicPlayerScreen
import app.swarm.tv.app.ui.screens.ActivationCodeScreen
import app.swarm.tv.app.ui.screens.ActivationRequestScreen
import app.swarm.tv.app.ui.screens.PlayerScreen
import app.swarm.tv.app.ui.screens.SeasonScreen
import app.swarm.tv.app.ui.screens.ShowShelfScreen
import app.swarm.tv.app.ui.screens.SwarmDashboardScreen
import app.swarm.tv.app.ui.screens.SwarmSettingsScreen
import app.swarm.tv.app.ui.theme.SwarmBackground
import app.swarm.tv.app.ui.theme.SwarmTvTheme
import app.swarm.tv.core.catalog.ArtistGroup
import app.swarm.tv.core.catalog.MergedEntry
import app.swarm.tv.core.catalog.SeasonGroup
import app.swarm.tv.core.catalog.ShowGroup
import app.swarm.tv.core.peer.MediaKind
import app.swarm.tv.core.rest.SwarmDevice
import app.swarm.tv.core.watch.WatchState
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collect

class MainActivity : ComponentActivity() {
    private var frameJankMonitor: FrameJankMonitor? = null

    override fun onStart() {
        super.onStart()
        if (applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE != 0) {
            frameJankMonitor = FrameJankMonitor().also(FrameJankMonitor::start)
        }
    }

    override fun onStop() {
        frameJankMonitor?.stop()
        frameJankMonitor = null
        super.onStop()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Real bug this fixes: without an explicit edge-to-edge opt-in, some
        // real TV hardware lays this Activity's window out inset from the
        // actual display bounds — content renders centered with unused
        // space around it instead of filling the screen. Compose already
        // owns every inset/margin decision this app needs (the manual
        // overscan-safe padding below), so there's nothing this should be
        // deferring to the system for.
        enableEdgeToEdge()

        val tokenStore = AndroidTokenStore(applicationContext)
        val watchStateStore = AndroidWatchStateStore(applicationContext)
        val watchlistStore = AndroidWatchlistStore(applicationContext)
        val connectionStore = AndroidConnectionStore(applicationContext)
        val likedEntriesStore = AndroidLikedEntriesStore(applicationContext)
        val kidModeStore = AndroidKidModeStore(applicationContext)
        val lanDiscovery = LanDiscoveryManager(applicationContext)
        val lanConnectionStore = AndroidLanConnectionStore(applicationContext)
        val disconnectedServerStore = AndroidDisconnectedServerStore(applicationContext)
        val catalogCache = AndroidCatalogCache(applicationContext)
        val machineId = androidMachineId(applicationContext)
        val defaultDeviceName = resolveDeviceName(applicationContext)
        val certFingerprint = AndroidDeviceIdentity.ensureFingerprint()
        val certificate = AndroidDeviceIdentity.certificate()
        val privateKey = AndroidDeviceIdentity.privateKey()
        val factory = object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T =
                SwarmViewModel(tokenStore, machineId, certFingerprint, certificate, privateKey, watchStateStore, watchlistStore, connectionStore, likedEntriesStore, kidModeStore, lanDiscovery, lanConnectionStore, disconnectedServerStore, catalogCache, BuildConfig.SWARM_RENDEZVOUS_URL) as T
        }

        setContent {
            SwarmTvTheme {
                Box(modifier = Modifier.fillMaxSize().background(SwarmBackground)) {
                    val viewModel: SwarmViewModel = viewModel(factory = factory)
                    val toastHostState = rememberClientToastHostState()
                    LaunchedEffect(viewModel) {
                        viewModel.notifications.collect(toastHostState::show)
                    }
                    val state by viewModel.state.collectAsState()
                    val likedFingerprints by viewModel.likedFingerprints.collectAsState()
                    val watchStates by viewModel.watchStates.collectAsState()
                    val watchlistKeys by viewModel.watchlistKeys.collectAsState()
                    val kidModeSettings by viewModel.kidModeSettings.collectAsState()
                    val shuffleEnabled by viewModel.shuffleEnabled.collectAsState()
                    val minimizedPlayer by viewModel.minimizedPlayer.collectAsState()
                    val browsePreview by viewModel.browsePreview.collectAsState()
                    val lanServers by viewModel.lanServers.collectAsState()
                    val lanPairingBusy by viewModel.lanPairingBusy.collectAsState()
                    val lanPairingActivation by viewModel.lanPairingActivation.collectAsState()
                    val lanError by viewModel.lanError.collectAsState()
                    val pairedLanFingerprints by viewModel.pairedLanFingerprints.collectAsState()
                    val disconnectedServerFingerprints by viewModel.disconnectedServerFingerprints.collectAsState()
                    val isLikedCallback: (MergedEntry) -> Boolean = remember(likedFingerprints) {
                        { entry -> entry.entry.fingerprint in likedFingerprints }
                    }
                    SwarmApp(
                        state = state,
                        defaultDeviceName = defaultDeviceName,
                        lanServers = lanServers,
                        lanPairingBusy = lanPairingBusy,
                        lanPairingActivation = lanPairingActivation,
                        lanError = lanError,
                        pairedLanFingerprints = pairedLanFingerprints,
                        disconnectedServerFingerprints = disconnectedServerFingerprints,
                        onConnectLan = viewModel::connectLanServer,
                        onStartLanPairing = viewModel::startLanPairing,
                        onCancelLanPairing = viewModel::cancelLanPairing,
                        onDisconnectServer = viewModel::disconnectSwarmServer,
                        onReconnectServer = viewModel::reconnectSwarmServer,
                        isLiked = isLikedCallback,
                        onToggleLike = viewModel::toggleLike,
                        watchStates = watchStates,
                        watchlistKeys = watchlistKeys,
                        onToggleMovieWatchlist = viewModel::toggleMovieWatchlist,
                        onToggleShowWatchlist = viewModel::toggleShowWatchlist,
                        kidModeSettings = kidModeSettings,
                        onEnableKidMode = viewModel::enableKidMode,
                        onUpdateKidModeRules = viewModel::updateKidModeRules,
                        onDisableKidMode = viewModel::disableKidMode,
                        shuffleEnabled = shuffleEnabled,
                        onToggleShuffle = viewModel::toggleShuffle,
                        minimizedPlayer = minimizedPlayer,
                        browsePreview = browsePreview,
                        onStartBrowsePreview = viewModel::startBrowsePreview,
                        onStopBrowsePreview = viewModel::stopBrowsePreview,
                        onFinishBrowsePreview = viewModel::finishBrowsePreview,
                        onMinimizePlayback = viewModel::minimizePlayback,
                        onRestoreMinimizedPlayback = viewModel::restoreMinimizedPlayback,
                        onStopMinimizedPlayback = viewModel::stopMinimizedPlayback,
                        onTrackPlaybackEnded = viewModel::onTrackPlaybackEnded,
                        artistPhotoUrl = viewModel::artistPhotoUrl,
                        artistPhotoThumbnailUrl = viewModel::artistPhotoThumbnailUrl,
                        fullArtworkUrl = viewModel::fullArtworkUrl,
                        onStartActivation = viewModel::startActivation,
                        onCancelActivation = viewModel::cancelActivation,
                        onBrowseCatalog = viewModel::browseCatalog,
                        onPlay = viewModel::play,
                        onPlayNext = viewModel::playNext,
                        onSeekPlayback = viewModel::seekPlayback,
                        onStopPlayback = viewModel::stopPlayback,
                        onBackToDashboard = viewModel::backToDashboard,
                        artworkUrl = viewModel::artworkUrl,
                        seasonArtworkUrl = viewModel::seasonArtworkUrl,
                        episodeArtworkUrl = viewModel::episodeArtworkUrl,
                        backdropUrl = viewModel::backdropUrl,
                        onReportProblem = viewModel::reportAssetProblem,
                        onSavePlaybackPosition = viewModel::savePlaybackPosition,
                        onPlaybackRuntimeError = viewModel::reportPlaybackRuntimeError,
                        onOpenSettings = viewModel::openSettings,
                        onUpdateBaseUrl = viewModel::updateBaseUrl,
                        onUpdateDeviceName = viewModel::updateDeviceName,
                        onBackFromSettings = viewModel::backFromSettings,
                        onOpenMovie = viewModel::openMovieDetail,
                        onBackFromMovie = viewModel::backFromMovieDetail,
                        onOpenMovieShelf = viewModel::openMovieShelf,
                        onBackFromMovieShelf = viewModel::backFromMovieShelf,
                        onOpenArtistShelf = viewModel::openArtistShelf,
                        onOpenArtist = viewModel::openArtistAlbums,
                        onBackFromArtistShelf = viewModel::backFromArtistShelf,
                        onBackFromArtistAlbums = viewModel::backFromArtistAlbums,
                        onOpenShowShelf = viewModel::openShowShelf,
                        onOpenShow = viewModel::openShowSeasons,
                        onSelectShowSeason = viewModel::selectShowSeason,
                        onBackFromShowShelf = viewModel::backFromShowShelf,
                        onBackFromShowSeasons = viewModel::backFromShowSeasons,
                    )
                    ClientToastHost(
                        state = toastHostState,
                        modifier = Modifier
                            .fillMaxSize()
                            .align(Alignment.BottomEnd)
                            .padding(bottom = if (minimizedPlayer != null) 66.dp else 0.dp),
                    )
                }
            }
        }
    }
}

@Composable
private fun SwarmApp(
    state: UiState,
    defaultDeviceName: String,
    lanServers: List<LanServer>,
    lanPairingBusy: Boolean,
    lanPairingActivation: LanPairingActivation?,
    lanError: String?,
    pairedLanFingerprints: Set<String>,
    disconnectedServerFingerprints: Set<String>,
    onConnectLan: (server: LanServer, deviceName: String) -> Unit,
    onStartLanPairing: (server: LanServer, deviceName: String) -> Unit,
    onCancelLanPairing: () -> Unit,
    onDisconnectServer: (SwarmDevice) -> Unit,
    onReconnectServer: (SwarmDevice) -> Unit,
    isLiked: (MergedEntry) -> Boolean,
    onToggleLike: (MergedEntry) -> Unit,
    watchStates: Map<String, WatchState>,
    watchlistKeys: Set<String>,
    onToggleMovieWatchlist: (MergedEntry) -> Unit,
    onToggleShowWatchlist: (ShowGroup) -> Unit,
    kidModeSettings: KidModeSettings?,
    onEnableKidMode: (pin: String, allowedKinds: Set<MediaKind>, allowedGenres: Set<String>?, maxMovieRating: String?, maxTvRating: String?) -> Unit,
    onUpdateKidModeRules: (allowedKinds: Set<MediaKind>, allowedGenres: Set<String>?, maxMovieRating: String?, maxTvRating: String?) -> Unit,
    onDisableKidMode: () -> Unit,
    shuffleEnabled: Boolean,
    onToggleShuffle: () -> Unit,
    minimizedPlayer: UiState.Player?,
    browsePreview: BrowsePreview?,
    onStartBrowsePreview: (MergedEntry) -> Unit,
    onStopBrowsePreview: () -> Unit,
    onFinishBrowsePreview: (String) -> Unit,
    onMinimizePlayback: () -> Unit,
    onRestoreMinimizedPlayback: () -> Unit,
    onStopMinimizedPlayback: () -> Unit,
    onTrackPlaybackEnded: () -> Unit,
    artistPhotoUrl: (MergedEntry) -> String?,
    artistPhotoThumbnailUrl: (MergedEntry) -> String?,
    fullArtworkUrl: (MergedEntry) -> String?,
    onStartActivation: (deviceName: String) -> Unit,
    onCancelActivation: () -> Unit,
    onBrowseCatalog: () -> Unit,
    onPlay: (MergedEntry) -> Unit,
    onPlayNext: () -> Unit,
    onSeekPlayback: (Double) -> Unit,
    onStopPlayback: () -> Unit,
    onBackToDashboard: () -> Unit,
    artworkUrl: (MergedEntry) -> String?,
    seasonArtworkUrl: (MergedEntry) -> String?,
    episodeArtworkUrl: (MergedEntry) -> String?,
    backdropUrl: (MergedEntry) -> String?,
    onReportProblem: (MergedEntry) -> Unit,
    onSavePlaybackPosition: (entry: MergedEntry, positionSecs: Double, durationSecs: Double) -> Unit,
    onPlaybackRuntimeError: (message: String, context: String?) -> Unit,
    onOpenSettings: () -> Unit,
    onUpdateBaseUrl: (baseUrl: String) -> Unit,
    onUpdateDeviceName: (name: String) -> Unit,
    onBackFromSettings: () -> Unit,
    onOpenMovie: (MergedEntry) -> Unit,
    onBackFromMovie: () -> Unit,
    onOpenMovieShelf: () -> Unit,
    onBackFromMovieShelf: () -> Unit,
    onOpenArtistShelf: () -> Unit,
    onOpenArtist: (ArtistGroup) -> Unit,
    onBackFromArtistShelf: () -> Unit,
    onBackFromArtistAlbums: () -> Unit,
    onOpenShowShelf: () -> Unit,
    onOpenShow: (ShowGroup) -> Unit,
    onSelectShowSeason: (SeasonGroup?) -> Unit,
    onBackFromShowShelf: () -> Unit,
    onBackFromShowSeasons: () -> Unit,
) {
    // Real Fire TV hardware (and TVs generally) can crop a border of the
    // rendered frame via overscan — content with no safe margin renders
    // correctly on this machine's screenshot but gets clipped by the
    // physical bezel on the actual TV. Google's TV design guidance is a
    // 5% action-safe margin (2.5% each side); computed from the real
    // reported screen size via LocalConfiguration rather than a fixed dp
    // value so it scales correctly across different real Fire TV models'
    // resolutions/densities. Not applied to PlayerScreen: video content is
    // meant to fill the screen edge-to-edge — padding it would just look
    // like unwanted letterboxing, and it's the one screen where overscan
    // cropping a sliver of picture is the normal, expected trade-off every
    // TV app makes.
    // Which card was last opened from CatalogScreen's own top-level Movies/
    // Shows/Music rows — hoisted here, not inside CatalogScreen itself,
    // because CatalogScreen is a brand-new composable instance every time
    // the UiState swaps away from and back to Catalog (its own `remember`ed
    // state doesn't survive that), while SwarmApp never gets torn down
    // across state changes. Read back by CatalogScreen's
    // initialFocus{Movie,Show,Artist}Key params below — see that screen's
    // own doc comment on those for how they're used.
    var lastFocusedMovieKey by remember { mutableStateOf<String?>(null) }
    var lastFocusedShowKey by remember { mutableStateOf<String?>(null) }
    var lastFocusedArtistKey by remember { mutableStateOf<String?>(null) }
    var catalogBrowseState by remember { mutableStateOf(CatalogBrowseState()) }
    var showCatalogExitConfirm by remember { mutableStateOf(false) }

    // The one track session actually live right now, whichever of the two
    // places it can be is holding it — see minimizePlayback's doc comment.
    // At most one of these is ever non-null.
    val activeMusicSession = (state as? UiState.Player)?.takeIf { it.entry.entry.kind == MediaKind.TRACK } ?: minimizedPlayer

    // Hoisted above both MusicPlayerScreen and MiniPlayerBar, keyed on
    // sessionId (not the whole session object — nextEntry alone changing,
    // e.g. via shuffle, must not tear down and rebuild a player that's
    // already mid-track): this is *why* track playback survives minimizing
    // away from MusicPlayerScreen's own composition, unlike PlayerScreen's
    // video player, which is deliberately still tied to its own screen
    // (movies/episodes never minimize, so there's nothing to hoist for).
    // `remember` with a key already gives "build a new one when the key
    // changes, otherwise keep the existing instance" for free — no separate
    // hand-rolled holder class needed.
    val context = LocalContext.current
    val musicPlayer = remember(activeMusicSession?.sessionId) {
        activeMusicSession?.let { session ->
            ExoPlayer.Builder(context).build().apply {
                setMediaItem(MediaItem.Builder().setUri(Uri.parse(session.url)).setMediaId(session.title).build())
                if (session.resumePositionSecs > 0) seekTo((session.resumePositionSecs * 1000).toLong())
                playWhenReady = true
                prepare()
            }
        }
    }
    var musicIsPlaying by remember(musicPlayer) { mutableStateOf(true) }
    var musicIsLoading by remember(musicPlayer) { mutableStateOf(true) }
    var musicPositionMs by remember(musicPlayer) { mutableLongStateOf(0L) }
    var musicPausedForPreview by remember(musicPlayer) { mutableStateOf(false) }

    // Inline previews intentionally include audio. If music was already
    // playing in the minimized bar, pause it for the preview and restore it
    // afterward; never mix two unrelated soundtracks together.
    LaunchedEffect(musicPlayer, browsePreview?.sessionId, browsePreview?.released) {
        val previewPlaying = browsePreview != null && !browsePreview.released
        if (previewPlaying && musicPlayer?.isPlaying == true) {
            musicPausedForPreview = true
            musicPlayer.pause()
        } else if (!previewPlaying && musicPausedForPreview) {
            musicPausedForPreview = false
            musicPlayer?.play()
        }
    }

    // Lyrics need a lightweight playhead clock, but only while the full
    // music screen is visible. The minimized player does not trigger a
    // quarter-second recomposition loop across the browsing UI.
    LaunchedEffect(musicPlayer, (state as? UiState.Player)?.sessionId) {
        val visibleSession = (state as? UiState.Player)?.takeIf { it.entry.entry.kind == MediaKind.TRACK }
            ?: return@LaunchedEffect
        while (true) {
            musicPositionMs = (visibleSession.positionOffsetSecs * 1000.0).toLong() + (musicPlayer?.currentPosition ?: 0L)
            delay(250)
        }
    }

    DisposableEffect(musicPlayer) {
        val player = musicPlayer
        val listener = object : Player.Listener {
            override fun onIsPlayingChanged(isPlaying: Boolean) {
                musicIsPlaying = isPlaying
            }
            override fun onPlaybackStateChanged(playbackState: Int) {
                if (playbackState == Player.STATE_READY) musicIsLoading = false
                // onTrackPlaybackEnded reads the *current* session fresh off
                // the ViewModel's own state rather than anything captured
                // here, so this stays correct even if nextEntry changed
                // (shuffle toggled) since this listener was attached.
                if (playbackState == Player.STATE_ENDED) onTrackPlaybackEnded()
            }
        }
        player?.addListener(listener)
        onDispose {
            player?.removeListener(listener)
            // Position save-on-exit for whichever session *this* player
            // instance was actually playing — mirrors PlayerScreen's own
            // onDispose save, needed here too since this player now
            // outlives any single screen's composition.
            activeMusicSession?.let { session ->
                val positionSecs = session.positionOffsetSecs + (player?.currentPosition ?: 0L) / 1000.0
                val durationSecs = player?.duration?.takeIf { it != C.TIME_UNSET }?.let { session.positionOffsetSecs + it / 1000.0 } ?: 0.0
                onSavePlaybackPosition(session.entry, positionSecs, session.mediaDurationSecs ?: durationSecs)
            }
            player?.release()
        }
    }

    // Prevent Fire TV's screensaver/sleep timeout from replacing SWARM with
    // a black screen or launcher while media is active. Music playback is
    // hoisted and can continue behind any browse screen, so this must live
    // here rather than only in MusicPlayerScreen. FLAG_KEEP_SCREEN_ON is
    // foreground-only and is cleared immediately on pause/end/disposal; a
    // broad wake lock would outlive the UI and is neither needed nor wanted.
    val videoPlaybackActive = (state as? UiState.Player)
        ?.entry?.entry?.kind
        ?.let { it != MediaKind.TRACK } == true
    KeepScreenAwakeWhile(
        videoPlaybackActive ||
            (activeMusicSession != null && musicIsPlaying) ||
            (browsePreview != null && !browsePreview.released),
    )

    val config = LocalConfiguration.current
    val contentModifier = if (state is UiState.Player) {
        Modifier.fillMaxSize()
    } else {
        Modifier.fillMaxSize().padding(
            horizontal = (config.screenWidthDp * 0.025f).dp,
            vertical = (config.screenHeightDp * 0.025f).dp,
        )
    }
    Box(modifier = contentModifier) {
        when (state) {
            is UiState.Loading ->
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    SwarmLoadingIndicator(messageOverride = "Stream Whatever, Anywhere — Remote Media")
                }
            is UiState.RequestingActivation ->
                ActivationRequestScreen(onCancel = onCancelActivation)
            is UiState.Activating ->
                ActivationCodeScreen(
                    code = state.code,
                    expiresAt = state.expiresAt,
                    errorMessage = state.error,
                    onCancel = onCancelActivation,
                )
            is UiState.Dashboard ->
                SwarmDashboardScreen(
                    swarm = state.swarm,
                    devices = state.devices,
                    lanServers = lanServers,
                    pairedLanFingerprints = pairedLanFingerprints,
                    disconnectedServerFingerprints = disconnectedServerFingerprints,
                    lanPairingBusy = lanPairingBusy,
                    lanPairingActivation = lanPairingActivation,
                    lanError = lanError,
                    deviceName = defaultDeviceName,
                    joiningServer = state.joiningServer,
                    joinServerError = state.joinServerError,
                    onBrowseCatalog = onBrowseCatalog,
                    onOpenSettings = onOpenSettings,
                    onAddServer = { onStartActivation(defaultDeviceName) },
                    onConnectLan = onConnectLan,
                    onStartLanPairing = onStartLanPairing,
                    onCancelLanPairing = onCancelLanPairing,
                    onDisconnectServer = onDisconnectServer,
                    onReconnectServer = onReconnectServer,
                    onBackToBrowse = onBrowseCatalog,
                )
            is UiState.Settings ->
                SwarmSettingsScreen(
                    baseUrl = state.baseUrl,
                    deviceName = state.deviceName,
                    busy = state.busy,
                    errorMessage = state.error,
                    onUpdateBaseUrl = onUpdateBaseUrl,
                    onUpdateDeviceName = onUpdateDeviceName,
                    onBack = onBackFromSettings,
                    kidModeSettings = kidModeSettings,
                    availableGenres = state.availableGenres,
                    onEnableKidMode = onEnableKidMode,
                    onUpdateKidModeRules = onUpdateKidModeRules,
                    onDisableKidMode = onDisableKidMode,
                )
            is UiState.Catalog ->
                CatalogScreen(
                    entries = state.entries,
                    loading = state.loading,
                    unreachable = state.unreachable,
                    playbackError = state.playbackError,
                    artworkUrl = artworkUrl,
                    artistPhotoUrl = artistPhotoThumbnailUrl,
                    onOpenMovie = { entry ->
                        lastFocusedMovieKey = entry.entry.entryKey
                        lastFocusedShowKey = null
                        lastFocusedArtistKey = null
                        onOpenMovie(entry)
                    },
                    onOpenMovieShelf = onOpenMovieShelf,
                    onOpenArtistShelf = onOpenArtistShelf,
                    onOpenArtist = { artist ->
                        lastFocusedMovieKey = null
                        lastFocusedShowKey = null
                        lastFocusedArtistKey = artist.artist
                        onOpenArtist(artist)
                    },
                    onOpenShowShelf = onOpenShowShelf,
                    onOpenShow = { show ->
                        lastFocusedMovieKey = null
                        lastFocusedShowKey = show.show
                        lastFocusedArtistKey = null
                        onOpenShow(show)
                    },
                    onOpenSwarm = {
                        showCatalogExitConfirm = false
                        onBackToDashboard()
                    },
                    onBack = {
                        val unreachableIds = state.unreachable.mapTo(mutableSetOf()) { it.deviceId }
                        val hasConnectedServer = state.devices.any {
                            (it.deviceType == app.swarm.tv.core.rest.DeviceType.SERVER ||
                                it.deviceType == app.swarm.tv.core.rest.DeviceType.BOTH) &&
                                it.online && it.deviceId !in unreachableIds
                        }
                        if (hasConnectedServer) {
                            showCatalogExitConfirm = true
                        } else {
                            onBackToDashboard()
                        }
                    },
                    initialFocusMovieKey = lastFocusedMovieKey,
                    initialFocusShowKey = lastFocusedShowKey,
                    initialFocusArtistKey = lastFocusedArtistKey,
                    isLiked = isLiked,
                    watchStates = watchStates,
                    watchlistKeys = watchlistKeys,
                    onPlay = onPlay,
                    preview = browsePreview,
                    onStartPreview = onStartBrowsePreview,
                    onStopPreview = onStopBrowsePreview,
                    onPreviewFinished = onFinishBrowsePreview,
                    initialBrowseState = catalogBrowseState,
                    onBrowseStateChange = { catalogBrowseState = it },
                )
            is UiState.ArtistShelf ->
                ArtistShelfScreen(
                    state.artists,
                    artworkUrl = artworkUrl,
                    artistPhotoUrl = artistPhotoThumbnailUrl,
                    onOpenArtist = { artist ->
                        lastFocusedMovieKey = null
                        lastFocusedShowKey = null
                        lastFocusedArtistKey = artist.artist
                        onOpenArtist(artist)
                    },
                    onBack = onBackFromArtistShelf,
                    initialFocusKey = lastFocusedArtistKey,
                )
            is UiState.ArtistAlbums ->
                AlbumScreen(state.artist, artworkUrl, onPlay = onPlay, onBack = onBackFromArtistAlbums)
            is UiState.MovieShelf ->
                MovieShelfScreen(
                    state.movies,
                    artworkUrl,
                    onOpenMovie = { entry ->
                        lastFocusedMovieKey = entry.entry.entryKey
                        lastFocusedShowKey = null
                        lastFocusedArtistKey = null
                        onOpenMovie(entry)
                    },
                    onBack = onBackFromMovieShelf,
                    initialFocusKey = lastFocusedMovieKey,
                )
            is UiState.MovieDetail ->
                MovieDetailScreen(
                    state.entry,
                    fullArtworkUrl,
                    backdropUrl,
                    onPlay = onPlay,
                    onBack = onBackFromMovie,
                    onReportProblem = onReportProblem,
                    isLiked = isLiked(state.entry),
                    onToggleLike = { onToggleLike(state.entry) },
                    isWatchlisted = WatchlistKeys.movie(state.entry) in watchlistKeys,
                    onToggleWatchlist = { onToggleMovieWatchlist(state.entry) },
                )
            is UiState.ShowShelf ->
                ShowShelfScreen(
                    state.shows,
                    artworkUrl,
                    onOpenShow = { show ->
                        lastFocusedMovieKey = null
                        lastFocusedShowKey = show.show
                        lastFocusedArtistKey = null
                        onOpenShow(show)
                    },
                    onBack = onBackFromShowShelf,
                    initialFocusKey = lastFocusedShowKey,
                )
            is UiState.ShowSeasons ->
                SeasonScreen(
                    state.show,
                    seasonArtworkUrl = seasonArtworkUrl,
                    episodeArtworkUrl = episodeArtworkUrl,
                    onPlayEpisode = onPlay,
                    onBack = onBackFromShowSeasons,
                    selectedSeason = state.selectedSeason,
                    onSelectSeason = onSelectShowSeason,
                    isWatchlisted = WatchlistKeys.show(state.show) in watchlistKeys,
                    onToggleWatchlist = { onToggleShowWatchlist(state.show) },
                )
            is UiState.Player ->
                if (state.entry.entry.kind == MediaKind.TRACK) {
                    MusicPlayerScreen(
                        entry = state.entry,
                        nextTitle = state.nextEntry?.let { it.entry.scrapedTitle ?: it.entry.title },
                        isPlaying = musicIsPlaying,
                        isLoading = musicIsLoading,
                        shuffleEnabled = shuffleEnabled,
                        isLiked = isLiked(state.entry),
                        artworkUrl = fullArtworkUrl(state.entry),
                        artistPhotoUrl = artistPhotoUrl(state.entry),
                        lyrics = state.lyrics,
                        positionMs = musicPositionMs,
                        onTogglePlayPause = { musicPlayer?.let { it.playWhenReady = !it.playWhenReady } },
                        onToggleShuffle = onToggleShuffle,
                        onToggleLike = { onToggleLike(state.entry) },
                        onSkipNext = onPlayNext,
                        onMinimize = onMinimizePlayback,
                        onClose = onStopPlayback,
                    )
                } else {
                    PlayerScreen(
                        url = state.url,
                        title = state.title,
                        playbackMode = state.playbackMode,
                        resumePositionSecs = state.resumePositionSecs,
                        positionOffsetSecs = state.positionOffsetSecs,
                        mediaDurationSecs = state.mediaDurationSecs,
                        maxBitrate = state.maxBitrate,
                        subtitles = state.subtitles,
                        hasNext = state.nextEntry != null,
                        nextTitle = state.nextEntry?.let { it.entry.scrapedTitle ?: it.entry.title },
                        onBack = onStopPlayback,
                        onPositionUpdate = { positionSecs, durationSecs ->
                            onSavePlaybackPosition(
                                state.entry,
                                positionSecs,
                                state.mediaDurationSecs ?: durationSecs,
                            )
                        },
                        onContinue = onPlayNext,
                        onSeekOutsideBuffer = onSeekPlayback,
                        onPlaybackRuntimeError = onPlaybackRuntimeError,
                    )
                }
        }

        // Rendered as an overlay sibling on top of whatever the when(state)
        // block above just showed, not instead of it — a track playing in
        // the background should stay visible/controllable no matter which
        // browse/detail screen the user is actually looking at. Never shown
        // while UiState.Player itself is the current screen (the full
        // MusicPlayerScreen already covers this exact session).
        if (minimizedPlayer != null) {
            MiniPlayerBar(
                entry = minimizedPlayer.entry,
                isPlaying = musicIsPlaying,
                artworkUrl = artworkUrl(minimizedPlayer.entry),
                onReopen = onRestoreMinimizedPlayback,
                onStop = onStopMinimizedPlayback,
                modifier = Modifier.align(Alignment.BottomEnd).padding(12.dp),
            )
        }
        if (state is UiState.Catalog && showCatalogExitConfirm) {
            ExitConfirmOverlay(
                onConfirmExit = { (context as? Activity)?.finish() },
                onDismiss = { showCatalogExitConfirm = false },
            )
        }
    }
}

@Composable
private fun KeepScreenAwakeWhile(enabled: Boolean) {
    val activity = LocalContext.current as? Activity
    DisposableEffect(activity, enabled) {
        if (enabled) {
            activity?.window?.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
        onDispose {
            if (enabled) {
                activity?.window?.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
        }
    }
}
