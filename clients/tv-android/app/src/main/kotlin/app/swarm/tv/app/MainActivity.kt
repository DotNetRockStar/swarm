package app.swarm.tv.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewmodel.compose.viewModel
import app.swarm.tv.app.data.AndroidDeviceIdentity
import app.swarm.tv.app.data.AndroidSwarmMembershipStore
import app.swarm.tv.app.data.AndroidTokenStore
import app.swarm.tv.app.data.AndroidWatchStateStore
import app.swarm.tv.app.data.SwarmViewModel
import app.swarm.tv.app.data.UiState
import app.swarm.tv.app.data.androidMachineId
import app.swarm.tv.app.ui.screens.AlbumScreen
import app.swarm.tv.app.ui.screens.ArtistShelfScreen
import app.swarm.tv.app.ui.screens.CatalogScreen
import app.swarm.tv.app.ui.screens.EpisodeDetailScreen
import app.swarm.tv.app.ui.screens.MovieDetailScreen
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

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val tokenStore = AndroidTokenStore(applicationContext)
        val watchStateStore = AndroidWatchStateStore(applicationContext)
        val membershipStore = AndroidSwarmMembershipStore(applicationContext)
        val machineId = androidMachineId(applicationContext)
        val certFingerprint = AndroidDeviceIdentity.ensureFingerprint()
        val certificate = AndroidDeviceIdentity.certificate()
        val privateKey = AndroidDeviceIdentity.privateKey()
        val factory = object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T =
                SwarmViewModel(tokenStore, machineId, certFingerprint, certificate, privateKey, watchStateStore, membershipStore) as T
        }

        setContent {
            SwarmTvTheme {
                Box(modifier = Modifier.fillMaxSize().background(SwarmBackground)) {
                    val viewModel: SwarmViewModel = viewModel(factory = factory)
                    val state by viewModel.state.collectAsState()
                    SwarmApp(
                        state = state,
                        onSubmit = viewModel::submitPasscode,
                        onResync = viewModel::resync,
                        onBrowseCatalog = viewModel::browseCatalog,
                        onPlay = viewModel::play,
                        onPlayNext = viewModel::playNext,
                        onStopPlayback = viewModel::stopPlayback,
                        onBackToDashboard = viewModel::backToDashboard,
                        artworkUrl = viewModel::artworkUrl,
                        backdropUrl = viewModel::backdropUrl,
                        onSavePlaybackPosition = viewModel::savePlaybackPosition,
                        onOpenSettings = viewModel::openSettings,
                        onJoinAdditionalSwarm = viewModel::joinAdditionalSwarm,
                        onLeaveSwarm = viewModel::leaveSwarm,
                        onSwitchActiveSwarm = viewModel::switchActiveSwarm,
                        onBackFromSettings = viewModel::backFromSettings,
                        onOpenMovie = viewModel::openMovieDetail,
                        onBackFromMovie = viewModel::backFromMovieDetail,
                        onOpenArtistShelf = viewModel::openArtistShelf,
                        onOpenArtist = viewModel::openArtistAlbums,
                        onBackFromArtistShelf = viewModel::backFromArtistShelf,
                        onBackFromArtistAlbums = viewModel::backFromArtistAlbums,
                        onOpenShowShelf = viewModel::openShowShelf,
                        onOpenShow = viewModel::openShowSeasons,
                        onBackFromShowShelf = viewModel::backFromShowShelf,
                        onBackFromShowSeasons = viewModel::backFromShowSeasons,
                        onOpenEpisode = viewModel::openEpisodeDetail,
                        onBackFromEpisode = viewModel::backFromEpisodeDetail,
                    )
                }
            }
        }
    }
}

@Composable
private fun SwarmApp(
    state: UiState,
    onSubmit: (baseUrl: String, code: String, deviceName: String) -> Unit,
    onResync: () -> Unit,
    onBrowseCatalog: () -> Unit,
    onPlay: (MergedEntry) -> Unit,
    onPlayNext: () -> Unit,
    onStopPlayback: () -> Unit,
    onBackToDashboard: () -> Unit,
    artworkUrl: (MergedEntry) -> String?,
    backdropUrl: (MergedEntry) -> String?,
    onSavePlaybackPosition: (fingerprint: String, positionSecs: Double, durationSecs: Double) -> Unit,
    onOpenSettings: () -> Unit,
    onJoinAdditionalSwarm: (code: String) -> Unit,
    onLeaveSwarm: (swarmId: String) -> Unit,
    onSwitchActiveSwarm: (swarmId: String) -> Unit,
    onBackFromSettings: () -> Unit,
    onOpenMovie: (MergedEntry) -> Unit,
    onBackFromMovie: () -> Unit,
    onOpenArtistShelf: () -> Unit,
    onOpenArtist: (ArtistGroup) -> Unit,
    onBackFromArtistShelf: () -> Unit,
    onBackFromArtistAlbums: () -> Unit,
    onOpenShowShelf: () -> Unit,
    onOpenShow: (ShowGroup) -> Unit,
    onBackFromShowShelf: () -> Unit,
    onBackFromShowSeasons: () -> Unit,
    onOpenEpisode: (MergedEntry) -> Unit,
    onBackFromEpisode: () -> Unit,
) {
    when (state) {
        is UiState.PasscodeEntry ->
            PasscodeEntryScreen(isSubmitting = false, errorMessage = null, onSubmit = onSubmit)
        is UiState.Registering ->
            PasscodeEntryScreen(isSubmitting = true, errorMessage = null, onSubmit = onSubmit)
        is UiState.Error ->
            PasscodeEntryScreen(isSubmitting = false, errorMessage = state.message, onSubmit = onSubmit)
        is UiState.Dashboard ->
            SwarmDashboardScreen(state.swarm, state.devices, state.resyncing, onResync, onBrowseCatalog, onOpenSettings)
        is UiState.Settings ->
            SwarmSettingsScreen(
                allSwarms = state.allSwarms,
                activeSwarmId = state.activeSwarmId,
                busy = state.busy,
                errorMessage = state.error,
                onJoin = onJoinAdditionalSwarm,
                onLeave = onLeaveSwarm,
                onSwitchActive = onSwitchActiveSwarm,
                onBack = onBackFromSettings,
            )
        is UiState.Catalog ->
            CatalogScreen(
                state.swarm,
                state.entries,
                state.loading,
                state.unreachable,
                state.playbackError,
                artworkUrl,
                onOpenMovie,
                onOpenArtistShelf,
                onOpenArtist,
                onOpenShowShelf,
                onOpenShow,
                onBackToDashboard,
            )
        is UiState.ArtistShelf ->
            ArtistShelfScreen(state.artists, onOpenArtist = onOpenArtist, onBack = onBackFromArtistShelf)
        is UiState.ArtistAlbums ->
            AlbumScreen(state.artist, artworkUrl, onPlay = onPlay, onBack = onBackFromArtistAlbums)
        is UiState.MovieDetail ->
            MovieDetailScreen(state.entry, artworkUrl, backdropUrl, onPlay = onPlay, onBack = onBackFromMovie)
        is UiState.ShowShelf ->
            ShowShelfScreen(state.shows, artworkUrl, onOpenShow = onOpenShow, onBack = onBackFromShowShelf)
        is UiState.ShowSeasons ->
            SeasonScreen(state.show, artworkUrl, onOpenEpisode = onOpenEpisode, onBack = onBackFromShowSeasons)
        is UiState.EpisodeDetail ->
            EpisodeDetailScreen(state.show, state.entry, artworkUrl, backdropUrl, onPlay = onPlay, onBack = onBackFromEpisode)
        is UiState.Player ->
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
            )
    }
}
