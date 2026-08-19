/**
 * Persists what [SwarmViewModel] needs to skip straight back to the
 * dashboard on a cold start instead of asking for the STUN URL/passcode
 * again every launch: the saved STUN connection (server URL, device name,
 * this device's assigned id) and its joined swarms — Room-backed, see
 * data/db/ for the schema. Plain (not encrypted) like the SharedPreferences
 * store this replaced: a swarm id/name or a STUN URL isn't a secret the
 * way the access token is — that stays in [AndroidTokenStore] regardless.
 *
 * [SwarmViewModel] keeps its own in-memory copy for the running session and
 * only writes through here, mirroring how it already treats [AndroidTokenStore].
 */
package app.swarm.tv.app.data

import android.content.Context
import app.swarm.tv.app.data.db.AppDatabase
import app.swarm.tv.app.data.db.ServerConnectionEntity
import app.swarm.tv.app.data.db.SwarmEntity
import app.swarm.tv.app.data.db.SINGLETON_ROW_ID
import app.swarm.tv.core.rest.SwarmSummary
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

data class SavedConnection(
    val baseUrl: String,
    val deviceName: String,
    val deviceId: String,
    val swarms: List<SwarmSummary>,
    val activeSwarmId: String?,
)

class AndroidConnectionStore(context: Context) {
    private val db = AppDatabase.getInstance(context)
    private val connectionDao = db.serverConnectionDao()
    private val swarmDao = db.swarmDao()

    suspend fun get(): SavedConnection? = withContext(Dispatchers.IO) {
        val connection = connectionDao.get() ?: return@withContext null
        val swarms = swarmDao.list()
        SavedConnection(
            baseUrl = connection.baseUrl,
            deviceName = connection.deviceName,
            deviceId = connection.deviceId,
            swarms = swarms.map { SwarmSummary(id = it.id, name = it.name) },
            activeSwarmId = swarms.find { it.isActive }?.id,
        )
    }

    /** First-time save right after a successful passcode registration. */
    suspend fun saveNewConnection(baseUrl: String, deviceName: String, deviceId: String, swarm: SwarmSummary) =
        withContext(Dispatchers.IO) {
            connectionDao.upsert(
                ServerConnectionEntity(baseUrl = baseUrl, deviceName = deviceName, deviceId = deviceId, updatedAt = System.currentTimeMillis()),
            )
            swarmDao.replaceAll(listOf(swarm.toEntity(isActive = true)))
        }

    /** Full authoritative swarm list from the STUN server after a join/leave round trip. */
    suspend fun updateSwarms(swarms: List<SwarmSummary>, activeSwarmId: String?) = withContext(Dispatchers.IO) {
        swarmDao.replaceAll(swarms.map { it.toEntity(isActive = it.id == activeSwarmId) })
    }

    suspend fun setActiveSwarm(swarmId: String?) = withContext(Dispatchers.IO) {
        swarmDao.setActive(swarmId)
    }

    /** Config-page edit: where this device connects next. Takes effect on the next connection attempt — doesn't touch anything already in flight. */
    suspend fun updateBaseUrl(baseUrl: String) = withContext(Dispatchers.IO) {
        val existing = connectionDao.get() ?: return@withContext
        connectionDao.upsert(existing.copy(baseUrl = baseUrl, updatedAt = System.currentTimeMillis()))
    }

    /**
     * Config-page edit: the locally-remembered device label only. The STUN
     * server has no endpoint to rename an already-registered device's
     * `name` field (`PATCH .../metadata` covers the separate free-form
     * metadata bag, not this), so this intentionally does not call the
     * server — it takes effect the next time this device registers fresh.
     */
    suspend fun updateDeviceName(name: String) = withContext(Dispatchers.IO) {
        connectionDao.renameDevice(name)
    }

    /** Forgets the saved connection entirely — cascades to every swarm row via the FK. */
    suspend fun clear() = withContext(Dispatchers.IO) {
        connectionDao.clear()
    }

    private fun SwarmSummary.toEntity(isActive: Boolean) =
        SwarmEntity(id = id, connectionId = SINGLETON_ROW_ID, name = name, isActive = isActive)
}
