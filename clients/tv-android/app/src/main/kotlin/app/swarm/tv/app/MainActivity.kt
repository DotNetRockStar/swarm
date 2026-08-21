package app.swarm.tv.app

import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
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
import app.swarm.tv.app.data.AndroidAppSettingsStore
import app.swarm.tv.app.data.AndroidConnectionStore
import app.swarm.tv.app.data.AndroidDeviceIdentity
import app.swarm.tv.app.data.AndroidKidModeStore
import app.swarm.tv.app.data.AndroidLanConnectionStore
import app.swarm.tv.app.data.AndroidLikedEntriesStore
import app.swarm.tv.app.data.AndroidTokenStore
import app.swarm.tv.app.data.KidModeSettings
import app.swarm.tv.app.data.LanDiscoveryManager
import app.swarm.tv.app.data.LanServer
import app.swarm.tv.app.data.AndroidWatchStateStore
import app.swarm.tv.app.data.SwarmViewModel
import app.swarm.tv.app.data.UiState
import app.swarm.tv.app.data.androidMachineId
import app.swarm.tv.app.data.resolveDeviceName
import app.swarm.tv.app.ui.components.SwarmLoadingIndicator
import app.swarm.tv.app.ui.screens.AlbumScreen
import app.swarm.tv.app.ui.screens.ArtistShelfScreen
import app.swarm.tv.app.ui.screens.CatalogScreen
import app.swarm.tv.app.ui.screens.MiniPlayerBar
import app.swarm.tv.app.ui.screens.MovieDetailScreen
import app.swarm.tv.app.ui.screens.MovieShelfScreen
import app.swarm.tv.app.ui.screens.MusicPlayerScreen
import app.swarm.tv.app.ui.screens.PasscodeEntryScreen
import app.swarm.tv.app.ui.screens.PlayerScreen
import app.swarm.tv.app.ui.screens.SeasonScreen
import app.swarm.tv.app.ui.screens.ShowShelfScreen
import app.swarm.tv.app.ui.screens.SwarmDashboardScreen
import app.swarm.tv.app.ui.screens.SwarmSettingsScreen
import app.swarm.tv.app.ui.theme.SwarmBackground
import app.swarm.tv.app.ui.theme.SwarmTvTheme
import app.swarm.tv.core.catalog.ArtistGroup
import app.swarm.tv.core.catalog.MergedEntry
import app.swarm.tv.core.catalog.ShowGroup
import app.swarm.tv.core.peer.MediaKind

class MainActivity : ComponentActivity() {
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
        val connectionStore = AndroidConnectionStore(applicationContext)
        val settingsStore = AndroidAppSettingsStore(applicationContext)
        val likedEntriesStore = AndroidLikedEntriesStore(applicationContext)
        val kidModeStore = AndroidKidModeStore(applicationContext)
        val lanDiscovery = LanDiscoveryManager(applicationContext)
        val lanConnectionStore = AndroidLanConnectionStore(applicationContext)
        val machineId = androidMachineId(applicationContext)
        val defaultDeviceName = resolveDeviceName(applicationContext)
        val certFingerprint = AndroidDeviceIdentity.ensureFingerprint()
        val certificate = AndroidDeviceIdentity.certificate()
        val privateKey = AndroidDeviceIdentity.privateKey()
        val factory = object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T =
                SwarmViewModel(tokenStore, machineId, certFingerprint, certificate, privateKey, watchStateStore, connectionStore, settingsStore, likedEntriesStore, kidModeStore, lanDiscovery, lanConnectionStore) as T
        }

        setContent {
            SwarmTvTheme {
                Box(modifier = Modifier.fillMaxSize().background(SwarmBackground)) {
                    val viewModel: SwarmViewModel = viewModel(factory = factory)
                    val state by viewModel.state.collectAsState()
                    val likedFingerprints by viewModel.likedFingerprints.collectAsState()
                    val kidModeSettings by viewModel.kidModeSettings.collectAsState()
                    val shuffleEnabled by viewModel.shuffleEnabled.collectAsState()
                    val minimizedPlayer by viewModel.minimizedPlayer.collectAsState()
                    val lanServers by viewModel.lanServers.collectAsState()
                    val lanPairingBusy by viewModel.lanPairingBusy.collectAsState()
                    val lanError by viewModel.lanError.collectAsState()
                    SwarmApp(
                        state = state,
                        defaultDeviceName = defaultDeviceName,
                        lanServers = lanServers,
                        lanPairingBusy = lanPairingBusy,
                        lanError = lanError,
                        onConnectLan = viewModel::connectLanServer,
                        onPairLan = viewModel::pairLanServer,
                        isLiked = { entry -> entry.entry.fingerprint in likedFingerprints },
                        onToggleLike = viewModel::toggleLike,
                        kidModeSettings = kidModeSettings,
                        onEnableKidMode = viewModel::enableKidMode,
                        onUpdateKidModeRules = viewModel::updateKidModeRules,
                        onDisableKidMode = viewModel::disableKidMode,
                        shuffleEnabled = shuffleEnabled,
                        onToggleShuffle = viewModel::toggleShuffle,
                        minimizedPlayer = minimizedPlayer,
                        onMinimizePlayback = viewModel::minimizePlayback,
                        onRestoreMinimizedPlayback = viewModel::restoreMinimizedPlayback,
                        onStopMinimizedPlayback = viewModel::stopMinimizedPlayback,
                        onTrackPlaybackEnded = viewModel::onTrackPlaybackEnded,
                        artistPhotoUrl = viewModel::artistPhotoUrl,
                        onSubmit = viewModel::submitPasscode,
                        onResync = viewModel::resync,
                        onBrowseCatalog = viewModel::browseCatalog,
                        onPlay = viewModel::play,
                        onPlayNext = viewModel::playNext,
                        onStopPlayback = viewModel::stopPlayback,
                        onBackToDashboard = viewModel::backToDashboard,
                        artworkUrl = viewModel::artworkUrl,
                        backdropUrl = viewModel::backdropUrl,
                        onReportProblem = viewModel::reportAssetProblem,
                        onSavePlaybackPosition = viewModel::savePlaybackPosition,
                        onPlaybackRuntimeError = viewModel::reportPlaybackRuntimeError,
                        onOpenSettings = viewModel::openSettings,
                        onJoinAdditionalSwarm = viewModel::joinAdditionalSwarm,
                        onLeaveSwarm = viewModel::leaveSwarm,
                        onSwitchActiveSwarm = viewModel::switchActiveSwarm,
                        onUpdateBaseUrl = viewModel::updateBaseUrl,
                        onUpdateDeviceName = viewModel::updateDeviceName,
                        onUpdateArtworkCacheMinutes = viewModel::updateArtworkCacheMinutes,
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
                        onBackFromShowShelf = viewModel::backFromShowShelf,
                        onBackFromShowSeasons = viewModel::backFromShowSeasons,
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
    lanError: String?,
    onConnectLan: (server: LanServer, deviceName: String) -> Unit,
    onPairLan: (server: LanServer, code: String, deviceName: String) -> Unit,
    isLiked: (MergedEntry) -> Boolean,
    onToggleLike: (MergedEntry) -> Unit,
    kidModeSettings: KidModeSettings?,
    onEnableKidMode: (pin: String, allowedKinds: Set<MediaKind>, allowedGenres: Set<String>?, maxMovieRating: String?, maxTvRating: String?) -> Unit,
    onUpdateKidModeRules: (allowedKinds: Set<MediaKind>, allowedGenres: Set<String>?, maxMovieRating: String?, maxTvRating: String?) -> Unit,
    onDisableKidMode: () -> Unit,
    shuffleEnabled: Boolean,
    onToggleShuffle: () -> Unit,
    minimizedPlayer: UiState.Player?,
    onMinimizePlayback: () -> Unit,
    onRestoreMinimizedPlayback: () -> Unit,
    onStopMinimizedPlayback: () -> Unit,
    onTrackPlaybackEnded: () -> Unit,
    artistPhotoUrl: (MergedEntry) -> String?,
    onSubmit: (baseUrl: String, code: String, deviceName: String) -> Unit,
    onResync: () -> Unit,
    onBrowseCatalog: () -> Unit,
    onPlay: (MergedEntry) -> Unit,
    onPlayNext: () -> Unit,
    onStopPlayback: () -> Unit,
    onBackToDashboard: () -> Unit,
    artworkUrl: (MergedEntry) -> String?,
    backdropUrl: (MergedEntry) -> String?,
    onReportProblem: (MergedEntry) -> Unit,
    onSavePlaybackPosition: (fingerprint: String, positionSecs: Double, durationSecs: Double) -> Unit,
    onPlaybackRuntimeError: (message: String) -> Unit,
    onOpenSettings: () -> Unit,
    onJoinAdditionalSwarm: (code: String) -> Unit,
    onLeaveSwarm: (swarmId: String) -> Unit,
    onSwitchActiveSwarm: (swarmId: String) -> Unit,
    onUpdateBaseUrl: (baseUrl: String) -> Unit,
    onUpdateDeviceName: (name: String) -> Unit,
    onUpdateArtworkCacheMinutes: (minutes: Int) -> Unit,
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
                onSavePlaybackPosition(session.fingerprint, positionSecs, session.mediaDurationSecs ?: durationSecs)
            }
            player?.release()
        }
    }

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
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { SwarmLoadingIndicator() }
            is UiState.PasscodeEntry ->
                PasscodeEntryScreen(
                    isSubmitting = false,
                    errorMessage = lanError,
                    defaultDeviceName = defaultDeviceName,
                    lanServers = lanServers,
                    lanPairingBusy = lanPairingBusy,
                    onConnectLan = onConnectLan,
                    onPairLan = onPairLan,
                    onSubmit = onSubmit,
                )
            is UiState.Registering ->
                PasscodeEntryScreen(
                    isSubmitting = true,
                    errorMessage = lanError,
                    defaultDeviceName = defaultDeviceName,
                    lanServers = lanServers,
                    lanPairingBusy = lanPairingBusy,
                    onConnectLan = onConnectLan,
                    onPairLan = onPairLan,
                    onSubmit = onSubmit,
                )
            is UiState.Error ->
                PasscodeEntryScreen(
                    isSubmitting = false,
                    errorMessage = state.message,
                    defaultDeviceName = defaultDeviceName,
                    lanServers = lanServers,
                    lanPairingBusy = lanPairingBusy,
                    onConnectLan = onConnectLan,
                    onPairLan = onPairLan,
                    onSubmit = onSubmit,
                )
            is UiState.Dashboard ->
                SwarmDashboardScreen(state.swarm, state.devices, state.resyncing, onResync, onBrowseCatalog, onOpenSettings)
            is UiState.Settings ->
                SwarmSettingsScreen(
                    allSwarms = state.allSwarms,
                    activeSwarmId = state.activeSwarmId,
                    baseUrl = state.baseUrl,
                    deviceName = state.deviceName,
                    artworkCacheMinutes = state.artworkCacheMinutes,
                    busy = state.busy,
                    errorMessage = state.error,
                    onJoin = onJoinAdditionalSwarm,
                    onLeave = onLeaveSwarm,
                    onSwitchActive = onSwitchActiveSwarm,
                    onUpdateBaseUrl = onUpdateBaseUrl,
                    onUpdateDeviceName = onUpdateDeviceName,
                    onUpdateArtworkCacheMinutes = onUpdateArtworkCacheMinutes,
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
                    onOpenMovie = { entry -> lastFocusedMovieKey = entry.entry.entryKey; onOpenMovie(entry) },
                    onOpenMovieShelf = onOpenMovieShelf,
                    onOpenArtistShelf = onOpenArtistShelf,
                    onOpenArtist = { artist -> lastFocusedArtistKey = artist.artist; onOpenArtist(artist) },
                    onOpenShowShelf = onOpenShowShelf,
                    onOpenShow = { show -> lastFocusedShowKey = show.show; onOpenShow(show) },
                    onBack = onBackToDashboard,
                    initialFocusMovieKey = lastFocusedMovieKey,
                    initialFocusShowKey = lastFocusedShowKey,
                    initialFocusArtistKey = lastFocusedArtistKey,
                    isLiked = isLiked,
                )
            is UiState.ArtistShelf ->
                ArtistShelfScreen(state.artists, onOpenArtist = onOpenArtist, onBack = onBackFromArtistShelf)
            is UiState.ArtistAlbums ->
                AlbumScreen(state.artist, artworkUrl, onPlay = onPlay, onBack = onBackFromArtistAlbums)
            is UiState.MovieShelf ->
                MovieShelfScreen(state.movies, artworkUrl, onOpenMovie = onOpenMovie, onBack = onBackFromMovieShelf)
            is UiState.MovieDetail ->
                MovieDetailScreen(
                    state.entry,
                    artworkUrl,
                    backdropUrl,
                    onPlay = onPlay,
                    onBack = onBackFromMovie,
                    onReportProblem = onReportProblem,
                    isLiked = isLiked(state.entry),
                    onToggleLike = { onToggleLike(state.entry) },
                )
            is UiState.ShowShelf ->
                ShowShelfScreen(state.shows, artworkUrl, onOpenShow = onOpenShow, onBack = onBackFromShowShelf)
            is UiState.ShowSeasons ->
                SeasonScreen(state.show, artworkUrl, onPlayEpisode = onPlay, onBack = onBackFromShowSeasons)
            is UiState.Player ->
                if (state.entry.entry.kind == MediaKind.TRACK) {
                    MusicPlayerScreen(
                        entry = state.entry,
                        nextTitle = state.nextEntry?.let { it.entry.scrapedTitle ?: it.entry.title },
                        isPlaying = musicIsPlaying,
                        isLoading = musicIsLoading,
                        shuffleEnabled = shuffleEnabled,
                        isLiked = isLiked(state.entry),
                        artworkUrl = artworkUrl(state.entry),
                        artistPhotoUrl = artistPhotoUrl(state.entry),
                        onTogglePlayPause = { musicPlayer?.let { it.playWhenReady = !it.playWhenReady } },
                        onToggleShuffle = onToggleShuffle,
                        onToggleLike = { onToggleLike(state.entry) },
                        onSkipNext = onPlayNext,
                        onMinimize = onMinimizePlayback,
                    )
                } else {
                    PlayerScreen(
                        url = state.url,
                        title = state.title,
                        resumePositionSecs = state.resumePositionSecs,
                        positionOffsetSecs = state.positionOffsetSecs,
                        maxBitrate = state.maxBitrate,
                        hasNext = state.nextEntry != null,
                        nextTitle = state.nextEntry?.let { it.entry.scrapedTitle ?: it.entry.title },
                        onBack = onStopPlayback,
                        onPositionUpdate = { positionSecs, durationSecs ->
                            onSavePlaybackPosition(
                                state.fingerprint,
                                positionSecs,
                                state.mediaDurationSecs ?: durationSecs,
                            )
                        },
                        onContinue = onPlayNext,
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
            )
        }
    }
}
