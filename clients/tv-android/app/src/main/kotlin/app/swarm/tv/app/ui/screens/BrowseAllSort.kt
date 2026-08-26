package app.swarm.tv.app.ui.screens

/**
 * Sort key for the full-grid "Browse All" screens
 * ([MovieShelfScreen]/[ShowShelfScreen]/[ArtistShelfScreen]) — alphabetical,
 * like a media library, where a leading standalone "The" doesn't separate a
 * title from the rest of its series. Unlike
 * `CatalogMerger.catalogSortTitle` (Rust-mirrored base catalog order, movies
 * only), this applies to every asset kind Browse All can show, since those
 * rows are otherwise ordered by rating rather than title.
 */
internal fun browseAllSortKey(title: String): String {
    val trimmed = title.trim()
    val withoutLeadingArticle = if (trimmed.startsWith("The ", ignoreCase = true)) {
        trimmed.substring(4).trimStart()
    } else {
        trimmed
    }
    return withoutLeadingArticle.lowercase()
}
