/**
 * Where a device's STUN access token lives at rest. The real implementation
 * (Android Keystore-backed, per `docs/PROTOCOL.md`'s identity section) lives
 * in the `:app` module since it needs Android framework APIs unavailable
 * here; this interface is what the rest of `:core` — and any test —
 * programs against.
 */
package app.swarm.tv.core.token

interface TokenStore {
    suspend fun save(token: String)
    suspend fun load(): String?
    suspend fun clear()
}

/** Test double / placeholder — never used for a real device's token. */
class InMemoryTokenStore : TokenStore {
    @Volatile
    private var value: String? = null

    override suspend fun save(token: String) {
        value = token
    }

    override suspend fun load(): String? = value

    override suspend fun clear() {
        value = null
    }
}
