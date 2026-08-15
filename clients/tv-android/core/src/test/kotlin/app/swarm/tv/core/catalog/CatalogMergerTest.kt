package app.swarm.tv.core.catalog

import app.swarm.tv.core.peer.CatalogEntry
import app.swarm.tv.core.peer.CatalogManifest
import app.swarm.tv.core.peer.MediaKind
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class CatalogMergerTest {

    private fun entry(
        entryKey: String,
        fingerprint: String,
        title: String,
        scrapedTitle: String? = null,
        artworkEtag: String? = null,
    ) = CatalogEntry(
        entryKey = entryKey,
        fingerprint = fingerprint,
        kind = MediaKind.MOVIE,
        title = title,
        size = 1000,
        scrapedTitle = scrapedTitle,
        artworkEtag = artworkEtag,
    )

    @Test
    fun `same fingerprint on two servers becomes one entry with two sources`() {
        val shared = entry("k1", fingerprint = "fp-heat", title = "Heat")
        val manifests = mapOf(
            "server-a" to CatalogManifest(thumbprint = "tp-a", entries = listOf(shared)),
            "server-b" to CatalogManifest(thumbprint = "tp-b", entries = listOf(shared.copy(entryKey = "k2"))),
        )
        val merged = CatalogMerger.merge(manifests)
        assertEquals(1, merged.size)
        assertEquals(listOf("server-a", "server-b"), merged[0].sources)
    }

    @Test
    fun `distinct fingerprints stay distinct entries`() {
        val manifests = mapOf(
            "server-a" to CatalogManifest(
                thumbprint = "tp-a",
                entries = listOf(entry("k1", "fp-heat", "Heat"), entry("k2", "fp-fargo", "Fargo")),
            ),
        )
        val merged = CatalogMerger.merge(manifests)
        assertEquals(2, merged.size)
        assertEquals(setOf("fp-heat", "fp-fargo"), merged.map { it.fingerprint }.toSet())
    }

    @Test
    fun `richer scraped copy wins the displayed entry`() {
        val bare = entry("k1", "fp-heat", "Heat.1995.mkv")
        val scraped = entry("k1", "fp-heat", "Heat.1995.mkv", scrapedTitle = "Heat", artworkEtag = "v1")
        val manifests = mapOf(
            "server-a" to CatalogManifest(thumbprint = "tp-a", entries = listOf(bare)),
            "server-b" to CatalogManifest(thumbprint = "tp-b", entries = listOf(scraped)),
        )
        val merged = CatalogMerger.merge(manifests)
        assertEquals(1, merged.size)
        assertEquals("Heat", merged[0].entry.scrapedTitle)
        // Both servers are still tracked as sources even though B's copy is
        // the one displayed — a client should still be able to play from A.
        assertEquals(setOf("server-a", "server-b"), merged[0].sources.toSet())
    }

    @Test
    fun `merge is deterministic regardless of map iteration order`() {
        val a = entry("k1", "fp-1", "Alpha")
        val b = entry("k2", "fp-2", "Beta")
        val manifestsOrder1 = linkedMapOf(
            "server-z" to CatalogManifest("tp", listOf(a)),
            "server-a" to CatalogManifest("tp", listOf(b)),
        )
        val manifestsOrder2 = linkedMapOf(
            "server-a" to CatalogManifest("tp", listOf(b)),
            "server-z" to CatalogManifest("tp", listOf(a)),
        )
        assertEquals(CatalogMerger.merge(manifestsOrder1), CatalogMerger.merge(manifestsOrder2))
    }

    @Test
    fun `results are sorted by title case-insensitively`() {
        val manifests = mapOf(
            "server-a" to CatalogManifest(
                thumbprint = "tp",
                entries = listOf(entry("k1", "fp-1", "zebra"), entry("k2", "fp-2", "Apple"), entry("k3", "fp-3", "mango")),
            ),
        )
        val merged = CatalogMerger.merge(manifests)
        assertEquals(listOf("Apple", "mango", "zebra"), merged.map { it.entry.title })
    }

    @Test
    fun `empty manifests produce an empty merge`() {
        assertTrue(CatalogMerger.merge(emptyMap()).isEmpty())
    }
}
