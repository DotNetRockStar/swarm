/**
 * Resume + watched state for one catalog entry, keyed by its cross-server
 * fingerprint (not a per-server entry key — the plan's "single profile,
 * resume + watched" scope, and a fingerprint is what stays stable across
 * which source a merged entry happens to stream from). The real
 * implementation (`SharedPreferences`-backed) lives in `:app` since it
 * needs Android framework APIs unavailable here; this interface is what
 * the rest of `:core` — and any test — programs against.
 *
 * Deliberately not Room, despite the project plan naming a "client-local
 * Room table": every other piece of local state in this app
 * (`TokenStore`) already uses a plain key/value store, not a database, for
 * data this simple (one JSON blob per fingerprint) — adding Room's KSP
 * annotation-processing toolchain for a single-table, no-query-beyond-
 * lookup-by-key need isn't a trade worth making. Revisit only if resume
 * state grows real query needs (e.g. "everything in progress," sorted)
 * that a key/value store can't answer cleanly.
 */
package app.swarm.tv.core.watch

import kotlinx.serialization.Serializable

@Serializable
data class WatchState(
    val positionSecs: Double,
    val durationSecs: Double,
    val watched: Boolean,
    val updatedAt: Long,
)

interface WatchStateStore {
    suspend fun get(fingerprint: String): WatchState?
    suspend fun set(fingerprint: String, state: WatchState)
    suspend fun clear(fingerprint: String)
}

/** Test double / placeholder — never used for a real device's watch state. */
class InMemoryWatchStateStore : WatchStateStore {
    private val states = mutableMapOf<String, WatchState>()

    override suspend fun get(fingerprint: String): WatchState? = states[fingerprint]

    override suspend fun set(fingerprint: String, state: WatchState) {
        states[fingerprint] = state
    }

    override suspend fun clear(fingerprint: String) {
        states.remove(fingerprint)
    }
}
