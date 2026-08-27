package app.swarm.tv.app.ui

/**
 * Compose `Modifier.testTag(...)` constants for the instrumented UAT suite
 * under `androidTest/.../uat`. Additive only — these exist purely so that
 * suite can find real on-screen elements reliably; they carry no runtime
 * behavior. See the `swarm-tv-uat-suite` skill for how they're used, and
 * `swarm-e2e-suite-lockdown` for the change policy on the suite itself
 * (this file is ordinary product/testability code, not suite test logic,
 * so it isn't covered by that lockdown).
 */
object UatTestTags {
    // CatalogScreen: top-level shelves.
    const val SHELF_MOVIES = "uat_shelf_movies"
    const val SHELF_SHOWS = "uat_shelf_shows"
    const val SHELF_MUSIC = "uat_shelf_music"
    const val ROW_CONTINUE_WATCHING = "uat_row_continue_watching"
    const val ROW_WATCHLIST = "uat_row_watchlist"

    // CatalogScreen: card tiles (suffix with the entry/show/artist key at the call site).
    const val CARD_MOVIE_PREFIX = "uat_card_movie_"
    const val CARD_SHOW_PREFIX = "uat_card_show_"
    const val CARD_ARTIST_PREFIX = "uat_card_artist_"
    const val CARD_QUICK_ACCESS_PREFIX = "uat_card_quick_access_"

    // CatalogScreen: filter rail.
    const val FILTER_RAIL = "uat_filter_rail"
    const val FILTER_KIND_PREFIX = "uat_filter_kind_" // + KindFilter.name (ALL/MOVIES/SHOWS/MUSIC)
    const val FILTER_LIKED_ONLY = "uat_filter_liked_only"
    const val FILTER_GENRE_PREFIX = "uat_filter_genre_" // + genre name
    const val FILTER_RATING_PREFIX = "uat_filter_rating_" // + rating value

    // CatalogScreen: top bar.
    const val OPEN_SWARM_BUTTON = "uat_open_swarm_button"
    const val DASHBOARD_SETTINGS_BUTTON = "uat_dashboard_settings_button"

    // MovieDetailScreen.
    const val MOVIE_DETAIL_ARTWORK = "uat_movie_detail_artwork"
    const val MOVIE_DETAIL_YEAR = "uat_movie_detail_year"
    const val MOVIE_DETAIL_GENRES = "uat_movie_detail_genres"
    const val MOVIE_DETAIL_CAST = "uat_movie_detail_cast"
    const val MOVIE_DETAIL_DESCRIPTION = "uat_movie_detail_description"
    const val MOVIE_DETAIL_PLAY_BUTTON = "uat_movie_detail_play_button"
    const val MOVIE_DETAIL_LIKE_BUTTON = "uat_movie_detail_like_button"
    const val MOVIE_DETAIL_WATCHLIST_BUTTON = "uat_movie_detail_watchlist_button"
    const val MOVIE_DETAIL_REPORT_PROBLEM_BUTTON = "uat_movie_detail_report_problem_button"

    // SeasonScreen.
    const val SEASON_SCREEN_SHOW_TITLE = "uat_season_screen_show_title"
    const val SEASON_SCREEN_WATCHLIST_BUTTON = "uat_season_screen_watchlist_button"
    const val SEASON_CARD_PREFIX = "uat_season_card_" // + season number
    const val EPISODE_ITEM_PREFIX = "uat_episode_item_" // + episode entryKey

    // AlbumScreen.
    const val ALBUM_CARD_PREFIX = "uat_album_card_" // + album key
    const val TRACK_ROW_PREFIX = "uat_track_row_" // + track entryKey

    // PlayerScreen / PauseOverlay.
    const val PLAYER_SURFACE = "uat_player_surface"
    const val PAUSE_LABEL = "uat_pause_label"
    const val PAUSE_TITLE = "uat_pause_title"
    // NOTE: year/duration/content-rating/community-rating+votes/resolution
    // are rendered as ONE joined Text line (`metadata.joinToString(" • ")`),
    // not separate elements — this single tag covers all of them; assert by
    // substring match, not by separately-tagged fields.
    const val PAUSE_METADATA = "uat_pause_metadata"
    const val PAUSE_GENRES = "uat_pause_genres"
    const val PAUSE_CAST = "uat_pause_cast"
    const val PAUSE_DESCRIPTION = "uat_pause_description"
    const val PAUSE_AUDIO_TRACK_PICKER = "uat_pause_audio_track_picker"
    const val PAUSE_SUBTITLE_TRACK_PICKER = "uat_pause_subtitle_track_picker"
    const val PAUSE_RESUME_BUTTON = "uat_pause_resume_button"
    const val PAUSE_NEXT_EPISODE_BUTTON = "uat_pause_next_episode_button"

    // MusicPlayerScreen.
    const val MUSIC_PLAYER_COVER = "uat_music_player_cover"
    const val MUSIC_PLAYER_TITLE = "uat_music_player_title"
    const val MUSIC_PLAYER_UP_NEXT = "uat_music_player_up_next"
    const val MUSIC_PLAYER_SHUFFLE_BUTTON = "uat_music_player_shuffle_button"
    const val MUSIC_PLAYER_PLAY_PAUSE_BUTTON = "uat_music_player_play_pause_button"
    const val MUSIC_PLAYER_SKIP_BUTTON = "uat_music_player_skip_button"
    const val MUSIC_PLAYER_LIKE_BUTTON = "uat_music_player_like_button"
    const val MUSIC_PLAYER_CLOSE_BUTTON = "uat_music_player_close_button"

    // MiniPlayerBar.
    const val MINI_PLAYER_REOPEN = "uat_mini_player_reopen"
    const val MINI_PLAYER_CLOSE_BUTTON = "uat_mini_player_close_button"

    // SwarmSettingsScreen / NotificationInbox. Reached via OPEN_SWARM_BUTTON
    // above, then this in-screen "Notifications" tab (there is no
    // separately-labeled "Settings" tab inside this screen — its tab row is
    // General/Family/Notifications/Testing).
    const val NOTIFICATIONS_TAB_BUTTON = "uat_notifications_tab_button"
    const val NOTIFICATION_ROW_PREFIX = "uat_notification_row_" // + notification key
    const val NOTIFICATION_DISMISS_BUTTON = "uat_notification_dismiss_button"
}
