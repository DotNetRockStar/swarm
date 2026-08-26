package app.swarm.tv.app.data

import android.content.Context
import app.swarm.tv.app.data.db.AppDatabase
import app.swarm.tv.app.data.db.ClientNotificationEntity
import app.swarm.tv.core.peer.ClientResolutionNotification
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map

data class ResolvedProblemNotification(
    val key: String,
    val serverId: String,
    val remoteId: Long,
    val serverName: String,
    val assetTitle: String?,
    val originalMessage: String,
    val comments: String?,
    val resolvedAtMs: Long,
)

class AndroidClientNotificationStore(context: Context) {
    private val dao = AppDatabase.getInstance(context).clientNotificationDao()

    fun observe(): Flow<List<ResolvedProblemNotification>> =
        dao.observeActive().map { rows -> rows.map(ClientNotificationEntity::toDomain) }

    /** Returns true only for a notification this install has never received. */
    suspend fun add(
        serverKey: String,
        serverId: String,
        serverName: String,
        notification: ClientResolutionNotification,
    ): Boolean {
        val key = "$serverKey:${notification.id}"
        return dao.insert(
            ClientNotificationEntity(
                key = key,
                serverId = serverId,
                remoteId = notification.id,
                serverName = serverName,
                assetTitle = notification.assetTitle,
                originalMessage = notification.originalMessage,
                comments = notification.comments,
                resolvedAt = notification.resolvedAtMs,
            ),
        ) != -1L
    }

    suspend fun dismiss(key: String) = dao.dismiss(key)
}

private fun ClientNotificationEntity.toDomain() = ResolvedProblemNotification(
    key = key,
    serverId = serverId,
    remoteId = remoteId,
    serverName = serverName,
    assetTitle = assetTitle,
    originalMessage = originalMessage,
    comments = comments,
    resolvedAtMs = resolvedAt,
)
