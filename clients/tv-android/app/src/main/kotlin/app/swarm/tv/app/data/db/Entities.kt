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

/**
 * Parental content controls — another singleton row, same pattern as
 * [AppSettingsEntity]. [pinHash]/[pinSalt] gate *managing* this feature
 * (opening this section in Settings to view/edit/disable it), not a
 * session-level unlock exposed anywhere else — see
 * [app.swarm.tv.app.data.RatingScale] for how [maxMovieRating]/[maxTvRating]
 * get compared against a real entry's rating. [allowedKinds]/[allowedGenres]
 * are comma-joined rather than a Room `TypeConverter`-backed collection —
 * simplest thing that works for two short string lists nothing else needs
 * to query into.
 */
@Entity(tableName = "kid_mode_settings")
data class KidModeSettingsEntity(
    @PrimaryKey val id: Long = SINGLETON_ROW_ID,
    val enabled: Boolean,
    @ColumnInfo(name = "pin_hash") val pinHash: String,
    @ColumnInfo(name = "pin_salt") val pinSalt: String,
    /** Comma-joined [app.swarm.tv.core.peer.MediaKind] names, e.g. "MOVIE,TRACK". Empty string means nothing is allowed (not "no restriction"). */
    @ColumnInfo(name = "allowed_kinds") val allowedKinds: String,
    /** Comma-joined genre names. Null means "every genre allowed" — the same "no filter" meaning `null` already carries throughout this app's genre-filter UI. */
    @ColumnInfo(name = "allowed_genres") val allowedGenres: String?,
    /** Null means "no rating restriction on movies". */
    @ColumnInfo(name = "max_movie_rating") val maxMovieRating: String?,
    /** Null means "no rating restriction on shows". */
    @ColumnInfo(name = "max_tv_rating") val maxTvRating: String?,
)

/**
 * A LAN server this client has successfully authenticated with. The
 * certificate fingerprint is the stable identity; host/ports are cached
 * discovery data and are refreshed whenever mDNS resolves the same server
 * again. Keeping every successful server lets the most recently used one be
 * restored without discarding older pairings.
 */
@Entity(
    tableName = "local_server_connection",
    indices = [Index(value = ["last_connected_at"])],
)
data class LocalServerConnectionEntity(
    @PrimaryKey @ColumnInfo(name = "cert_fingerprint") val certFingerprint: String,
    @ColumnInfo(name = "service_name") val serviceName: String,
    val name: String,
    val host: String,
    @ColumnInfo(name = "peer_port") val peerPort: Int,
    @ColumnInfo(name = "pairing_port") val pairingPort: Int,
    @ColumnInfo(name = "device_name") val deviceName: String,
    @ColumnInfo(name = "last_connected_at") val lastConnectedAt: Long,
)

/** Durable client inbox. Dismissed rows remain as tombstones so a failed
 * remote acknowledgement cannot make the same resolution reappear later. */
@Entity(
    tableName = "client_notification",
    indices = [Index(value = ["resolved_at"])],
)
data class ClientNotificationEntity(
    @PrimaryKey val key: String,
    @ColumnInfo(name = "server_id") val serverId: String,
    @ColumnInfo(name = "remote_id") val remoteId: Long,
    @ColumnInfo(name = "server_name") val serverName: String,
    @ColumnInfo(name = "asset_title") val assetTitle: String?,
    @ColumnInfo(name = "original_message") val originalMessage: String,
    val comments: String?,
    @ColumnInfo(name = "resolved_at") val resolvedAt: Long,
    val dismissed: Boolean = false,
)
