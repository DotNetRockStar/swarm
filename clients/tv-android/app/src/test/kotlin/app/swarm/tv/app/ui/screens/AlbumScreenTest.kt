package app.swarm.tv.app.ui.screens

import app.swarm.tv.core.catalog.AlbumGroup
import app.swarm.tv.core.catalog.ArtistGroup
import app.swarm.tv.core.catalog.MergedEntry
import app.swarm.tv.core.peer.CatalogEntry
import app.swarm.tv.core.peer.MediaKind
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Test

class AlbumScreenTest {
    private fun track(key: String, album: String) = MergedEntry(
        fingerprint = key,
        sources = listOf("server-a"),
        entry = CatalogEntry(entryKey = key, fingerprint = key, kind = MediaKind.TRACK, title = key, size = 1, artist = "Air", album = album),
    )

    private val artist = ArtistGroup(
        artist = "Air",
        albums = listOf(
            AlbumGroup("Moon Safari", listOf(track("a1", "Moon Safari"), track("a2", "Moon Safari"))),
            AlbumGroup("Talkie Walkie", listOf(track("b1", "Talkie Walkie"))),
        ),
    )

    @Test
    fun `albumForKey resolves the album to reopen on Back from the player`() {
        assertEquals("Moon Safari", albumForKey(artist, "Moon Safari")?.album)
        assertEquals("Talkie Walkie", albumForKey(artist, "Talkie Walkie")?.album)
    }

    @Test
    fun `albumForKey is null for a plain open or an unknown album`() {
        assertNull(albumForKey(artist, null))
        assertNull(albumForKey(artist, "Pocket Symphony"))
    }
}
