package app.swarm.tv.core.watch

import app.swarm.tv.core.rest.SwarmJson
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.decodeFromString
import kotlinx.serialization.encodeToString
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class WatchStateTest {
    @Test
    fun `ninety five percent is watched but just under is not`() {
        assertTrue(WatchState.fromPlayback(95.0, 100.0, 1).watched)
        assertFalse(WatchState.fromPlayback(94.9, 100.0, 1).watched)
    }

    @Test
    fun `wire shape uses snake_case fields`() {
        val state = WatchState(positionSecs = 125.5, durationSecs = 5400.0, watched = false, updatedAt = 1_700_000_000_000)
        val json = SwarmJson.encodeToString(state)
        assertEquals(
            """{"position_secs":125.5,"duration_secs":5400.0,"watched":false,"updated_at":1700000000000}""",
            json,
        )
        assertEquals(state, SwarmJson.decodeFromString<WatchState>(json))
    }

    @Test
    fun `in-memory store roundtrips per fingerprint`() = runBlocking {
        val store = InMemoryWatchStateStore()
        assertNull(store.get("fp-1"))

        val state = WatchState(positionSecs = 42.0, durationSecs = 100.0, watched = false, updatedAt = 1)
        store.set("fp-1", state)
        assertEquals(state, store.get("fp-1"))
        assertEquals(mapOf("fp-1" to state), store.all())
        assertNull(store.get("fp-2")) // a different fingerprint is unaffected

        store.clear("fp-1")
        assertNull(store.get("fp-1"))
    }

    @Test
    fun `setting again overwrites rather than accumulates`() = runBlocking {
        val store = InMemoryWatchStateStore()
        store.set("fp-1", WatchState(10.0, 100.0, false, 1))
        store.set("fp-1", WatchState(90.0, 100.0, true, 2))
        assertEquals(WatchState(90.0, 100.0, true, 2), store.get("fp-1"))
    }
}
