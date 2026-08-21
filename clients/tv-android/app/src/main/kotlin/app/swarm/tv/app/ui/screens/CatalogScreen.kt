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
 * button would. The search box + Filter button along the top replaces
 * that space with something the media server's own browse page already
 * has and this screen didn't.
 */
package app.swarm.tv.app.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
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
import androidx.compose.foundation.lazy.grid.itemsIndexed as gridItemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import app.swarm.tv.R
import app.swarm.tv.app.ui.components.SelectableChip
import app.swarm.tv.app.ui.components.SwarmLoadingIndicator
import app.swarm.tv.app.ui.components.TvOutlinedTextField
import app.swarm.tv.app.ui.components.swarmActionButtonColors
import app.swarm.tv.app.ui.PrefetchArtworkRow
import app.swarm.tv.app.ui.theme.SwarmAccent
import app.swarm.tv.app.ui.theme.SwarmAccentHot
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

/** Mirrors the media server's own browse-page filter (`media.js`'s `kindFilter`) — same four choices, same meaning. */
private enum class KindFilter(val label: String) {
    ALL("All"), MOVIES("Movies"), SHOWS("Shows"), MUSIC("Music"),
}

@Composable
fun CatalogScreen(
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
) {
    BackHandler(onBack = onBack)
    // searchText is what's live in the field as the user types; appliedSearchQuery
    // is what actually drives filtering below. Keeping them separate is the fix for
    // a real bug: when a single `searchQuery` backed both the field and the
    // `remember(entries, searchQuery, kindFilter)` filter, every keystroke forced a
    // full recomposition of the catalog list underneath the still-focused field,
    // which disrupted focus/IME state and made the search box appear to "exit" after
    // one letter. appliedSearchQuery now only updates from TvOutlinedTextField's
    // onSubmit (D-pad Enter/Done), so typing no longer touches the list at all.
    var searchText by remember { mutableStateOf("") }
    var appliedSearchQuery by remember { mutableStateOf("") }
    var kindFilter by remember { mutableStateOf(KindFilter.ALL) }
    // Categories = genres, same field the media server's own category picker
    // writes — null means "no genre filter", matching that server-side
    // filter's "All categories" option.
    var genreFilter by remember { mutableStateOf<String?>(null) }
    var ratingFilter by remember { mutableStateOf<String?>(null) }
    var likedOnly by remember { mutableStateOf(false) }
    var showFilterOverlay by remember { mutableStateOf(false) }
    val anyFilterActive = kindFilter != KindFilter.ALL || genreFilter != null || ratingFilter != null || likedOnly
    // The controls are the first item in the same lazy catalog so they scroll
    // away with the shelves. Every possible layout still attaches this
    // requester to its first active card; the controls' DOWN handler has one
    // deterministic way into content even when lazy children are not yet
    // candidates for geometric focus search.
    val catalogEntryFocusRequester = remember { FocusRequester() }
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
            anyFilterActive = anyFilterActive,
            onOpenFilter = { showFilterOverlay = true },
            unreachable = unreachable,
            playbackError = playbackError,
            showFilterOverlay = showFilterOverlay,
            catalogEntryFocusRequester = catalogEntryFocusRequester,
        )
    }

    // Outer Box, not just a Column: real bug, found live — with a Column as
    // this screen's sole root, FilterOverlay (a Column *child*, below) got
    // laid out sequentially after the rest of the screen's content instead
    // of stacking on top of it, squeezed into whatever space happened to be
    // left over at the bottom (usually ~none, since the content above
    // already fills the screen) — the Filter button appeared to do nothing
    // because the overlay it opened was real but invisible/zero-height. A
    // Box lets the picker sit in its own full-screen layer above everything
    // else, the same pattern PlayerScreen.kt's ContinueOverlay already uses
    // correctly.
    Box(modifier = Modifier.fillMaxSize()) {
        // Small padding here, not the ~40dp this screen used to carry: MainActivity's
        // contentModifier already reserves the TV-safe overscan margin around every
        // non-Player screen, so a second, separate margin here just doubled up as extra
        // dead space on every edge — confirmed live as a persistent empty border around
        // the browse page no matter how this screen's own padding was tuned.
        Column(
            modifier = Modifier.fillMaxSize().padding(horizontal = 8.dp, vertical = 8.dp)
                // Real bug, found live: the filter overlay below is only
                // ever a *visual* layer on top of this content — with
                // nothing else stopping it, Compose's D-pad focus search
                // still freely finds and jumps to whatever's underneath, so
                // pressing DOWN inside the (still-visible) overlay silently
                // moved focus onto the browse cards behind it instead of
                // navigating the overlay itself. Disabling this whole
                // subtree's focusability while the overlay is open is what
                // actually confines the D-pad to the modal.
                .focusProperties { canFocus = !showFilterOverlay },
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
                                requestInitialFocus = !showFilterOverlay,
                                header = catalogControls,
                            )
                        } else {
                            LazyColumn(modifier = Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(28.dp)) {
                                item(key = "catalog-controls", contentType = "controls") { catalogControls() }
                                if (movies.isNotEmpty()) {
                                    item {
                                        MovieRow(
                                            "Movies", movies, artworkUrl, onOpenMovie, onOpenMovieShelf, movieRestoreIndex,
                                            isDefaultFocusRow = firstSection == "movies",
                                            isLiked = isLiked,
                                            defaultFocusRequester = catalogEntryFocusRequester.takeIf { firstSection == "movies" },
                                            requestInitialFocus = !showFilterOverlay,
                                        )
                                    }
                                }
                                items(movieGenreShelves, key = { "movie-genre-${it.first}" }) { (genre, genreMovies) ->
                                    MovieRow(genre, genreMovies, artworkUrl, onOpenMovie, onOpenShelf = null, restoreFocusIndex = null, isDefaultFocusRow = false, isLiked = isLiked)
                                }
                                if (shows.isNotEmpty()) {
                                    item {
                                        ShowShelfRow(
                                            "Shows", shows, artworkUrl, onOpenShowShelf, onOpenShow, showRestoreIndex,
                                            isDefaultFocusRow = firstSection == "shows",
                                            defaultFocusRequester = catalogEntryFocusRequester.takeIf { firstSection == "shows" },
                                            requestInitialFocus = !showFilterOverlay,
                                        )
                                    }
                                }
                                items(showGenreShelves, key = { "show-genre-${it.first}" }) { (genre, genreShows) ->
                                    ShowShelfRow(genre, genreShows, artworkUrl, onOpenShowShelf = null, onOpenShow = onOpenShow, restoreFocusIndex = null, isDefaultFocusRow = false)
                                }
                                if (artists.isNotEmpty()) {
                                    item {
                                        ArtistShelfRow(
                                            "Music", artists, artworkUrl, artistPhotoUrl, onOpenArtistShelf, onOpenArtist, artistRestoreIndex,
                                            isDefaultFocusRow = firstSection == "music",
                                            defaultFocusRequester = catalogEntryFocusRequester.takeIf { firstSection == "music" },
                                            requestInitialFocus = !showFilterOverlay,
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

        if (showFilterOverlay) {
            // Scoped to the currently-selected kind, not the full unfiltered
            // library — picking "Movies" then opening Genre/Rating only
            // offers values that could actually narrow *this* result set,
            // same reasoning the media server dashboard's own category
            // picker already follows.
            val kindScoped = remember(entries, kindFilter) {
                if (kindFilter == KindFilter.ALL) entries else entries.filter { kindMatches(it, kindFilter) }
            }
            val allGenres = remember(kindScoped) { kindScoped.flatMap { it.entry.genres }.distinct().sortedWith(String.CASE_INSENSITIVE_ORDER) }
            val allRatings = remember(kindScoped) { kindScoped.mapNotNull { it.entry.rating }.distinct().sortedWith(String.CASE_INSENSITIVE_ORDER) }
            FilterOverlay(
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
                onDismiss = { showFilterOverlay = false },
            )
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
    anyFilterActive: Boolean,
    onOpenFilter: () -> Unit,
    unreachable: List<SwarmDevice>,
    playbackError: String?,
    showFilterOverlay: Boolean,
    catalogEntryFocusRequester: FocusRequester,
) {
    Column {
        Row(
            modifier = Modifier.fillMaxWidth().onPreviewKeyEvent { event ->
                if (!showFilterOverlay && event.type == KeyEventType.KeyDown && event.key == Key.DirectionDown) {
                    runCatching { catalogEntryFocusRequester.requestFocus() }.isSuccess
                } else {
                    false
                }
            },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Image(
                painter = painterResource(R.drawable.mascot),
                contentDescription = null,
                modifier = Modifier.size(32.dp),
            )
            TvOutlinedTextField(
                value = searchText,
                onValueChange = onSearchTextChange,
                placeholder = { Text("Search title, artist, show…", color = SwarmMuted) },
                colors = searchFieldColors(),
                onSubmit = onSubmitSearch,
                modifier = Modifier.weight(1f),
            )
            if (showClear) {
                Button(
                    onClick = onClear,
                    colors = swarmActionButtonColors(),
                ) {
                    Text("Clear", fontSize = 13.sp)
                }
            }
            Button(
                onClick = onOpenFilter,
                colors = swarmActionButtonColors(),
            ) {
                Text("Filter", fontSize = 13.sp)
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
    header: @Composable () -> Unit,
) {
    LaunchedEffect(movies, shows, artists, requestInitialFocus) {
        if (requestInitialFocus) {
            runCatching { firstFocusRequester.requestFocus() }
        }
    }
    val firstSection = when {
        movies.isNotEmpty() -> "movies"
        shows.isNotEmpty() -> "shows"
        artists.isNotEmpty() -> "music"
        else -> null
    }

    LazyVerticalGrid(
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
                    focusRequester = if (firstSection == "movies" && index == 0) firstFocusRequester else null,
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
                    focusRequester = if (firstSection == "shows" && index == 0) firstFocusRequester else null,
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
                    focusRequester = if (firstSection == "music" && index == 0) firstFocusRequester else null,
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

/**
 * Consolidates what used to be three separate always-visible controls
 * (Movies/Shows/Music kind buttons, a standalone Genre button) plus the new
 * Rating/Liked facets into one modal — real feedback from live use: the
 * search row was running out of room, and every facet here follows the
 * same "tag cloud" pattern for the same reason the old genre-only picker
 * did (see the pre-consolidation doc comment this replaced): a single-
 * column list needs too many DOWN-presses, a rigid fixed-column grid
 * stretches short labels into oddly padded cells, a centered wrapping
 * FlowRow does neither. Each section applies its own selection immediately
 * on click — no separate Apply/Save step — and the whole modal stays open
 * across multiple picks so all four facets can be tuned in one visit; only
 * the physical Back button (wired via [BackHandler]) closes it, same
 * reasoning [CatalogScreen]'s own header dropped an on-screen Back button
 * for.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun FilterOverlay(
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
    onDismiss: () -> Unit,
) {
    BackHandler(onBack = onDismiss)
    val firstFocusRequester = remember { FocusRequester() }
    LaunchedEffect(Unit) { firstFocusRequester.requestFocus() }
    val config = LocalConfiguration.current

    Box(modifier = Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.85f)), contentAlignment = Alignment.Center) {
        Column(
            modifier = Modifier.width((config.screenWidthDp * 0.85f).dp)
                .heightIn(max = (config.screenHeightDp * 0.85f).dp)
                .verticalScroll(rememberScrollState())
                .padding(24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text("Filter", color = SwarmText, fontSize = 20.sp, fontWeight = FontWeight.Black)
            Spacer(Modifier.height(20.dp))

            FilterSection("Show") {
                for ((index, kind) in KindFilter.entries.withIndex()) {
                    SelectableChip(
                        kind.label,
                        isSelected = kindFilter == kind,
                        onClick = { onKindSelect(kind) },
                        focusRequester = if (index == 0) firstFocusRequester else null,
                    )
                }
            }

            Spacer(Modifier.height(20.dp))
            FilterSection("Liked") {
                SelectableChip("♥ Liked only", isSelected = likedOnly, onClick = onLikedOnlyToggle, focusRequester = null)
            }

            if (genres.isNotEmpty()) {
                Spacer(Modifier.height(20.dp))
                FilterSection("Genre") {
                    SelectableChip("All genres", isSelected = genreFilter == null, onClick = { onGenreSelect(null) }, focusRequester = null)
                    for (genre in genres) {
                        SelectableChip(genre, isSelected = genre == genreFilter, onClick = { onGenreSelect(genre) }, focusRequester = null)
                    }
                }
            }

            if (ratings.isNotEmpty()) {
                Spacer(Modifier.height(20.dp))
                FilterSection("Rating") {
                    SelectableChip("Any rating", isSelected = ratingFilter == null, onClick = { onRatingSelect(null) }, focusRequester = null)
                    for (rating in ratings) {
                        SelectableChip(rating, isSelected = rating == ratingFilter, onClick = { onRatingSelect(rating) }, focusRequester = null)
                    }
                }
            }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun FilterSection(title: String, content: @Composable () -> Unit) {
    Text(title, color = SwarmMuted, fontSize = 13.sp, fontWeight = FontWeight.Black)
    Spacer(Modifier.height(10.dp))
    FlowRow(
        horizontalArrangement = Arrangement.spacedBy(10.dp, Alignment.CenterHorizontally),
        verticalArrangement = Arrangement.spacedBy(10.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        content()
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
                CatalogCard(entry, artworkUrls[index], onClick = { onOpenMovie(entry) }, focusRequester = if (index == targetIndex) focusRequester else null, isLiked = isLiked(entry))
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
) {
    val isTopLevel = onOpenShowShelf != null
    val listState = rememberLazyListState()
    val artworkUrls = remember(shows, artworkUrl) {
        shows.map { show -> show.seasons.firstOrNull()?.episodes?.firstOrNull()?.let(artworkUrl) }
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
                    subtitle = "${show.seasons.size} season" + if (show.seasons.size == 1) "" else "s",
                    artworkUrl = artworkUrls[index],
                    onClick = { onOpenShow(show) },
                    focusRequester = if (index == targetIndex) focusRequester else null,
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

@Composable
private fun CatalogCard(
    merged: MergedEntry,
    artworkUrl: String?,
    onClick: () -> Unit,
    focusRequester: FocusRequester?,
    widthModifier: Modifier = Modifier.width(CARD_WIDTH),
    isLiked: Boolean = false,
) {
    val focusModifier = if (focusRequester != null) Modifier.focusRequester(focusRequester) else Modifier
    Card(
        onClick = onClick,
        colors = CardDefaults.colors(containerColor = SwarmSurface),
        modifier = focusModifier.then(widthModifier),
    ) {
        Column {
            Box {
                ArtworkImage(
                    label = merged.entry.scrapedTitle ?: merged.entry.title,
                    placeholderType = "Movie",
                    primaryUrl = artworkUrl,
                    modifier = Modifier.fillMaxWidth().aspectRatio(2f / 3f).clip(RoundedCornerShape(4.dp)),
                )
                if (isLiked) {
                    Text(
                        "♥",
                        color = SwarmAccentHot,
                        fontSize = 16.sp,
                        fontWeight = FontWeight.Black,
                        modifier = Modifier.align(Alignment.TopEnd).padding(6.dp),
                    )
                }
            }
            Column(modifier = Modifier.padding(10.dp)) {
                Text(
                    merged.entry.scrapedTitle ?: merged.entry.title,
                    color = SwarmText,
                    fontSize = 13.sp,
                    fontWeight = FontWeight.SemiBold,
                    // minLines, not just maxLines: without it, a one-line
                    // title takes half the vertical space of a two-line one,
                    // so cards side by side in the same row visibly differ
                    // in height depending on nothing but title length — real
                    // feedback from live use. Reserving both lines' worth of
                    // space always, whether or not this title wraps, is what
                    // actually makes every card in a row match.
                    minLines = 2,
                    maxLines = 2,
                )
                if (merged.sources.size > 1) {
                    Spacer(Modifier.height(4.dp))
                    Text("${merged.sources.size} sources", color = SwarmAccent, fontSize = 10.sp)
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
) {
    val focusModifier = if (focusRequester != null) Modifier.focusRequester(focusRequester) else Modifier
    Card(
        onClick = onClick,
        colors = CardDefaults.colors(containerColor = SwarmSurface),
        modifier = focusModifier.then(widthModifier),
    ) {
        Column {
            ArtworkImage(
                label = title,
                placeholderType = placeholderType,
                primaryUrl = artworkUrl,
                fallbackUrl = fallbackArtworkUrl,
                modifier = Modifier.fillMaxWidth().aspectRatio(artworkAspectRatio).clip(RoundedCornerShape(4.dp)),
            )
            Column(modifier = Modifier.padding(10.dp)) {
                Text(title, color = SwarmText, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, minLines = 2, maxLines = 2)
                Spacer(Modifier.height(4.dp))
                Text(subtitle, color = SwarmMuted, fontSize = 10.sp)
            }
        }
    }
}
