/**
 * The merged multi-server catalog — the payoff of `peer_addr` self-report
 * plus [app.swarm.tv.core.catalog.CatalogSession]: every server in the
 * swarm that's currently dialable is connected to directly, and their
 * libraries appear as one browsable list grouped by kind. Movies stay a
 * flat shelf (each is its own leaf); Shows and Music are grouped
 * client-side ([app.swarm.tv.core.catalog.CatalogGrouping]) into Show and
 * Artist shelves — clicking a card in either goes straight one level
 * deeper ([SeasonScreen]/[AlbumScreen]), and the row's own header opens a
 * fuller grid ([ShowShelfScreen]/[ArtistShelfScreen]) for browsing many at
 * once.
 *
 * No title/subtitle/Back button here on purpose — every pixel is real
 * estate a 10-foot UI is short on, and the remote's own physical Back
 * button (wired via [BackHandler]) already does what an on-screen "Back"
 * button would. Search stays with the catalog, while filters live in the
 * persistent rail on the left so they are always discoverable by D-pad.
 */
package app.swarm.tv.app.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.focusable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.grid.itemsIndexed as gridItemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.ButtonDefaults
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import app.swarm.tv.R
import app.swarm.tv.app.data.BrowsePreview
import app.swarm.tv.app.data.WatchlistKeys
import app.swarm.tv.app.ui.components.SwarmLoadingIndicator
import app.swarm.tv.app.ui.components.TvOutlinedTextField
import app.swarm.tv.app.ui.components.swarmActionButtonColors
import app.swarm.tv.app.ui.PrefetchArtworkRow
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmLike
import app.swarm.tv.app.ui.theme.SwarmBorder
import app.swarm.tv.app.ui.theme.SwarmMuted
import app.swarm.tv.app.ui.theme.SwarmSurface
import app.swarm.tv.app.ui.theme.SwarmSurfaceMuted
import app.swarm.tv.app.ui.theme.SwarmText
import app.swarm.tv.core.catalog.ArtistGroup
import app.swarm.tv.core.catalog.CatalogGrouping
import app.swarm.tv.core.catalog.MergedEntry
import app.swarm.tv.core.catalog.ShowGroup
import app.swarm.tv.core.peer.MediaKind
import app.swarm.tv.core.rest.SwarmDevice
import app.swarm.tv.core.watch.WatchState
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** Mirrors the media server's own browse-page filter (`media.js`'s `kindFilter`) — same four choices, same meaning. */
internal enum class KindFilter(val label: String) {
    ALL("All"), MOVIES("Movies"), SHOWS("Shows"), MUSIC("Music"),
}

internal data class CatalogBrowseState(
    val searchText: String = "",
    val appliedSearchQuery: String = "",
    val kindFilter: KindFilter = KindFilter.ALL,
    val genreFilter: String? = null,
    val ratingFilter: String? = null,
    val likedOnly: Boolean = false,
)

private enum class QuickAccessKind { MOVIE, EPISODE, SHOW }

private data class QuickAccessItem(
    val key: String,
    val title: String,
    val subtitle: String,
    val representative: MergedEntry,
    val kind: QuickAccessKind,
    val progress: Float? = null,
    val updatedAt: Long = 0,
    val show: ShowGroup? = null,
)

@Composable
internal fun CatalogScreen(
    entries: List<MergedEntry>,
    loading: Boolean,
    unreachable: List<SwarmDevice>,
    playbackError: String?,
    artworkUrl: (MergedEntry) -> String?,
    artistPhotoUrl: (MergedEntry) -> String?,
    onOpenMovie: (MergedEntry) -> Unit,
    onOpenMovieShelf: () -> Unit,
    onOpenArtistShelf: () -> Unit,
    onOpenArtist: (ArtistGroup) -> Unit,
    onOpenShowShelf: () -> Unit,
    onOpenShow: (ShowGroup) -> Unit,
    onOpenSwarm: () -> Unit,
    onBack: () -> Unit,
    // Which card should get initial D-pad focus in the Movies/Shows/Music
    // *top-level* row specifically (not a genre sub-shelf — see MainActivity's
    // doc comment on where these come from) — set from whichever card was
    // last opened into a detail/season/album screen, so coming back via the
    // remote's Back button lands focus where the user actually was instead
    // of always resetting to the first card. `null` (a first-ever visit, or
    // the remembered card no longer matches anything currently shown) falls
    // back to the previous "focus the first card of the first non-empty
    // section" behavior.
    initialFocusMovieKey: String? = null,
    initialFocusShowKey: String? = null,
    initialFocusArtistKey: String? = null,
    isLiked: (MergedEntry) -> Boolean = { false },
    watchStates: Map<String, WatchState>,
    watchlistKeys: Set<String>,
    onPlay: (MergedEntry) -> Unit,
    preview: BrowsePreview?,
    onStartPreview: (MergedEntry) -> Unit,
    onStopPreview: () -> Unit,
    onPreviewFinished: (String) -> Unit,
    initialBrowseState: CatalogBrowseState = CatalogBrowseState(),
    onBrowseStateChange: (CatalogBrowseState) -> Unit = {},
) {
    var focusedPreviewEntry by remember { mutableStateOf<MergedEntry?>(null) }
    var expandedPreviewEntryKey by remember { mutableStateOf<String?>(null) }
    val previewFinished: (String) -> Unit = { sessionId ->
        if (preview?.sessionId == sessionId && preview.entryKey == expandedPreviewEntryKey) {
            expandedPreviewEntryKey = null
        }
        onPreviewFinished(sessionId)
    }
    val previewFocusChanged: (MergedEntry, Boolean) -> Unit = { entry, focused ->
        if (focused) {
            focusedPreviewEntry = entry
        } else if (focusedPreviewEntry?.entry?.entryKey == entry.entry.entryKey) {
            focusedPreviewEntry = null
        }
    }
    // Warm the stream halfway through the dwell, but keep the card at poster
    // width and the player paused/hidden until all four seconds have elapsed.
    // Moving focus cancels both stages and releases any session already made.
    LaunchedEffect(focusedPreviewEntry?.entry?.entryKey) {
        onStopPreview()
        expandedPreviewEntryKey = null
        val entry = focusedPreviewEntry ?: return@LaunchedEffect
        delay(2_000)
        onStartPreview(entry)
        delay(2_000)
        expandedPreviewEntryKey = entry.entry.entryKey
    }
    // searchText is what's live in the field as the user types; appliedSearchQuery
    // is what actually drives filtering below. Keeping them separate is the fix for
    // a real bug: when a single `searchQuery` backed both the field and the
    // `remember(entries, searchQuery, kindFilter)` filter, every keystroke forced a
    // full recomposition of the catalog list underneath the still-focused field,
    // which disrupted focus/IME state and made the search box appear to "exit" after
    // one letter. appliedSearchQuery now only updates from TvOutlinedTextField's
    // onSubmit (D-pad Enter/Done), so typing no longer touches the list at all.
    var searchText by remember { mutableStateOf(initialBrowseState.searchText) }
    var appliedSearchQuery by remember { mutableStateOf(initialBrowseState.appliedSearchQuery) }
    var kindFilter by remember { mutableStateOf(initialBrowseState.kindFilter) }
    // Categories = genres, same field the media server's own category picker
    // writes — null means "no genre filter", matching that server-side
    // filter's "All categories" option.
    var genreFilter by remember { mutableStateOf(initialBrowseState.genreFilter) }
    var ratingFilter by remember { mutableStateOf(initialBrowseState.ratingFilter) }
    var likedOnly by remember { mutableStateOf(initialBrowseState.likedOnly) }
    var filterRailExpanded by remember { mutableStateOf(false) }
    var automaticInitialFocusEnabled by remember { mutableStateOf(true) }
    var initialSelectionRestorePending by remember(
        initialFocusMovieKey,
        initialFocusShowKey,
        initialFocusArtistKey,
    ) {
        mutableStateOf(initialFocusMovieKey != null || initialFocusShowKey != null || initialFocusArtistKey != null)
    }
    LaunchedEffect(loading, entries, initialFocusMovieKey, initialFocusShowKey, initialFocusArtistKey) {
        if (!loading && entries.isEmpty()) initialSelectionRestorePending = false
    }
    val currentBrowseState by rememberUpdatedState(
        CatalogBrowseState(searchText, appliedSearchQuery, kindFilter, genreFilter, ratingFilter, likedOnly),
    )
    DisposableEffect(Unit) {
        onDispose {
            onStopPreview()
            onBrowseStateChange(currentBrowseState)
        }
    }
    BackHandler(enabled = filterRailExpanded) { filterRailExpanded = false }
    BackHandler(enabled = !filterRailExpanded, onBack = onBack)
    val anyFilterActive = kindFilter != KindFilter.ALL || genreFilter != null || ratingFilter != null || likedOnly
    // The controls are the first item in the same lazy catalog so they scroll
    // away with the shelves. Every possible layout still attaches this
    // requester to its first active card; the controls' DOWN handler has one
    // deterministic way into content even when lazy children are not yet
    // candidates for geometric focus search.
    val catalogEntryFocusRequester = remember { FocusRequester() }
    val filterRailFocusRequester = remember { FocusRequester() }
    val watchlistRowFocusRequester = remember { FocusRequester() }
    val catalogListState = rememberLazyListState()
    val focusNavigationScope = rememberCoroutineScope()
    val catalogControls: @Composable () -> Unit = {
        CatalogControls(
            searchText = searchText,
            onSearchTextChange = { searchText = it },
            onSubmitSearch = { appliedSearchQuery = searchText },
            showClear = searchText.isNotEmpty() || appliedSearchQuery.isNotEmpty() || anyFilterActive,
            onClear = {
                searchText = ""
                appliedSearchQuery = ""
                kindFilter = KindFilter.ALL
                genreFilter = null
                ratingFilter = null
                likedOnly = false
            },
            onOpenSwarm = onOpenSwarm,
            unreachable = unreachable,
            playbackError = playbackError,
            catalogEntryFocusRequester = catalogEntryFocusRequester,
            filterRailFocusRequester = filterRailFocusRequester,
        )
    }

    // Genre and rating choices follow the selected media kind. This keeps
    // the rail concise and prevents it from offering filters that cannot
    // produce any results.
    val kindScoped = remember(entries, kindFilter) {
        if (kindFilter == KindFilter.ALL) entries else entries.filter { kindMatches(it, kindFilter) }
    }
    val allGenres = remember(kindScoped) {
        kindScoped.flatMap { it.entry.genres }.distinct().sortedWith(String.CASE_INSENSITIVE_ORDER)
    }
    val allRatings = remember(kindScoped) {
        kindScoped.mapNotNull { it.entry.rating }.distinct().sortedWith(String.CASE_INSENSITIVE_ORDER)
    }

    Row(modifier = Modifier.fillMaxSize()) {
        FilterRail(
            expanded = filterRailExpanded,
            // On return from a detail screen Compose can briefly assign
            // focus to the first focusable item in layout order (the rail)
            // before the off-screen selected card has been composed. Do not
            // interpret that transient focus as an intentional LEFT press;
            // doing so expands the rail and cancels the card restoration.
            onExpand = {
                if (!initialSelectionRestorePending) filterRailExpanded = true
            },
            firstFocusRequester = filterRailFocusRequester,
            contentFocusRequester = catalogEntryFocusRequester,
            kindFilter = kindFilter,
            onKindSelect = { kindFilter = it },
            genres = allGenres,
            genreFilter = genreFilter,
            onGenreSelect = { genreFilter = it },
            ratings = allRatings,
            ratingFilter = ratingFilter,
            onRatingSelect = { ratingFilter = it },
            likedOnly = likedOnly,
            onLikedOnlyToggle = { likedOnly = !likedOnly },
            anyFilterActive = anyFilterActive,
            onClear = {
                kindFilter = KindFilter.ALL
                genreFilter = null
                ratingFilter = null
                likedOnly = false
            },
        )
        // Small padding here, not the ~40dp this screen used to carry: MainActivity's
        // contentModifier already reserves the TV-safe overscan margin around every
        // non-Player screen, so a second, separate margin here just doubled up as extra
        // dead space on every edge — confirmed live as a persistent empty border around
        // the browse page no matter how this screen's own padding was tuned.
        Column(
            modifier = Modifier.weight(1f).fillMaxSize()
                .padding(horizontal = 8.dp, vertical = 8.dp)
                .onFocusChanged { state ->
                    if (state.hasFocus) {
                        initialSelectionRestorePending = false
                        filterRailExpanded = false
                    }
                },
        ) {
            when {
                // Same GIF/caption treatment PlayerScreen's own "negotiated,
                // now waiting" state uses — real feedback from live use:
                // merging every reachable server's catalog is a real,
                // sometimes-noticeable network wait too, and there's no
                // reason it should feel less alive than the player's.
                loading -> Column(Modifier.fillMaxSize()) {
                    catalogControls()
                    Box(Modifier.fillMaxWidth().weight(1f), contentAlignment = Alignment.Center) { SwarmLoadingIndicator() }
                }
                entries.isEmpty() -> Column {
                    catalogControls()
                    Text("Nothing in the catalog yet.", color = SwarmMuted, fontSize = 14.sp)
                }
                else -> {
                    // Same multi-field match the media server's own search box
                    // uses (`media.js`'s `filteredEntries`) — matches on
                    // whichever identifying name field is present, so
                    // searching a show/artist name keeps every episode/track
                    // under it even though the entry's own title might not
                    // mention it.
                    val filtered = remember(entries, appliedSearchQuery, kindFilter, genreFilter, ratingFilter, likedOnly) {
                        val q = appliedSearchQuery.trim().lowercase()
                        entries.filter { merged ->
                            val e = merged.entry
                            if (!kindMatches(merged, kindFilter)) return@filter false
                            if (genreFilter != null && !e.genres.contains(genreFilter)) return@filter false
                            if (ratingFilter != null && e.rating != ratingFilter) return@filter false
                            if (likedOnly && !isLiked(merged)) return@filter false
                            if (q.isEmpty()) return@filter true
                            listOfNotNull(e.scrapedTitle, e.title, e.artist, e.album, e.showTitle).any { it.lowercase().contains(q) }
                        }
                    }
                    val movies = remember(filtered) { filtered.filter { it.entry.kind == MediaKind.MOVIE } }
                    val shows = remember(filtered) { CatalogGrouping.groupEpisodesByShowSeason(filtered) }
                    val artists = remember(filtered) { CatalogGrouping.groupTracksByArtistAlbum(filtered) }

                    // Quick-access rows intentionally use the full currently-visible
                    // catalog, not a genre/search subset, and disappear while a user
                    // is actively filtering. That keeps them predictable home rows
                    // rather than making saved items appear to vanish mid-search.
                    val showQuickAccess = appliedSearchQuery.isBlank() && kindFilter == KindFilter.ALL &&
                        genreFilter == null && ratingFilter == null && !likedOnly
                    val allShows = remember(entries) { CatalogGrouping.groupEpisodesByShowSeason(entries) }
                    val showByEpisode = remember(allShows) {
                        buildMap {
                            for (show in allShows) {
                                for (season in show.seasons) {
                                    for (episode in season.episodes) put(episode.entry.fingerprint, show)
                                }
                            }
                        }
                    }
                    val continueWatching = remember(entries, watchStates, showQuickAccess, showByEpisode) {
                        if (!showQuickAccess) {
                            emptyList()
                        } else {
                            val inProgress = entries.mapNotNull { entry ->
                                if (entry.entry.kind == MediaKind.TRACK) return@mapNotNull null
                                val saved = watchStates[entry.entry.fingerprint] ?: return@mapNotNull null
                                if (saved.watched || saved.positionSecs <= 0.0) return@mapNotNull null
                                entry to saved
                            }
                            val movieItems = inProgress
                                .filter { it.first.entry.kind == MediaKind.MOVIE }
                                .map { (entry, saved) ->
                                    QuickAccessItem(
                                        key = "continue-movie-${entry.entry.fingerprint}",
                                        title = entry.entry.scrapedTitle ?: entry.entry.title,
                                        subtitle = "Movie • ${saved.percentComplete()}% watched",
                                        representative = entry,
                                        kind = QuickAccessKind.MOVIE,
                                        progress = saved.progressFraction(),
                                        updatedAt = saved.updatedAt,
                                    )
                                }
                            val episodeItems = inProgress
                                .filter { it.first.entry.kind == MediaKind.EPISODE }
                                .groupBy { (entry, _) -> showByEpisode[entry.entry.fingerprint]?.show ?: entry.entry.showTitle.orEmpty() }
                                .values
                                .mapNotNull { candidates -> candidates.maxByOrNull { it.second.updatedAt } }
                                .map { (entry, saved) ->
                                    val show = showByEpisode[entry.entry.fingerprint]
                                    val episodeLabel = listOfNotNull(
                                        entry.entry.season?.let { "S$it" },
                                        entry.entry.episode?.let { "E$it" },
                                    ).joinToString(" ")
                                    QuickAccessItem(
                                        key = "continue-episode-${entry.entry.fingerprint}",
                                        title = show?.show ?: entry.entry.showTitle ?: entry.entry.title,
                                        subtitle = listOf(episodeLabel, "${saved.percentComplete()}% watched").filter { it.isNotBlank() }.joinToString(" • "),
                                        representative = entry,
                                        kind = QuickAccessKind.EPISODE,
                                        progress = saved.progressFraction(),
                                        updatedAt = saved.updatedAt,
                                        show = show,
                                    )
                                }
                            (movieItems + episodeItems).sortedByDescending { it.updatedAt }
                        }
                    }
                    val watchlist = remember(entries, allShows, watchStates, watchlistKeys, showQuickAccess) {
                        if (!showQuickAccess) {
                            emptyList()
                        } else {
                            val movieItems = entries
                                .filter { entry ->
                                    entry.entry.kind == MediaKind.MOVIE &&
                                        WatchlistKeys.movie(entry) in watchlistKeys &&
                                        watchStates[entry.entry.fingerprint]?.watched != true
                                }
                                .map { entry ->
                                    QuickAccessItem(
                                        key = WatchlistKeys.movie(entry),
                                        title = entry.entry.scrapedTitle ?: entry.entry.title,
                                        subtitle = "Movie",
                                        representative = entry,
                                        kind = QuickAccessKind.MOVIE,
                                    )
                                }
                            val showItems = allShows
                                .filter { show -> WatchlistKeys.show(show) in watchlistKeys && !show.isWatched(watchStates) }
                                .mapNotNull { show ->
                                    val representative = CatalogGrouping.previewSeasons(show).firstOrNull()?.episodes?.firstOrNull()
                                        ?: show.seasons.firstOrNull()?.episodes?.firstOrNull()
                                        ?: return@mapNotNull null
                                    val seasons = CatalogGrouping.previewSeasons(show).size
                                    QuickAccessItem(
                                        key = WatchlistKeys.show(show),
                                        title = show.show,
                                        subtitle = "$seasons season" + if (seasons == 1) "" else "s",
                                        representative = representative,
                                        kind = QuickAccessKind.SHOW,
                                        show = show,
                                    )
                                }
                            (movieItems + showItems).sortedBy { it.title.lowercase() }
                        }
                    }

                    // Netflix-style "Top picks in <genre>" sub-shelves, one per
                    // kind — only while browsing unfiltered-by-genre (once a
                    // genre is actually picked via the Genre button, `filtered`
                    // above already reduces every existing shelf to just that
                    // genre, so a further breakdown would be redundant).
                    val movieGenreShelves = remember(movies, genreFilter) {
                        if (genreFilter != null) emptyList() else topGenreShelves(movies) { it }
                    }
                    val showGenreShelves = remember(filtered, genreFilter) {
                        if (genreFilter != null) emptyList() else {
                            topGenreShelves(filtered.filter { it.entry.kind == MediaKind.EPISODE }) { CatalogGrouping.groupEpisodesByShowSeason(it) }
                        }
                    }
                    val musicGenreShelves = remember(filtered, genreFilter) {
                        if (genreFilter != null) emptyList() else {
                            topGenreShelves(filtered.filter { it.entry.kind == MediaKind.TRACK }) { CatalogGrouping.groupTracksByArtistAlbum(it) }
                        }
                    }

                    if (movies.isEmpty() && shows.isEmpty() && artists.isEmpty()) {
                        Column {
                            catalogControls()
                            Text("No matches for the current search/filter.", color = SwarmMuted, fontSize = 14.sp)
                        }
                    } else {
                        // Which top-level row gets *default* (first-card)
                        // focus when nothing is being restored — unchanged
                        // from before, just no longer paired with a single
                        // externally-owned FocusRequester (each row now
                        // decides its own target index, see MovieRow/
                        // ShowShelfRow/ArtistShelfRow's restoreFocusIndex).
                        val firstSection = when {
                            continueWatching.isNotEmpty() -> "continue"
                            watchlist.isNotEmpty() -> "watchlist"
                            movies.isNotEmpty() -> "movies"
                            shows.isNotEmpty() -> "shows"
                            artists.isNotEmpty() -> "music"
                            else -> null
                        }
                        // -1 (not found) becomes null: "nothing to restore in
                        // this particular row" is exactly the same case as
                        // "nothing was ever remembered" from the row's own
                        // point of view.
                        val movieRestoreIndex = remember(movies, initialFocusMovieKey) {
                            initialFocusMovieKey?.let { key -> movies.indexOfFirst { it.entry.entryKey == key }.takeIf { it >= 0 } }
                        }
                        val showRestoreIndex = remember(shows, initialFocusShowKey) {
                            initialFocusShowKey?.let { key -> shows.indexOfFirst { it.show == key }.takeIf { it >= 0 } }
                        }
                        val artistRestoreIndex = remember(artists, initialFocusArtistKey) {
                            initialFocusArtistKey?.let { key -> artists.indexOfFirst { it.artist == key }.takeIf { it >= 0 } }
                        }

                        // The horizontal row containing a restored card may
                        // be well below the viewport and therefore not yet
                        // composed. Scroll the parent list to that row first;
                        // the row's own focus-restoration effect then scrolls
                        // horizontally and focuses the exact selected title.
                        var nextSectionIndex = 1 // Search controls.
                        if (continueWatching.isNotEmpty()) nextSectionIndex++
                        val watchlistSectionIndex = nextSectionIndex.takeIf { watchlist.isNotEmpty() }
                        if (watchlist.isNotEmpty()) nextSectionIndex++
                        val movieSectionIndex = nextSectionIndex.takeIf { movies.isNotEmpty() }
                        if (movies.isNotEmpty()) nextSectionIndex++
                        nextSectionIndex += movieGenreShelves.size
                        val showSectionIndex = nextSectionIndex.takeIf { shows.isNotEmpty() }
                        if (shows.isNotEmpty()) nextSectionIndex++
                        nextSectionIndex += showGenreShelves.size
                        val artistSectionIndex = nextSectionIndex.takeIf { artists.isNotEmpty() }
                        val restoreSectionIndex = when {
                            movieRestoreIndex != null -> movieSectionIndex
                            showRestoreIndex != null -> showSectionIndex
                            artistRestoreIndex != null -> artistSectionIndex
                            else -> null
                        }
                        LaunchedEffect(restoreSectionIndex) {
                            if (restoreSectionIndex != null) {
                                filterRailExpanded = false
                                catalogListState.scrollToItem(restoreSectionIndex)
                            } else {
                                // A remembered title can disappear after a
                                // rescan/filter change. In that case allow
                                // normal rail focus and fall back to the
                                // first visible content card.
                                initialSelectionRestorePending = false
                            }
                        }
                        val restoringSelection = restoreSectionIndex != null

                        // A genre is selected: swap the Netflix-style
                        // horizontal shelves for the same full-grid "Browse
                        // all" layout MovieShelfScreen/ShowShelfScreen/
                        // ArtistShelfScreen already use, one section per
                        // kind — real feedback from live use. A single
                        // horizontal row is a fine width for "here's a taste
                        // of Action movies" browsing, but a bad one for
                        // "show me everything tagged Action", which is
                        // exactly what picking a genre is asking for.
                        if (genreFilter != null) {
                            GenreFilteredGrid(
                                movies,
                                shows,
                                artists,
                                artworkUrl,
                                artistPhotoUrl,
                                onOpenMovie,
                                onOpenShow,
                                onOpenArtist,
                                isLiked,
                                firstFocusRequester = catalogEntryFocusRequester,
                                requestInitialFocus = automaticInitialFocusEnabled && !filterRailExpanded,
                                initialFocusMovieKey = initialFocusMovieKey,
                                initialFocusShowKey = initialFocusShowKey,
                                initialFocusArtistKey = initialFocusArtistKey,
                                header = catalogControls,
                            )
                        } else {
                            LazyColumn(
                                state = catalogListState,
                                modifier = Modifier.fillMaxSize(),
                                verticalArrangement = Arrangement.spacedBy(28.dp),
                            ) {
                                item(key = "catalog-controls", contentType = "controls") { catalogControls() }
                                if (continueWatching.isNotEmpty()) {
                                    item(key = "continue-watching", contentType = "quick-access") {
                                        QuickAccessRow(
                                            title = "Continue Watching",
                                            items = continueWatching,
                                            artworkUrl = artworkUrl,
                                            onClick = { item -> onPlay(item.representative) },
                                            isLiked = isLiked,
                                            isDefaultFocusRow = firstSection == "continue",
                                            defaultFocusRequester = catalogEntryFocusRequester.takeIf { !restoringSelection && firstSection == "continue" },
                                            firstCardFocusRequester = catalogEntryFocusRequester.takeIf { !restoringSelection && firstSection == "continue" },
                                            onNavigateDown = watchlistSectionIndex?.let { sectionIndex ->
                                                {
                                                    automaticInitialFocusEnabled = false
                                                    focusNavigationScope.launch {
                                                        catalogListState.scrollToItem(sectionIndex)
                                                        withFrameNanos {}
                                                        runCatching { watchlistRowFocusRequester.requestFocus() }
                                                    }
                                                }
                                            },
                                            requestInitialFocus = automaticInitialFocusEnabled && !filterRailExpanded,
                                        )
                                    }
                                }
                                if (watchlist.isNotEmpty()) {
                                    item(key = "watchlist", contentType = "quick-access") {
                                        QuickAccessRow(
                                            title = "Watchlist",
                                            items = watchlist,
                                            artworkUrl = artworkUrl,
                                            onClick = { item ->
                                                if (item.kind == QuickAccessKind.SHOW) item.show?.let(onOpenShow)
                                                else onOpenMovie(item.representative)
                                            },
                                            isLiked = isLiked,
                                            isDefaultFocusRow = firstSection == "watchlist",
                                            defaultFocusRequester = catalogEntryFocusRequester.takeIf { !restoringSelection && firstSection == "watchlist" },
                                            firstCardFocusRequester = when {
                                                !restoringSelection && firstSection == "watchlist" -> catalogEntryFocusRequester
                                                else -> watchlistRowFocusRequester
                                            },
                                            requestInitialFocus = automaticInitialFocusEnabled && !filterRailExpanded,
                                        )
                                    }
                                }
                                if (movies.isNotEmpty()) {
                                    item {
                                        MovieRow(
                                            "Movies", movies, artworkUrl, onOpenMovie, onOpenMovieShelf, movieRestoreIndex,
                                            isDefaultFocusRow = firstSection == "movies",
                                            isLiked = isLiked,
                                            defaultFocusRequester = catalogEntryFocusRequester.takeIf {
                                                movieRestoreIndex != null || (!restoringSelection && firstSection == "movies")
                                            },
                                            requestInitialFocus = automaticInitialFocusEnabled && !filterRailExpanded,
                                            preview = preview,
                                            expandedPreviewEntryKey = expandedPreviewEntryKey,
                                            onPreviewFocusChanged = previewFocusChanged,
                                            onPreviewFinished = previewFinished,
                                        )
                                    }
                                }
                                items(movieGenreShelves, key = { "movie-genre-${it.first}" }) { (genre, genreMovies) ->
                                    MovieRow(
                                        genre,
                                        genreMovies,
                                        artworkUrl,
                                        onOpenMovie,
                                        onOpenShelf = null,
                                        restoreFocusIndex = null,
                                        isDefaultFocusRow = false,
                                        isLiked = isLiked,
                                        preview = preview,
                                        expandedPreviewEntryKey = expandedPreviewEntryKey,
                                        onPreviewFocusChanged = previewFocusChanged,
                                        onPreviewFinished = previewFinished,
                                    )
                                }
                                if (shows.isNotEmpty()) {
                                    item {
                                        ShowShelfRow(
                                            "Shows", shows, artworkUrl, onOpenShowShelf, onOpenShow, showRestoreIndex,
                                            isDefaultFocusRow = firstSection == "shows",
                                            defaultFocusRequester = catalogEntryFocusRequester.takeIf {
                                                showRestoreIndex != null || (!restoringSelection && firstSection == "shows")
                                            },
                                            requestInitialFocus = automaticInitialFocusEnabled && !filterRailExpanded,
                                            preview = preview,
                                            expandedPreviewEntryKey = expandedPreviewEntryKey,
                                            onPreviewFocusChanged = previewFocusChanged,
                                            onPreviewFinished = previewFinished,
                                        )
                                    }
                                }
                                items(showGenreShelves, key = { "show-genre-${it.first}" }) { (genre, genreShows) ->
                                    ShowShelfRow(
                                        genre,
                                        genreShows,
                                        artworkUrl,
                                        onOpenShowShelf = null,
                                        onOpenShow = onOpenShow,
                                        restoreFocusIndex = null,
                                        isDefaultFocusRow = false,
                                        preview = preview,
                                        expandedPreviewEntryKey = expandedPreviewEntryKey,
                                        onPreviewFocusChanged = previewFocusChanged,
                                        onPreviewFinished = previewFinished,
                                    )
                                }
                                if (artists.isNotEmpty()) {
                                    item {
                                        ArtistShelfRow(
                                            "Music", artists, artworkUrl, artistPhotoUrl, onOpenArtistShelf, onOpenArtist, artistRestoreIndex,
                                            isDefaultFocusRow = firstSection == "music",
                                            defaultFocusRequester = catalogEntryFocusRequester.takeIf {
                                                artistRestoreIndex != null || (!restoringSelection && firstSection == "music")
                                            },
                                            requestInitialFocus = automaticInitialFocusEnabled && !filterRailExpanded,
                                        )
                                    }
                                }
                                items(musicGenreShelves, key = { "music-genre-${it.first}" }) { (genre, genreArtists) ->
                                    ArtistShelfRow(
                                        genre,
                                        genreArtists,
                                        artworkUrl,
                                        artistPhotoUrl,
                                        onOpenArtistShelf = null,
                                        onOpenArtist = onOpenArtist,
                                        restoreFocusIndex = null,
                                        isDefaultFocusRow = false,
                                    )
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

private fun kindMatches(entry: MergedEntry, filter: KindFilter): Boolean = when (filter) {
    KindFilter.ALL -> true
    KindFilter.MOVIES -> entry.entry.kind == MediaKind.MOVIE
    KindFilter.SHOWS -> entry.entry.kind == MediaKind.EPISODE
    KindFilter.MUSIC -> entry.entry.kind == MediaKind.TRACK
}

@Composable
private fun CatalogControls(
    searchText: String,
    onSearchTextChange: (String) -> Unit,
    onSubmitSearch: () -> Unit,
    showClear: Boolean,
    onClear: () -> Unit,
    onOpenSwarm: () -> Unit,
    unreachable: List<SwarmDevice>,
    playbackError: String?,
    catalogEntryFocusRequester: FocusRequester,
    filterRailFocusRequester: FocusRequester,
) {
    Column {
        Row(
            modifier = Modifier.fillMaxWidth()
                .focusProperties { left = filterRailFocusRequester }
                .onPreviewKeyEvent { event ->
                    if (event.type == KeyEventType.KeyDown && event.key == Key.DirectionDown) {
                        runCatching { catalogEntryFocusRequester.requestFocus() }.isSuccess
                    } else {
                        false
                    }
                },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            TvOutlinedTextField(
                value = searchText,
                onValueChange = onSearchTextChange,
                placeholder = { Text("Search title, artist, show…", color = SwarmMuted) },
                colors = searchFieldColors(),
                onSubmit = onSubmitSearch,
                modifier = Modifier.weight(1f),
            )
            Button(
                onClick = onOpenSwarm,
                colors = swarmActionButtonColors(),
                contentPadding = PaddingValues(10.dp),
                modifier = Modifier.size(44.dp),
            ) {
                Icon(
                    painter = painterResource(R.drawable.ic_settings),
                    contentDescription = "Open SWARM",
                    modifier = Modifier.size(22.dp),
                )
            }
            if (showClear) {
                Button(
                    onClick = onClear,
                    colors = swarmActionButtonColors(),
                ) {
                    Text("Clear", fontSize = 13.sp)
                }
            }
        }
        if (unreachable.isNotEmpty()) {
            Spacer(Modifier.height(10.dp))
            Text(
                "${unreachable.size} server(s) not reachable yet: ${unreachable.joinToString { it.name }}",
                color = SwarmMuted,
                fontSize = 12.sp,
            )
        }
        if (playbackError != null) {
            Spacer(Modifier.height(10.dp))
            Text(playbackError, color = SwarmAccent, fontSize = 12.sp)
        }
        Spacer(Modifier.height(16.dp))
    }
}

/** Ranks [entries]' genres by how many entries in this specific kind carry each one (descending), takes the top 5 (or fewer, if the kind has fewer distinct genres), and groups each genre's matching subset via [group] — [ShowGroup]/[ArtistGroup] for Shows/Music, the identity function for the already-flat Movies list. */
private fun <T> topGenreShelves(entries: List<MergedEntry>, group: (List<MergedEntry>) -> List<T>): List<Pair<String, List<T>>> =
    entries.flatMap { it.entry.genres }
        .groupingBy { it }
        .eachCount()
        .entries
        .sortedByDescending { it.value }
        .take(5)
        .map { (genre, _) -> genre to group(entries.filter { it.entry.genres.contains(genre) }) }

private fun WatchState.progressFraction(): Float =
    if (durationSecs <= 0.0) 0f else (positionSecs / durationSecs).coerceIn(0.0, 1.0).toFloat()

private fun WatchState.percentComplete(): Int = (progressFraction() * 100).toInt()

private fun ShowGroup.isWatched(states: Map<String, WatchState>): Boolean {
    val episodes = CatalogGrouping.previewSeasons(this).flatMap { it.episodes }
    return episodes.isNotEmpty() && episodes.all { states[it.entry.fingerprint]?.watched == true }
}

/**
 * The genre-filtered view — the "Browse all" full-grid layout
 * ([MovieShelfScreen]/[ShowShelfScreen]/[ArtistShelfScreen]'s own visual
 * style), one section per kind, instead of the horizontal-shelf browsing
 * [CatalogScreen] otherwise uses. A single scrolling row is fine for "here's
 * a taste of Action movies"; picking a genre is asking to actually see
 * everything tagged with it, which wants a real grid. All three sections
 * share one [LazyVerticalGrid] (full-width header items via
 * `GridItemSpan(maxLineSpan)`, ordinary single-cell items otherwise) rather
 * than three separate grids, since a second scrollable nested inside
 * another of the same orientation doesn't work in Compose without a bounded
 * height — one grid with section headers sidesteps that entirely.
 */
@Composable
private fun GenreFilteredGrid(
    movies: List<MergedEntry>,
    shows: List<ShowGroup>,
    artists: List<ArtistGroup>,
    artworkUrl: (MergedEntry) -> String?,
    artistPhotoUrl: (MergedEntry) -> String?,
    onOpenMovie: (MergedEntry) -> Unit,
    onOpenShow: (ShowGroup) -> Unit,
    onOpenArtist: (ArtistGroup) -> Unit,
    isLiked: (MergedEntry) -> Boolean,
    firstFocusRequester: FocusRequester,
    requestInitialFocus: Boolean,
    initialFocusMovieKey: String?,
    initialFocusShowKey: String?,
    initialFocusArtistKey: String?,
    header: @Composable () -> Unit,
) {
    val gridState = rememberLazyGridState()
    val firstSection = when {
        movies.isNotEmpty() -> "movies"
        shows.isNotEmpty() -> "shows"
        artists.isNotEmpty() -> "music"
        else -> null
    }
    var nextGridIndex = 1 // Search controls.
    var restoreGridIndex: Int? = null
    if (movies.isNotEmpty()) {
        nextGridIndex++ // Movies header.
        val selected = initialFocusMovieKey?.let { key -> movies.indexOfFirst { it.entry.entryKey == key } } ?: -1
        if (selected >= 0) restoreGridIndex = nextGridIndex + selected
        nextGridIndex += movies.size
    }
    if (shows.isNotEmpty()) {
        nextGridIndex++ // Shows header.
        val selected = initialFocusShowKey?.let { key -> shows.indexOfFirst { it.show == key } } ?: -1
        if (selected >= 0) restoreGridIndex = nextGridIndex + selected
        nextGridIndex += shows.size
    }
    if (artists.isNotEmpty()) {
        nextGridIndex++ // Music header.
        val selected = initialFocusArtistKey?.let { key -> artists.indexOfFirst { it.artist == key } } ?: -1
        if (selected >= 0) restoreGridIndex = nextGridIndex + selected
    }
    LaunchedEffect(restoreGridIndex, movies, shows, artists, requestInitialFocus) {
        if (requestInitialFocus) {
            if (restoreGridIndex != null) {
                gridState.scrollToItem(restoreGridIndex!!)
                withFrameNanos {}
            }
            runCatching { firstFocusRequester.requestFocus() }
        }
    }

    LazyVerticalGrid(
        state = gridState,
        columns = GridCells.Fixed(5),
        verticalArrangement = Arrangement.spacedBy(20.dp),
        horizontalArrangement = Arrangement.spacedBy(20.dp),
        // top = 32.dp, not the flat 12.dp every other edge gets — same
        // focus-scale headroom fix as MovieShelfScreen/ShowShelfScreen/
        // ArtistShelfScreen's identical grids, needed here too since this
        // is the same "browse all" full-grid style reached a different way
        // (picking a genre) rather than via those screens' own "Browse all"
        // button.
        contentPadding = PaddingValues(start = 12.dp, end = 12.dp, top = 32.dp, bottom = 12.dp),
    ) {
        item(key = "catalog-controls", span = { GridItemSpan(maxLineSpan) }, contentType = "controls") { header() }
        if (movies.isNotEmpty()) {
            item(span = { GridItemSpan(maxLineSpan) }) { GridSectionHeader("Movies") }
            gridItemsIndexed(
                items = movies,
                key = { _, entry -> "movie-${entry.entry.entryKey}" },
                contentType = { _, _ -> "movie" },
            ) { index, entry ->
                CatalogCard(
                    entry,
                    artworkUrl(entry),
                    onClick = { onOpenMovie(entry) },
                    focusRequester = if (
                        entry.entry.entryKey == initialFocusMovieKey ||
                        (restoreGridIndex == null && firstSection == "movies" && index == 0)
                    ) firstFocusRequester else null,
                    widthModifier = Modifier.fillMaxWidth(),
                    isLiked = isLiked(entry),
                )
            }
        }
        if (shows.isNotEmpty()) {
            item(span = { GridItemSpan(maxLineSpan) }) { GridSectionHeader("Shows") }
            gridItemsIndexed(
                items = shows,
                key = { _, show -> "show-${show.show}" },
                contentType = { _, _ -> "show" },
            ) { index, show ->
                val representative = show.seasons.firstOrNull()?.episodes?.firstOrNull()
                GroupCard(
                    title = show.show,
                    subtitle = "${show.seasons.size} season" + if (show.seasons.size == 1) "" else "s",
                    artworkUrl = representative?.let(artworkUrl),
                    onClick = { onOpenShow(show) },
                    focusRequester = if (
                        show.show == initialFocusShowKey ||
                        (restoreGridIndex == null && firstSection == "shows" && index == 0)
                    ) firstFocusRequester else null,
                    widthModifier = Modifier.fillMaxWidth(),
                )
            }
        }
        if (artists.isNotEmpty()) {
            item(span = { GridItemSpan(maxLineSpan) }) { GridSectionHeader("Music") }
            gridItemsIndexed(
                items = artists,
                key = { _, artist -> "artist-${artist.artist}" },
                contentType = { _, _ -> "artist" },
            ) { index, artist ->
                val albumCount = artist.albums.size
                val artistArtwork = artist.artworkUrls(artworkUrl, artistPhotoUrl)
                GroupCard(
                    title = artist.artist,
                    subtitle = "$albumCount album" + if (albumCount == 1) "" else "s",
                    artworkUrl = artistArtwork.artistPhoto,
                    fallbackArtworkUrl = artistArtwork.albumCoverFallback,
                    artworkAspectRatio = 1f,
                    placeholderType = "Artist",
                    onClick = { onOpenArtist(artist) },
                    focusRequester = if (
                        artist.artist == initialFocusArtistKey ||
                        (restoreGridIndex == null && firstSection == "music" && index == 0)
                    ) firstFocusRequester else null,
                    widthModifier = Modifier.fillMaxWidth(),
                )
            }
        }
    }
}

@Composable
private fun GridSectionHeader(label: String) {
    Text(label, color = SwarmMuted, fontSize = TOP_LEVEL_TITLE_SIZE, fontWeight = FontWeight.Black, modifier = Modifier.padding(bottom = 4.dp))
}

@Composable
private fun FilterRail(
    expanded: Boolean,
    onExpand: () -> Unit,
    firstFocusRequester: FocusRequester,
    contentFocusRequester: FocusRequester,
    kindFilter: KindFilter,
    onKindSelect: (KindFilter) -> Unit,
    genres: List<String>,
    genreFilter: String?,
    onGenreSelect: (String?) -> Unit,
    ratings: List<String>,
    ratingFilter: String?,
    onRatingSelect: (String?) -> Unit,
    likedOnly: Boolean,
    onLikedOnlyToggle: () -> Unit,
    anyFilterActive: Boolean,
    onClear: () -> Unit,
) {
    val railWidth by animateDpAsState(
        targetValue = if (expanded) 190.dp else 42.dp,
        label = "catalog-filter-rail-width",
    )
    var headerFocused by remember { mutableStateOf(false) }

    Column(
        modifier = Modifier.width(railWidth).fillMaxSize()
            .clip(RoundedCornerShape(topEnd = 14.dp, bottomEnd = 14.dp))
            .background(SwarmSurface)
            .padding(horizontal = if (expanded) 10.dp else 4.dp, vertical = 8.dp)
            .focusProperties { right = contentFocusRequester }
            .onPreviewKeyEvent { event ->
                if (expanded && event.type == KeyEventType.KeyDown && event.key == Key.DirectionRight) {
                    runCatching { contentFocusRequester.requestFocus() }.isSuccess
                } else {
                    false
                }
            },
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        // This is a focus target, not a button. Merely arriving from the
        // catalog expands the rail; D-pad Center has no action to invoke and
        // therefore cannot reopen the retired modal/crash path.
        Row(
            modifier = Modifier.fillMaxWidth()
                .clip(RoundedCornerShape(8.dp))
                .background(if (headerFocused) SwarmSurfaceMuted else Color.Transparent)
                .focusRequester(firstFocusRequester)
                .onFocusChanged {
                    headerFocused = it.isFocused
                    if (it.isFocused) onExpand()
                }
                .focusable()
                .padding(horizontal = if (expanded) 8.dp else 2.dp, vertical = 5.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
        ) {
            Image(
                painter = painterResource(R.drawable.mascot),
                contentDescription = "Filters",
                modifier = Modifier.size(if (expanded) 30.dp else 26.dp),
            )
            if (expanded) Text("Filters", color = SwarmText, fontSize = 16.sp, fontWeight = FontWeight.Black)
        }

        if (!expanded) {
            Spacer(Modifier.height(12.dp))
            FilterRailIcon("▦", kindFilter != KindFilter.ALL)
            FilterRailIcon("♥", likedOnly)
            FilterRailIcon("◈", genreFilter != null)
            FilterRailIcon("★", ratingFilter != null)
            return@Column
        }

        Column(
            modifier = Modifier.fillMaxWidth().weight(1f)
                .verticalScroll(rememberScrollState())
                .padding(top = 12.dp, bottom = 8.dp),
        ) {
            FilterRailSection("Media type") {
                for (kind in KindFilter.entries) {
                    FilterRailOption(
                        label = kind.label,
                        isSelected = kindFilter == kind,
                        onClick = { onKindSelect(kind) },
                    )
                }
            }

            Spacer(Modifier.height(14.dp))
            FilterRailSection("Favorites") {
                FilterRailOption("♥  Liked only", isSelected = likedOnly, onClick = onLikedOnlyToggle)
            }

            if (genres.isNotEmpty()) {
                Spacer(Modifier.height(14.dp))
                FilterRailSection("Genre") {
                    FilterRailOption("All genres", isSelected = genreFilter == null, onClick = { onGenreSelect(null) })
                    for (genre in genres) {
                        FilterRailOption(genre, isSelected = genre == genreFilter, onClick = { onGenreSelect(genre) })
                    }
                }
            }

            if (ratings.isNotEmpty()) {
                Spacer(Modifier.height(14.dp))
                FilterRailSection("Content rating") {
                    FilterRailOption("Any rating", isSelected = ratingFilter == null, onClick = { onRatingSelect(null) })
                    for (rating in ratings) {
                        FilterRailOption(rating, isSelected = rating == ratingFilter, onClick = { onRatingSelect(rating) })
                    }
                }
            }
        }

        if (anyFilterActive) {
            Button(onClick = onClear, colors = swarmActionButtonColors(), modifier = Modifier.fillMaxWidth()) {
                Text("Clear filters", fontSize = 13.sp)
            }
        }
    }
}

@Composable
private fun FilterRailIcon(icon: String, active: Boolean) {
    Text(
        icon,
        color = if (active) SwarmAccent else SwarmMuted,
        fontSize = 16.sp,
        fontWeight = if (active) FontWeight.Black else FontWeight.Normal,
        modifier = Modifier.padding(vertical = 7.dp),
    )
}

@Composable
private fun FilterRailSection(title: String, content: @Composable () -> Unit) {
    Text(title, color = SwarmMuted, fontSize = 12.sp, fontWeight = FontWeight.Black)
    Spacer(Modifier.height(6.dp))
    Column(
        verticalArrangement = Arrangement.spacedBy(5.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        content()
    }
}

@Composable
private fun FilterRailOption(label: String, isSelected: Boolean, onClick: () -> Unit) {
    var isFocused by remember { mutableStateOf(false) }
    Button(
        onClick = onClick,
        modifier = Modifier.fillMaxWidth().height(36.dp).onFocusChanged { isFocused = it.isFocused },
        contentPadding = PaddingValues(horizontal = 10.dp, vertical = 2.dp),
        colors = ButtonDefaults.colors(
            containerColor = if (isSelected) SwarmAccent else SwarmSurfaceMuted,
            contentColor = if (isSelected) Color(0xFF04263A) else SwarmText,
            focusedContainerColor = SwarmAccent,
            focusedContentColor = Color(0xFF04263A),
            pressedContainerColor = SwarmAccent,
            pressedContentColor = Color(0xFF04263A),
        ),
    ) {
        Text(
            label,
            color = if (isSelected || isFocused) Color(0xFF04263A) else SwarmText,
            fontSize = 12.sp,
            maxLines = 1,
            modifier = Modifier.fillMaxWidth(),
        )
    }
}

@Composable
private fun searchFieldColors() = OutlinedTextFieldDefaults.colors(
    focusedTextColor = SwarmText,
    unfocusedTextColor = SwarmText,
    focusedBorderColor = SwarmAccent,
    unfocusedBorderColor = SwarmBorder,
    cursorColor = SwarmAccent,
)

// Top-level shelf titles (Movies/Shows/Music) read noticeably larger/bolder
// than genre sub-shelf titles beneath them, so the row hierarchy is visible
// at a glance rather than every shelf title looking like the same kind of
// heading — real feedback from live use.
private val TOP_LEVEL_TITLE_SIZE = 20.sp
private val GENRE_TITLE_SIZE = 16.sp

/** [onOpenAll] null (a genre sub-shelf) skips the "Browse all" button — there's no full-grid screen for "this kind, filtered to this one genre" today, just the row itself. */
@Composable
private fun ShelfHeader(label: String, onOpenAll: (() -> Unit)?, fontSize: TextUnit) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(label, color = SwarmMuted, fontSize = fontSize, fontWeight = FontWeight.Black)
        if (onOpenAll != null) {
            Button(onClick = onOpenAll, colors = swarmActionButtonColors()) {
                Text("Browse all", fontSize = 12.sp)
            }
        }
    }
}

// Extra clearance below a genre sub-shelf's (smaller, tighter-packed) title
// specifically: real bug, found live — tv-material3's focus-scale animation
// on the first card in the row below grows upward too, and with shelves
// stacked as densely as the genre breakdown now does, the default spacing
// let a focused card's top edge cover its own row's title. Top-level shelf
// titles get a smaller bump for the same reason at a smaller scale (they're
// larger text needing a little more clearance than before, but nowhere near
// as many rows stacked close together).
private val TOP_LEVEL_TITLE_SPACING = 14.dp
private val GENRE_TITLE_SPACING = 22.dp

/**
 * Shared by [MovieRow]/[ShowShelfRow]/[ArtistShelfRow]: decides which index
 * (if any) this row should send initial D-pad focus to — [restoreFocusIndex]
 * when a remembered card is actually present in this row, else index 0 when
 * this is the [isDefaultFocusRow] (first non-empty top-level section, same
 * fallback as before this existed), else nothing. `scrollToItem` before
 * `requestFocus`, not just the latter alone: a restored index can be well
 * outside the row's initially-composed window, and Compose can only focus an
 * item that's actually been laid out — the `withFrameNanos` gives that
 * freshly-scrolled-to item one frame to actually compose before the focus
 * request, which would otherwise silently no-op against an item not there yet.
 */
@Composable
private fun rememberRowFocusTarget(
    itemCount: Int,
    restoreFocusIndex: Int?,
    isDefaultFocusRow: Boolean,
    listState: LazyListState,
    defaultFocusRequester: FocusRequester? = null,
    requestInitialFocus: Boolean = true,
): Pair<Int?, FocusRequester> {
    val localFocusRequester = remember { FocusRequester() }
    val focusRequester = defaultFocusRequester ?: localFocusRequester
    val targetIndex = restoreFocusIndex ?: (0.takeIf { isDefaultFocusRow })
    LaunchedEffect(targetIndex, itemCount, requestInitialFocus) {
        if (requestInitialFocus && targetIndex != null && targetIndex < itemCount) {
            listState.scrollToItem(targetIndex)
            withFrameNanos {}
            runCatching { focusRequester.requestFocus() }
        }
    }
    return targetIndex to focusRequester
}

@Composable
private fun QuickAccessRow(
    title: String,
    items: List<QuickAccessItem>,
    artworkUrl: (MergedEntry) -> String?,
    onClick: (QuickAccessItem) -> Unit,
    isLiked: (MergedEntry) -> Boolean,
    isDefaultFocusRow: Boolean,
    defaultFocusRequester: FocusRequester?,
    firstCardFocusRequester: FocusRequester?,
    onNavigateDown: (() -> Unit)? = null,
    requestInitialFocus: Boolean,
) {
    val listState = rememberLazyListState()
    val artworkUrls = remember(items, artworkUrl) { items.map { artworkUrl(it.representative) } }
    PrefetchArtworkRow(listState, artworkUrls)
    val (targetIndex, focusRequester) = rememberRowFocusTarget(
        items.size,
        restoreFocusIndex = null,
        isDefaultFocusRow = isDefaultFocusRow,
        listState = listState,
        defaultFocusRequester = defaultFocusRequester,
        requestInitialFocus = requestInitialFocus,
    )
    Column {
        ShelfHeader(title, onOpenAll = null, fontSize = TOP_LEVEL_TITLE_SIZE)
        Spacer(Modifier.height(TOP_LEVEL_TITLE_SPACING))
        LazyRow(
            state = listState,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            contentPadding = PaddingValues(horizontal = 12.dp),
        ) {
            itemsIndexed(
                items = items,
                key = { _, item -> item.key },
                contentType = { _, item -> "quick-${item.kind}" },
            ) { index, item ->
                CatalogCard(
                    merged = item.representative,
                    artworkUrl = artworkUrls[index],
                    onClick = { onClick(item) },
                    focusRequester = when {
                        index == 0 && firstCardFocusRequester != null -> firstCardFocusRequester
                        index == targetIndex -> focusRequester
                        else -> null
                    },
                    onNavigateDown = onNavigateDown,
                    isLiked = isLiked(item.representative),
                    titleOverride = item.title,
                    subtitle = item.subtitle,
                    progress = item.progress,
                    placeholderType = if (item.kind == QuickAccessKind.MOVIE) "Movie" else "Show",
                )
            }
        }
    }
}

@Composable
private fun MovieRow(
    title: String,
    movies: List<MergedEntry>,
    artworkUrl: (MergedEntry) -> String?,
    onOpenMovie: (MergedEntry) -> Unit,
    onOpenShelf: (() -> Unit)?,
    restoreFocusIndex: Int?,
    isDefaultFocusRow: Boolean,
    isLiked: (MergedEntry) -> Boolean,
    defaultFocusRequester: FocusRequester? = null,
    requestInitialFocus: Boolean = true,
    preview: BrowsePreview?,
    expandedPreviewEntryKey: String?,
    onPreviewFocusChanged: (MergedEntry, Boolean) -> Unit,
    onPreviewFinished: (String) -> Unit,
) {
    val isTopLevel = onOpenShelf != null
    val listState = rememberLazyListState()
    val artworkUrls = remember(movies, artworkUrl) { movies.map(artworkUrl) }
    PrefetchArtworkRow(listState, artworkUrls)
    val (targetIndex, focusRequester) = rememberRowFocusTarget(
        movies.size,
        restoreFocusIndex,
        isDefaultFocusRow,
        listState,
        defaultFocusRequester,
        requestInitialFocus,
    )
    Column {
        ShelfHeader(title, onOpenShelf, if (isTopLevel) TOP_LEVEL_TITLE_SIZE else GENRE_TITLE_SIZE)
        Spacer(Modifier.height(if (isTopLevel) TOP_LEVEL_TITLE_SPACING else GENRE_TITLE_SPACING))
        // contentPadding, not just the Column's own outer padding: tv-material3's
        // Card scales up in place when it gains focus, so the leftmost/topmost card
        // in an unpadded LazyRow/LazyColumn scales outward past the layout's own
        // bounds and gets clipped by the scrolling container itself — confirmed live
        // (left edge of the first card in each shelf renders off-screen when focused).
        // Reserving a little extra space inside the scrollable area gives the scale
        // animation room without moving any card's resting position.
        LazyRow(state = listState, horizontalArrangement = Arrangement.spacedBy(12.dp), contentPadding = PaddingValues(horizontal = 12.dp)) {
            itemsIndexed(
                items = movies,
                key = { _, entry -> entry.entry.entryKey },
                contentType = { _, _ -> "movie" },
            ) { index, entry ->
                CatalogCard(
                    entry,
                    artworkUrls[index],
                    onClick = { onOpenMovie(entry) },
                    focusRequester = if (index == targetIndex) focusRequester else null,
                    isLiked = isLiked(entry),
                    preview = preview,
                    expandedPreviewEntryKey = expandedPreviewEntryKey,
                    onPreviewFocusChanged = onPreviewFocusChanged,
                    onPreviewFinished = onPreviewFinished,
                )
            }
        }
    }
}

@Composable
private fun ShowShelfRow(
    title: String,
    shows: List<ShowGroup>,
    artworkUrl: (MergedEntry) -> String?,
    onOpenShowShelf: (() -> Unit)?,
    onOpenShow: (ShowGroup) -> Unit,
    restoreFocusIndex: Int?,
    isDefaultFocusRow: Boolean,
    defaultFocusRequester: FocusRequester? = null,
    requestInitialFocus: Boolean = true,
    preview: BrowsePreview?,
    expandedPreviewEntryKey: String?,
    onPreviewFocusChanged: (MergedEntry, Boolean) -> Unit,
    onPreviewFinished: (String) -> Unit,
) {
    val isTopLevel = onOpenShowShelf != null
    val listState = rememberLazyListState()
    // Keep each card's random choice stable across recompositions/focus
    // animation. A refreshed show list produces a fresh season+episode pick.
    val previewEntries = remember(shows) {
        shows.map(CatalogGrouping::randomPreviewEpisode)
    }
    val artworkUrls = remember(previewEntries, artworkUrl) {
        previewEntries.map { it?.let(artworkUrl) }
    }
    val realSeasonCounts = remember(shows) {
        shows.map { CatalogGrouping.previewSeasons(it).size }
    }
    PrefetchArtworkRow(listState, artworkUrls)
    val (targetIndex, focusRequester) = rememberRowFocusTarget(
        shows.size,
        restoreFocusIndex,
        isDefaultFocusRow,
        listState,
        defaultFocusRequester,
        requestInitialFocus,
    )
    Column {
        ShelfHeader(title, onOpenShowShelf, if (isTopLevel) TOP_LEVEL_TITLE_SIZE else GENRE_TITLE_SIZE)
        Spacer(Modifier.height(if (isTopLevel) TOP_LEVEL_TITLE_SPACING else GENRE_TITLE_SPACING))
        LazyRow(state = listState, horizontalArrangement = Arrangement.spacedBy(12.dp), contentPadding = PaddingValues(horizontal = 12.dp)) {
            itemsIndexed(
                items = shows,
                key = { _, show -> show.show },
                contentType = { _, _ -> "show" },
            ) { index, show ->
                GroupCard(
                    title = show.show,
                    subtitle = "${realSeasonCounts[index]} season" + if (realSeasonCounts[index] == 1) "" else "s",
                    artworkUrl = artworkUrls[index],
                    onClick = { onOpenShow(show) },
                    focusRequester = if (index == targetIndex) focusRequester else null,
                    previewEntry = previewEntries[index],
                    preview = preview,
                    expandedPreviewEntryKey = expandedPreviewEntryKey,
                    onPreviewFocusChanged = onPreviewFocusChanged,
                    onPreviewFinished = onPreviewFinished,
                )
            }
        }
    }
}

@Composable
private fun ArtistShelfRow(
    title: String,
    artists: List<ArtistGroup>,
    artworkUrl: (MergedEntry) -> String?,
    artistPhotoUrl: (MergedEntry) -> String?,
    onOpenArtistShelf: (() -> Unit)?,
    onOpenArtist: (ArtistGroup) -> Unit,
    restoreFocusIndex: Int?,
    isDefaultFocusRow: Boolean,
    defaultFocusRequester: FocusRequester? = null,
    requestInitialFocus: Boolean = true,
) {
    val isTopLevel = onOpenArtistShelf != null
    val listState = rememberLazyListState()
    val artistArtwork = remember(artists, artworkUrl, artistPhotoUrl) {
        artists.map { it.artworkUrls(artworkUrl, artistPhotoUrl) }
    }
    PrefetchArtworkRow(listState, artistArtwork.map { it.artistPhoto ?: it.albumCoverFallback })
    val (targetIndex, focusRequester) = rememberRowFocusTarget(
        artists.size,
        restoreFocusIndex,
        isDefaultFocusRow,
        listState,
        defaultFocusRequester,
        requestInitialFocus,
    )
    Column {
        ShelfHeader(title, onOpenArtistShelf, if (isTopLevel) TOP_LEVEL_TITLE_SIZE else GENRE_TITLE_SIZE)
        Spacer(Modifier.height(if (isTopLevel) TOP_LEVEL_TITLE_SPACING else GENRE_TITLE_SPACING))
        LazyRow(state = listState, horizontalArrangement = Arrangement.spacedBy(12.dp), contentPadding = PaddingValues(horizontal = 12.dp)) {
            itemsIndexed(
                items = artists,
                key = { _, artist -> artist.artist },
                contentType = { _, _ -> "artist" },
            ) { index, artist ->
                val albumCount = artist.albums.size
                val artwork = artistArtwork[index]
                GroupCard(
                    title = artist.artist,
                    subtitle = "$albumCount album" + if (albumCount == 1) "" else "s",
                    artworkUrl = artwork.artistPhoto,
                    fallbackArtworkUrl = artwork.albumCoverFallback,
                    artworkAspectRatio = 1f,
                    placeholderType = "Artist",
                    onClick = { onOpenArtist(artist) },
                    focusRequester = if (index == targetIndex) focusRequester else null,
                )
            }
        }
    }
}

// Card width: smaller than this screen used to be (was 160.dp) so more
// fit across one horizontal row at once — the same "see more per row"
// request the media server's browse grid already satisfies with its own,
// much smaller thumbnails.
private val CARD_WIDTH = 130.dp
private val CARD_MEDIA_HEIGHT = 195.dp
private val PREVIEW_CARD_WIDTH = 347.dp

@Composable
private fun CatalogCard(
    merged: MergedEntry,
    artworkUrl: String?,
    onClick: () -> Unit,
    focusRequester: FocusRequester?,
    widthModifier: Modifier = Modifier.width(CARD_WIDTH),
    isLiked: Boolean = false,
    preview: BrowsePreview? = null,
    expandedPreviewEntryKey: String? = null,
    onPreviewFocusChanged: ((MergedEntry, Boolean) -> Unit)? = null,
    onPreviewFinished: (String) -> Unit = {},
    titleOverride: String? = null,
    subtitle: String? = null,
    progress: Float? = null,
    placeholderType: String = "Movie",
    onNavigateDown: (() -> Unit)? = null,
) {
    var isFocused by remember(merged.entry.entryKey) { mutableStateOf(false) }
    val isPreviewExpanded = isFocused && expandedPreviewEntryKey == merged.entry.entryKey
    val animatedWidth by animateDpAsState(if (isPreviewExpanded) PREVIEW_CARD_WIDTH else CARD_WIDTH)
    val previewAlpha by animateFloatAsState(if (isPreviewExpanded) 1f else 0f, label = "movie-preview-alpha")
    val focusModifier = Modifier
        .then(
            if (onNavigateDown != null) {
                Modifier.onPreviewKeyEvent { event ->
                    if (event.type == KeyEventType.KeyDown && event.key == Key.DirectionDown) {
                        onNavigateDown()
                        true
                    } else {
                        false
                    }
                }
            } else {
                Modifier
            },
        )
        .then(if (focusRequester != null) Modifier.focusRequester(focusRequester) else Modifier)
    val resolvedWidth = if (onPreviewFocusChanged != null) Modifier.width(animatedWidth) else widthModifier
    val showCardText = merged.entry.kind == MediaKind.TRACK || artworkUrl == null
    Card(
        onClick = onClick,
        colors = CardDefaults.colors(containerColor = SwarmSurface),
        scale = CardDefaults.scale(scale = 1f, focusedScale = 1f, pressedScale = 0.99f),
        modifier = focusModifier.then(resolvedWidth).onFocusChanged { focusState ->
            if (isFocused != focusState.isFocused) {
                isFocused = focusState.isFocused
                onPreviewFocusChanged?.invoke(merged, focusState.isFocused)
            }
        },
    ) {
        Column {
            Box(modifier = Modifier.fillMaxWidth().height(CARD_MEDIA_HEIGHT).clip(RoundedCornerShape(4.dp))) {
                ArtworkImage(
                    label = merged.entry.scrapedTitle ?: merged.entry.title,
                    placeholderType = placeholderType,
                    primaryUrl = artworkUrl,
                    modifier = Modifier.fillMaxSize(),
                )
                val activePreview = preview?.takeIf { isFocused && it.entryKey == merged.entry.entryKey }
                if (isPreviewExpanded && activePreview == null) {
                    PreviewLoadingIndicator(Modifier.fillMaxSize())
                }
                activePreview?.let {
                    BrowsePreviewPlayer(
                        preview = it,
                        shouldPlay = isPreviewExpanded,
                        onFinished = onPreviewFinished,
                        modifier = Modifier.fillMaxSize().alpha(previewAlpha),
                    )
                }
                if (isLiked) {
                    Text(
                        "♥",
                        color = SwarmLike,
                        fontSize = 16.sp,
                        fontWeight = FontWeight.Black,
                        modifier = Modifier.align(Alignment.TopEnd).padding(6.dp),
                    )
                }
                progress?.let { fraction ->
                    Box(
                        modifier = Modifier.align(Alignment.BottomStart).fillMaxWidth().height(4.dp)
                            .background(Color.Black.copy(alpha = 0.65f)),
                    ) {
                        Box(
                            modifier = Modifier.fillMaxWidth(fraction.coerceIn(0f, 1f)).height(4.dp)
                                .background(SwarmAccent),
                        )
                    }
                }
            }
            if (showCardText) {
                Column(modifier = Modifier.padding(10.dp)) {
                    Text(
                        titleOverride ?: merged.entry.scrapedTitle ?: merged.entry.title,
                        color = SwarmText,
                        fontSize = 13.sp,
                        fontWeight = FontWeight.SemiBold,
                        minLines = 2,
                        maxLines = 2,
                    )
                    if (subtitle != null) {
                        Spacer(Modifier.height(4.dp))
                        Text(subtitle, color = SwarmMuted, fontSize = 10.sp, maxLines = 1)
                    }
                    if (merged.sources.size > 1) {
                        Spacer(Modifier.height(4.dp))
                        Text("${merged.sources.size} sources", color = SwarmAccent, fontSize = 10.sp)
                    }
                }
            }
        }
    }
}

/** Shared grouped-media card: shows use representative poster art; artists prefer a photo and then an album cover. */
@Composable
private fun GroupCard(
    title: String,
    subtitle: String,
    artworkUrl: String?,
    onClick: () -> Unit,
    focusRequester: FocusRequester?,
    widthModifier: Modifier = Modifier.width(CARD_WIDTH),
    fallbackArtworkUrl: String? = null,
    artworkAspectRatio: Float = 2f / 3f,
    placeholderType: String = "Show",
    previewEntry: MergedEntry? = null,
    preview: BrowsePreview? = null,
    expandedPreviewEntryKey: String? = null,
    onPreviewFocusChanged: ((MergedEntry, Boolean) -> Unit)? = null,
    onPreviewFinished: (String) -> Unit = {},
) {
    var isFocused by remember(title, previewEntry?.entry?.entryKey) { mutableStateOf(false) }
    val previewEnabled = previewEntry != null && onPreviewFocusChanged != null
    val isPreviewExpanded = isFocused && previewEntry?.entry?.entryKey == expandedPreviewEntryKey
    val animatedWidth by animateDpAsState(if (isPreviewExpanded) PREVIEW_CARD_WIDTH else CARD_WIDTH)
    val previewAlpha by animateFloatAsState(if (isPreviewExpanded) 1f else 0f, label = "show-preview-alpha")
    val focusModifier = if (focusRequester != null) Modifier.focusRequester(focusRequester) else Modifier
    val resolvedWidth = if (previewEnabled) Modifier.width(animatedWidth) else widthModifier
    val showCardText = placeholderType == "Artist" || (artworkUrl == null && fallbackArtworkUrl == null)
    Card(
        onClick = onClick,
        colors = CardDefaults.colors(containerColor = SwarmSurface),
        scale = CardDefaults.scale(scale = 1f, focusedScale = 1f, pressedScale = 0.99f),
        modifier = focusModifier.then(resolvedWidth).onFocusChanged { focusState ->
            if (isFocused != focusState.isFocused) {
                isFocused = focusState.isFocused
                previewEntry?.let { onPreviewFocusChanged?.invoke(it, focusState.isFocused) }
            }
        },
    ) {
        Column {
            if (previewEnabled) {
                Box(modifier = Modifier.fillMaxWidth().height(CARD_MEDIA_HEIGHT).clip(RoundedCornerShape(4.dp))) {
                    ArtworkImage(
                        label = title,
                        placeholderType = placeholderType,
                        primaryUrl = artworkUrl,
                        fallbackUrl = fallbackArtworkUrl,
                        modifier = Modifier.fillMaxSize(),
                    )
                    val activePreview = preview?.takeIf { isFocused && it.entryKey == previewEntry?.entry?.entryKey }
                    if (isPreviewExpanded && activePreview == null) {
                        PreviewLoadingIndicator(Modifier.fillMaxSize())
                    }
                    activePreview?.let {
                        BrowsePreviewPlayer(
                            preview = it,
                            shouldPlay = isPreviewExpanded,
                            onFinished = onPreviewFinished,
                            modifier = Modifier.fillMaxSize().alpha(previewAlpha),
                        )
                    }
                }
            } else {
                ArtworkImage(
                    label = title,
                    placeholderType = placeholderType,
                    primaryUrl = artworkUrl,
                    fallbackUrl = fallbackArtworkUrl,
                    modifier = Modifier.fillMaxWidth().aspectRatio(artworkAspectRatio).clip(RoundedCornerShape(4.dp)),
                )
            }
            if (showCardText) {
                Column(modifier = Modifier.padding(10.dp)) {
                    Text(title, color = SwarmText, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, minLines = 2, maxLines = 2)
                    Spacer(Modifier.height(4.dp))
                    Text(subtitle, color = SwarmMuted, fontSize = 10.sp)
                }
            }
        }
    }
}
