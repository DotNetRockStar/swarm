package app.swarm.tv.app.data

import android.content.Context
import app.swarm.tv.app.data.db.AppDatabase
import app.swarm.tv.app.data.db.LocalServerConnectionEntity
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

data class SavedLanConnection(
    val server: LanServer,
    val deviceName: String,
)

/** Room-backed history of successfully authenticated LAN servers. */
class AndroidLanConnectionStore(context: Context) {
    private val dao = AppDatabase.getInstance(context).localServerConnectionDao()

    suspend fun mostRecent(): SavedLanConnection? = withContext(Dispatchers.IO) {
        dao.mostRecent()?.let { entity ->
            SavedLanConnection(
                server = LanServer(
                    serviceName = entity.serviceName,
                    name = entity.name,
                    host = entity.host,
                    peerPort = entity.peerPort,
                    pairingPort = entity.pairingPort,
                    certFingerprint = entity.certFingerprint,
                ),
                deviceName = entity.deviceName,
            )
        }
    }

    suspend fun save(server: LanServer, deviceName: String) = withContext(Dispatchers.IO) {
        dao.upsert(
            LocalServerConnectionEntity(
                certFingerprint = server.certFingerprint,
                serviceName = server.serviceName,
                name = server.name,
                host = server.host,
                peerPort = server.peerPort,
                pairingPort = server.pairingPort,
                deviceName = deviceName,
                lastConnectedAt = System.currentTimeMillis(),
            ),
        )
    }

    suspend fun forget(fingerprint: String) = withContext(Dispatchers.IO) {
        dao.delete(fingerprint)
    }
}
