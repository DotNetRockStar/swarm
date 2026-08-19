/**
 * Schema for what this device remembers locally between launches: the
 * saved STUN connection (server URL, device name, this device's assigned
 * id) and the swarms registered on it, plus app-level settings (artwork
 * cache TTL). Deliberately separate from [app.swarm.tv.app.data.AndroidTokenStore]
 * (the access token stays in `EncryptedSharedPreferences` — a secret has no
 * business in a plain SQLite file even a rooted device's file explorer can
 * read) and from [app.swarm.tv.app.data.AndroidWatchStateStore] (per-title
 * resume/watched state is genuinely a flat key/value shape with no
 * relations to model — see its own doc comment).
 *
 * [ServerConnectionEntity] is a singleton row (`id` always 1, upserted in
 * place) rather than one-row-per-device: this app registers as exactly one
 * device against exactly one STUN server at a time — [SwarmEntity] is the
 * one-to-many side, FK'd to it.
 */
package app.swarm.tv.app.data.db

import androidx.room.ColumnInfo
import androidx.room.Entity
import androidx.room.ForeignKey
import androidx.room.Index
import androidx.room.PrimaryKey

const val SINGLETON_ROW_ID = 1L

@Entity(tableName = "server_connection")
data class ServerConnectionEntity(
    @PrimaryKey val id: Long = SINGLETON_ROW_ID,
    @ColumnInfo(name = "base_url") val baseUrl: String,
    @ColumnInfo(name = "device_name") val deviceName: String,
    @ColumnInfo(name = "device_id") val deviceId: String,
    @ColumnInfo(name = "updated_at") val updatedAt: Long,
)

/**
 * One row per swarm this device belongs to on [ServerConnectionEntity]'s
 * server. At most one row has `is_active = 1` at a time — enforced by
 * [app.swarm.tv.app.data.db.SwarmDao.setActive] always sweeping every row
 * in one statement (never a separate clear-then-set pair, which could
 * leave two rows active if interrupted between them) rather than by a DB
 * constraint: SQLite partial unique indexes aren't expressible through
 * Room's `@Index` annotation, and hand-writing one outside Room's schema
 * tracking would desync its migration checksum validation.
 */
@Entity(
    tableName = "swarm",
    foreignKeys = [
        ForeignKey(
            entity = ServerConnectionEntity::class,
            parentColumns = ["id"],
            childColumns = ["connection_id"],
            onDelete = ForeignKey.CASCADE,
        ),
    ],
    indices = [Index("connection_id")],
)
data class SwarmEntity(
    @PrimaryKey val id: String,
    @ColumnInfo(name = "connection_id") val connectionId: Long,
    val name: String,
    @ColumnInfo(name = "is_active") val isActive: Boolean,
)

@Entity(tableName = "app_settings")
data class AppSettingsEntity(
    @PrimaryKey val id: Long = SINGLETON_ROW_ID,
    @ColumnInfo(name = "artwork_cache_minutes") val artworkCacheMinutes: Int,
)
